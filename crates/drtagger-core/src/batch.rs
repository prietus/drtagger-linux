//! Runs the queue unattended, beets-style (the Mac app's BatchRunner).
//! Network workers identify (four albums at a time; the provider clients
//! are shared, so the services' rate limits hold), one CPU worker extracts
//! or splits and writes tags. Albums without a sure match are set aside for
//! review and the batch goes on. Statuses live on the records, so a batch
//! resumes after quitting.

use crate::i18n::{tr, trf};
use crate::identify::{self, USER_AGENT};
use crate::library_index::{LibraryIndex, release_key};
use crate::settings::Settings;
use crate::store::{AlbumRecord, Store};
use crate::tagging::{self, TagPlan, Tagger, chosen_candidate, needs_files};
use crate::{disc, sets};
use identifykit::{
    CandidateOrigin, FormatCompatibility, IdentificationResult, LocalFormat, MatchScorer, ScoredCandidate,
};
use librarykit::AlbumKind;
use providerkit::Candidate;
use splitkit::FFmpegTool;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Where an album is in a batch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BatchStatus {
    /// Not started.
    Pending,
    /// SACD extracted, fingerprints possible.
    ReadyToIdentify,
    /// Confident match: files (if needed), tags, organise.
    ReadyToFinish,
    /// A worker has it right now.
    Working,
    /// Set aside: not sure, or something to look at.
    Review,
    Done,
    Failed,
}

impl BatchStatus {
    pub fn raw(self) -> &'static str {
        match self {
            BatchStatus::Pending => "pending",
            BatchStatus::ReadyToIdentify => "readyToIdentify",
            BatchStatus::ReadyToFinish => "readyToFinish",
            BatchStatus::Working => "working",
            BatchStatus::Review => "review",
            BatchStatus::Done => "done",
            BatchStatus::Failed => "failed",
        }
    }

    pub fn from_raw(raw: &str) -> Option<BatchStatus> {
        Some(match raw {
            "pending" => BatchStatus::Pending,
            "readyToIdentify" => BatchStatus::ReadyToIdentify,
            "readyToFinish" => BatchStatus::ReadyToFinish,
            "working" => BatchStatus::Working,
            "review" => BatchStatus::Review,
            "done" => BatchStatus::Done,
            "failed" => BatchStatus::Failed,
            _ => return None,
        })
    }

    pub fn of(r: &AlbumRecord) -> Option<BatchStatus> {
        r.batch_status.as_deref().and_then(BatchStatus::from_raw)
    }

    pub fn is_open(self) -> bool {
        matches!(
            self,
            BatchStatus::Pending | BatchStatus::ReadyToIdentify | BatchStatus::ReadyToFinish | BatchStatus::Working
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Organise into the library after tagging.
    ImportToLibrary,
    /// Tags only, files stay where they are.
    UpdateInPlace,
}

impl Mode {
    pub fn raw(self) -> &'static str {
        match self {
            Mode::ImportToLibrary => "importToLibrary",
            Mode::UpdateInPlace => "updateInPlace",
        }
    }

    pub fn from_raw(raw: &str) -> Mode {
        if raw == "updateInPlace" { Mode::UpdateInPlace } else { Mode::ImportToLibrary }
    }
}

/// Counts of the albums in a batch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BatchCounts {
    pub total: usize,
    pub done: usize,
    pub review: usize,
    pub failed: usize,
    pub open: usize,
}

impl BatchCounts {
    pub fn new(records: &[AlbumRecord]) -> BatchCounts {
        let mut c = BatchCounts::default();
        for r in records {
            let Some(s) = BatchStatus::of(r) else { continue };
            c.total += 1;
            match s {
                BatchStatus::Done => c.done += 1,
                BatchStatus::Review => c.review += 1,
                BatchStatus::Failed => c.failed += 1,
                _ => c.open += 1,
            }
        }
        c
    }

    pub fn finished(&self) -> usize {
        self.done + self.review + self.failed
    }
}

/// The sidebar's order: newest scan first, then by title.
pub fn queue_order(a: &AlbumRecord, b: &AlbumRecord) -> std::cmp::Ordering {
    b.added_at.cmp(&a.added_at).then_with(|| librarykit::natural_cmp(&a.display_title, &b.display_title))
}

pub fn describe(c: Option<&Candidate>) -> String {
    let Some(c) = c else { return "?".into() };
    let details: Vec<&str> =
        [c.year.as_deref(), c.country.as_deref(), c.catalog_number.as_deref()].into_iter().flatten().collect();
    format!("{} – {} ({})", c.artist, c.title, details.join(", "))
}

