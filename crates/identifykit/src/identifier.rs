//! The whole identification of one album (or set): signals, candidate pool
//! from every source, fingerprints, details, Discogs, scoring.

use crate::collector::{CollectorOptions, SetMember, SignalCollector};
use crate::fingerprint::{FingerprintOutcome, FingerprintService};
use crate::scoring::*;
use crate::signals::{AlbumSignals, LocalFormat, SignalOrigin};
use librarykit::DetectedAlbum;
use librarykit::DiscTOC;
use providerkit::*;
use serde::{Deserialize, Serialize};
use splitkit::FFmpegTool;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone)]
pub struct IdentifierConfig {
    pub user_agent: String,
    pub acoust_id_key: Option<String>,
    pub discogs_token: Option<String>,
    pub scan_artwork: bool,
    pub use_fingerprints: bool,
    pub max_detail_fetches: usize,
    /// Batches over a tagged library: when the files' release ID fits them,
    /// only that release is checked (no searches, no fingerprints).
    pub fast_path_when_tagged: bool,
    /// A barcode that names one edition fitting the tracks decides it.
    pub fast_path_with_barcode: bool,
    pub max_candidates: usize,
    /// Reads codes off the artwork (vision phase).
    pub artwork_scanner: Option<Arc<dyn crate::collector::ArtworkScanning>>,
}

impl IdentifierConfig {
    pub fn new(user_agent: &str) -> IdentifierConfig {
        IdentifierConfig {
            user_agent: user_agent.into(),
            acoust_id_key: None,
            discogs_token: None,
            scan_artwork: true,
            use_fingerprints: true,
            max_detail_fetches: 12,
            fast_path_when_tagged: false,
            fast_path_with_barcode: false,
            max_candidates: 25,
            artwork_scanner: None,
        }
    }
}

/// One client per service and credential for the whole process, so the
/// rate limits hold when several albums are identified at once.
pub struct ProviderClients {
    pub music_brainz: MusicBrainzClient,
    pub discogs: Option<DiscogsClient>,
    pub acoust_id: Option<AcoustIDClient>,
}

impl ProviderClients {
    pub fn shared(user_agent: &str, acoust_id_key: Option<&str>, discogs_token: Option<&str>) -> Arc<ProviderClients> {
        static CACHE: OnceLock<Mutex<HashMap<String, Arc<ProviderClients>>>> = OnceLock::new();
        let key = [user_agent, acoust_id_key.unwrap_or(""), discogs_token.unwrap_or("")].join("\u{1}");
        let mut cache = CACHE.get_or_init(Default::default).lock().unwrap();
        cache
            .entry(key)
            .or_insert_with(|| {
                Arc::new(ProviderClients {
                    music_brainz: MusicBrainzClient::new(user_agent),
                    discogs: discogs_token.filter(|t| !t.is_empty()).map(|t| DiscogsClient::new(user_agent, t)),
                    acoust_id: acoust_id_key.filter(|k| !k.is_empty()).map(|k| AcoustIDClient::new(k, user_agent)),
                })
            })
            .clone()
    }
}

/// Seconds since 2001-01-01, as Foundation's `Date` encodes.
pub fn reference_date_now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64()) - 978_307_200.0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentificationResult {
    pub signals: AlbumSignals,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprints: Option<FingerprintOutcome>,
    /// Best first.
    pub candidates: Vec<ScoredCandidate>,
    /// For the credits merge later.
    pub discogs_candidates: Vec<Candidate>,
    pub log: Vec<String>,
    /// Seconds since 2001-01-01.
    pub started_at: f64,
    pub elapsed_seconds: f64,
}

impl IdentificationResult {
    pub fn best(&self) -> Option<&ScoredCandidate> {
        self.candidates.first()
    }

    pub fn is_confident(&self) -> bool {
        let Some(best) = self.best() else { return false };
        if best.confidence != MatchConfidence::Confident {
            return false;
        }
        // A confident pick must stand clear of the runner-up.
        !self
            .runner_up()
            .is_some_and(|second| second.confidence == MatchConfidence::Confident && second.score >= best.score - 5.0)
    }

