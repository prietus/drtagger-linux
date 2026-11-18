//! Tag plans from the chosen release, writing them with backups and audio
//! verification, the library layout, and restoring the originals (the Mac
//! app's TagService). A release set passes all its members; the first is
//! the leader whose identification and choices count.

use crate::i18n::{tr, trf};
use crate::identify::USER_AGENT;
use crate::settings::Settings;
use crate::store::{AlbumRecord, AlbumState, Store, now_ms};
use identifykit::{ArtworkOption, CoverArtFetcher, FetchedArtwork, LocalFormat, MatchScorer};
use librarykit::{AlbumKind, DetectedAlbum};
use providerkit::{Candidate, CandidateMedium, DiscogsClient, MusicBrainzClient, Source};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;
use tagkit::{
    ArtworkProcessor, ChangeKind, LibraryOrganizer, PicardMapper, Picture, PictureInfo, PictureKind, TagBackup,
    TagChange, TagContainer, TagContext, TagMerge, TagSet, TagWriteReport, TaggedFile, field,
};

/// What is running now, for the progress line.
#[derive(Clone, Debug, PartialEq)]
pub struct Activity {
    pub message: String,
    pub fraction: Option<f64>,
}

impl Activity {
    pub fn new(message: String, fraction: Option<f64>) -> Activity {
        Activity { message, fraction }
    }
}

/// One file of the album with what it has, what the release says and the merge.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackPlan {
    pub path: PathBuf,
    /// 0-based album track index.
    pub index: usize,
    /// 1-based disc position on the release.
    pub disc: i64,
    /// The record that owns the file.
    pub owner: PathBuf,
    pub existing: TagSet,
    pub proposed: TagSet,
    pub result: TagSet,
    pub changes: Vec<TagChange>,
    pub existing_pictures: Vec<PictureInfo>,
    pub audio_digest: String,
}

impl TrackPlan {
    pub fn changed_fields(&self) -> Vec<&TagChange> {
        self.changes.iter().filter(|c| c.kind() != ChangeKind::Unchanged).collect()
    }
}

#[derive(Clone, Debug)]
pub struct TagPlan {
    pub candidate_id: String,
    /// Every file already carries this release's ID: nothing to decide.
    pub already_tagged: bool,
    pub tracks: Vec<TrackPlan>,
    pub artwork_options: Vec<ArtworkOption>,
    /// None: keep the pictures the files have.
    pub cover_option_id: Option<String>,
    pub cover: Option<FetchedArtwork>,
    /// False: `cover` only becomes the folder's cover file.
    pub embed_cover: bool,
    pub warnings: Vec<String>,
    pub log: Vec<String>,
}

impl TagPlan {
    /// Fields that change the same way on every track: shown once.
    pub fn album_changes(&self) -> Vec<TagChange> {
        let Some(first) = self.tracks.first() else { return vec![] };
        first
            .changes
            .iter()
            .filter(|c| self.tracks.iter().all(|t| t.changes.iter().find(|x| x.name == c.name) == Some(*c)))
            .cloned()
            .collect()
    }

    pub fn track_only_changes(&self, track: &TrackPlan) -> Vec<TagChange> {
        let shared: HashSet<String> = self.album_changes().into_iter().map(|c| c.name).collect();
        track.changes.iter().filter(|c| !shared.contains(&c.name)).cloned().collect()
    }

    pub fn changed_track_count(&self) -> usize {
        self.tracks.iter().filter(|t| !t.changed_fields().is_empty()).count()
    }

    pub fn changed_field_count(&self) -> usize {
        self.tracks
            .iter()
            .flat_map(|t| t.changed_fields().into_iter().map(|c| c.name.clone()))
            .collect::<HashSet<_>>()
            .len()
    }

    pub fn has_work(&self) -> bool {
        self.changed_track_count() > 0 || self.cover.is_some()
    }

    /// The merge again after the locks changed.
    pub fn relock(&mut self, locks: &BTreeSet<String>, album_only: bool) {
        let locks: HashSet<String> = locks.iter().cloned().collect();
        let kept = kept_fields(album_only);
        for t in &mut self.tracks {
            (t.result, t.changes) = TagMerge::merge(&t.existing, &t.proposed, &locks, &kept);
        }
    }
}