/// The release the files already name, if they agree on one.
pub fn existing_release_key(plan: &TagPlan) -> Option<String> {
    let keys: Vec<Option<String>> = plan
        .tracks
        .iter()
        .map(|t| {
            if let Some(mb) = t.existing.first(tagkit::field::MB_ALBUM_ID).filter(|s| !s.is_empty()) {
                return Some(format!("mb:{}", mb.to_lowercase()));
            }
            if let Some(d) = t.existing.first(tagkit::field::DISCOGS_RELEASE_ID).filter(|s| !s.is_empty()) {
                return Some(format!("discogs:{d}"));
            }
            None
        })
        .collect();
    let first = keys.first()?.clone()?;
    keys.iter().all(|k| k.as_ref() == Some(&first)).then_some(first)
}

/// Albums a batch was working on when the app quit start that step again.
pub fn reset_interrupted(store: &Store) -> anyhow::Result<()> {
    for r in store.all()? {
        if BatchStatus::of(&r) != Some(BatchStatus::Working) {
            continue;
        }
        let back = if r.kind == AlbumKind::SacdIso && r.sacd_outcomes().is_empty() {
            BatchStatus::Pending
        } else if r.identification().is_some_and(|i| i.is_confident()) {
            BatchStatus::ReadyToFinish
        } else {
            BatchStatus::Pending
        };
        store.set_batch(&r.path, Some(back.raw()), None)?;
    }
    Ok(())
}

/// Leaves the batch: statuses are cleared, the albums stay in the queue.
pub fn clear(store: &Store) -> anyhow::Result<()> {
    for r in store.all()? {
        if r.batch_status.is_some() || r.batch_note.is_some() {
            store.set_batch(&r.path, None, None)?;
        }
    }
    Ok(())
}

/// Marks an album (every member) done without writing anything.
pub fn leave_as_is(store: &Store, members: &mut [AlbumRecord]) -> anyhow::Result<()> {
    for m in members.iter_mut() {
        m.batch_status = Some(BatchStatus::Done.raw().into());
        m.batch_note = Some(tr("Left as is"));
        store.set_batch(&m.path, m.batch_status.as_deref(), m.batch_note.as_deref())?;
    }
    Ok(())
}

/// What the batch tells whoever shows it.
#[derive(Clone, Debug)]
pub enum BatchEvent {
    /// Records saved by a worker (paths before and after a move).
    Records(Vec<PathBuf>),
    /// The activity lines changed.
    Activity,
    Finished,
}

pub type Listener = Arc<dyn Fn(BatchEvent) + Send + Sync>;

/// What the workers are doing, for the status line.
#[derive(Clone, Debug, Default)]
pub struct BatchActivity {
    pub indexing: Option<String>,
    pub identifying: Vec<String>,
    pub cpu: Option<String>,
}

impl BatchActivity {
    pub fn network(&self) -> Option<String> {
        match self.identifying.len() {
            0 => None,
            1 => Some(trf("Identifying {}", &[&self.identifying[0]])),
            n => Some(trf("Identifying {} albums: {}", &[&n, &self.identifying.join(", ")])),
        }
    }

    /// The one line the status bar shows.
    pub fn line(&self) -> Option<String> {
        self.indexing.clone().or_else(|| self.network()).or_else(|| self.cpu.clone())
    }
}

/// Albums identified at once.
pub const IDENTIFICATION_WORKERS: usize = 4;

pub struct BatchRunner {
    running: AtomicBool,
    stopping: AtomicBool,
    extracting: AtomicUsize,
    activity: Mutex<BatchActivity>,
    /// Picking an album and marking it working is one step.
    claim: Mutex<()>,
    started: Mutex<Option<(Instant, usize)>>,
    config: Mutex<Option<RunConfig>>,
    pub index: Arc<LibraryIndex>,
    pub tagger: Arc<Tagger>,
    listener: Mutex<Option<Listener>>,
    /// Where split and extracted tracks go before tagging (the inbox's
    /// staging folder); the library folder when None.
    files_root: Mutex<Option<PathBuf>>,
    deferred: Mutex<Deferred>,
}

#[derive(Clone)]
struct RunConfig {
    mode: Mode,
    dry_run: bool,
    settings: Settings,
    tool: Option<FFmpegTool>,
}

/// Rips handed over while a preview runs: imported when it ends.
#[derive(Default)]
struct Deferred {
    paths: Vec<PathBuf>,
    config: Option<RunConfig>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Worker {
    Network,
    Cpu,
}

impl BatchRunner {
    pub fn new(index: Arc<LibraryIndex>, tagger: Arc<Tagger>) -> Arc<BatchRunner> {
        Arc::new(BatchRunner {
            running: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            extracting: AtomicUsize::new(0),
            activity: Mutex::new(BatchActivity::default()),
            claim: Mutex::new(()),
            started: Mutex::new(None),
            config: Mutex::new(None),
            index,
            tagger,
            listener: Mutex::new(None),
            files_root: Mutex::new(None),
            deferred: Mutex::new(Deferred::default()),
        })
    }

