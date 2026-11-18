//! Photos of candidate releases, loaded on demand and cached (the Mac
//! app's ReleaseImagesService): Cover Art Archive (typed: Front, Back,
//! Obi…) and, with a Discogs token, the linked Discogs page, where obi
//! photos usually are. The gallery and compare-covers dialogs.

use crate::actions::spawn;
use crate::image_viewer::{self, Source as ImageSource};
use crate::state::State;
use crate::thumbnails;
use drtagger_core::i18n::{tr, trf};
use drtagger_core::identify::USER_AGENT;
use gtk::prelude::*;
use gtk::{gio, glib};
use identifykit::ScoredCandidate;
use providerkit::{Candidate, CoverArtArchiveClient, DiscogsClient, MusicBrainzClient, Source};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Obi,
    Front,
    Back,
    Medium,
    Booklet,
    Other,
    Discogs,
}

#[derive(Clone, Debug)]
pub struct GalleryImage {
    pub url: String,
    pub thumbnail: Option<String>,
    pub kind: Kind,
    pub label: String,
}

type Waiter = Box<dyn FnOnce(&[GalleryImage])>;

thread_local! {
    static CACHE: RefCell<HashMap<String, Option<Vec<GalleryImage>>>> = RefCell::default();
    static WAITING: RefCell<HashMap<String, Vec<Waiter>>> = RefCell::default();
}

fn fetch(c: &Candidate, discogs_token: Option<String>) -> Vec<GalleryImage> {
    let mut list = vec![];
    if c.source == Source::Musicbrainz
        && let Ok(images) = CoverArtArchiveClient::new(USER_AGENT).images(&c.provider_id)
    {
        for i in images {
            let types: Vec<String> = i.types.iter().map(|t| t.to_lowercase()).collect();
            let kind = if types.iter().any(|t| t == "obi") {
                Kind::Obi
            } else if i.is_front {
                Kind::Front
            } else if i.is_back {
                Kind::Back
            } else if types.iter().any(|t| t == "medium") {
                Kind::Medium
            } else if types.iter().any(|t| t == "booklet") {
                Kind::Booklet
            } else {
                Kind::Other
            };
            let label = if i.types.is_empty() { tr("Image") } else { i.types.join(", ") };
            list.push(GalleryImage {
                url: i.large.clone().unwrap_or(i.full_url.clone()),
                thumbnail: i.small.clone().or(i.thumbnail.clone()),
                kind,
                label,
            });
        }
    }
    if let Some(token) = discogs_token {
        let ids: Vec<String> = match c.source {
            Source::Discogs => vec![c.provider_id.clone()],
            Source::Musicbrainz => {
                MusicBrainzClient::new(USER_AGENT).discogs_release_ids(&c.provider_id).unwrap_or_default()
            }
            _ => vec![],
        };
        if let Some(id) = ids.first()
            && let Ok(photos) = DiscogsClient::new(USER_AGENT, &token).images(id)
        {
            for (n, p) in photos.into_iter().enumerate() {
                list.push(GalleryImage {
                    url: p.url,
                    thumbnail: p.thumbnail,
                    kind: Kind::Discogs,
                    label: format!("Discogs {}", n + 1),
                });
            }
        }
    }
    list.sort_by_key(|i| i.kind);
    list
}

/// Calls `done` with the release's photos (at once when cached).
pub fn load(state: &State, c: &Candidate, done: impl FnOnce(&[GalleryImage]) + 'static) {
    let id = c.id();
    if let Some(Some(list)) = CACHE.with(|cache| cache.borrow().get(&id).cloned()) {
        done(&list);
        return;
    }
    let first = WAITING.with(|w| {
        let mut w = w.borrow_mut();
        let first = !w.contains_key(&id);
        w.entry(id.clone()).or_default().push(Box::new(done));
        first
    });
    if !first {
        return;
    }
    CACHE.with(|cache| cache.borrow_mut().insert(id.clone(), None));
    let token = state.core_settings().discogs_token().map(str::to_string);
    let c = c.clone();
    spawn(async move {
        let list = gio::spawn_blocking(move || fetch(&c, token)).await.unwrap_or_default();
        CACHE.with(|cache| cache.borrow_mut().insert(id.clone(), Some(list.clone())));
        let waiters = WAITING.with(|w| w.borrow_mut().remove(&id).unwrap_or_default());
        for w in waiters {
            w(&list);
        }
    });
}