fn kept_fields(album_only: bool) -> HashSet<String> {
    if album_only { field::EDITION_FIELDS.iter().map(|f| f.to_string()).collect() } else { HashSet::new() }
}

/// A file that receives tags: where it maps on the release and who owns it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub path: PathBuf,
    pub disc: i64,
    pub track: usize,
    pub owner: PathBuf,
}

/// The release the tags come from: the user's pick, else a confident best.
pub fn chosen_candidate(record: &AlbumRecord) -> Option<Candidate> {
    if let Some(c) = record.selected_candidate() {
        return Some(c.candidate);
    }
    let r = record.identification()?;
    if r.is_confident() { r.best().map(|b| b.candidate.clone()) } else { None }
}

/// Records whose tags need files that do not exist yet.
pub fn needs_files(r: &AlbumRecord) -> bool {
    match r.kind {
        AlbumKind::SacdIso => r.sacd_outcomes().is_empty(),
        AlbumKind::CueImage => r.split_outcomes().is_empty(),
        AlbumKind::CueMultiFile | AlbumKind::TrackFolder => false,
    }
}

/// Files that receive the tags, each with the disc position and track
/// index it maps to on the release; Err says why there are none yet.
pub fn target_files(members: &[AlbumRecord]) -> Result<Vec<Target>, String> {
    let mut out = vec![];
    for m in members {
        if m.state == AlbumState::Applying {
            return Err(trf(
                "Disc {} is still being extracted or split; wait for it to finish.",
                &[&m.disc_position()],
            ));
        }
        let position = m.disc_position();
        match m.kind {
            AlbumKind::SacdIso => {
                let outcomes = m.sacd_outcomes();
                if outcomes.is_empty() {
                    return Err(tr("Extract the SACD first; the DSF tracks receive the tags."));
                }
                for o in &outcomes {
                    for t in &o.tracks {
                        out.push(Target {
                            path: m.resolved(&t.url),
                            disc: position,
                            track: (t.number - 1).max(0) as usize,
                            owner: m.path.clone(),
                        });
                    }
                }
            }
            AlbumKind::CueImage => {
                let outcomes = m.split_outcomes();
                if outcomes.is_empty() {
                    return Err(tr("Split the image first; the FLAC tracks receive the tags."));
                }
                let detected = m.detected();
                let disc_numbers: Vec<i64> = match &detected {
                    Some(d) if d.discs.len() > 1 => {
                        let mut n: Vec<i64> = d.discs.iter().map(|d| d.number as i64).collect();
                        n.sort();
                        n
                    }
                    _ => vec![position],
                };
                for (i, o) in outcomes.iter().enumerate() {
                    let disc = disc_numbers.get(i).copied().unwrap_or(i as i64 + 1);
                    for t in o.tracks.iter().filter(|t| t.number > 0) {
                        out.push(Target {
                            path: m.resolved(&t.url),
                            disc,
                            track: (t.number - 1) as usize,
                            owner: m.path.clone(),
                        });
                    }
                }
            }
            AlbumKind::CueMultiFile | AlbumKind::TrackFolder => {
                let Some(detected) = m.detected() else { continue };
                let mut discs: Vec<_> = detected.discs.iter().collect();
                discs.sort_by_key(|d| d.number);
                let several = discs.len() > 1;
                for disc in discs {
                    let p = if several { disc.number as i64 } else { position };
                    for (i, f) in disc.track_files.iter().enumerate() {
                        out.push(Target { path: m.resolved(&f.url), disc: p, track: i, owner: m.path.clone() });
                    }
                }
            }
        }
    }
    Ok(out)
}

/// folder.jpg / cover.jpg / front.jpg next to the tracks.
pub fn folder_cover(album: Option<&DetectedAlbum>) -> Option<Vec<u8>> {
    let album = album?;
    let names = ["folder", "cover", "front"];
    let file = album.artwork_files.iter().find(|p| {
        let stem = p.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
        names.contains(&stem.as_str()) && p.parent() == Some(album.url.as_path())
    })?;
    std::fs::read(file).ok()
}

