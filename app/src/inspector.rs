//! Right column: one album (the Mac app's AlbumInspectorView). The header,
//! the step sections (workflow, SACD extraction, identification, disc
//! identity, tags) and the scan details as reference.

use crate::image_viewer::{self, Source};
use crate::sidebar::state_icon;
use crate::state::{Event, State};
use crate::thumbnails;
use crate::ui;
use drtagger_core::i18n::{tr, trf};
use drtagger_core::store::AlbumRecord;
use gtk::glib;
use gtk::prelude::*;
use librarykit::{AlbumKind, CueSheet, DetectedAlbum, DetectedDisc, DetectedTrackFile, ScanIssue};
use std::path::{Path, PathBuf};

/// A section of the inspector that redraws itself; built per album.
pub type Section = fn(&State, &AlbumRecord) -> Option<gtk::Widget>;

pub struct Inspector {
    pub widget: gtk::Widget,
    pub scrolled: gtk::ScrolledWindow,
}

pub fn build(state: &State, sections: Vec<Section>) -> Inspector {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 20);
    content.set_margin_top(24);
    content.set_margin_bottom(24);
    content.set_margin_start(24);
    content.set_margin_end(24);
    let clamp = adw::Clamp::builder().maximum_size(1100).tightening_threshold(900).child(&content).build();
    let scrolled =
        gtk::ScrolledWindow::builder().child(&clamp).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
    let empty = adw::StatusPage::builder()
        .icon_name("media-optical-symbolic")
        .title(tr("No Album Selected"))
        .description(tr("Add folders with SACD ISOs, CD images or album tracks to get started."))
        .build();
    let stack = gtk::Stack::new();
    stack.add_named(&empty, Some("empty"));
    stack.add_named(&scrolled, Some("album"));

    let render = glib::clone!(
        #[weak]
        state,
        #[weak]
        content,
        #[weak]
        stack,
        move || {
            while let Some(child) = content.first_child() {
                content.remove(&child);
            }
            let Some(record) = state.selected_record() else {
                stack.set_visible_child_name("empty");
                return;
            };
            stack.set_visible_child_name("album");
            content.append(&header(&record));
            let detected = record.detected();
            if let Some(detected) = &detected {
                for section in &sections {
                    if let Some(w) = section(&state, &record) {
                        content.append(&w);
                    }
                }
                content.append(&details(&state, &record, detected));
            } else {
                content.append(&ui::dim(&tr("No scan details stored for this album. Use Rescan.")));
            }
            let issues = record.issues();
            if !issues.is_empty() {
                content.append(&issues_section(&issues));
            }
            if let Some(error) = &record.error_message {
                content.append(&ui::error_line(error));
            }
        }
    );
    render();
    let previous: std::rc::Rc<std::cell::RefCell<Option<PathBuf>>> = Default::default();
    state.listen(glib::clone!(
        #[weak]
        state,
        #[weak]
        scrolled,
        move |e| {
            let shown = state.selection.borrow().clone();
            let redraw = match e {
                Event::Selection | Event::Queue => true,
                Event::Record(p) => shown.as_ref().is_some_and(|s| s == p || is_member(&state, s, p)),
                _ => false,
            };
            if !redraw {
                return;
            }
            // Another album starts at the top; the same one keeps its place.
            let same = *previous.borrow() == shown;
            let at = scrolled.vadjustment().value();
            render();
            if same {
                let adj = scrolled.vadjustment();
                glib::idle_add_local_once(move || adj.set_value(at));
            } else {
                scrolled.vadjustment().set_value(0.0);
            }
            previous.replace(shown);
        }
    ));
    Inspector { widget: stack.upcast(), scrolled }
}

fn is_member(state: &State, shown: &Path, changed: &Path) -> bool {
    state.record(shown).is_some_and(|r| r.set_id.is_some() && state.members(&r).iter().any(|m| m.path == changed))
}

// MARK: Header

