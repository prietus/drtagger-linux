//! Application services shared by the GTK app and the CLI.

pub mod batch;
pub mod cddb_server;
pub mod disc;
pub mod ffmpeg;
pub mod i18n;
pub mod identify;
pub mod inbox;
pub mod keys;
pub mod library_index;
pub mod paths;
pub mod sets;
pub mod settings;
pub mod store;
pub mod tagging;
pub mod trash;

use std::sync::OnceLock;

pub const APP_ID: &str = "us.priet.drtagger";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The process-wide tokio runtime for network and keyring work. The GTK app
/// awaits its join handles from the GLib main loop.
pub fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .thread_name("drtagger-io")
            .enable_all()
            .build()
            .expect("tokio runtime")
    })
}