/// The album's own cover: the first file's embedded front, else the folder image.
pub fn local_cover(members: &[AlbumRecord]) -> Option<Vec<u8>> {
    let first = target_files(members).ok().and_then(|t| t.into_iter().next());
    first
        .and_then(|t| TaggedFile::read_for_preview(&t.path).ok())
        .and_then(|c| c.pictures.into_iter().find(|p| p.kind == PictureKind::FRONT).map(|p| p.data))
        .or_else(|| folder_cover(members.first().and_then(|m| m.detected()).as_ref()))
}

/// A folder album moved into the library: its scan details again, from
/// the new folder, so identifying it later reads the files where they are
/// (the Mac app keeps the old paths; see DESIGN.md, Mac bugs). Titles and
/// hints stay as they were.
fn refresh_details(m: &mut AlbumRecord) {
    let result = librarykit::LibraryScanner::new().scan(std::slice::from_ref(&m.path));
    if let [album] = result.albums.as_slice()
        && album.id() == m.path
        && album.kind == m.kind
    {
        m.details_json = serde_json::to_string(album).ok();
        m.issues_json = serde_json::to_string(&crate::store::issues_for(&result, album)).ok();
    }
}

/// Builds plans and keeps what is worth not fetching twice: release
/// details and cover distances.
#[derive(Default)]
pub struct Tagger {
    details: Mutex<HashMap<String, Candidate>>,
    cover_distances: Mutex<HashMap<String, f32>>,
}

/// What an apply did.
#[derive(Clone, Debug, Default)]
pub struct ApplyOutcome {
    pub problems: Vec<String>,
    /// Seconds per phase: loudness, write, organize.
    pub timings: Vec<(String, f64)>,
}

impl Tagger {
    pub fn new() -> Tagger {
        Tagger::default()
    }

    /// Candidates found by search carry no tracklist: fetch it for the chosen one.
    fn with_tracklist(&self, candidate: Candidate, settings: &Settings) -> Option<Candidate> {
        if !candidate.all_tracks_is_empty() {
            return Some(candidate);
        }
        if let Some(c) = self.details.lock().unwrap().get(&candidate.id()) {
            return Some(c.clone());
        }
        let full = match candidate.source {
            Source::Musicbrainz => MusicBrainzClient::new(USER_AGENT).release_detail(&candidate.provider_id).ok(),
            Source::Discogs => settings
                .discogs_token()
                .and_then(|t| DiscogsClient::new(USER_AGENT, t).release_detail(&candidate.provider_id).ok()),
            _ => None,
        }?;
        if full.all_tracks_is_empty() {
            return None;
        }
        self.details.lock().unwrap().insert(candidate.id(), full.clone());
        Some(full)
    }

