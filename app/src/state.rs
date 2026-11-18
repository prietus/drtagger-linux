//! The app's model on the main thread (the Mac app's LibraryController and
//! the services' observable state): the queue store, the list the sidebar
//! shows, scans, what each album is doing, tag plans, and change events
//! the views listen to. Long work runs on worker threads with their own
//! store connection; results come back through `refresh`.

use crate::album_object::AlbumObject;
use drtagger_core::i18n::{tr, trf};
use drtagger_core::identify::ProviderKeys;
use drtagger_core::settings::Settings;
use drtagger_core::store::{AlbumRecord, AlbumState, Store, UpsertSummary};
use drtagger_core::tagging::{Activity, TagPlan, Tagger};
use gtk::prelude::*;
use gtk::{gio, glib};
use librarykit::{LibraryScanner, ScanResult};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

/// Something the views may need to redraw for.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Albums were added or removed.
    Queue,
    /// One album's record changed.
    Record(PathBuf),
    /// One album's progress line changed.
    Activity(PathBuf),
    /// Scan status (spinner, folder, summary).
    Scan,
    /// The inspector shows another album (or none).
    Selection,
    /// The batch: counts, activity, running or not, the filter.
    Batch,
}

#[derive(Clone, Debug, Default)]
pub struct ScanStatus {
    pub active: usize,
    pub current_folder: Option<String>,
    pub last_summary: Option<String>,
    /// Roots that have not answered for a while (a network volume that
    /// stopped responding); the read cannot be cancelled.
    pub stalled: Vec<PathBuf>,
}

impl ScanStatus {
    pub fn is_scanning(&self) -> bool {
        self.active > 0
    }
}

const STALL_SECONDS: u32 = 15;

pub struct AppState {
    pub store: RefCell<Store>,
    /// `AlbumObject`s, newest scan first, then by title.
    pub albums: gio::ListStore,
    pub settings: gio::Settings,
    pub keys: RefCell<ProviderKeys>,
    pub scan: RefCell<ScanStatus>,
    pub selection: RefCell<Option<PathBuf>>,
    pub tagger: Arc<Tagger>,
    /// The batch over the queue and the library index it checks.
    pub batch: Arc<drtagger_core::batch::BatchRunner>,
    /// Which albums the sidebar shows while a batch exists.
    pub batch_filter: std::cell::Cell<crate::batch::BatchFilter>,
    /// The batch table instead of the inspector.
    pub show_batch_table: std::cell::Cell<bool>,
    /// The rip inbox's share of the running batch, for its notification.
    pub inbox_run: RefCell<crate::inbox::InboxRun>,
    activities: RefCell<HashMap<PathBuf, Activity>>,
    plans: RefCell<HashMap<PathBuf, TagPlan>>,
    errors: RefCell<HashMap<PathBuf, String>>,
    busy: RefCell<HashMap<PathBuf, usize>>,
    /// View state that outlives a redraw: expanded rows, open logs…
    flags: RefCell<HashMap<String, bool>>,
    /// What an album last looked like when its tags were previewed
    /// automatically, so the preview runs once per change.
    previewed: RefCell<HashMap<PathBuf, String>>,
    /// The local CDDB server for rippers, while enabled.
    pub cddb: RefCell<Option<drtagger_core::cddb_server::CddbServer>>,
    pub cddb_error: RefCell<Option<String>>,
    /// Each returns false once it is done listening (its widget is gone).
    listeners: RefCell<Vec<Listener>>,
}

pub type State = Rc<AppState>;

type Listener = Rc<dyn Fn(&Event) -> bool>;

/// The service a progress line, an error or a busy flag belongs to (the
/// Mac app keeps them per service: identify, disc, tagging).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Job {
    Identify,
    Disc,
    Tags,
}

/// The album a job key belongs to.
pub fn album_of(key: &Path) -> PathBuf {
    let s = key.to_string_lossy();
    PathBuf::from(s.split('\u{1}').next().unwrap_or_default())
}

/// The key of `job` on the album at `path`.
pub fn job(path: &Path, job: Job) -> PathBuf {
    PathBuf::from(format!("{}\u{1}{job:?}", path.display()))
}

