//! The rip inbox in the app: the folder is looked at every few seconds
//! while enabled; finished rips go to the queue and to the batch (import),
//! and a notification says what came of them. With the inbox on, closing
//! the window keeps the app running; "Start at login" registers it.

use crate::batch::BatchFilter;
use crate::state::{AppState, Event, State};
use drtagger_core::batch::BatchCounts;
use drtagger_core::i18n::{tr, trf};
use drtagger_core::inbox::{self, Inbox};
use drtagger_core::store::Store;
use gtk::prelude::*;
use gtk::{gio, glib};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const POLL_SECONDS: u32 = 15;

/// What the inbox has handed to the batch since it last reported.
#[derive(Default)]
pub struct InboxRun {
    /// Counts when the first rip of this run was handed over.
    pub before: Option<BatchCounts>,
}

fn tool(settings: &drtagger_core::settings::Settings) -> Option<splitkit::FFmpegTool> {
    drtagger_core::ffmpeg::locate(&settings.ffmpeg_override_path)
        .filter(|l| l.ffprobe.is_some())
        .map(|l| drtagger_core::ffmpeg::tool(&l))
}

impl AppState {
    /// The inbox folder when watching is on and the folder exists.
    pub fn inbox_folder(&self) -> Option<PathBuf> {
        let folder = self.settings.string("inbox-folder");
        (self.settings.boolean("inbox-enabled") && !folder.is_empty())
            .then(|| PathBuf::from(folder.as_str()))
            .filter(|p| p.is_dir())
    }

    pub(crate) fn setup_inbox(self: &Rc<Self>) {
        let watcher: Arc<Mutex<Option<Inbox>>> = Arc::default();
        let configure = {
            let weak = Rc::downgrade(self);
            let watcher = watcher.clone();
            move || {
                let Some(state) = weak.upgrade() else { return };
                let folder = state.inbox_folder();
                let quiet = Duration::from_secs(state.settings.int("inbox-quiet-minutes").max(1) as u64 * 60);
                state.batch.set_files_root(folder.as_ref().map(|f| f.join(inbox::STAGING)));
                *watcher.lock().unwrap() = folder.map(|f| Inbox::new(&f, quiet));
                state.emit(Event::Batch);
            }
        };
        configure();
        for key in ["inbox-enabled", "inbox-folder", "inbox-quiet-minutes"] {
            let configure = configure.clone();
            self.settings.connect_changed(Some(key), move |_, _| configure());
        }
        let busy = Rc::new(std::cell::Cell::new(false));
        let weak = Rc::downgrade(self);
        glib::timeout_add_seconds_local(POLL_SECONDS, move || {
            let Some(state) = weak.upgrade() else { return glib::ControlFlow::Break };
            if busy.get() || watcher.lock().unwrap().is_none() || state.scan.borrow().is_scanning() {
                return glib::ControlFlow::Continue;
            }
            busy.set(true);
            let watcher = watcher.clone();
            let busy = busy.clone();
            glib::spawn_future_local(async move {
                state.poll_inbox(watcher).await;
                busy.set(false);
            });
            glib::ControlFlow::Continue
        });
        // Recognition models are let go once nothing identifies for a while.
        let weak = Rc::downgrade(self);
        let idle_since: Rc<std::cell::Cell<Option<Instant>>> = Rc::default();
        let released = Rc::new(std::cell::Cell::new(true));
        glib::timeout_add_seconds_local(30, move || {
            let Some(state) = weak.upgrade() else { return glib::ControlFlow::Break };
            if state.batch.is_running() || state.is_identifying() {
                idle_since.set(None);
                released.set(false);
            } else if released.get() {
            } else if idle_since.get().is_none() {
                idle_since.set(Some(Instant::now()));
            } else if idle_since.get().is_some_and(|t| t.elapsed() >= Duration::from_secs(60)) {
                released.set(true);
                gio::spawn_blocking(visionkit::release_idle_sessions);
            }
            glib::ControlFlow::Continue
        });
        // A batch interrupted by quitting goes on by itself.
        if self.inbox_folder().is_some() && self.batch_counts().open > 0 {
            self.resume_batch();
        }
        self.update_login_item();
        let weak = Rc::downgrade(self);
        self.settings.connect_changed(Some("start-at-login"), move |_, _| {
            if let Some(s) = weak.upgrade() {
                s.update_login_item();
            }
        });
    }