pub fn edition(c: &Candidate) -> String {
    [&c.year, &c.country, &c.media_format, &c.label, &c.catalog_number, &c.barcode]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(" · ")
}

fn badge(text: &str, obi: bool) -> gtk::Widget {
    let l = gtk::Label::new(Some(text));
    l.add_css_class("caption-heading");
    l.add_css_class("photo-badge");
    if obi {
        l.add_css_class("obi");
    }
    l.set_halign(gtk::Align::Start);
    l.set_valign(gtk::Align::Start);
    l.set_margin_start(6);
    l.set_margin_top(6);
    l.upcast()
}

/// Every photo of one release, obi first; click one to see it large.
pub fn gallery(parent: &gtk::Widget, state: &State, scored: &ScoredCandidate, selected: bool, on_choose: Rc<dyn Fn()>) {
    let c = scored.candidate.clone();
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&adw::WindowTitle::new(&format!("{} – {}", c.artist, c.title), &edition(&c))));
    let choose = gtk::Button::with_label(&if selected { tr("Chosen") } else { tr("Choose This Release") });
    choose.add_css_class("suggested-action");
    choose.set_sensitive(!selected);
    header.pack_end(&choose);
    let flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .column_spacing(14)
        .row_spacing(14)
        .homogeneous(true)
        .min_children_per_line(2)
        .max_children_per_line(6)
        .build();
    flow.set_margin_top(18);
    flow.set_margin_bottom(18);
    flow.set_margin_start(18);
    flow.set_margin_end(18);
    let stack = gtk::Stack::new();
    stack.add_named(&adw::Spinner::builder().width_request(32).height_request(32).build(), Some("loading"));
    stack.add_named(&gtk::ScrolledWindow::builder().child(&flow).vexpand(true).build(), Some("photos"));
    let has_token = state.core_settings().discogs_token().is_some();
    let empty = adw::StatusPage::builder()
        .icon_name("image-missing-symbolic")
        .title(tr("No photos for this release"))
        .description(if has_token {
            tr("Neither Cover Art Archive nor Discogs has images of it.")
        } else {
            tr("Cover Art Archive has none; add a Discogs token in Preferences to see Discogs photos, where obis usually are.")
        })
        .build();
    stack.add_named(&empty, Some("empty"));
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&stack));
    let dialog = adw::Dialog::builder().child(&view).content_width(760).content_height(600).build();
    choose.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |_| {
            on_choose();
            adw::prelude::AdwDialogExt::close(&dialog);
        }
    ));
    let title = c.title.clone();
    load(
        state,
        &c,
        glib::clone!(
            #[weak]
            flow,
            #[weak]
            stack,
            move |list| {
                if list.is_empty() {
                    stack.set_visible_child_name("empty");
                    return;
                }
                for img in list {
                    let cell = gtk::Box::new(gtk::Orientation::Vertical, 4);
                    let button = gtk::Button::builder()
                        .child(&thumbnails::remote(Some(img.thumbnail.as_deref().unwrap_or(&img.url)), 190))
                        .build();
                    button.add_css_class("flat");
                    button.add_css_class("thumbnail-button");
                    let (url, label) = (img.url.clone(), img.label.clone());
                    let viewer_title = format!("{title} · {label}");
                    button.connect_clicked(move |b| {
                        let host = url.split('/').nth(2).map(str::to_string);
                        image_viewer::show(b, &viewer_title, host.as_deref(), ImageSource::Url(url.clone()))
                    });
                    cell.append(&button);
                    let caption = gtk::Label::new(Some(&if img.kind == Kind::Obi {
                        "OBI".to_string()
                    } else {
                        img.label.clone()
                    }));
                    caption.add_css_class("caption");
                    if img.kind == Kind::Obi {
                        caption.add_css_class("heading");
                    }
                    cell.append(&caption);
                    flow.append(&cell);
                }
                stack.set_visible_child_name("photos");
            }
        ),
    );
    adw::prelude::AdwDialogExt::present(&dialog, Some(parent));
}