fn header(record: &AlbumRecord) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    let detected = record.detected();
    match detected.as_ref().and_then(|d| d.artwork_files.first().cloned()) {
        Some(cover) => {
            let button = gtk::Button::builder()
                .child(&thumbnails::thumbnail(&cover, 96))
                .tooltip_text(tr("Click to view larger"))
                .build();
            button.add_css_class("flat");
            button.add_css_class("thumbnail-button");
            button.set_valign(gtk::Align::Start);
            button.connect_clicked(move |b| {
                let name = file_name(&cover);
                image_viewer::show(b, &name, None, Source::File(cover.clone()));
            });
            row.append(&button);
        }
        None => {
            let placeholder = gtk::Image::from_icon_name("media-optical-symbolic");
            placeholder.set_pixel_size(48);
            placeholder.add_css_class("cover-placeholder");
            placeholder.set_size_request(96, 96);
            placeholder.set_valign(gtk::Align::Start);
            row.append(&placeholder);
        }
    }
    let texts = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let title = ui::selectable(&record.display_title);
    title.add_css_class("title-2");
    texts.append(&title);
    if let Some(artist) = record.artist_hint.as_ref().filter(|a| !a.is_empty()) {
        let a = ui::selectable(artist);
        a.add_css_class("title-4");
        a.add_css_class("dim-label");
        texts.append(&a);
    }
    let status = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    status.append(&gtk::Image::from_icon_name(state_icon(record.state)));
    let mut line = tr(record.state.label());
    if let Some(year) = &record.year_hint {
        line = format!("{line} · {year}");
    }
    status.append(&gtk::Label::new(Some(&line)));
    status.add_css_class("dim-label");
    texts.append(&status);
    let path = ui::selectable(&record.path.to_string_lossy());
    path.add_css_class("caption");
    path.add_css_class("dim-label");
    path.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    path.set_lines(2);
    texts.append(&path);
    row.append(&texts);
    row.upcast()
}

// MARK: Details

fn details(state: &State, record: &AlbumRecord, album: &DetectedAlbum) -> gtk::Widget {
    let body = gtk::Box::new(gtk::Orientation::Vertical, 20);
    body.set_margin_top(10);
    body.append(&overview(album));
    if let Some(sacd) = &album.sacd {
        body.append(&sacd_section(sacd));
    }
    let several = album.discs.len() > 1;
    for disc in &album.discs {
        body.append(&disc_section(disc, several));
    }
    if !album.artwork_files.is_empty() {
        body.append(&artwork_section(&album.artwork_files));
    }
    let expander = gtk::Expander::builder().child(&body).build();
    let label = gtk::Label::new(Some(&tr("Details")));
    label.add_css_class("heading");
    expander.set_label_widget(Some(&label));
    state.settings.bind("inspector-show-details", &expander, "expanded").build();
    let _ = record;
    expander.upcast()
}

fn overview(album: &DetectedAlbum) -> gtk::Widget {
    let mut rows = vec![(tr("Kind"), tr(album.kind.display_name())), (tr("Tracks"), album.track_count().to_string())];
    if album.kind != AlbumKind::SacdIso {
        rows.push((tr("Discs"), album.discs.len().to_string()));
        let mut formats: Vec<&str> = album.formats().iter().map(|f| f.display_name()).collect();
        formats.sort();
        rows.push((tr("Formats"), formats.join(", ")));
    }
    if let Some(iso) = &album.iso_file {
        rows.push((tr("Image size"), glib::format_size(iso.file_size as u64).to_string()));
    }
    ui::group(&tr("Source"), &ui::grid(&rows))
}

fn sacd_section(sacd: &librarykit::SACDInfo) -> gtk::Widget {
    let mut rows = vec![];
    if let Some(t) = sacd.title() {
        rows.push((tr("Disc title"), t));
    }
    if let Some(a) = sacd.artist() {
        rows.push((tr("Disc artist"), a));
    }
    if !sacd.disc_catalog_number.is_empty() {
        rows.push((tr("Catalog number"), sacd.disc_catalog_number.clone()));
    }
    if let Some(d) = &sacd.disc_date {
        rows.push((tr("Disc date"), d.clone()));
    }
    if sacd.album_set_size > 1 {
        rows.push((tr("Album set"), trf("Disc {} of {}", &[&sacd.album_sequence_number, &sacd.album_set_size])));
    }
    for area in &sacd.areas {
        let name = if area.is_multichannel { tr("Multichannel area") } else { tr("Stereo area") };
        let value = trf(
            "{} ch · {} · {} tracks · {}",
            &[
                &area.channel_count,
                &(if area.is_dst() { "DST" } else { "DSD" }),
                &area.track_count,
                &duration(area.play_time_seconds),
            ],
        );
        rows.push((name, value));
    }
    ui::group("SACD", &ui::grid(&rows))
}

fn disc_section(disc: &DetectedDisc, show_number: bool) -> gtk::Widget {
    let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
    if let Some(cue) = &disc.cue {
        body.append(&cue_summary(cue, disc.cue_url.as_deref()));
    }
    match (&disc.image_file, &disc.cue) {
        (Some(image), Some(cue)) => {
            body.append(&ui::dim(&trf(
                "Image: {} ({})",
                &[&image.file_name(), &glib::format_size(image.file_size as u64)],
            )));
            body.append(&cue_tracks(cue));
        }
        _ => body.append(&file_tracks(&disc.track_files, disc.cue.as_ref())),
    }
    let title = if show_number { trf("Disc {}", &[&disc.number]) } else { tr("Tracks") };
    ui::group(&title, &body)
}