    pub fn set_listener(&self, listener: Listener) {
        *self.listener.lock().unwrap() = Some(listener);
    }

    fn emit(&self, event: BatchEvent) {
        let listener = self.listener.lock().unwrap().clone();
        if let Some(l) = listener {
            l(event);
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub fn is_stopping(&self) -> bool {
        self.stopping.load(Ordering::SeqCst)
    }

    pub fn activity(&self) -> BatchActivity {
        self.activity.lock().unwrap().clone()
    }

    fn update_activity(&self, change: impl FnOnce(&mut BatchActivity)) {
        change(&mut self.activity.lock().unwrap());
        self.emit(BatchEvent::Activity);
    }

    /// Albums still to go and the pace so far: an estimate of the time left.
    pub fn estimated_remaining(&self, counts: &BatchCounts) -> Option<Duration> {
        let (started, finished_at_start) = (*self.started.lock().unwrap())?;
        if !self.is_running() || counts.finished() <= finished_at_start || counts.open == 0 {
            return None;
        }
        let rate = (counts.finished() - finished_at_start) as f64 / started.elapsed().as_secs_f64();
        (rate > 0.0).then(|| Duration::from_secs_f64(counts.open as f64 / rate))
    }

    // MARK: Control

    /// Marks every unfinished album of the queue and starts the workers.
    /// Every album not written by an earlier batch starts over (a preview
    /// run before a real one, failures, earlier reviews).
    pub fn start(
        self: &Arc<Self>,
        store: &Store,
        mode: Mode,
        dry_run: bool,
        settings: Settings,
        tool: Option<FFmpegTool>,
    ) -> anyhow::Result<Option<std::thread::JoinHandle<()>>> {
        if self.is_running() {
            return Ok(None);
        }
        for r in store.all()? {
            if r.tagged_at.is_some() && BatchStatus::of(&r) == Some(BatchStatus::Done) {
                continue;
            }
            store.set_batch(&r.path, Some(BatchStatus::Pending.raw()), None)?;
        }
        Ok(Some(self.run(store, RunConfig { mode, dry_run, settings, tool })))
    }

    pub fn resume(
        self: &Arc<Self>,
        store: &Store,
        mode: Mode,
        dry_run: bool,
        settings: Settings,
        tool: Option<FFmpegTool>,
    ) -> Option<std::thread::JoinHandle<()>> {
        if self.is_running() {
            return None;
        }
        Some(self.run(store, RunConfig { mode, dry_run, settings, tool }))
    }

    pub fn set_files_root(&self, root: Option<PathBuf>) {
        *self.files_root.lock().unwrap() = root;
    }

    /// Imports these albums (rips from the inbox): they join a running
    /// import, wait for a running preview to end, or start a batch.
    pub fn enqueue(
        self: &Arc<Self>,
        store: &Store,
        paths: &[PathBuf],
        settings: Settings,
        tool: Option<FFmpegTool>,
    ) -> anyhow::Result<()> {
        let config = RunConfig { mode: Mode::ImportToLibrary, dry_run: false, settings, tool };
        if self.is_running() && self.config.lock().unwrap().as_ref().is_some_and(|c| c.dry_run) {
            let mut d = self.deferred.lock().unwrap();
            d.paths.extend(paths.iter().cloned());
            d.config = Some(config);
            return Ok(());
        }
        Self::mark_pending(store, paths)?;
        if !self.is_running() {
            self.run(store, config);
        }
        Ok(())
    }

    fn mark_pending(store: &Store, paths: &[PathBuf]) -> anyhow::Result<()> {
        let all = store.all()?;
        for p in paths {
            let Some(r) = all.iter().find(|r| &r.path == p) else { continue };
            for m in sets::members_in(&all, r) {
                store.set_batch(&m.path, Some(BatchStatus::Pending.raw()), None)?;
            }
        }
        Ok(())
    }

    /// Finishes the albums in progress, leaves the rest pending.
    pub fn stop(&self) {
        if self.is_running() {
            self.stopping.store(true, Ordering::SeqCst);
            self.emit(BatchEvent::Activity);
        }
    }

    fn run(self: &Arc<Self>, store: &Store, config: RunConfig) -> std::thread::JoinHandle<()> {
        self.running.store(true, Ordering::SeqCst);
        self.stopping.store(false, Ordering::SeqCst);
        let finished = store.all().map(|all| BatchCounts::new(&all).finished()).unwrap_or(0);
        *self.started.lock().unwrap() = Some((Instant::now(), finished));
        *self.config.lock().unwrap() = Some(config.clone());
        self.emit(BatchEvent::Activity);
        let me = self.clone();
        std::thread::spawn(move || {
            // Duplicates are looked up in the library index.
            if let Some(root) = &config.settings.library_root {
                me.update_activity(|a| a.indexing = Some(tr("Indexing the library…")));
                let progress_owner = me.clone();
                me.index.ensure(root, &mut |count| {
                    progress_owner
                        .update_activity(|a| a.indexing = Some(trf("Indexing the library… {} folders", &[&count])))
                });
                me.update_activity(|a| a.indexing = None);
            }
            let store = Store::open_default().ok();
            loop {
                let mut workers = vec![];
                for _ in 0..IDENTIFICATION_WORKERS {
                    let me = me.clone();
                    workers.push(std::thread::spawn(move || me.worker(Worker::Network)));
                }
                {
                    let me = me.clone();
                    workers.push(std::thread::spawn(move || me.worker(Worker::Cpu)));
                }
                for w in workers {
                    let _ = w.join();
                }
                let Some(store) = &store else { break };
                // Rips that waited for a preview to end are imported now.
                let deferred = std::mem::take(&mut *me.deferred.lock().unwrap());
                if let Some(config) = deferred.config
                    && !deferred.paths.is_empty()
                {
                    *me.config.lock().unwrap() = Some(config);
                    let _ = Self::mark_pending(store, &deferred.paths);
                }
                // Work handed over while the workers were finishing.
                let open = store.all().is_ok_and(|all| {
                    all.iter().any(|r| {
                        matches!(
                            BatchStatus::of(r),
                            Some(BatchStatus::Pending | BatchStatus::ReadyToIdentify | BatchStatus::ReadyToFinish)
                        )
                    })
                });
                if me.is_stopping() || !open {
                    break;
                }
            }
            me.running.store(false, Ordering::SeqCst);
            me.stopping.store(false, Ordering::SeqCst);
            *me.activity.lock().unwrap() = BatchActivity::default();
            me.emit(BatchEvent::Finished);
        })
    }

    fn config(&self) -> RunConfig {
        self.config.lock().unwrap().clone().expect("batch config")
    }

    // MARK: Workers

    fn worker(self: Arc<Self>, worker: Worker) {
        let store = match Store::open_default() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("batch: cannot open the store: {e:#}");
                return;
            }
        };
        let config = self.config();
        while !self.is_stopping() {
            match self.claim(&store, &config, worker) {
                Some((leader, members, was)) => {
                    let result = match (worker, was) {
                        (Worker::Network, _) => self.identify_step(&store, &config, members),
                        (Worker::Cpu, BatchStatus::Pending) => self.extract_step(&store, &config, members),
                        (Worker::Cpu, _) => self.finish_step(&store, &config, members),
                    };
                    if let Err(e) = result {
                        eprintln!("batch: {}: {e:#}", leader.display());
                    }
                }
                None => {
                    if self.has_work(&store, worker) {
                        std::thread::sleep(Duration::from_secs(if worker == Worker::Network { 2 } else { 1 }));
                        continue;
                    }
                    return;
                }
            }
        }
    }