    /// The plan for the leader's chosen release over every member's files.
    /// `bulk`: a batch over many albums. A cover the files have stays, a
    /// missing one is never embedded, and artwork is looked up only when
    /// the folder has no cover file to leave beside the tracks.
    pub fn build_plan(
        &self,
        members: &[AlbumRecord],
        settings: &Settings,
        bulk: bool,
        activity: &dyn Fn(Activity),
    ) -> Result<TagPlan, String> {
        let leader = members.first().ok_or_else(|| tr("No audio files to tag."))?;
        let candidate = chosen_candidate(leader).ok_or_else(|| tr("Choose a release in Identification first."))?;
        let targets = target_files(members)?;
        if targets.is_empty() {
            return Err(tr("No audio files to tag."));
        }
        activity(Activity::new(tr("Reading tags…"), None));
        let candidate = if candidate.all_tracks_is_empty() {
            activity(Activity::new(tr("Fetching release details…"), None));
            let full = self.with_tracklist(candidate, settings).ok_or_else(|| {
                tr("The tracklist of this release could not be downloaded; try again or choose another release.")
            })?;
            activity(Activity::new(tr("Reading tags…"), None));
            full
        } else {
            candidate
        };

        let identification = leader.identification();
        let local_format = identification.as_ref().map_or(LocalFormat::Unknown, |i| i.signals.local_format());
        let media = MatchScorer::relevant_media(&candidate, local_format);
        let media_list = if media.is_empty() {
            vec![CandidateMedium::new(1, None, None, candidate.tracks.clone(), vec![], None)]
        } else {
            media.clone()
        };
        let toc = leader.toc();
        let mut context = TagContext {
            discogs: identification.as_ref().and_then(|i| i.discogs_candidates.first().cloned()),
            musicbrainz_disc_id: toc.as_ref().map(|t| t.musicbrainz_disc_id()),
            freedb_disc_id: toc.as_ref().map(|t| t.freedb_disc_id()),
            write_genres: settings.write_genres_from_discogs,
            acoust_ids: identification
                .as_ref()
                .and_then(|i| i.fingerprints.as_ref())
                .map(|f| f.acoust_ids.clone())
                .unwrap_or_default(),
            ..Default::default()
        };
        if leader.album_only {
            context.album_only = true;
            context.media_override = (local_format == LocalFormat::Sacd).then(|| "SACD".to_string());
        }
        let locks: HashSet<String> = leader.tag_locks().into_iter().collect();
        let kept = kept_fields(leader.album_only);
        let mut warnings = vec![];
        // Per disc: what the release lists against what the files cover.
        let local_discs: BTreeSet<i64> = targets.iter().map(|t| t.disc).collect();
        for &disc in &local_discs {
            let local = targets.iter().filter(|t| t.disc == disc).count();
            if disc as usize > media_list.len() {
                warnings.push(trf(
                    "Disc {}: the release has only {} disc(s); its files are left untouched.",
                    &[&disc, &media_list.len()],
                ));
            } else if disc >= 1 && media_list[disc as usize - 1].tracks.len() != local {
                warnings.push(trf(
                    "Disc {}: the release lists {} tracks, the files are {}; tags are mapped by position.",
                    &[&disc, &media_list[disc as usize - 1].tracks.len(), &local],
                ));
            }
        }
        if media_list.len() > 1 {
            let missing: Vec<String> =
                (1..=media_list.len() as i64).filter(|d| !local_discs.contains(d)).map(|d| d.to_string()).collect();
            if !missing.is_empty() {
                warnings.push(trf("Discs not present: {} of {}.", &[&missing.join(", "), &media_list.len()]));
            }
        }

        let mut tracks = vec![];
        let mut local_cover: Option<Vec<u8>> = None;
        for t in &targets {
            match TaggedFile::read_for_preview(&t.path) {
                Err(e) => warnings.push(format!("{}: {e}", file_name(&t.path))),
                Ok(contents) => {
                    if local_cover.is_none() {
                        local_cover =
                            contents.pictures.iter().find(|p| p.kind == PictureKind::FRONT).map(|p| p.data.clone());
                    }
                    let Some(proposed) =
                        PicardMapper::track_tags_by_disc(&candidate, &media, t.disc as usize, t.track, &context)
                    else {
                        continue;
                    };
                    let (result, changes) = TagMerge::merge(&contents.tags, &proposed, &locks, &kept);
                    tracks.push(TrackPlan {
                        path: t.path.clone(),
                        index: tracks.len(),
                        disc: t.disc,
                        owner: t.owner.clone(),
                        existing: contents.tags,
                        proposed,
                        result,
                        changes,
                        existing_pictures: contents.pictures.iter().map(PictureInfo::from).collect(),
                        audio_digest: contents.audio_digest,
                    });
                }
            }
        }
        if tracks.is_empty() {
            return Err(warnings.into_iter().next().unwrap_or_else(|| tr("No file could be read.")));
        }

        let id_field =
            if candidate.source == Source::Musicbrainz { field::MB_ALBUM_ID } else { field::DISCOGS_RELEASE_ID };
        let already_tagged = tracks
            .iter()
            .all(|t| t.existing.first(id_field).map(str::to_lowercase) == Some(candidate.provider_id.to_lowercase()));
        let mut plan = TagPlan {
            candidate_id: candidate.id(),
            already_tagged,
            tracks,
            artwork_options: vec![],
            cover_option_id: None,
            cover: None,
            embed_cover: !bulk,
            warnings,
            log: vec![],
        };
        // Files that already have a front cover keep it unless one is chosen.
        let keep_cover = leader.cover_option_id.is_none()
            && plan.tracks.iter().all(|t| t.existing_pictures.iter().any(|p| p.kind == PictureKind::FRONT));
        let detected = leader.detected();
        let wants_artwork = if bulk { folder_cover(detected.as_ref()).is_none() } else { settings.embed_front_cover };
        if wants_artwork {
            activity(Activity::new(tr("Looking for artwork…"), None));
            let log = Mutex::new(vec![]);
            let fetcher = CoverArtFetcher::new(USER_AGENT, settings.fanart_key());
            // Batches leave a cover the files already have alone and do not
            // ask four services for alternatives nobody will look at.
            let options = if keep_cover && bulk {
                vec![]
            } else {
                fetcher
                    .options(&candidate, context.discogs.as_ref(), detected.as_ref(), &|l| log.lock().unwrap().push(l))
            };
            let preferred =
                leader.cover_option_id.as_ref().and_then(|id| options.iter().find(|o| &o.id() == id)).cloned();
            if keep_cover {
                log.lock().unwrap().push(
                    "Artwork: the files already have a front cover; it is kept unless you choose another.".into(),
                );
            }
            if !keep_cover {
                for option in preferred.iter().chain(options.iter()) {
                    if let Some(art) = fetcher.fetch(option) {
                        log.lock().unwrap().push(format!("Artwork: {}, {}×{}.", option.label, art.width, art.height));
                        plan.cover_option_id = Some(option.id());
                        plan.cover = Some(art);
                        break;
                    }
                }
            }
            plan.artwork_options = options;
            plan.log = log.into_inner().unwrap();
        }
        if let Some(local) = local_cover.or_else(|| folder_cover(detected.as_ref())) {
            let key = format!("{}\u{1}{}", leader.path.display(), candidate.id());
            let cached = self.cover_distances.lock().unwrap().get(&key).copied();
            let distance = cached.unwrap_or_else(|| {
                activity(Activity::new(tr("Comparing covers…"), None));
                let check = visionkit::CoverCheck::new(USER_AGENT, settings.discogs_token());
                let d = check.distance(&local, &candidate).unwrap_or(-1.0);
                self.cover_distances.lock().unwrap().insert(key, d);
                d
            });
            if distance >= visionkit::CoverCheck::DIFFERENT_THRESHOLD {
                plan.warnings.push(tr(
                    "The album's cover does not look like this release's: check that it is the same edition.",
                ));
            }
        }
        Ok(plan)
    }