impl AppState {
    pub fn new() -> anyhow::Result<State> {
        let store = Store::open_default()?;
        let settings = gio::Settings::new(drtagger_core::APP_ID);
        let tagger = Arc::new(Tagger::new());
        let index = Arc::new(drtagger_core::library_index::LibraryIndex::load());
        let state = Rc::new(AppState {
            store: RefCell::new(store),
            albums: gio::ListStore::new::<AlbumObject>(),
            settings,
            keys: RefCell::new(ProviderKeys::default()),
            scan: RefCell::new(ScanStatus::default()),
            selection: RefCell::new(None),
            batch: drtagger_core::batch::BatchRunner::new(index, tagger.clone()),
            batch_filter: Default::default(),
            show_batch_table: Default::default(),
            inbox_run: Default::default(),
            tagger,
            activities: RefCell::default(),
            plans: RefCell::default(),
            errors: RefCell::default(),
            busy: RefCell::default(),
            flags: RefCell::default(),
            previewed: RefCell::default(),
            listeners: RefCell::default(),
            cddb: RefCell::default(),
            cddb_error: RefCell::default(),
        });
        // The queue is a work session: it starts empty unless the user
        // keeps it, or a batch's albums are still open.
        let has_batch = state.records().iter().any(|r| r.batch_status.is_some());
        if has_batch {
            // A batch was interrupted, or its report is still open: keep
            // its albums to resume it or review them.
            let _ = drtagger_core::batch::reset_interrupted(&state.store.borrow());
        } else if !state.settings.boolean("keep-queue-between-launches") {
            let _ = state.store.borrow().clear();
        }
        state.reload();
        state.connect_batch();
        state.setup_inbox();
        state.load_keys();
        state.update_cddb();
        for key in ["cddb-server-enabled", "cddb-port"] {
            let weak = Rc::downgrade(&state);
            state.settings.connect_changed(Some(key), move |_, _| {
                if let Some(s) = weak.upgrade() {
                    s.update_cddb();
                }
            });
        }
        Ok(state)
    }

    /// Starts, restarts or stops the CDDB server to match the settings.
    pub fn update_cddb(&self) {
        let enabled = self.settings.boolean("cddb-server-enabled");
        let port = self.settings.int("cddb-port").clamp(1024, 65535) as u16;
        let running_port = self.cddb.borrow().as_ref().map(|s| s.port);
        if enabled && running_port == Some(port) {
            return;
        }
        if let Some(server) = self.cddb.borrow_mut().take() {
            server.stop();
        }
        self.cddb_error.replace(None);
        if enabled {
            match drtagger_core::cddb_server::CddbServer::start(port) {
                Ok(server) => {
                    self.cddb.replace(Some(server));
                }
                Err(e) => {
                    self.cddb_error.replace(Some(trf("Port {} is not available: {}", &[&port, &e])));
                }
            }
        }
    }

    /// "Listening on 127.0.0.1:8880 · 3 request(s)", the error, or None when off.
    pub fn cddb_status(&self) -> Option<(String, bool)> {
        if let Some(s) = self.cddb.borrow().as_ref() {
            return Some((trf("Listening on 127.0.0.1:{} · {} request(s)", &[&s.port, &s.request_count()]), true));
        }
        self.cddb_error.borrow().clone().map(|e| (e, false))
    }

    // MARK: Events

    pub fn listen(&self, f: impl Fn(&Event) + 'static) {
        self.listeners.borrow_mut().push(Rc::new(move |e| {
            f(e);
            true
        }));
    }

    /// Listens for as long as `widget` lives.
    pub fn listen_for<W: IsA<glib::Object>>(&self, widget: &W, f: impl Fn(&W, &Event) + 'static) {
        let weak = widget.downgrade();
        self.listeners.borrow_mut().push(Rc::new(move |e| match weak.upgrade() {
            Some(w) => {
                f(&w, e);
                true
            }
            None => false,
        }));
    }

    pub fn emit(&self, event: Event) {
        let listeners: Vec<_> = self.listeners.borrow().clone();
        let mut dead = vec![];
        for l in listeners {
            if !l(&event) {
                dead.push(l);
            }
        }
        if !dead.is_empty() {
            self.listeners.borrow_mut().retain(|l| !dead.iter().any(|d| Rc::ptr_eq(d, l)));
        }
    }

    // MARK: Settings

    /// Provider keys from the keyring (read off the main thread).
    pub fn load_keys(self: &Rc<Self>) {
        let state = self.clone();
        glib::spawn_future_local(async move {
            if let Ok(keys) = gio::spawn_blocking(ProviderKeys::load).await {
                state.keys.replace(keys);
            }
        });
    }

