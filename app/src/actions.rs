//! What the album sections can do (the Mac app's IdentifyService,
//! DiscService, TagService and Workflow, driven from the main thread).
//! Each runs on a worker with its own store connection and refreshes the
//! records it touched.

use crate::state::{AppState, Event, Job, job};
use drtagger_core::i18n::tr;
use drtagger_core::store::{AlbumRecord, AlbumState};
use drtagger_core::tagging::{self, Activity, TagPlan};
use drtagger_core::{disc, ffmpeg, identify};
use gtk::prelude::*;
use gtk::{gio, glib};
use identifykit::ScoredCandidate;
use std::path::{Path, PathBuf};
use std::rc::Rc;

fn paths(members: &[AlbumRecord]) -> Vec<PathBuf> {
    members.iter().map(|m| m.path.clone()).collect()
}

/// The records again, as the worker saved them.
fn reload(store: &drtagger_core::store::Store, paths: &[PathBuf]) -> Vec<AlbumRecord> {
    paths.iter().filter_map(|p| store.fetch(p).ok().flatten()).collect()
}

fn tool(settings: &drtagger_core::settings::Settings) -> Result<splitkit::FFmpegTool, String> {
    let location = ffmpeg::locate(&settings.ffmpeg_override_path).ok_or_else(|| tr(ffmpeg::NOT_FOUND))?;
    if location.ffprobe.is_none() {
        return Err(tr(ffmpeg::NOT_FOUND));
    }
    Ok(ffmpeg::tool(&location))
}

impl AppState {
    /// Identifies an album (or its whole release set). Confident results
    /// preselect the best candidate; anything else leaves the choice.
    pub async fn identify(self: &Rc<Self>, record: &AlbumRecord) {
        let members = self.members(record);
        let key = job(&self.leader_path(record), Job::Identify);
        if self.is_busy(&key) {
            return;
        }
        self.set_error(&key, None);
        let settings = self.core_settings();
        let positions: Vec<i64> = members.iter().map(|m| m.set_position.unwrap_or(1)).collect();
        let member_paths = paths(&members);
        let worker_paths = member_paths.clone();
        let result = self
            .run(&key, member_paths, move |store, report| {
                let tool = tool(&settings)?;
                let mut members = reload(store, &worker_paths);
                // The disc ID, as the disc section computes it for a CUE album.
                if members.len() == 1
                    && members[0].has_cue_sheet()
                    && members[0].toc().is_none()
                    && let Ok(Some(toc)) = disc::compute_toc(&members[0], &tool)
                {
                    members[0].set_toc(Some(&toc));
                    let _ = store.save(&members[0]);
                }
                let progress = |p: identifykit::IdentifyProgress| report(Activity::new(p.message, Some(p.fraction)));
                identify::identify(store, &mut members, &positions, &tool, &settings.keys, false, &progress)
                    .map(|_| ())
                    .map_err(|e| format!("{e:#}"))
            })
            .await;
        if let Some(Err(e)) = result {
            self.set_error(&key, Some(e));
        }
    }

    /// The user's pick (None clears it); every member follows.
    pub fn choose_candidate(&self, record: &AlbumRecord, candidate: Option<&ScoredCandidate>) {
        let leader = self.leader_path(record);
        for mut m in self.members(record) {
            m.selected_candidate_id = candidate.map(|c| c.id());
            m.album_only = false;
            if candidate.is_some() && m.state == AlbumState::Scanned {
                m.state = AlbumState::NeedsReview;
            }
            let _ = self.store.borrow().save(&m);
            self.refresh(&m.path);
        }
        // A plan for another release is stale.
        self.set_plan(&leader, None);
    }

    /// Tag the album without any edition's details.
    pub fn choose_album_only(&self, record: &AlbumRecord, candidate: &ScoredCandidate) {
        let leader = self.leader_path(record);
        for mut m in self.members(record) {
            m.selected_candidate_id = Some(candidate.id());
            m.album_only = true;
            let _ = self.store.borrow().save(&m);
            self.refresh(&m.path);
        }
        self.set_plan(&leader, None);
    }

    // MARK: Disc