    /// None keeps the files' own pictures.
    pub fn choose_cover(&self, plan: &mut TagPlan, option_id: Option<&str>, settings: &Settings) {
        let Some(option) = option_id.and_then(|id| plan.artwork_options.iter().find(|o| o.id() == id)).cloned() else {
            plan.cover = None;
            plan.cover_option_id = None;
            return;
        };
        match CoverArtFetcher::new(USER_AGENT, settings.fanart_key()).fetch(&option) {
            Some(art) => {
                plan.cover = Some(art);
                plan.cover_option_id = Some(option.id());
            }
            None => plan.warnings.push(trf("{}: could not be downloaded.", &[&option.label])),
        }
    }

    /// Writes the plan into every member's files: ReplayGain measured first
    /// so it lands in the same write, originals backed up, the audio
    /// verified untouched, the cover file left beside the tracks, then the
    /// library layout. States, backups, reports and moves are stored.
    pub fn apply(
        &self,
        store: &Store,
        members: &mut [AlbumRecord],
        plan: &TagPlan,
        settings: &Settings,
        organize: Option<bool>,
        activity: &(dyn Fn(Activity) + Sync),
    ) -> ApplyOutcome {
        let leader_path = members[0].path.clone();
        let leader_locks = members[0].tag_locks();
        activity(Activity::new(tr("Writing tags…"), Some(0.0)));
        for m in members.iter_mut() {
            m.state = AlbumState::Applying;
            let _ = store.save(m);
        }
        let embed: Option<Picture> = if plan.embed_cover {
            plan.cover.as_ref().and_then(|c| {
                ArtworkProcessor::prepared(&c.data, PictureKind::FRONT, settings.embed_cover_max_pixels).ok()
            })
        } else {
            None
        };
        let cover_data =
            if settings.save_cover_file || !plan.embed_cover { plan.cover.as_ref().map(|c| &c.data) } else { None };
        let cover_extension = plan
            .cover
            .as_ref()
            .map_or("jpg", |c| if tagkit::mime_type(&c.data) == Some("image/png") { "png" } else { "jpg" });
        let mut timings = vec![];
        let mut phase_start = Instant::now();
        let mut phase = |name: &str, timings: &mut Vec<(String, f64)>| {
            timings.push((name.to_string(), phase_start.elapsed().as_secs_f64()));
            phase_start = Instant::now();
        };
        let mut work: Vec<TrackPlan> = plan.tracks.clone();
        let mut problems = vec![];

        // 1. Loudness; files that already carry ReplayGain keep what they have.
        let has_replay_gain = !work.is_empty()
            && work.iter().all(|t| {
                !t.existing.get("REPLAYGAIN_TRACK_GAIN").is_empty()
                    && !t.existing.get("REPLAYGAIN_ALBUM_GAIN").is_empty()
            });
        if settings.compute_replay_gain
            && !has_replay_gain
            && let Some(location) = crate::ffmpeg::locate(&settings.ffmpeg_override_path)
            && location.ffprobe.is_some()
        {
            activity(Activity::new(tr("Measuring loudness…"), Some(0.0)));
            let analyzer = loudnesskit::LoudnessAnalyzer::new(crate::ffmpeg::tool(&location));
            let paths: Vec<PathBuf> = work.iter().map(|t| t.path.clone()).collect();
            let progress = |done: usize, total: usize| {
                activity(Activity::new(tr("Measuring loudness…"), Some(done as f64 / total.max(1) as f64)))
            };
            match analyzer.analyze_album(&paths, &progress) {
                Ok(album) => {
                    for (i, t) in work.iter_mut().enumerate() {
                        let Some(track) = album.tracks.get(i) else { break };
                        let dsd =
                            matches!(TagContainer::from_path(&t.path), Some(TagContainer::Dsf | TagContainer::Dff));
                        for (name, value) in loudnesskit::replaygain::tags(track, Some(&album), dsd) {
                            if !leader_locks.contains(&name) {
                                t.result.set(&name, Some(&value));
                            }
                        }
                    }
                }
                Err(e) => problems.push(trf("Loudness measurement failed: {}", &[&e])),
            }
        }
        phase("loudness", &mut timings);

        // 2. Write, with backups of the originals.
        let total = work.len();
        let mut backups: Vec<TagBackup> = vec![];
        let mut reports: Vec<TagWriteReport> = vec![];
        let mut write_problems = vec![];
        for (i, t) in work.iter().enumerate() {
            let written = (|| -> Result<(TagBackup, TagWriteReport), tagkit::TagFileError> {
                let container = TaggedFile::container(&t.path)?;
                let contents = TaggedFile::read(&t.path)?;
                let backup = TagBackup {
                    path: t.path.clone(),
                    container,
                    raw_metadata: contents.raw_metadata,
                    audio_digest: contents.audio_digest,
                    tags: contents.tags,
                    pictures: contents.pictures.iter().map(PictureInfo::from).collect(),
                    created_at: now_ms() as f64 / 1000.0,
                };
                let pictures: Option<Vec<Picture>> =
                    embed.as_ref().map(|front| TaggedFile::replacing_front(&contents.pictures, front.clone()));
                let report =
                    TaggedFile::write(&t.path, &t.result, pictures.as_deref(), Some(backup.audio_digest.as_str()))?;
                Ok((backup, report))
            })();
            match written {
                Ok((b, r)) => {
                    backups.push(b);
                    reports.push(r);
                }
                Err(e) => write_problems.push(format!("{}: {e}", file_name(&t.path))),
            }
            activity(Activity::new(tr("Writing tags…"), Some((i + 1) as f64 / total.max(1) as f64)));
        }
        if let (Some(data), Some(first)) = (cover_data, work.first())
            && let Some(folder) = first.path.parent()
        {
            let exists = ["jpg", "jpeg", "png"].iter().any(|e| folder.join(format!("cover.{e}")).exists());
            if !exists {
                let _ = std::fs::write(folder.join(format!("cover.{cover_extension}")), data);
            }
        }
        problems.extend(write_problems.iter().cloned());
        phase("write", &mut timings);

        // Every file belongs to one member; backups, reports and moves stay with it.
        let owner_of: HashMap<PathBuf, PathBuf> = work.iter().map(|t| (t.path.clone(), t.owner.clone())).collect();
        let owner = |p: &Path| owner_of.get(p).cloned().unwrap_or_else(|| leader_path.clone());
        let mut backups_by_owner: HashMap<PathBuf, Vec<TagBackup>> = HashMap::new();
        for b in backups {
            backups_by_owner.entry(owner(&b.path)).or_default().push(b);
        }
        let mut reports_by_owner: HashMap<PathBuf, Vec<TagWriteReport>> = HashMap::new();
        for r in &reports {
            reports_by_owner.entry(owner(&r.path)).or_default().push(r.clone());
        }
        let mut new_paths: HashMap<PathBuf, PathBuf> = HashMap::new();

        // 3. Library layout from the final tags.
        if organize.unwrap_or(settings.organize_after_apply)
            && let Some(root) = &settings.library_root
            && write_problems.is_empty()
            && !reports.is_empty()
        {
            let files: Vec<(PathBuf, TagSet)> = work.iter().map(|t| (t.path.clone(), t.result.clone())).collect();
            let planned = LibraryOrganizer::plan(&files, root, &settings.organizer_options());
            if !planned.is_empty() {
                activity(Activity::new(tr("Organizing files…"), None));
                match LibraryOrganizer::perform(&planned) {
                    Ok(moved) => {
                        let map: HashMap<PathBuf, PathBuf> =
                            moved.iter().map(|m| (m.from.clone(), m.to.clone())).collect();
                        for m in members.iter_mut() {
                            let mut moves = m.file_moves();
                            for (from, to) in &map {
                                let mine = match owner_of.get(from) {
                                    Some(o) => o == &m.path,
                                    None => m.path == leader_path,
                                };
                                if mine {
                                    moves.insert(from.clone(), to.clone());
                                }
                            }
                            m.set_file_moves(&moves);
                        }
                        for list in backups_by_owner.values_mut() {
                            *list = list.iter().map(|b| map.get(&b.path).map_or(b.clone(), |to| b.moved(to))).collect();
                        }
                        for list in reports_by_owner.values_mut() {
                            *list = list.iter().map(|r| moved_report(r, &map)).collect();
                        }
                        reports = reports.iter().map(|r| moved_report(r, &map)).collect();
                        // A loose folder that moved as a whole is now the album's home.
                        for m in members.iter() {
                            if matches!(m.kind, AlbumKind::TrackFolder | AlbumKind::CueMultiFile) {
                                let folders: BTreeSet<PathBuf> = reports_by_owner
                                    .get(&m.path)
                                    .map(|l| l.iter().filter_map(|r| r.path.parent().map(Path::to_path_buf)).collect())
                                    .unwrap_or_default();
                                if folders.len() == 1
                                    && let Some(folder) = folders.into_iter().next()
                                    && folder != m.path
                                {
                                    new_paths.insert(m.path.clone(), folder);
                                }
                            }
                        }
                    }
                    Err(e) => problems.push(trf("Could not move files into the library: {}", &[&e])),
                }
            }
        }
        phase("organize", &mut timings);

        for m in members.iter_mut() {
            let old = m.path.clone();
            // The first backups are the true originals; later applies keep them.
            if m.tag_backups().is_empty() {
                m.set_tag_backups(backups_by_owner.get(&old).map(Vec::as_slice).unwrap_or(&[]));
            }
            m.set_tag_reports(reports_by_owner.get(&old).map(Vec::as_slice).unwrap_or(&[]));
            m.tagged_at = Some(now_ms());
            m.cover_option_id = plan.cover_option_id.clone();
            let reviewed = problems.is_empty() && m.batch_status.as_deref() == Some("review");
            if problems.is_empty() {
                m.state = AlbumState::Done;
                m.error_message = None;
            } else {
                m.state = if reports.is_empty() { AlbumState::Error } else { AlbumState::Done };
                m.error_message = Some(problems.join("\n"));
            }
            match new_paths.get(&old) {
                Some(new) => {
                    m.path = new.clone();
                    refresh_details(m);
                    let _ = store.save_moved(&old, m);
                }
                None => {
                    let _ = store.save(m);
                }
            }
            // Applied by hand after the batch set it aside: done.
            if reviewed {
                m.batch_status = Some("done".into());
                m.batch_note = None;
                let _ = store.set_batch(&m.path, Some("done"), None);
            }
        }
        ApplyOutcome { problems, timings }
    }

