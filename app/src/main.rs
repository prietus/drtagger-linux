mod actions;
mod album_object;
mod batch;
mod image_viewer;
mod inbox;
mod inspector;
mod preferences;
mod release_images;
mod sections;
mod sidebar;
mod state;
mod thumbnails;
mod ui;
mod window;

use adw::prelude::*;
use drtagger_core::{APP_ID, VERSION};
use gtk::{gio, glib};
use std::path::PathBuf;

use crate::preferences::PreferencesDialog;
use crate::window::Window;

/// The compiled translations: $DRTAGGER_LOCALEDIR, the install's
/// share/locale (/app/share/locale in the Flatpak), else the build's.
fn locale_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("DRTAGGER_LOCALEDIR") {
        return d.into();
    }
    let installed = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.join("../share/locale")))
        .filter(|d| d.join("es/LC_MESSAGES/drtagger.mo").is_file());
    installed.unwrap_or_else(|| PathBuf::from(env!("DRTAGGER_DEV_LOCALEDIR")))
}

fn main() -> glib::ExitCode {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).init();
    drtagger_core::i18n::init(&locale_dir());
    gio::resources_register_include!("drtagger.gresource").expect("registering resources");

    let app = adw::Application::builder().application_id(APP_ID).flags(gio::ApplicationFlags::HANDLES_OPEN).build();
    app.add_main_option(
        "background",
        glib::Char::from(b'b'),
        glib::OptionFlags::NONE,
        glib::OptionArg::None,
        "Start without a window (the rip inbox keeps working)",
        None,
    );
    let background = std::rc::Rc::new(std::cell::Cell::new(false));
    app.connect_handle_local_options(glib::clone!(
        #[strong]
        background,
        move |_, options| {
            background.set(options.contains("background"));
            std::ops::ControlFlow::Continue(())
        }
    ));
    app.connect_startup(setup_actions);
    app.connect_activate(move |app| {
        if let Some(window) = window(app) {
            // Started at login: the window waits until it is asked for.
            if background.replace(false) {
                return;
            }
            window.present();
        }
    });
    // Folders opened from a file manager or `drtagger <paths>`: one window.
    app.connect_open(|app, files, _| {
        let paths: Vec<PathBuf> = files.iter().filter_map(|f| f.path()).collect();
        if let Some(window) = window(app) {
            window.present();
            window.open_paths(paths);
        }
    });
    app.run()
}

fn window(app: &adw::Application) -> Option<Window> {
    // A window closed while the inbox is on is only hidden.
    if let Some(w) = app.windows().into_iter().find_map(|w| w.downcast::<Window>().ok()) {
        return Some(w);
    }
    match Window::new(app) {
        Ok(w) => Some(w),
        Err(e) => {
            let dialog = adw::AlertDialog::new(Some("drtagger could not open its queue"), Some(&format!("{e:#}")));
            dialog.add_response("close", "Close");
            let w = adw::ApplicationWindow::builder().application(app).build();
            dialog.present(Some(&w));
            None
        }
    }
}

fn setup_actions(app: &adw::Application) {
    let quit = gio::ActionEntry::builder("quit").activate(|app: &adw::Application, _, _| app.quit()).build();
    let preferences = gio::ActionEntry::builder("preferences")
        .activate(|app: &adw::Application, _, _| {
            let state = app.active_window().and_downcast::<Window>().map(|w| w.state().clone());
            PreferencesDialog::new(state).present(app.active_window().as_ref());
        })
        .build();
    let about = gio::ActionEntry::builder("about").activate(|app: &adw::Application, _, _| show_about(app)).build();
    // Notifications open the window.
    let activate =
        gio::ActionEntry::builder("activate").activate(|app: &adw::Application, _, _| app.activate()).build();
    app.add_action_entries([quit, preferences, about, activate]);
    app.set_accels_for_action("app.quit", &["<Control>q"]);
    app.set_accels_for_action("app.preferences", &["<Control>comma"]);
    app.set_accels_for_action("window.close", &["<Control>w"]);
    app.set_accels_for_action("win.add-folders", &["<Control>o"]);
    app.set_accels_for_action("win.clear-queue", &["<Control><Shift>Delete"]);
    app.set_accels_for_action("win.rescan", &["<Control>r"]);
    app.set_accels_for_action("win.identify", &["<Control>i"]);
    app.set_accels_for_action("win.find", &["<Control>f"]);
}

fn show_about(app: &adw::Application) {
    let about = adw::AboutDialog::builder()
        .application_name("drtagger")
        .application_icon(APP_ID)
        .developer_name("Carlos Prieto Ortiz")
        .version(VERSION)
        .website("https://github.com/prietus/drtagger-linux")
        .issue_url("https://github.com/prietus/drtagger-linux/issues")
        .license_type(gtk::License::MitX11)
        .comments("Identify, split and tag audiophile rips: SACD ISOs, CD images and lossless folders.")
        .build();
    about.add_legal_section(
        "FFmpeg, Chromaprint and the DST decoder",
        None,
        gtk::License::Lgpl21,
        Some("drtagger bundles an LGPL build of FFmpeg and libchromaprint, and its DST decoder is a port of FFmpeg's LGPL dstdec.c."),
    );
    about.add_legal_section(
        "zxing-cpp, ONNX Runtime, PaddleOCR and DINOv2",
        None,
        gtk::License::Apache20,
        Some("Barcodes are read with zxing-cpp (Apache-2.0); OCR and cover comparison run PaddleOCR PP-OCRv5 and DINOv2 models (Apache-2.0) on ONNX Runtime (MIT)."),
    );
    about.present(app.active_window().as_ref());
}