    pub async fn compute_toc(self: &Rc<Self>, record: &AlbumRecord) {
        let path = record.path.clone();
        let key = job(&path, Job::Disc);
        if self.is_busy(&key) {
            return;
        }
        let settings = self.core_settings();
        let p = path.clone();
        let result = self
            .run(&key, vec![path.clone()], move |store, report| {
                report(Activity::new(tr("Reading disc layout…"), None));
                let tool = tool(&settings)?;
                let mut r = store.fetch(&p).ok().flatten().ok_or_else(String::new)?;
                let toc = disc::compute_toc(&r, &tool).map_err(|e| format!("{e:#}"))?;
                r.set_toc(toc.as_ref());
                store.save(&r).map_err(|e| e.to_string())
            })
            .await;
        if let Some(Err(e)) = result
            && !e.is_empty()
        {
            self.set_error(&key, Some(e));
        }
    }

    pub async fn check_ctdb(self: &Rc<Self>, record: &AlbumRecord) {
        let path = record.path.clone();
        let key = job(&path, Job::Disc);
        if self.is_busy(&key) {
            return;
        }
        self.set_error(&key, None);
        let settings = self.core_settings();
        let p = path.clone();
        let result = self
            .run(&key, vec![path.clone()], move |store, report| {
                report(Activity::new(tr("Asking CUETools DB…"), None));
                let tool = tool(&settings)?;
                let mut r = store.fetch(&p).ok().flatten().ok_or_else(String::new)?;
                identify::check_ctdb(store, &mut r, &tool).map_err(|e| format!("{e:#}"))
            })
            .await;
        if let Some(Err(e)) = result
            && !e.is_empty()
        {
            self.set_error(&key, Some(e));
        }
    }

    /// The library root, asking once when it is not set.
    pub async fn destination(self: &Rc<Self>, parent: &gtk::Widget, message: &str) -> Option<PathBuf> {
        if let Some(root) = self.core_settings().library_root {
            return Some(root);
        }
        let dialog = gtk::FileDialog::builder().title(message).modal(true).build();
        let window = parent.root().and_downcast::<gtk::Window>();
        let folder = dialog.select_folder_future(window.as_ref()).await.ok()?.path()?;
        let _ = self.settings.set_string("library-root", &folder.to_string_lossy());
        Some(folder)
    }

    /// Splits a CUE image into the library; a fresh split has CRCs, so the
    /// CUETools DB verdict is asked again.
    pub async fn split(self: &Rc<Self>, record: &AlbumRecord, destination: PathBuf, overwrite: bool) -> bool {
        let path = record.path.clone();
        let key = job(&path, Job::Disc);
        if self.is_busy(&key) {
            return false;
        }
        self.set_error(&key, None);
        let settings = self.core_settings();
        let p = path.clone();
        let result = self
            .run(&key, vec![path.clone()], move |store, report| {
                let tool = tool(&settings)?;
                let mut r = store.fetch(&p).ok().flatten().ok_or_else(String::new)?;
                report(Activity::new(tr("Preparing…"), Some(0.0)));
                let outcomes = disc::split(
                    store,
                    &mut r,
                    &destination,
                    &tool,
                    &settings.split_options(overwrite),
                    settings.move_originals_to_trash,
                    &mut |message, fraction| report(Activity::new(tr(message), Some(fraction))),
                )
                .map_err(|e| format!("{e:#}"))?;
                if outcomes.first().and_then(|o| o.ctdb_track_crc32s()).is_some() {
                    report(Activity::new(tr("Asking CUETools DB…"), None));
                    let _ = identify::check_ctdb(store, &mut r, &tool);
                }
                Ok::<(), String>(())
            })
            .await;
        match result {
            Some(Ok(())) => true,
            Some(Err(e)) => {
                if !e.is_empty() {
                    self.set_error(&key, Some(e));
                }
                false
            }
            None => false,
        }
    }