    /// The configuration the services read, from GSettings and the keyring.
    pub fn core_settings(&self) -> Settings {
        let s = &self.settings;
        let root = s.string("library-root");
        Settings {
            library_root: (!root.is_empty()).then(|| PathBuf::from(root.as_str())),
            album_folder_template: s.string("album-folder-template").into(),
            track_file_template: s.string("track-file-template").into(),
            ascii_file_names: s.boolean("ascii-file-names"),
            organize_after_apply: s.boolean("organize-after-apply"),
            move_originals_to_trash: s.boolean("move-originals-to-trash"),
            keep_queue_between_launches: s.boolean("keep-queue-between-launches"),
            extract_multichannel: s.boolean("extract-multichannel"),
            sacd_pause_policy: if s.enum_("sacd-pause-policy") == 1 {
                sacdkit::PausePolicy::Drop
            } else {
                sacdkit::PausePolicy::AppendToPrevious
            },
            embed_front_cover: s.boolean("embed-front-cover"),
            embed_cover_max_pixels: s.int("embed-cover-max-pixels").max(300) as u32,
            save_cover_file: s.boolean("save-cover-file"),
            write_genres_from_discogs: s.boolean("genres-from-discogs"),
            compute_replay_gain: s.boolean("compute-replaygain"),
            ffmpeg_override_path: s.string("ffmpeg-override-path").into(),
            cddb_server_enabled: s.boolean("cddb-server-enabled"),
            cddb_port: s.int("cddb-port").clamp(1024, 65535) as u16,
            keys: self.keys.borrow().clone(),
        }
    }

    // MARK: Records

    pub fn records(&self) -> Vec<AlbumRecord> {
        self.store.borrow().all().unwrap_or_default()
    }

    pub fn record(&self, path: &Path) -> Option<AlbumRecord> {
        self.store.borrow().fetch(path).ok().flatten()
    }

    pub fn object(&self, path: &Path) -> Option<AlbumObject> {
        (0..self.albums.n_items())
            .filter_map(|i| self.albums.item(i).and_downcast::<AlbumObject>())
            .find(|o| o.path() == path)
    }

    pub fn selected_record(&self) -> Option<AlbumRecord> {
        let path = self.selection.borrow().clone()?;
        self.record(&path)
    }

    pub fn select(&self, path: Option<PathBuf>) {
        if *self.selection.borrow() != path {
            self.selection.replace(path);
            self.emit(Event::Selection);
        }
    }

    /// Rebuilds the list from the store: newest scan first, then by title.
    pub fn reload(&self) {
        let mut records = self.records();
        records.sort_by(|a, b| {
            b.added_at
                .cmp(&a.added_at)
                .then_with(|| a.display_title.to_lowercase().cmp(&b.display_title.to_lowercase()))
        });
        let objects: Vec<AlbumObject> = records.into_iter().map(AlbumObject::new).collect();
        self.albums.splice(0, self.albums.n_items(), &objects);
        let selected = self.selection.borrow().clone();
        if let Some(p) = selected
            && self.object(&p).is_none()
        {
            self.select(None);
        }
        self.emit(Event::Queue);
    }

    /// Reads one album again from the store (after a worker saved it).
    pub fn refresh(&self, path: &Path) {
        match (self.record(path), self.object(path)) {
            (Some(r), Some(o)) => o.set_record(r),
            _ => {
                self.reload();
                return;
            }
        }
        self.emit(Event::Record(path.to_path_buf()));
    }

    /// Records that are one release together with `record`, in disc order.
    pub fn members(&self, record: &AlbumRecord) -> Vec<AlbumRecord> {
        let Some(id) = &record.set_id else { return vec![record.clone()] };
        let mut all: Vec<AlbumRecord> = self.records().into_iter().filter(|r| r.set_id.as_ref() == Some(id)).collect();
        all.sort_by_key(|r| (r.set_position.unwrap_or(0), r.added_at));
        all
    }

    /// The key under which a set's plan and progress are kept: its leader.
    pub fn leader_path(&self, record: &AlbumRecord) -> PathBuf {
        self.members(record).first().map_or_else(|| record.path.clone(), |m| m.path.clone())
    }

    // MARK: View state

    pub fn flag(&self, path: &Path, name: &str) -> Option<bool> {
        self.flags.borrow().get(&format!("{}|{name}", path.display())).copied()
    }

    pub fn set_flag(&self, path: &Path, name: &str, value: bool) {
        self.flags.borrow_mut().insert(format!("{}|{name}", path.display()), value);
    }