fn cue_summary(cue: &CueSheet, url: Option<&Path>) -> gtk::Widget {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    head.append(&gtk::Image::from_icon_name("text-x-generic-symbolic"));
    head.append(&gtk::Label::new(Some(&url.map(file_name).unwrap_or_else(|| "CUE".into()))));
    let enc = gtk::Label::new(Some(&format!("({})", cue.encoding_name)));
    enc.add_css_class("dim-label");
    head.append(&enc);
    b.append(&head);
    let catalogs = cue.catalog_number_hints();
    let hints: Vec<(String, Option<String>)> = vec![
        (tr("Performer"), cue.performer.clone()),
        (tr("Title"), cue.title.clone()),
        (tr("Date"), cue.date().map(str::to_string)),
        (tr("Genre"), cue.genre().map(str::to_string)),
        ("DiscID".into(), cue.disc_id().map(str::to_string)),
        (tr("Barcode"), cue.barcode_hint()),
        (tr("Catalog #"), (!catalogs.is_empty()).then(|| catalogs.join(", "))),
    ];
    for (name, value) in hints {
        if let Some(v) = value {
            let l = gtk::Label::builder().label(format!("{name}: {v}")).xalign(0.0).selectable(true).wrap(true).build();
            l.add_css_class("caption");
            b.append(&l);
        }
    }
    b.upcast()
}

fn track_line(number: usize, title: &str, trailing: &[(String, &str)]) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let n = gtk::Label::new(Some(&format!("{number:02}")));
    n.add_css_class("dim-label");
    n.add_css_class("numeric");
    n.set_width_chars(3);
    n.set_xalign(1.0);
    row.append(&n);
    let t =
        gtk::Label::builder().label(title).xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).build();
    row.append(&t);
    for (text, class) in trailing {
        let l = gtk::Label::new(Some(text));
        l.add_css_class(class);
        l.add_css_class("numeric");
        row.append(&l);
    }
    row.upcast()
}

fn cue_tracks(cue: &CueSheet) -> gtk::Widget {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
    for t in cue.audio_tracks() {
        let title = t.title.clone().unwrap_or_else(|| trf("Track {}", &[&t.number]));
        let trailing: Vec<(String, &str)> = t.start().map(|s| (s.msf_string(), "dim-label")).into_iter().collect();
        b.append(&track_line(t.number as usize, &title, &trailing));
    }
    b.upcast()
}

fn file_tracks(files: &[DetectedTrackFile], cue: Option<&CueSheet>) -> gtk::Widget {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let cue_tracks = cue.map(|c| c.audio_tracks()).unwrap_or_default();
    for (i, f) in files.iter().enumerate() {
        let title = cue_tracks
            .iter()
            .find(|t| t.number == i as i64 + 1)
            .and_then(|t| t.title.clone())
            .unwrap_or_else(|| f.file_name());
        let row = track_line(
            i + 1,
            &title,
            &[
                (f.format.display_name().to_string(), "dim-label"),
                (glib::format_size(f.file_size as u64).to_string(), "dim-label"),
            ],
        );
        row.set_tooltip_text(Some(&f.url.to_string_lossy()));
        b.append(&row);
    }
    b.upcast()
}

fn artwork_section(files: &[PathBuf]) -> gtk::Widget {
    let strip = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    for path in files {
        let cell = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let button = gtk::Button::builder()
            .child(&thumbnails::thumbnail(path, 110))
            .tooltip_text(path.to_string_lossy())
            .build();
        button.add_css_class("flat");
        button.add_css_class("thumbnail-button");
        let p = path.clone();
        button.connect_clicked(move |b| image_viewer::show(b, &file_name(&p), None, Source::File(p.clone())));
        cell.append(&button);
        let name = gtk::Label::builder()
            .label(file_name(path))
            .max_width_chars(14)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .build();
        name.add_css_class("caption");
        cell.append(&name);
        strip.append(&cell);
    }
    let scroller = gtk::ScrolledWindow::builder()
        .child(&strip)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(150)
        .build();
    ui::group(&trf("Artwork ({})", &[&files.len()]), &scroller)
}

fn issues_section(issues: &[ScanIssue]) -> gtk::Widget {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
    for issue in issues {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.append(&gtk::Image::from_icon_name("dialog-information-symbolic"));
        let l = gtk::Label::builder()
            .label(format!("{}: {}", file_name(&issue.url), issue.message))
            .xalign(0.0)
            .wrap(true)
            .build();
        row.append(&l);
        b.append(&row);
    }
    ui::group(&tr("Notes"), &b)
}

pub fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn duration(seconds: i64) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}