    /// Picks the next album for `worker` and marks every member working.
    fn claim(
        &self,
        store: &Store,
        config: &RunConfig,
        worker: Worker,
    ) -> Option<(PathBuf, Vec<AlbumRecord>, BatchStatus)> {
        let _guard = self.claim.lock().unwrap();
        let all = store.all().ok()?;
        let mut by_set: HashMap<String, Vec<AlbumRecord>> = HashMap::new();
        for r in &all {
            if let Some(id) = &r.set_id {
                by_set.entry(id.clone()).or_default().push(r.clone());
            }
        }
        for members in by_set.values_mut() {
            members.sort_by_key(|r| (r.set_position.unwrap_or(0), r.added_at));
        }
        let mut ordered = all.clone();
        ordered.sort_by(queue_order);
        for r in ordered {
            let members = r.set_id.as_ref().and_then(|id| by_set.get(id).cloned()).unwrap_or_else(|| vec![r.clone()]);
            if members.first().map(|m| &m.path) != Some(&r.path) {
                continue;
            }
            let Some(status) = BatchStatus::of(&r) else { continue };
            let extract_first = self.extract_first(config, &r, &members);
            let fits = match worker {
                Worker::Network => {
                    status == BatchStatus::ReadyToIdentify || (status == BatchStatus::Pending && !extract_first)
                }
                Worker::Cpu => {
                    status == BatchStatus::ReadyToFinish || (status == BatchStatus::Pending && extract_first)
                }
            };
            if fits {
                let note = match (worker, status) {
                    (Worker::Network, _) => tr("Identifying"),
                    (Worker::Cpu, BatchStatus::Pending) => tr("Extracting"),
                    (Worker::Cpu, _) => tr("Writing tags"),
                };
                if worker == Worker::Cpu && status == BatchStatus::Pending {
                    self.extracting.fetch_add(1, Ordering::SeqCst);
                }
                let mut members = members;
                self.set(store, &mut members, BatchStatus::Working, Some(note));
                return Some((r.path.clone(), members, status));
            }
        }
        None
    }