    /// Moves an album into the library layout using the tags it already
    /// has, without writing anything into the files.
    pub fn organize_only(
        &self,
        store: &Store,
        members: &mut [AlbumRecord],
        plan: &TagPlan,
        settings: &Settings,
    ) -> Result<(), String> {
        let Some(root) = &settings.library_root else { return Ok(()) };
        let files: Vec<(PathBuf, TagSet)> = plan.tracks.iter().map(|t| (t.path.clone(), t.existing.clone())).collect();
        let planned = LibraryOrganizer::plan(&files, root, &settings.organizer_options());
        if planned.is_empty() {
            return Ok(());
        }
        let owner_of: HashMap<PathBuf, PathBuf> =
            plan.tracks.iter().map(|t| (t.path.clone(), t.owner.clone())).collect();
        let moved =
            LibraryOrganizer::perform(&planned).map_err(|e| trf("Could not move files into the library: {}", &[&e]))?;
        let map: HashMap<PathBuf, PathBuf> = moved.iter().map(|m| (m.from.clone(), m.to.clone())).collect();
        for m in members.iter_mut() {
            let old = m.path.clone();
            let mut moves = m.file_moves();
            for (from, to) in &map {
                if owner_of.get(from).is_none_or(|o| o == &m.path) {
                    moves.insert(from.clone(), to.clone());
                }
            }
            m.set_file_moves(&moves);
            let backups: Vec<TagBackup> =
                m.tag_backups().iter().map(|b| map.get(&b.path).map_or(b.clone(), |to| b.moved(to))).collect();
            m.set_tag_backups(&backups);
            let reports: Vec<TagWriteReport> = m.tag_reports().iter().map(|r| moved_report(r, &map)).collect();
            m.set_tag_reports(&reports);
            let mut new_path = None;
            if matches!(m.kind, AlbumKind::TrackFolder | AlbumKind::CueMultiFile) {
                let folders: BTreeSet<PathBuf> = plan
                    .tracks
                    .iter()
                    .filter(|t| t.owner == old)
                    .filter_map(|t| map.get(&t.path).and_then(|p| p.parent().map(Path::to_path_buf)))
                    .collect();
                if folders.len() == 1 {
                    new_path = folders.into_iter().next();
                }
            }
            match new_path {
                Some(p) => {
                    m.path = p;
                    refresh_details(m);
                    store.save_moved(&old, m).map_err(|e| e.to_string())?;
                }
                None => store.save(m).map_err(|e| e.to_string())?,
            }
        }
        Ok(())
    }