/// The candidates side by side, each with its obi (or front): pick the
/// pressing that matches the disc in your hands.
pub fn compare(
    parent: &gtk::Widget,
    state: &State,
    candidates: Vec<ScoredCandidate>,
    selected: Option<String>,
    on_choose: Rc<dyn Fn(&ScoredCandidate)>,
) {
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&adw::WindowTitle::new(
        &tr("Compare covers"),
        &tr("Click the pressing that matches your disc. Obis are shown when a photo of one exists."),
    )));
    let flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .column_spacing(16)
        .row_spacing(16)
        .homogeneous(true)
        .min_children_per_line(2)
        .max_children_per_line(6)
        .build();
    flow.set_margin_top(18);
    flow.set_margin_bottom(18);
    flow.set_margin_start(18);
    flow.set_margin_end(18);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&gtk::ScrolledWindow::builder().child(&flow).vexpand(true).build()));
    let dialog = adw::Dialog::builder().child(&view).content_width(900).content_height(680).build();
    for s in candidates {
        let c = s.candidate.clone();
        let tile = gtk::Box::new(gtk::Orientation::Vertical, 4);
        tile.set_width_request(200);
        let overlay = gtk::Overlay::new();
        let image_slot = gtk::Box::new(gtk::Orientation::Vertical, 0);
        image_slot.append(&thumbnails::remote(c.cover_art_url.as_deref(), 200));
        overlay.set_child(Some(&image_slot));
        if selected.as_deref() == Some(s.id().as_str()) {
            let check = gtk::Image::from_icon_name("object-select-symbolic");
            check.set_pixel_size(28);
            check.add_css_class("accent");
            check.set_halign(gtk::Align::End);
            check.set_valign(gtk::Align::End);
            check.set_margin_end(6);
            check.set_margin_bottom(6);
            overlay.add_overlay(&check);
        }
        let pick = gtk::Button::builder().child(&overlay).build();
        pick.add_css_class("flat");
        pick.add_css_class("thumbnail-button");
        let chosen = s.clone();
        let choose = on_choose.clone();
        pick.connect_clicked(glib::clone!(
            #[weak]
            dialog,
            move |_| {
                choose(&chosen);
                adw::prelude::AdwDialogExt::close(&dialog);
            }
        ));
        tile.append(&pick);
        let t = gtk::Label::builder().label(&c.title).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).build();
        t.add_css_class("heading");
        tile.append(&t);
        let e = gtk::Label::builder().label(edition(&c)).xalign(0.0).wrap(true).lines(2).build();
        e.add_css_class("caption");
        e.add_css_class("dim-label");
        tile.append(&e);
        let foot = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let score = gtk::Label::new(Some(&format!("{:.0}", s.score)));
        score.add_css_class("caption");
        score.add_css_class("dim-label");
        score.set_hexpand(true);
        score.set_xalign(0.0);
        foot.append(&score);
        let all = gtk::Button::builder().build();
        all.add_css_class("flat");
        all.add_css_class("caption");
        all.set_visible(false);
        foot.append(&all);
        tile.append(&foot);
        let (state2, s2, sel2, choose2) = (state.clone(), s.clone(), selected.clone(), on_choose.clone());
        all.connect_clicked(move |b| {
            let choose = choose2.clone();
            let s3 = s2.clone();
            gallery(
                b.upcast_ref(),
                &state2,
                &s2,
                sel2.as_deref() == Some(s2.id().as_str()),
                Rc::new(move || choose(&s3)),
            );
        });
        // The obi (or front) once the photos are known.
        load(
            state,
            &c,
            glib::clone!(
                #[weak]
                image_slot,
                #[weak]
                overlay,
                #[weak]
                all,
                move |list| {
                    if let Some(best) = list.iter().min_by_key(|i| i.kind) {
                        while let Some(child) = image_slot.first_child() {
                            image_slot.remove(&child);
                        }
                        image_slot
                            .append(&thumbnails::remote(Some(best.thumbnail.as_deref().unwrap_or(&best.url)), 200));
                        overlay.add_overlay(&badge(
                            &if best.kind == Kind::Obi { "OBI".to_string() } else { best.label.clone() },
                            best.kind == Kind::Obi,
                        ));
                    }
                    if !list.is_empty() {
                        all.set_label(&trf("All {} photos", &[&list.len()]));
                        all.set_visible(true);
                    }
                }
            ),
        );
        flow.append(&tile);
    }
    adw::prelude::AdwDialogExt::present(&dialog, Some(parent));
}