    pub async fn extract(self: &Rc<Self>, record: &AlbumRecord, destination: PathBuf, overwrite: bool) -> bool {
        let path = record.path.clone();
        let key = job(&path, Job::Disc);
        if self.is_busy(&key) {
            return false;
        }
        self.set_error(&key, None);
        let settings = self.core_settings();
        let p = path.clone();
        let result = self
            .run(&key, vec![path.clone()], move |store, report| {
                let mut r = store.fetch(&p).ok().flatten().ok_or_else(String::new)?;
                let job = disc::SacdJob {
                    multichannel: settings.extract_multichannel,
                    trash_originals: settings.move_originals_to_trash,
                };
                disc::extract_sacd(
                    store,
                    &mut r,
                    &destination,
                    &settings.extract_options(overwrite),
                    job,
                    None,
                    &mut |message, fraction| report(Activity::new(message.to_string(), Some(fraction))),
                )
                .map(|_| ())
                .map_err(|e| format!("{e:#}"))
            })
            .await;
        match result {
            Some(Ok(())) => true,
            Some(Err(e)) => {
                if !e.is_empty() {
                    self.set_error(&key, Some(e));
                }
                false
            }
            None => false,
        }
    }

    /// Extracts or splits every member that still needs files. False when
    /// the folder choice was cancelled or a member ended in error. Files
    /// left by an earlier session are replaced: the output is identical
    /// and verified again.
    pub async fn produce_files(self: &Rc<Self>, record: &AlbumRecord, parent: &gtk::Widget) -> bool {
        let pending: Vec<AlbumRecord> = self.members(record).into_iter().filter(tagging::needs_files).collect();
        if pending.is_empty() {
            return true;
        }
        let Some(root) = self.destination(parent, &tr("Choose the library folder where the tracks are written.")).await
        else {
            return false;
        };
        for m in pending {
            let ok = if m.kind == librarykit::AlbumKind::SacdIso {
                self.extract(&m, root.clone(), true).await
            } else {
                self.split(&m, root.clone(), true).await
            };
            let after = self.record(&m.path);
            if !ok || after.as_ref().is_none_or(|r| r.state == AlbumState::Error || tagging::needs_files(r)) {
                return false;
            }
        }
        true
    }

    // MARK: Tags

    pub async fn build_plan(self: &Rc<Self>, record: &AlbumRecord) {
        let members = self.members(record);
        let leader = self.leader_path(record);
        let key = job(&leader, Job::Tags);
        if self.is_busy(&key) {
            return;
        }
        self.set_error(&key, None);
        let settings = self.core_settings();
        let tagger = self.tagger.clone();
        let member_paths = paths(&members);
        let worker_paths = member_paths.clone();
        let result = self
            .run(&key, member_paths, move |store, report| {
                let members = reload(store, &worker_paths);
                tagger.build_plan(&members, &settings, false, report)
            })
            .await;
        match result {
            Some(Ok(plan)) => self.set_plan(&leader, Some(plan)),
            Some(Err(e)) => self.set_error(&key, Some(e)),
            None => {}
        }
    }

    /// None keeps the files' own pictures.
    pub async fn choose_cover(self: &Rc<Self>, record: &AlbumRecord, option_id: Option<String>) {
        let leader = self.leader_path(record);
        let key = job(&leader, Job::Tags);
        let Some(mut plan) = self.plan(&leader) else { return };
        let settings = self.core_settings();
        let tagger = self.tagger.clone();
        let result = self
            .run(&key, vec![], move |_, report| {
                report(Activity::new(tr("Downloading artwork…"), None));
                tagger.choose_cover(&mut plan, option_id.as_deref(), &settings);
                plan
            })
            .await;
        if let Some(plan) = result {
            self.set_plan(&leader, Some(plan));
        }
    }

    pub fn toggle_lock(&self, record: &AlbumRecord, name: &str) {
        let key = self.leader_path(record);
        let mut members = self.members(record);
        let mut plan = self.plan(&key);
        tagging::toggle_lock(&self.store.borrow(), &mut members, name, plan.as_mut());
        for m in &members {
            self.refresh(&m.path);
        }
        self.set_plan(&key, plan);
    }

    /// Whether the cover becomes the folder's cover file only.
    pub fn set_embed_cover(&self, record: &AlbumRecord, embed: bool) {
        let key = self.leader_path(record);
        if let Some(mut plan) = self.plan(&key) {
            plan.embed_cover = embed;
            self.set_plan(&key, Some(plan));
        }
    }