    /// Something the other worker may still hand over. Identification only
    /// gets albums back from an extraction; finishing gets every album an
    /// identification is working on.
    fn has_work(&self, store: &Store, worker: Worker) -> bool {
        if worker == Worker::Network && self.extracting.load(Ordering::SeqCst) > 0 {
            return true;
        }
        let Ok(all) = store.all() else { return false };
        all.iter().any(|r| {
            let Some(s) = BatchStatus::of(r) else { return false };
            match worker {
                Worker::Network => matches!(s, BatchStatus::Pending | BatchStatus::ReadyToIdentify),
                Worker::Cpu => matches!(s, BatchStatus::Working | BatchStatus::ReadyToIdentify | BatchStatus::Pending),
            }
        })
    }

    /// SACD images are extracted before they are identified (fingerprints).
    fn extract_first(&self, config: &RunConfig, r: &AlbumRecord, members: &[AlbumRecord]) -> bool {
        !config.dry_run
            && config.mode == Mode::ImportToLibrary
            && r.kind == AlbumKind::SacdIso
            && members.iter().any(needs_files)
    }

    fn set(&self, store: &Store, members: &mut [AlbumRecord], status: BatchStatus, note: Option<String>) {
        let mut paths = vec![];
        for m in members.iter_mut() {
            // A worker's copy may be older than the store's (another step
            // saved it): only the batch columns are this step's to change.
            if let Ok(Some(fresh)) = store.fetch(&m.path) {
                *m = fresh;
            }
            m.batch_status = Some(status.raw().into());
            m.batch_note = note.clone();
            let _ = store.set_batch(&m.path, Some(status.raw()), note.as_deref());
            paths.push(m.path.clone());
        }
        self.emit(BatchEvent::Records(paths));
    }

    fn reload(store: &Store, members: &[AlbumRecord]) -> Vec<AlbumRecord> {
        members.iter().map(|m| store.fetch(&m.path).ok().flatten().unwrap_or_else(|| m.clone())).collect()
    }

    // MARK: Steps

    fn extract_step(&self, store: &Store, config: &RunConfig, members: Vec<AlbumRecord>) -> anyhow::Result<()> {
        let title = members[0].display_title.clone();
        self.update_activity(|a| a.cpu = Some(trf("Extracting {}", &[&title])));
        let (ok, error) = self.produce_files(store, config, &members);
        self.extracting.fetch_sub(1, Ordering::SeqCst);
        self.update_activity(|a| a.cpu = None);
        let mut members = Self::reload(store, &members);
        if ok {
            self.set(store, &mut members, BatchStatus::ReadyToIdentify, None);
        } else {
            let note = error.unwrap_or_else(|| tr("Extraction failed"));
            self.set(store, &mut members, BatchStatus::Failed, Some(note));
        }
        Ok(())
    }

    /// Extracts or splits every member that still needs files into the
    /// library folder. Files left by an earlier session are replaced: the
    /// output is identical and verified again.
    fn produce_files(&self, store: &Store, config: &RunConfig, members: &[AlbumRecord]) -> (bool, Option<String>) {
        let pending: Vec<AlbumRecord> = members.iter().filter(|m| needs_files(m)).cloned().collect();
        if pending.is_empty() {
            return (true, None);
        }
        let staging = self.files_root.lock().unwrap().clone();
        let Some(root) = staging.or_else(|| config.settings.library_root.clone()) else {
            return (false, Some(tr("Choose a library folder in Settings first.")));
        };
        let settings = &config.settings;
        for mut m in pending {
            let result = if m.kind == AlbumKind::SacdIso {
                let job = disc::SacdJob {
                    multichannel: settings.extract_multichannel,
                    trash_originals: settings.move_originals_to_trash,
                };
                disc::extract_sacd(store, &mut m, &root, &settings.extract_options(true), job, None, &mut |_, _| {})
                    .map(|_| ())
            } else {
                match &config.tool {
                    Some(tool) => disc::split(
                        store,
                        &mut m,
                        &root,
                        tool,
                        &settings.split_options(true),
                        settings.move_originals_to_trash,
                        &mut |_, _| {},
                    )
                    .map(|outcomes| {
                        if outcomes.first().and_then(|o| o.ctdb_track_crc32s()).is_some() {
                            let _ = identify::check_ctdb(store, &mut m, tool);
                        }
                    }),
                    None => Err(anyhow::anyhow!(tr(crate::ffmpeg::NOT_FOUND))),
                }
            };
            let after = store.fetch(&m.path).ok().flatten();
            if let Err(e) = result {
                return (false, Some(after.and_then(|r| r.error_message).unwrap_or_else(|| format!("{e:#}"))));
            }
            if after.as_ref().is_none_or(|r| r.state == crate::store::AlbumState::Error || needs_files(r)) {
                return (false, after.and_then(|r| r.error_message));
            }
        }
        (true, None)
    }