    /// Puts back the original metadata of every file Apply wrote.
    pub fn restore(&self, store: &Store, members: &mut [AlbumRecord]) -> Result<(), String> {
        let backups: Vec<TagBackup> = members.iter().flat_map(|m| m.tag_backups()).collect();
        if backups.is_empty() {
            return Ok(());
        }
        let mut problems = vec![];
        for b in &backups {
            if let Err(e) = TaggedFile::restore(&b.path, b) {
                problems.push(format!("{}: {e}", file_name(&b.path)));
            }
        }
        if !problems.is_empty() {
            return Err(problems.join("\n"));
        }
        for m in members.iter_mut() {
            m.set_tag_backups(&[]);
            m.set_tag_reports(&[]);
            m.tagged_at = None;
            m.error_message = None;
            m.state = if m.selected_candidate_id.is_some() { AlbumState::NeedsReview } else { AlbumState::Scanned };
            store.save(m).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

/// Locks flip on every member; the plan follows.
pub fn toggle_lock(store: &Store, members: &mut [AlbumRecord], name: &str, plan: Option<&mut TagPlan>) {
    let mut locks = members[0].tag_locks();
    if !locks.remove(name) {
        locks.insert(name.to_string());
    }
    for m in members.iter_mut() {
        m.set_tag_locks(&locks);
        let _ = store.save(m);
    }
    if let Some(plan) = plan {
        plan.relock(&locks, members[0].album_only);
    }
}

fn moved_report(r: &TagWriteReport, map: &HashMap<PathBuf, PathBuf>) -> TagWriteReport {
    match map.get(&r.path) {
        Some(to) => TagWriteReport { path: to.clone(), ..r.clone() },
        None => r.clone(),
    }
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Tags of a plan keyed by file, for dumps and tests.
pub fn planned_tags(plan: &TagPlan) -> BTreeMap<PathBuf, Vec<(String, String)>> {
    plan.tracks.iter().map(|t| (t.path.clone(), t.result.pairs())).collect()
}
