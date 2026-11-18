//! A cover or scan to look at closely: a local file, downloaded bytes or a
//! remote image, fitted to the window, with its pixel size.

use crate::thumbnails::texture;
use drtagger_core::i18n::{tr, trf};
use gtk::prelude::*;
use gtk::{gio, glib};
use std::path::PathBuf;

pub enum Source {
    File(PathBuf),
    Data(Vec<u8>),
    Url(String),
}

/// Decoded at about screen size: a 6000 px booklet page must not cost
/// hundreds of megabytes.
const MAX_PIXEL: u32 = 2800;

pub fn show(parent: &impl IsA<gtk::Widget>, title: &str, subtitle: Option<&str>, source: Source) {
    let header = adw::HeaderBar::new();
    let title_widget = adw::WindowTitle::new(title, subtitle.unwrap_or(""));
    header.set_title_widget(Some(&title_widget));
    if let Source::File(path) = &source {
        let open =
            gtk::Button::builder().icon_name("folder-open-symbolic").tooltip_text(tr("Open Containing Folder")).build();
        let file = gio::File::for_path(path);
        open.connect_clicked(move |b| {
            let launcher = gtk::FileLauncher::new(Some(&file));
            let window = b.root().and_downcast::<gtk::Window>();
            launcher.open_containing_folder(window.as_ref(), gio::Cancellable::NONE, |_| {});
        });
        header.pack_start(&open);
    }
    let picture = gtk::Picture::builder().content_fit(gtk::ContentFit::Contain).hexpand(true).vexpand(true).build();
    let spinner = adw::Spinner::builder().width_request(32).height_request(32).build();
    let stack = gtk::Stack::new();
    stack.add_named(&spinner, Some("loading"));
    stack.add_named(&picture, Some("image"));
    let failed =
        adw::StatusPage::builder().icon_name("image-missing-symbolic").title(tr("The image could not be read")).build();
    stack.add_named(&failed, Some("failed"));
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&stack));
    let dialog = adw::Dialog::builder().child(&view).content_width(900).content_height(900).title(title).build();
    adw::prelude::AdwDialogExt::present(&dialog, Some(parent));

    glib::spawn_future_local(async move {
        let loaded = gio::spawn_blocking(move || {
            let data = match source {
                Source::File(p) => std::fs::read(p).ok()?,
                Source::Data(d) => d,
                Source::Url(u) => {
                    let request = providerkit::http::Request::get(u)
                        .header("User-Agent", drtagger_core::identify::USER_AGENT)
                        .timeout(30);
                    let response = providerkit::http::default_transport().send(&request).ok()?;
                    (response.status == 200).then_some(response.body)?
                }
            };
            let size = tagkit::dimensions(&data);
            let image = visionkit::imaging::load_data(&data, MAX_PIXEL)?;
            Some((image, size))
        })
        .await
        .ok()
        .flatten();
        match loaded {
            Some((image, size)) => {
                picture.set_paintable(Some(&texture(&image)));
                stack.set_visible_child_name("image");
                let px = size.map(|(w, h)| trf("{} × {} px", &[&w, &h]));
                let sub = [px, subtitle_owned(&title_widget)].into_iter().flatten().filter(|s| !s.is_empty());
                title_widget.set_subtitle(&sub.collect::<Vec<_>>().join(" · "));
            }
            None => stack.set_visible_child_name("failed"),
        }
    });
}

fn subtitle_owned(t: &adw::WindowTitle) -> Option<String> {
    let s = t.subtitle();
    (!s.is_empty()).then(|| s.to_string())
}