    fn identify_step(&self, store: &Store, config: &RunConfig, members: Vec<AlbumRecord>) -> anyhow::Result<()> {
        let title = members[0].display_title.clone();
        self.update_activity(|a| a.identifying.push(title.clone()));
        let started = Instant::now();
        let mut members = Self::reload(store, &members);
        let positions: Vec<i64> = members.iter().map(|m| m.set_position.unwrap_or(1)).collect();
        let result = match &config.tool {
            None => Err(tr(crate::ffmpeg::NOT_FOUND)),
            Some(tool) if members[0].detected().is_some() => {
                identify::identify(store, &mut members, &positions, tool, &config.settings.keys, true, &|_| {})
                    .map_err(|e| format!("{e:#}"))
            }
            Some(_) => Err(tr("The album has no scan details; rescan it.")),
        };
        if let Ok(result) = &result {
            let how = if result.log.iter().any(|l| l.starts_with("Fast path")) {
                "fast path"
            } else if result.fingerprints.is_some() {
                "fingerprints"
            } else {
                "searches"
            };
            timing(&format!("{title}: identify {} ({how})", seconds(started)));
        }
        self.update_activity(|a| {
            if let Some(i) = a.identifying.iter().position(|t| t == &title) {
                a.identifying.remove(i);
            }
        });
        let result = match result {
            Ok(r) => r,
            Err(e) => {
                self.set(store, &mut members, BatchStatus::Failed, Some(e));
                return Ok(());
            }
        };
        let (status, note) = self.decide(store, config, &result, &mut members);
        self.set(store, &mut members, status, note);
        Ok(())
    }

    /// What an identification leaves the album to: finishing, or review.
    fn decide(
        &self,
        store: &Store,
        config: &RunConfig,
        result: &IdentificationResult,
        members: &mut [AlbumRecord],
    ) -> (BatchStatus, Option<String>) {
        let ready = (BatchStatus::ReadyToFinish, None);
        if result.is_confident() && chosen_candidate(&members[0]).is_some() {
            return ready;
        }
        if let Some(best) = result.best()
            && best.origins.contains(&CandidateOrigin::TagMbid)
            && best.track_count_matches == Some(true)
            && best.duration_fit.unwrap_or(1.0) >= 0.8
            && !MatchScorer::contradicts_local_codes(&best.candidate, &result.signals)
        {
            // The release the files already name, and it fits: confirmed.
            select(store, members, best, false);
            return ready;
        }
        if result.is_clear_barcode_match()
            && let Some(best) = result.best()
        {
            select(store, members, best, false);
            return ready;
        }
        if result.is_clear_fingerprint_match()
            && let Some(best) = result.best()
        {
            select(store, members, best, false);
            return ready;
        }
        if let Some(pick) = self.cover_tie_break(config, result, members) {
            select(store, members, &pick, false);
            return ready;
        }
        if let Some(pick) = result.album_only_pick() {
            select(store, members, pick, true);
            return ready;
        }
        if result.signals.local_format() == LocalFormat::Sacd
            && !result.candidates.iter().any(|c| MatchScorer::is_sacd_edition(&c.candidate))
            && result.best().and_then(|b| b.format_compatibility) != Some(FormatCompatibility::Fits)
        {
            return (BatchStatus::Review, Some(tr("DSD files, but no SACD edition is known; the candidates are CDs")));
        }
        if !result.candidates.is_empty() {
            let best = result
                .best()
                .map(|b| format!("{} – {} ({})", b.candidate.artist, b.candidate.title, b.confidence.raw()))
                .unwrap_or_default();
            return (BatchStatus::Review, Some(trf("No sure match; best: {}", &[&best])));
        }
        (BatchStatus::Review, Some(tr("No release found")))
    }

    /// Editions tied on proven audio: the album's own cover decides when
    /// it looks like one of them only.
    fn cover_tie_break(
        &self,
        config: &RunConfig,
        result: &IdentificationResult,
        members: &[AlbumRecord],
    ) -> Option<ScoredCandidate> {
        let contenders = result.cover_contenders();
        if contenders.is_empty() {
            return None;
        }
        let local = tagging::local_cover(members)?;
        let check = visionkit::CoverCheck::new(USER_AGENT, config.settings.discogs_token());
        let mut distances = HashMap::new();
        for c in contenders {
            if let Some(d) = check.distance(&local, &c.candidate) {
                distances.insert(c.id(), d);
            }
        }
        result.cover_tie_break(&distances).cloned()
    }