    /// The best candidate's closest rival. The same pressing entered twice
    /// (one barcode or catalog number) is not a rival.
    pub fn runner_up(&self) -> Option<&ScoredCandidate> {
        self.rival(self.best()?)
    }

    pub fn rival(&self, pick: &ScoredCandidate) -> Option<&ScoredCandidate> {
        self.candidates.iter().find(|c| c.id() != pick.id() && !MatchScorer::are_twins(&c.candidate, &pick.candidate))
    }

    /// Fingerprints alone, but unmistakable: every track matches, count and
    /// durations fit, nothing within 30 points, no local code against it.
    pub fn is_clear_fingerprint_match(&self) -> bool {
        let Some(best) = self.best() else { return false };
        if best.fingerprint_coverage.unwrap_or(0.0) < 0.999
            || self.fingerprints.as_ref().map(|f| f.fingerprinted_tracks) != Some(self.signals.track_count())
            || best.track_count_matches != Some(true)
            || best.duration_fit.unwrap_or(0.0) < 0.75
            || MatchScorer::contradicts_local_codes(&best.candidate, &self.signals)
        {
            return false;
        }
        best.score - self.runner_up().map_or(0.0, |r| r.score) >= 30.0
    }

    /// The album's barcode names the best candidate, which fits the files,
    /// and every other edition with it is the same pressing; anything else
    /// stands 30 points below.
    pub fn is_clear_barcode_match(&self) -> bool {
        let Some(best) = self.best() else { return false };
        if best.track_count_matches != Some(true)
            || best.duration_fit.unwrap_or(0.0) < 0.75
            || best.format_compatibility == Some(FormatCompatibility::Unlikely)
        {
            return false;
        }
        let Some(code) = &best.candidate.barcode else { return false };
        if !self.signals.unique_barcodes().iter().any(|b| MatchScorer::barcodes_equal(b, code))
            || MatchScorer::contradicts_local_codes(&best.candidate, &self.signals)
        {
            return false;
        }
        self.candidates
            .iter()
            .skip(1)
            .all(|c| MatchScorer::are_twins(&c.candidate, &best.candidate) || best.score - c.score >= 30.0)
    }

    /// Editions whose audio is proven on every track but that tie on the
    /// rest: the album's own cover can still tell them apart.
    pub fn cover_contenders(&self) -> Vec<&ScoredCandidate> {
        let Some(best) = self.best() else { return vec![] };
        if self.fingerprints.as_ref().map_or(0, |f| f.fingerprinted_tracks) != self.signals.track_count()
            || self.signals.track_count() == 0
        {
            return vec![];
        }
        let close: Vec<&ScoredCandidate> = self.candidates.iter().filter(|c| best.score - c.score < 30.0).collect();
        if !close.iter().any(|c| Self::proves_audio(c, &self.signals)) || close.len() <= 1 {
            return vec![];
        }
        close.into_iter().take(6).collect()
    }

    /// Points for a cover that looks like the album's own.
    pub fn cover_points(distance: Option<f32>) -> f64 {
        match distance {
            Some(d) if d < 0.35 => 30.0,
            Some(d) if d < 0.6 => 15.0,
            _ => 0.0,
        }
    }

    /// The contender that, with its cover's points, stands 30 points clear
    /// of every other edition; None when the covers do not decide.
    pub fn cover_tie_break(&self, distances: &HashMap<String, f32>) -> Option<&ScoredCandidate> {
        let contenders = self.cover_contenders();
        if contenders.is_empty() {
            return None;
        }
        let total = |c: &ScoredCandidate| c.score + Self::cover_points(distances.get(&c.id()).copied());
        // The first of the best, as Swift's max(by:) picks.
        let mut pick = contenders[0];
        for c in &contenders[1..] {
            if total(c) > total(pick) {
                pick = c;
            }
        }
        if !Self::proves_audio(pick, &self.signals) {
            return None;
        }
        let rival_best = self
            .candidates
            .iter()
            .filter(|c| c.id() != pick.id() && !MatchScorer::are_twins(&c.candidate, &pick.candidate))
            .map(|c| if contenders.iter().any(|x| x.id() == c.id()) { total(c) } else { c.score })
            .fold(None::<f64>, |m, v| Some(m.map_or(v, |m| m.max(v))))
            .unwrap_or(0.0);
        if total(pick) - rival_best >= 30.0 { Some(pick) } else { None }
    }