    /// True when `signature` differs from the one seen last for `path`
    /// (the first sight only records it), as SwiftUI's onChange.
    pub fn changed(&self, path: &Path, signature: String) -> bool {
        let mut seen = self.previewed.borrow_mut();
        match seen.insert(path.to_path_buf(), signature.clone()) {
            Some(old) => old != signature,
            None => false,
        }
    }

    // MARK: Activity, plans, errors

    pub fn activity(&self, path: &Path) -> Option<Activity> {
        self.activities.borrow().get(path).cloned()
    }

    pub fn set_activity(&self, path: &Path, activity: Option<Activity>) {
        match activity {
            Some(a) => self.activities.borrow_mut().insert(path.to_path_buf(), a),
            None => self.activities.borrow_mut().remove(path),
        };
        self.emit(Event::Activity(path.to_path_buf()));
    }

    pub fn is_busy(&self, key: &Path) -> bool {
        self.busy.borrow().get(key).is_some_and(|n| *n > 0)
    }

    /// Any service working on the album at `path`.
    pub fn is_busy_any(&self, path: &Path) -> bool {
        [Job::Identify, Job::Disc, Job::Tags].into_iter().any(|j| self.is_busy(&job(path, j)))
    }

    /// Some album is being identified (its models are in use).
    pub fn is_identifying(&self) -> bool {
        self.busy.borrow().iter().any(|(k, n)| *n > 0 && k.to_string_lossy().ends_with("\u{1}Identify"))
    }

    pub fn plan(&self, leader: &Path) -> Option<TagPlan> {
        self.plans.borrow().get(leader).cloned()
    }

    pub fn set_plan(&self, leader: &Path, plan: Option<TagPlan>) {
        match plan {
            Some(p) => self.plans.borrow_mut().insert(leader.to_path_buf(), p),
            None => self.plans.borrow_mut().remove(leader),
        };
        self.emit(Event::Record(leader.to_path_buf()));
    }

    pub fn error(&self, leader: &Path) -> Option<String> {
        self.errors.borrow().get(leader).cloned()
    }

    pub fn set_error(&self, leader: &Path, error: Option<String>) {
        match error {
            Some(e) => self.errors.borrow_mut().insert(leader.to_path_buf(), e),
            None => self.errors.borrow_mut().remove(leader),
        };
        self.emit(Event::Record(album_of(leader)));
    }