    fn finish_step(&self, store: &Store, config: &RunConfig, members: Vec<AlbumRecord>) -> anyhow::Result<()> {
        let mut members = Self::reload(store, &members);
        let title = members[0].display_title.clone();
        let review = |me: &Self, members: &mut [AlbumRecord], note: String| {
            me.update_activity(|a| a.cpu = None);
            me.set(store, members, BatchStatus::Review, Some(note));
        };
        let fail = |me: &Self, members: &mut [AlbumRecord], note: String| {
            me.update_activity(|a| a.cpu = None);
            me.set(store, members, BatchStatus::Failed, Some(note));
        };
        let chosen = chosen_candidate(&members[0]);
        let needs = members.iter().any(needs_files);
        if config.dry_run && needs {
            let what = if members[0].kind == AlbumKind::SacdIso { tr("extract") } else { tr("split") };
            let note = trf("Would {} the disc and tag it as {}", &[&what, &describe(chosen.as_ref())]);
            self.set(store, &mut members, BatchStatus::Done, Some(note));
            return Ok(());
        }
        if config.mode == Mode::UpdateInPlace && needs {
            // Updating in place never creates new track files in the library.
            let note = tr("SACD or CD image: new tracks would have to be written; use Import to extract or split it");
            self.set(store, &mut members, BatchStatus::Review, Some(note));
            return Ok(());
        }
        if needs {
            self.update_activity(|a| a.cpu = Some(trf("Preparing files for {}", &[&title])));
            let (ok, error) = self.produce_files(store, config, &members);
            members = Self::reload(store, &members);
            if !ok {
                let note = error.unwrap_or_else(|| tr("Could not produce the track files"));
                fail(self, &mut members, note);
                return Ok(());
            }
        }
        self.update_activity(|a| a.cpu = Some(trf("Tagging {}", &[&title])));
        let started = Instant::now();
        let plan = self.tagger.build_plan(&members, &config.settings, true, &|_| {});
        timing(&format!("{title}: plan {}", seconds(started)));
        let plan = match plan {
            Ok(p) => p,
            Err(e) => {
                review(self, &mut members, e);
                return Ok(());
            }
        };
        if !plan.warnings.is_empty() {
            // Track counts or files that do not line up: a person decides.
            review(self, &mut members, plan.warnings.join(" "));
            return Ok(());
        }
        // Files that name another release than the one identified: never
        // overwrite a choice someone made; show both and let them decide.
        if !plan.already_tagged
            && let Some(existing) = existing_release_key(&plan)
            && let Some(chosen) = &chosen
            && existing != release_key(chosen)
        {
            let note =
                trf("Tagged as another release ({}); drtagger suggests {}", &[&existing, &describe(Some(chosen))]);
            review(self, &mut members, note);
            return Ok(());
        }
        // The same release elsewhere in the library (not for an album
        // tagged without an edition: a CD rip of it is another pressing).
        if let Some(chosen) = &chosen
            && !members[0].album_only
        {
            let own: HashSet<PathBuf> =
                plan.tracks.iter().filter_map(|t| t.path.parent().map(librarykit::standardized)).collect();
            if let Some(dup) = self.index.duplicates(chosen, &own).first() {
                let note = trf("Already in the library: {}", &[&dup.folder.display()]);
                review(self, &mut members, note);
                return Ok(());
            }
        }
        if config.dry_run {
            self.update_activity(|a| a.cpu = None);
            let mut fields: Vec<String> = vec![];
            for t in &plan.tracks {
                for c in t.changed_fields() {
                    if !fields.contains(&c.name) {
                        fields.push(c.name.clone());
                    }
                }
            }
            let mut note = if plan.already_tagged {
                tr("Already tagged with this release")
            } else if !plan.has_work() {
                tr("Tags already match")
            } else {
                let shown = fields.iter().take(6).cloned().collect::<Vec<_>>().join(", ");
                let more = if fields.len() > 6 { "…" } else { "" };
                trf("Would write {} field(s): {}", &[&fields.len(), &format!("{shown}{more}")])
                    + &(if plan.cover.is_some() { tr(" + cover") } else { String::new() })
            };
            if members[0].album_only {
                note = tr("Album only, no edition: ") + &note;
            }
            self.set(store, &mut members, BatchStatus::Done, Some(note));
            return Ok(());
        }
        // Already tagged with this very release, or nothing would change:
        // leave the files alone (only move them when importing).
        if plan.already_tagged || !plan.has_work() {
            let mut note = if plan.already_tagged {
                tr("Already tagged with this release; nothing written")
            } else {
                tr("Tags already matched; nothing written")
            };
            if config.mode == Mode::ImportToLibrary {
                let before: Vec<PathBuf> = members.iter().map(|m| m.path.clone()).collect();
                if let Err(problem) = self.tagger.organize_only(store, &mut members, &plan, &config.settings) {
                    fail(self, &mut members, problem);
                    return Ok(());
                }
                self.emit(BatchEvent::Records(before));
                note += &tr("; moved into the library");
            }
            self.update_activity(|a| a.cpu = None);
            self.set(store, &mut members, BatchStatus::Done, Some(note));
            if config.mode == Mode::ImportToLibrary {
                self.carry_inbox_leftovers(&members);
            }
            return Ok(());
        }
        let before: Vec<PathBuf> = members.iter().map(|m| m.path.clone()).collect();
        let outcome = self.tagger.apply(
            store,
            &mut members,
            &plan,
            &config.settings,
            Some(config.mode == Mode::ImportToLibrary),
            &|_| {},
        );
        self.emit(BatchEvent::Records(before));
        self.update_activity(|a| a.cpu = None);
        if let Some(problem) = outcome.problems.first() {
            self.set(store, &mut members, BatchStatus::Failed, Some(problem.clone()));
            return Ok(());
        }
        let note = members[0].album_only.then(|| tr("Tagged as the album, without an edition"));
        self.set(store, &mut members, BatchStatus::Done, note);
        self.carry_inbox_leftovers(&members);
        let folders: BTreeSet<PathBuf> = members
            .iter()
            .flat_map(|m| m.tag_reports())
            .filter_map(|r| r.path.parent().map(Path::to_path_buf))
            .collect();
        let started = Instant::now();
        self.index.update(&folders.into_iter().collect::<Vec<_>>());
        let mut phases: Vec<String> = outcome.timings.iter().map(|(n, s)| format!("{n} {s:.1} s")).collect();
        phases.push(format!("index {}", seconds(started)));
        timing(&format!("{title}: {}", phases.join(" · ")));
        Ok(())
    }
}

impl BatchRunner {
    /// CUE-image rips from the inbox: what is left beside the image goes
    /// to the album's library folder (see `inbox::carry_leftovers`).
    fn carry_inbox_leftovers(&self, members: &[AlbumRecord]) {
        let Some(inbox) = self.files_root.lock().unwrap().as_ref().and_then(|s| s.parent().map(Path::to_path_buf))
        else {
            return;
        };
        for m in members.iter().filter(|m| m.kind == AlbumKind::CueImage && m.path.starts_with(&inbox)) {
            let moved: Vec<PathBuf> =
                m.tag_reports().into_iter().map(|r| r.path).chain(m.file_moves().into_values()).collect();
            let Some(folder) = moved.iter().find(|p| !p.starts_with(&inbox)).and_then(|p| p.parent()) else { continue };
            if let Err(e) = crate::inbox::carry_leftovers(&m.path, folder) {
                eprintln!("inbox: {}: {e:#}", m.path.display());
            }
        }
    }
}

/// The batch's pick for every member.
fn select(store: &Store, members: &mut [AlbumRecord], candidate: &ScoredCandidate, album_only: bool) {
    for m in members.iter_mut() {
        if let Ok(Some(fresh)) = store.fetch(&m.path) {
            *m = fresh;
        }
        m.selected_candidate_id = Some(candidate.id());
        m.album_only = album_only;
        if m.state == crate::store::AlbumState::Scanned {
            m.state = crate::store::AlbumState::NeedsReview;
        }
        let _ = store.save(m);
    }
}

/// Where each album's time goes, on stderr as it happens ("batch-time:").
fn timing(line: &str) {
    eprintln!("batch-time: {line}");
}

fn seconds(since: Instant) -> String {
    format!("{:.1} s", since.elapsed().as_secs_f64())
}

/// Every unfinished leader a new batch would process.
pub fn candidates(all: &[AlbumRecord]) -> Vec<AlbumRecord> {
    all.iter()
        .filter(|r| sets::members_in(all, r).first().map(|m| &m.path) == Some(&r.path))
        .filter(|r| {
            r.tagged_at.is_none() && !matches!(BatchStatus::of(r), Some(BatchStatus::Done) | Some(BatchStatus::Review))
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_round_trip() {
        for s in [
            BatchStatus::Pending,
            BatchStatus::ReadyToIdentify,
            BatchStatus::ReadyToFinish,
            BatchStatus::Working,
            BatchStatus::Review,
            BatchStatus::Done,
            BatchStatus::Failed,
        ] {
            assert_eq!(BatchStatus::from_raw(s.raw()), Some(s));
        }
        assert_eq!(Mode::from_raw("updateInPlace"), Mode::UpdateInPlace);
        assert_eq!(Mode::from_raw(""), Mode::ImportToLibrary);
    }
}