    pub async fn apply(self: &Rc<Self>, record: &AlbumRecord, organize: Option<bool>) {
        let leader = self.leader_path(record);
        let key = job(&leader, Job::Tags);
        let Some(plan) = self.plan(&leader) else { return };
        if self.is_busy(&key) {
            return;
        }
        self.set_error(&key, None);
        let settings = self.core_settings();
        let tagger = self.tagger.clone();
        let member_paths = paths(&self.members(record));
        let worker_paths = member_paths.clone();
        // Taken now: a folder that moves drops out of the list on refresh.
        let selected = self.selection.borrow().clone();
        let result = self
            .run(&key, member_paths.clone(), move |store, report| {
                let mut members = reload(store, &worker_paths);
                let outcome = tagger.apply(store, &mut members, &plan, &settings, organize, report);
                (outcome, paths(&members))
            })
            .await;
        if let Some((outcome, new_paths)) = result {
            self.set_plan(&leader, None);
            if let Some(first) = outcome.problems.first() {
                self.set_error(&key, Some(first.clone()));
            }
            // A loose folder that moved as a whole has a new path.
            if new_paths != member_paths {
                self.reload();
                if let Some(i) = selected.and_then(|s| member_paths.iter().position(|p| *p == s)) {
                    self.select(new_paths.get(i).cloned());
                }
            }
        }
    }

    /// Moves the album into the library layout with the tags it has.
    pub async fn organize_only(self: &Rc<Self>, record: &AlbumRecord) {
        let leader = self.leader_path(record);
        let key = job(&leader, Job::Tags);
        let Some(plan) = self.plan(&leader) else { return };
        let settings = self.core_settings();
        let tagger = self.tagger.clone();
        let member_paths = paths(&self.members(record));
        let worker_paths = member_paths.clone();
        let selected = self.selection.borrow().clone();
        let result = self
            .run(&key, member_paths.clone(), move |store, report| {
                report(Activity::new(tr("Organizing files…"), None));
                let mut members = reload(store, &worker_paths);
                tagger.organize_only(store, &mut members, &plan, &settings).map(|()| paths(&members))
            })
            .await;
        match result {
            Some(Ok(new_paths)) => {
                self.set_plan(&leader, None);
                self.reload();
                if let Some(i) = selected.and_then(|s| member_paths.iter().position(|p| *p == s)) {
                    self.select(new_paths.get(i).cloned());
                }
            }
            Some(Err(e)) => self.set_error(&key, Some(e)),
            None => {}
        }
    }

    pub async fn restore(self: &Rc<Self>, record: &AlbumRecord) {
        let leader = self.leader_path(record);
        let key = job(&leader, Job::Tags);
        if self.is_busy(&key) {
            return;
        }
        self.set_error(&key, None);
        let tagger = self.tagger.clone();
        let member_paths = paths(&self.members(record));
        let worker_paths = member_paths.clone();
        let result = self
            .run(&key, member_paths, move |store, report| {
                report(Activity::new(tr("Restoring original tags…"), None));
                let mut members = reload(store, &worker_paths);
                tagger.restore(store, &mut members)
            })
            .await;
        self.set_plan(&leader, None);
        if let Some(Err(e)) = result {
            self.set_error(&key, Some(e));
        }
    }

    /// The plan of an album's release set, by any member.
    pub fn plan_for(&self, record: &AlbumRecord) -> Option<TagPlan> {
        self.plan(&self.leader_path(record))
    }

    /// Opens the folder that holds `path` in the file manager.
    pub fn reveal(&self, parent: &gtk::Widget, path: &Path) {
        let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(path)));
        let window = parent.root().and_downcast::<gtk::Window>();
        launcher.open_containing_folder(window.as_ref(), gio::Cancellable::NONE, |_| {});
    }

    /// Re-renders listeners of one album without touching its record.
    pub fn poke(&self, path: &Path) {
        self.emit(Event::Record(path.to_path_buf()));
    }
}

/// Runs an async action from a button without holding the button.
pub fn spawn(f: impl std::future::Future<Output = ()> + 'static) {
    glib::spawn_future_local(f);
}