    /// DSD files whose SACD no database lists: the album is still certain
    /// when every track's fingerprint matches and the editions close to the
    /// best are one release group.
    pub fn album_only_pick(&self) -> Option<&ScoredCandidate> {
        let s = &self.signals;
        if s.local_format() != LocalFormat::Sacd
            || s.track_count() == 0
            || self.fingerprints.as_ref().map(|f| f.fingerprinted_tracks) != Some(s.track_count())
            || self.candidates.iter().any(|c| MatchScorer::is_sacd_edition(&c.candidate))
        {
            return None;
        }
        let proven: Vec<&ScoredCandidate> = self
            .candidates
            .iter()
            .filter(|c| {
                c.fingerprint_coverage.unwrap_or(0.0) >= 0.999
                    && c.track_count_matches == Some(true)
                    && c.candidate.source == Source::Musicbrainz
                    && !MatchScorer::contradicts_local_codes(&c.candidate, s)
            })
            .collect();
        let pick = *proven.first()?;
        let group = pick.candidate.release_group_id.as_ref()?;
        let close: Vec<&ScoredCandidate> = proven.iter().copied().filter(|c| pick.score - c.score < 30.0).collect();
        if !close.iter().all(|c| c.candidate.release_group_id.as_ref() == Some(group)) {
            return None;
        }
        // CD and digital editions number their tracks 1…n, vinyl by side.
        close
            .iter()
            .copied()
            .find(|c| {
                c.candidate
                    .media
                    .iter()
                    .filter_map(|m| m.format.clone())
                    .chain(std::iter::once(c.candidate.media_format.clone().unwrap_or_default()))
                    .any(|f| {
                        let f = f.to_lowercase();
                        f.contains("cd") || f.contains("digital")
                    })
            })
            .or(Some(pick))
    }