    async fn poll_inbox(self: &Rc<Self>, watcher: Arc<Mutex<Option<Inbox>>>) {
        let poller = watcher.clone();
        let ready = gio::spawn_blocking(move || {
            poller.lock().unwrap().as_mut().map(|i| i.poll(Instant::now())).unwrap_or_default()
        })
        .await
        .unwrap_or_default();
        if ready.is_empty() {
            return;
        }
        let entries = ready.clone();
        let leaders: Vec<PathBuf> = gio::spawn_blocking(move || {
            let Ok(mut store) = Store::open_default() else { return vec![] };
            let mut leaders = vec![];
            for entry in &entries {
                match inbox::add_to_queue(&mut store, entry) {
                    Ok(l) => leaders.extend(l),
                    Err(e) => tracing::warn!("inbox: {}: {e:#}", entry.display()),
                }
            }
            leaders
        })
        .await
        .unwrap_or_default();
        {
            let mut w = watcher.lock().unwrap();
            if let Some(inbox) = w.as_mut() {
                for entry in &ready {
                    inbox.mark_seen(entry);
                }
            }
        }
        self.reload();
        if leaders.is_empty() {
            return;
        }
        {
            let mut run = self.inbox_run.borrow_mut();
            if run.before.is_none() {
                run.before = Some(self.batch_counts());
            }
        }
        let settings = self.core_settings();
        let tool = tool(&settings);
        if let Err(e) = self.batch.enqueue(&self.store.borrow(), &leaders, settings, tool) {
            tracing::warn!("inbox: {e:#}");
        }
        self.reload();
        self.emit(Event::Batch);
    }

    /// Called when a batch ends: what the inbox's rips came to.
    pub(crate) fn report_inbox(&self) {
        let Some(before) = self.inbox_run.borrow_mut().before.take() else { return };
        let after = self.batch_counts();
        let done = after.done.saturating_sub(before.done);
        let review = after.review.saturating_sub(before.review);
        let failed = after.failed.saturating_sub(before.failed);
        let mut lines = vec![];
        if done > 0 {
            lines.push(trf("{} album(s) imported into the library", &[&done]));
        }
        if review > 0 {
            lines.push(trf("{} album(s) to review", &[&review]));
        }
        if failed > 0 {
            lines.push(trf("{} album(s) could not be imported", &[&failed]));
        }
        if lines.is_empty() {
            return;
        }
        let n = gio::Notification::new(&tr("Rips processed"));
        n.set_body(Some(&lines.join("\n")));
        if review + failed > 0 {
            // Opening shows what needs a look.
            self.batch_filter.set(if review > 0 { BatchFilter::Review } else { BatchFilter::Failed });
            n.set_priority(gio::NotificationPriority::High);
        }
        n.set_default_action("app.activate");
        if let Some(app) = gio::Application::default() {
            app.send_notification(Some("inbox"), &n);
        }
    }

    /// Registers or removes the login item: the Background portal inside
    /// the Flatpak, an XDG autostart file outside it.
    fn update_login_item(&self) {
        let enable = self.settings.boolean("start-at-login");
        if std::path::Path::new("/.flatpak-info").exists() {
            glib::spawn_future_local(request_background(enable));
            return;
        }
        let dir = glib::user_config_dir().join("autostart");
        let file = dir.join(format!("{}.desktop", drtagger_core::APP_ID));
        if !enable {
            let _ = std::fs::remove_file(&file);
            return;
        }
        let exe = std::env::current_exe().map(|e| e.display().to_string()).unwrap_or_else(|_| "drtagger".into());
        let entry = format!(
            "[Desktop Entry]\nType=Application\nName=drtagger\nExec=\"{exe}\" --background\nIcon={}\nX-GNOME-Autostart-enabled=true\nNoDisplay=true\n",
            drtagger_core::APP_ID
        );
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(&file, entry);
    }
}

async fn request_background(enable: bool) {
    let Ok(bus) = gio::bus_get_future(gio::BusType::Session).await else { return };
    let options = glib::VariantDict::new(None);
    options.insert("reason", tr("Watch the rip folder in the background"));
    options.insert("autostart", enable);
    options.insert("commandline", vec!["drtagger".to_string(), "--background".to_string()]);
    options.insert("dbus-activatable", false);
    let args = ("", options.end()).to_variant();
    if let Err(e) = bus
        .call_future(
            Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.Background",
            "RequestBackground",
            Some(&args),
            None,
            gio::DBusCallFlags::NONE,
            -1,
        )
        .await
    {
        tracing::warn!("background portal: {e}");
    }
}

/// Keeps the app running with the window closed while the inbox is on.
pub fn keep_running(state: &State, window: &impl IsA<gtk::Window>) {
    let window = window.as_ref().clone();
    let update = glib::clone!(
        #[weak]
        state,
        #[weak]
        window,
        move || window.set_hide_on_close(state.settings.boolean("inbox-enabled"))
    );
    update();
    state.settings.connect_changed(Some("inbox-enabled"), move |_, _| update());
}