    /// Runs `work` on a worker thread with its own store connection; its
    /// `Activity` reports become `key`'s progress line. The albums in
    /// `touched` are read again when it ends.
    pub async fn run<T: Send + 'static>(
        self: &Rc<Self>,
        key: &Path,
        touched: Vec<PathBuf>,
        work: impl FnOnce(&Store, &(dyn Fn(Activity) + Sync)) -> T + Send + 'static,
    ) -> Option<T> {
        *self.busy.borrow_mut().entry(key.to_path_buf()).or_insert(0) += 1;
        // Buttons follow the busy state.
        self.emit(Event::Record(album_of(key)));
        let (tx, rx) = async_channel::unbounded::<Activity>();
        let state = self.clone();
        let key_owned = key.to_path_buf();
        let listener = glib::spawn_future_local(async move {
            while let Ok(a) = rx.recv().await {
                state.set_activity(&key_owned, Some(a));
            }
        });
        let result = gio::spawn_blocking(move || {
            let store = Store::open_default().ok()?;
            let report = move |a: Activity| {
                let _ = tx.send_blocking(a);
            };
            Some(work(&store, &report))
        })
        .await
        .ok()
        .flatten();
        listener.abort();
        if let Some(n) = self.busy.borrow_mut().get_mut(key) {
            *n = n.saturating_sub(1);
        }
        self.set_activity(key, None);
        self.emit(Event::Record(album_of(key)));
        for p in touched {
            self.refresh(&p);
        }
        result
    }

    // MARK: Scanning

    /// Scans the given roots and stores every album found. Existing
    /// records (same path) are refreshed instead of duplicated.
    pub async fn add_roots(self: &Rc<Self>, roots: Vec<PathBuf>) -> Option<UpsertSummary> {
        if roots.is_empty() {
            return None;
        }
        let network = roots.iter().any(|r| librarykit::volume::is_network_volume(r));
        {
            let mut scan = self.scan.borrow_mut();
            scan.active += 1;
            if network {
                scan.last_summary = Some(tr("Reading from a network volume…"));
            }
        }
        self.emit(Event::Scan);

        // Watchdog: report a stall instead of an endless "Scanning…".
        let watchdog = {
            let state = self.clone();
            let roots = roots.clone();
            glib::timeout_add_seconds_local_once(STALL_SECONDS, move || {
                let name = roots.first().and_then(|r| r.file_name()).map(|n| n.to_string_lossy().into_owned());
                let name = name.unwrap_or_default();
                let mut scan = state.scan.borrow_mut();
                scan.stalled.extend(roots.iter().cloned());
                scan.last_summary = Some(if network {
                    trf(
                        "{} is on a network volume that is not responding. Check the mount or use a local copy; the app keeps working.",
                        &[&name],
                    )
                } else {
                    trf("Still reading {}… the volume is slow or not responding.", &[&name])
                });
                drop(scan);
                state.emit(Event::Scan);
            })
        };

        let (tx, rx) = async_channel::unbounded::<String>();
        let state = self.clone();
        let folders = glib::spawn_future_local(async move {
            while let Ok(folder) = rx.recv().await {
                state.scan.borrow_mut().current_folder = Some(folder);
                state.emit(Event::Scan);
            }
        });
        let scan_roots = roots.clone();
        let result: ScanResult = gio::spawn_blocking(move || {
            LibraryScanner::new().scan_with_progress(&scan_roots, &mut |folder| {
                let name = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let _ = tx.send_blocking(name);
            })
        })
        .await
        .unwrap_or_default();
        folders.abort();
        watchdog.remove();

        let summary = self.store.borrow_mut().upsert(&result).ok();
        // Release sets: siblings by name, folders by the release their files name.
        let grouped = Arc::new(result);
        let worker = grouped.clone();
        let _ = gio::spawn_blocking(move || {
            Store::open_default().and_then(|store| drtagger_core::sets::after_scan(&store, &worker))
        })
        .await;
        let result = grouped;
        {
            let mut scan = self.scan.borrow_mut();
            scan.active -= 1;
            if scan.active == 0 {
                scan.current_folder = None;
            }
            scan.stalled.retain(|s| !roots.contains(s));
            if let Some(s) = &summary {
                scan.last_summary =
                    Some(trf("{} added, {} updated, {} folders scanned", &[&s.added, &s.updated, &s.folders_visited]));
            }
        }
        self.reload();
        if self.selection.borrow().is_none()
            && let Some(first) = result.albums.first()
        {
            self.select(Some(first.id()));
        }
        self.emit(Event::Scan);
        summary
    }

    /// Adds folders opened from another app and selects the album asked for.
    pub async fn open(self: &Rc<Self>, paths: Vec<PathBuf>) {
        self.add_roots(paths.clone()).await;
        let wanted: Vec<PathBuf> = paths.iter().map(|p| librarykit::standardized(p)).collect();
        if let Some(r) = self.records().into_iter().find(|r| wanted.iter().any(|w| r.path.starts_with(w))) {
            self.select(Some(r.path));
        }
    }

    /// Detection again for one album; states that carry identification
    /// progress survive, only pending / scanning / error collapse.
    pub async fn rescan(self: &Rc<Self>, path: PathBuf) {
        let Some(mut record) = self.record(&path) else { return };
        let previous = record.state;
        record.state = AlbumState::Scanning;
        let _ = self.store.borrow().save(&record);
        self.refresh(&path);
        self.add_roots(vec![path.clone()]).await;
        let Some(mut record) = self.record(&path) else { return };
        if record.state == AlbumState::Scanning {
            record.state = AlbumState::Error;
            record.error_message = Some(tr("The album is no longer detected at this path."));
        } else if !matches!(previous, AlbumState::Pending | AlbumState::Scanning | AlbumState::Error) {
            record.state = previous;
        }
        let _ = self.store.borrow().save(&record);
        self.refresh(&path);
    }

    pub fn remove(&self, paths: &[PathBuf]) {
        for p in paths {
            let _ = self.store.borrow().remove(p);
            self.plans.borrow_mut().remove(p);
            self.errors.borrow_mut().remove(p);
            if self.selection.borrow().as_ref() == Some(p) {
                self.selection.replace(None);
                self.emit(Event::Selection);
            }
        }
        self.reload();
    }

    pub fn remove_all(&self) {
        let paths: Vec<PathBuf> = self.records().into_iter().map(|r| r.path).collect();
        self.remove(&paths);
        self.scan.borrow_mut().last_summary = None;
        self.emit(Event::Scan);
    }
}