    pub fn proves_audio(c: &ScoredCandidate, signals: &AlbumSignals) -> bool {
        c.fingerprint_coverage.unwrap_or(0.0) >= 0.999
            && c.track_count_matches == Some(true)
            && c.duration_fit.unwrap_or(0.0) >= 0.75
            && c.confidence >= MatchConfidence::Likely
            && !MatchScorer::contradicts_local_codes(&c.candidate, signals)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct IdentifyProgress {
    pub fraction: f64,
    pub message: String,
}

/// The candidate pool keyed by MBID (fingerprint votes use it) or
/// "discogs:<id>", remembering how each one got in.
#[derive(Default)]
struct Pool {
    candidates: HashMap<String, Candidate>,
    origins: HashMap<String, BTreeSet<CandidateOrigin>>,
    order: Vec<String>,
}

impl Pool {
    fn key(c: &Candidate) -> String {
        if c.source == Source::Musicbrainz { c.provider_id.clone() } else { c.id() }
    }

    fn add(&mut self, c: Candidate, origin: CandidateOrigin) {
        if c.source != Source::Musicbrainz && c.source != Source::Discogs {
            return;
        }
        let k = Self::key(&c);
        match self.candidates.get(&k) {
            None => {
                self.order.push(k.clone());
                self.candidates.insert(k.clone(), c);
            }
            Some(existing) if existing.all_tracks_is_empty() && !c.all_tracks_is_empty() => {
                self.candidates.insert(k.clone(), c);
            }
            _ => {}
        }
        self.origins.entry(k).or_default().insert(origin);
    }

    fn origins_of(&self, id: &str) -> Vec<CandidateOrigin> {
        self.origins.get(id).map(|o| o.iter().copied().collect()).unwrap_or_default()
    }

    fn has_origin(&self, id: &str, origin: CandidateOrigin) -> bool {
        self.origins.get(id).is_some_and(|o| o.contains(&origin))
    }

    fn get(&self, id: &str) -> &Candidate {
        &self.candidates[id]
    }
}

pub struct Identifier {
    pub config: IdentifierConfig,
    pub tool: FFmpegTool,
    clients: Arc<ProviderClients>,
}

impl Identifier {
    pub fn new(config: IdentifierConfig, tool: FFmpegTool) -> Identifier {
        let clients = ProviderClients::shared(
            &config.user_agent,
            config.acoust_id_key.as_deref(),
            config.discogs_token.as_deref(),
        );
        Identifier { config, tool, clients }
    }

    fn collector_options(&self) -> CollectorOptions {
        CollectorOptions {
            scan_artwork: self.config.scan_artwork,
            scanner: self.config.artwork_scanner.clone(),
            ..Default::default()
        }
    }

    pub fn identify(
        &self,
        album: &DetectedAlbum,
        toc: Option<&DiscTOC>,
        ctdb: &[CtdbMetadata],
        extracted_tracks: &[PathBuf],
        progress: &(dyn Fn(IdentifyProgress) + Sync),
    ) -> IdentificationResult {
        let started = (reference_date_now(), Instant::now());
        let lines = Mutex::new(vec![]);
        let log = |s: String| lines.lock().unwrap().push(s);
        progress(IdentifyProgress { fraction: 0.05, message: "Reading signals".into() });
        let signals = SignalCollector::new(&self.tool).collect(
            album,
            toc,
            ctdb,
            extracted_tracks,
            &self.collector_options(),
            &log,
        );
        self.pipeline(signals, started, &lines, progress)
    }

    /// Several albums that are one release, matched medium by medium.
    pub fn identify_set(
        &self,
        members: &[SetMember],
        declared_total: Option<i64>,
        ctdb: &[CtdbMetadata],
        progress: &(dyn Fn(IdentifyProgress) + Sync),
    ) -> IdentificationResult {
        let started = (reference_date_now(), Instant::now());
        let lines = Mutex::new(vec![]);
        let log = |s: String| lines.lock().unwrap().push(s);
        progress(IdentifyProgress { fraction: 0.05, message: format!("Reading signals of {} discs", members.len()) });
        let signals = SignalCollector::new(&self.tool).collect_set(
            members,
            declared_total,
            ctdb,
            &self.collector_options(),
            &log,
        );
        self.pipeline(signals, started, &lines, progress)
    }

    fn pipeline(
        &self,
        signals: AlbumSignals,
        started: (f64, Instant),
        lines: &Mutex<Vec<String>>,
        progress: &(dyn Fn(IdentifyProgress) + Sync),
    ) -> IdentificationResult {
        let log = |s: String| lines.lock().unwrap().push(s);
        let mb = &self.clients.music_brainz;
        let set_note = if signals.is_set() {
            format!(
                ", discs {} of {}",
                signals.present_discs.iter().map(|d| d.to_string()).collect::<Vec<_>>().join(","),
                signals.disc_count
            )
        } else {
            String::new()
        };
        log(format!(
            "Signals: {} barcode(s), {} catalog number(s), {} MBID(s), disc ID {}, {} track(s){set_note}.",
            signals.unique_barcodes().len(),
            signals.unique_catalog_numbers().len(),
            signals.unique_release_ids().len(),
            signals.disc_id.as_deref().unwrap_or("none"),
            signals.track_count()
        ));

        let mut pool = Pool::default();

        // 1. MBIDs from tags / CUETools DB.
        for id in &signals.mb_release_ids {
            if let Ok(c) = mb.release_detail(&id.value) {
                let origin =
                    if id.origin == SignalOrigin::Tags { CandidateOrigin::TagMbid } else { CandidateOrigin::CtdbMbid };
                pool.add(c, origin);
            }
        }

        // The files name a release that fits them: in fast mode that
        // release is checked and nothing is searched.
        let tagged_fit = pool.order.iter().any(|id| {
            if !pool.has_origin(id, CandidateOrigin::TagMbid) {
                return false;
            }
            let c = pool.get(id);
            let s = MatchScorer::score(c, &signals, &[CandidateOrigin::TagMbid], None, 0);
            s.track_count_matches == Some(true)
                && s.duration_fit.unwrap_or(1.0) >= 0.8
                && !MatchScorer::contradicts_local_codes(c, &signals)
        });
        if !tagged_fit
            && pool.order.iter().any(|id| pool.has_origin(id, CandidateOrigin::TagMbid))
            && (!signals.unique_barcodes().is_empty() || !signals.unique_catalog_numbers().is_empty())
        {
            log("The release ID in the tags does not match the barcode / catalog number found; searching every edition."
                .into());
        }
        let fast = self.config.fast_path_when_tagged && tagged_fit;
        let mut coded_fit = false;
        let mut detail_fetches = 0;
        let mut fingerprints: Option<FingerprintOutcome> = None;
        let mut discogs_candidates: Vec<Candidate> = vec![];
        let vote_for = |fp: &Option<FingerprintOutcome>, id: &str| {
            fp.as_ref().and_then(|f| f.votes.iter().find(|v| v.release_id == id).cloned())
        };
        let fingerprinted = |fp: &Option<FingerprintOutcome>| fp.as_ref().map_or(0, |f| f.fingerprinted_tracks);

        if fast {
            log("Fast path: the release ID in the tags fits the tracks; no other release searched.".into());
        } else {
            // 2. Disc ID / TOC, one lookup per local disc.
            let mut lookups: Vec<(i64, String, Option<String>)> =
                signals.disc_ids.iter().map(|(p, id)| (*p, id.clone(), signals.toc_strings.get(p).cloned())).collect();
            if lookups.is_empty()
                && let Some(id) = &signals.disc_id
            {
                lookups = vec![(1, id.clone(), signals.toc_string.clone())];
            }
            let many = lookups.len() > 1;
            for (position, disc_id, toc) in lookups.iter().take(4) {
                progress(IdentifyProgress {
                    fraction: 0.2,
                    message: if many { format!("Looking up disc {position} ID") } else { "Looking up disc ID".into() },
                });
                if let Ok(releases) = mb.lookup_disc_id(disc_id, toc.as_deref()) {
                    let n = releases.len();
                    for r in releases {
                        let origin = if r.all_disc_ids().contains(disc_id) {
                            CandidateOrigin::DiscId
                        } else {
                            CandidateOrigin::TocMatch
                        };
                        pool.add(r, origin);
                    }
                    log(format!("MusicBrainz TOC lookup (disc {position}): {n} release(s)."));
                }
            }

            // 3. Barcodes and catalog numbers.
            progress(IdentifyProgress { fraction: 0.3, message: "Searching by barcode and catalog number".into() });
            for barcode in signals.unique_barcodes().iter().take(3) {
                match mb.search_by_barcode(barcode, 10) {
                    Ok(rs) => {
                        let n = rs.len();
                        for r in rs {
                            pool.add(r, CandidateOrigin::Barcode);
                        }
                        log(format!("Barcode {barcode}: {n} MusicBrainz release(s)."));
                    }
                    Err(e) => log(format!("Barcode {barcode}: search failed ({e}).")),
                }
            }
            let local_count = signals.track_count() as i64;
            for catno in signals.unique_catalog_numbers().iter().take(4) {
                match mb.search_by_catalog(catno, 8) {
                    Ok(rs) => {
                        let n = rs.len();
                        let plausible: Vec<Candidate> = rs
                            .into_iter()
                            .filter(|r| {
                                signals.is_set()
                                    || local_count == 0
                                    || r.track_count.is_none()
                                    || r.plausible_track_counts().contains(&local_count)
                            })
                            .collect();
                        let p = plausible.len();
                        for r in plausible {
                            pool.add(r, CandidateOrigin::CatalogNumber);
                        }
                        log(format!("Catalog {catno}: {n} release(s), {p} with a plausible track count."));
                    }
                    Err(e) => log(format!("Catalog {catno}: search failed ({e}).")),
                }
            }

            // 3b. One edition carries the album's barcode and fits the
            // tracks: the edition is decided.
            if self.config.fast_path_with_barcode && !signals.unique_barcodes().is_empty() {
                let hits: Vec<String> =
                    pool.order.iter().filter(|id| pool.has_origin(id, CandidateOrigin::Barcode)).cloned().collect();
                for id in hits.iter().take(4) {
                    let c = pool.get(id);
                    if c.source == Source::Musicbrainz
                        && c.all_tracks_is_empty()
                        && let Ok(full) = mb.release_detail(id)
                    {
                        pool.candidates.insert(id.clone(), full);
                        detail_fetches += 1;
                    }
                }
                let fitting: Vec<&String> = hits
                    .iter()
                    .filter(|id| {
                        let c = pool.get(id);
                        let s = MatchScorer::score(c, &signals, &pool.origins_of(id), None, 0);
                        s.track_count_matches == Some(true)
                            && s.duration_fit.unwrap_or(0.0) >= 0.75
                            && s.format_compatibility != Some(FormatCompatibility::Unlikely)
                            && !MatchScorer::contradicts_local_codes(c, &signals)
                    })
                    .collect();
                if let Some(first) = fitting.first()
                    && fitting.iter().all(|id| MatchScorer::are_twins(pool.get(id), pool.get(first)))
                {
                    coded_fit = true;
                    log("Fast path: the barcode names one edition that fits the tracks; fingerprints and text searches skipped."
                        .into());
                } else if hits.len() > 1 {
                    log(format!(
                        "The barcode names {} fitting edition(s) of {}: no fast path.",
                        fitting.len(),
                        hits.len()
                    ));
                }
            }

            // 4. Fingerprints, unless the files' own release ID or the
            // barcode already fit them.
            let acoust_id = self.clients.acoust_id.as_ref();
            if (tagged_fit || coded_fit) && self.config.use_fingerprints && acoust_id.is_some() {
                log(if tagged_fit {
                    "Fingerprints skipped: the release ID in the tags fits the tracks.".into()
                } else {
                    "Fingerprints skipped: the barcode decided the edition.".into()
                });
            } else if self.config.use_fingerprints
                && let Some(acoust_id) = acoust_id
            {
                progress(IdentifyProgress { fraction: 0.45, message: "Fingerprinting tracks".into() });
                let outcome = FingerprintService::new(&self.tool, acoust_id).run(&signals.tracks, &log);
                let mut groups: Vec<String> = vec![];
                for vote in outcome.votes.iter().take(8) {
                    if pool.candidates.contains_key(&vote.release_id) {
                        let c = pool.get(&vote.release_id).clone();
                        pool.origins.entry(c.provider_id.clone()).or_default().insert(CandidateOrigin::Fingerprint);
                        if let Some(g) = c.release_group_id
                            && !groups.contains(&g)
                        {
                            groups.push(g);
                        }
                    } else if let Ok(c) = mb.release_detail(&vote.release_id) {
                        let g = c.release_group_id.clone();
                        pool.add(c, CandidateOrigin::Fingerprint);
                        if let Some(g) = g
                            && !groups.contains(&g)
                        {
                            groups.push(g);
                        }
                    }
                }
                // Sibling editions of the strongest groups: the fingerprint
                // knows the recording, not the pressing.
                for g in groups.iter().take(2) {
                    if let Ok(siblings) = mb.releases_in_group(g, 100) {
                        for s in siblings {
                            pool.add(s, CandidateOrigin::ReleaseGroup);
                        }
                    }
                }
                fingerprints = Some(outcome);
            } else if self.config.use_fingerprints {
                log("Fingerprints skipped: no AcoustID key configured.".into());
            }

            // 5. Text search: cheap, and the only way to editions the strong
            // signals missed.
            if !coded_fit && let Some(title) = signals.album_hint.as_ref().filter(|t| !t.is_empty()) {
                progress(IdentifyProgress { fraction: 0.7, message: "Searching by title".into() });
                let artist = signals.artist_hint.clone().unwrap_or_default();
                // With an edition in the name, ask for it first.
                if let Some(edition) = &signals.edition_hint {
                    match mb.search_releases(&artist, &format!("{title} {edition}"), None, 10) {
                        Ok(rs) => {
                            let n = rs.len();
                            for r in rs {
                                pool.add(r, CandidateOrigin::TextSearch);
                            }
                            log(format!("Text search \"{title} {edition}\": {n} release(s)."));
                        }
                        Err(e) => log(format!("Text search with edition failed: {e}")),
                    }
                }
                let limit = if pool.candidates.len() < 3 { 15 } else { 8 };
                let result = mb.search_releases(&artist, title, None, limit).and_then(|rs| {
                    let n = rs.len();
                    for r in rs {
                        pool.add(r, CandidateOrigin::TextSearch);
                    }
                    log(format!("Text search \"{title}\": {n} release(s)."));
                    // Rippers sometimes leave another edition's title (a box
                    // set's disc title): try the folder's title.
                    if n == 0
                        && let Some(folder_title) = signals.folder_album_hint.as_ref().filter(|t| !t.is_empty())
                        && MatchScorer::normalize(folder_title) != MatchScorer::normalize(title)
                    {
                        let more = mb.search_releases(&artist, folder_title, None, 10)?;
                        let m = more.len();
                        for r in more {
                            pool.add(r, CandidateOrigin::TextSearch);
                        }
                        log(format!("Text search \"{folder_title}\" (folder name): {m} release(s)."));
                    }
                    Ok(())
                });
                if let Err(e) = result {
                    log(format!("Text search failed: {e}"));
                }
            }

            // DSD files come from a SACD (or a DSD download): when none of
            // the editions found is one, ask for SACDs by the name
            // MusicBrainz gives the album; the artist stays the files' own.
            let canonical = pool
                .order
                .iter()
                .find(|id| pool.has_origin(id, CandidateOrigin::Fingerprint))
                .map(|id| pool.get(id).clone());
            let sacd_artist = signals
                .artist_hint
                .clone()
                .or_else(|| canonical.as_ref().map(|c| c.artist.clone()))
                .unwrap_or_default();
            // A SACD image carries its own title in the master TOC: better
            // than the name of the file ("iso.iso").
            let disc_title = if signals.is_sacd_image { signals.album_hint.clone() } else { None };
            let sacd_title = canonical
                .as_ref()
                .map(|c| c.title.clone())
                .or(disc_title)
                .or_else(|| signals.folder_album_hint.clone())
                .or_else(|| signals.album_hint.clone())
                .unwrap_or_default();
            let needs_sacd = |pool: &Pool| {
                signals.local_format() == LocalFormat::Sacd
                    && !sacd_title.is_empty()
                    && !pool.order.iter().any(|id| MatchScorer::is_sacd_edition(pool.get(id)))
            };
            if needs_sacd(&pool) {
                let rs = mb.search_releases(&sacd_artist, &sacd_title, Some("SACD"), 10).unwrap_or_default();
                let n = rs.len();
                for r in rs {
                    pool.add(r, CandidateOrigin::TextSearch);
                }
                log(format!("SACD search \"{sacd_title}\": {n} release(s)."));
            }

            // 6. Details for candidates that arrived without tracks, best first.
            progress(IdentifyProgress { fraction: 0.8, message: "Fetching release details".into() });
            let mut provisional: Vec<(String, f64)> = pool
                .order
                .iter()
                .map(|id| {
                    let s = MatchScorer::score(
                        pool.get(id),
                        &signals,
                        &pool.origins_of(id),
                        vote_for(&fingerprints, id).as_ref(),
                        fingerprinted(&fingerprints),
                    );
                    (id.clone(), s.score)
                })
                .collect();
            provisional.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
            for (id, _) in &provisional {
                let c = pool.get(id);
                if c.source == Source::Musicbrainz
                    && c.all_tracks_is_empty()
                    && detail_fetches < self.config.max_detail_fetches
                    && let Ok(full) = mb.release_detail(id)
                {
                    pool.candidates.insert(id.clone(), full);
                    detail_fetches += 1;
                }
            }

            // 7. Discogs: editions MusicBrainz lacks (many Japanese
            // pressings). Barcode / catalog hits are fetched and scored; all
            // feed the credits merge.
            if let Some(discogs) = &self.clients.discogs {
                progress(IdentifyProgress { fraction: 0.88, message: "Searching Discogs".into() });
                let mut hits: Vec<(Candidate, CandidateOrigin)> = vec![];
                for barcode in signals.unique_barcodes().iter().take(2) {
                    if let Ok(rs) = discogs.search_by_barcode(barcode, 10) {
                        hits.extend(rs.into_iter().map(|r| (r, CandidateOrigin::Barcode)));
                    }
                }
                for catno in signals.unique_catalog_numbers().iter().take(2) {
                    if hits.len() >= 6 {
                        continue;
                    }
                    if let Ok(rs) = discogs.search_by_catalog(catno, 10) {
                        hits.extend(rs.into_iter().map(|r| (r, CandidateOrigin::CatalogNumber)));
                    }
                }
                let mut fetched: HashSet<String> = HashSet::new();
                for (hit, origin) in &hits {
                    if fetched.len() < 4
                        && fetched.insert(hit.provider_id.clone())
                        && let Ok(full) = discogs.release_detail(&hit.provider_id)
                    {
                        discogs_candidates.push(full.clone());
                        pool.add(full, *origin);
                    }
                }
                if !hits.is_empty() {
                    log(format!(
                        "Discogs: {} hit(s) by barcode or catalog, {} fetched and scored.",
                        hits.len(),
                        fetched.len()
                    ));
                }
                // Still no SACD edition: Discogs knows many MusicBrainz lacks.
                if needs_sacd(&pool) {
                    let rs = discogs.search_releases(&sacd_artist, &sacd_title, Some("SACD"), 10).unwrap_or_default();
                    let mut added = 0;
                    for hit in &rs {
                        if added < 3
                            && fetched.insert(hit.provider_id.clone())
                            && let Ok(full) = discogs.release_detail(&hit.provider_id)
                        {
                            discogs_candidates.push(full.clone());
                            pool.add(full, CandidateOrigin::TextSearch);
                            added += 1;
                        }
                    }
                    log(format!(
                        "Discogs SACD search \"{sacd_title}\": {} release(s), {added} fetched and scored.",
                        rs.len()
                    ));
                }
            }
        }

        // 8. Final scoring.
        progress(IdentifyProgress { fraction: 0.95, message: "Scoring".into() });
        let mut scored: Vec<ScoredCandidate> = pool
            .order
            .iter()
            .map(|id| {
                let mut origins = pool.origins_of(id);
                origins.sort_by_key(|o| o.raw());
                MatchScorer::score(
                    pool.get(id),
                    &signals,
                    &origins,
                    vote_for(&fingerprints, id).as_ref(),
                    fingerprinted(&fingerprints),
                )
            })
            .collect();
        scored.sort_by(|a, b| (b.score, b.confidence).partial_cmp(&(a.score, a.confidence)).unwrap());
        scored.truncate(self.config.max_candidates);
        match scored.first() {
            Some(best) => log(format!(
                "Best: {} – {} ({}, {}) score {} {}.",
                best.candidate.artist,
                best.candidate.title,
                best.candidate.year.as_deref().unwrap_or("?"),
                best.candidate.country.as_deref().unwrap_or("?"),
                best.score as i64,
                best.confidence.raw()
            )),
            None => log("No candidates found.".into()),
        }
        progress(IdentifyProgress { fraction: 1.0, message: "Done".into() });

        IdentificationResult {
            signals,
            fingerprints,
            candidates: scored,
            discogs_candidates,
            log: lines.lock().unwrap().clone(),
            started_at: started.0,
            elapsed_seconds: started.1.elapsed().as_secs_f64(),
        }
    }
}
