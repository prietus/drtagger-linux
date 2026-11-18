//! Chromaprint + AcoustID over the album's tracks, aggregated into votes per
//! MusicBrainz release with drtagger's consensus filter: the real album is
//! the release most tracks hit; releases sharing a song or two are
//! compilations and are dropped.

use crate::signals::LocalTrack;
use providerkit::{AcoustIDClient, AcoustIDMatch};
use serde::{Deserialize, Serialize};
use splitkit::FFmpegTool;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Mutex;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseVote {
    #[serde(rename = "releaseID")]
    pub release_id: String,
    pub track_indices: Vec<usize>,
    pub best_score: f64,
}

impl ReleaseVote {
    pub fn hit_count(&self) -> usize {
        self.track_indices.len()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FingerprintOutcome {
    pub fingerprinted_tracks: usize,
    pub tracks_with_hits: usize,
    /// After the consensus filter, best first.
    pub votes: Vec<ReleaseVote>,
    /// Local track index → recording MBID of the best hit.
    #[serde(rename = "recordingIDs", default)]
    pub recording_ids: BTreeMap<usize, String>,
    /// Local track index → AcoustID id of the best hit.
    #[serde(rename = "acoustIDs", default)]
    pub acoust_ids: BTreeMap<usize, String>,
}

impl FingerprintOutcome {
    pub fn coverage(&self) -> f64 {
        match self.votes.first() {
            Some(best) if self.fingerprinted_tracks > 0 => best.hit_count() as f64 / self.fingerprinted_tracks as f64,
            _ => 0.0,
        }
    }
}

pub struct FingerprintService<'a> {
    pub tool: &'a FFmpegTool,
    pub acoust_id: &'a AcoustIDClient,
    pub seconds_per_track: f64,
    pub max_concurrent_decodes: usize,
}

struct TrackHit {
    index: usize,
    matches: Vec<AcoustIDMatch>,
}

impl<'a> FingerprintService<'a> {
    pub fn new(tool: &'a FFmpegTool, acoust_id: &'a AcoustIDClient) -> FingerprintService<'a> {
        FingerprintService { tool, acoust_id, seconds_per_track: 120.0, max_concurrent_decodes: 3 }
    }

    /// The fingerprint of one track's first `seconds_per_track` seconds.
    pub fn fingerprint(&self, track: &LocalTrack) -> Result<chromaprintkit::Fingerprint, String> {
        let url = track.url.as_ref().ok_or("no file")?;
        let (samples, rate, channels) = self
            .tool
            .decode_snippet(url, track.image_start.unwrap_or(0.0), self.seconds_per_track)
            .map_err(|e| e.to_string())?;
        if samples.is_empty() {
            return Err(String::new());
        }
        let seconds =
            track.duration_seconds.unwrap_or_else(|| (samples.len() as i64 / channels.max(1)) as f64 / rate as f64);
        let duration = (seconds.round() as i64).max(1);
        chromaprintkit::fingerprint(&samples, rate, channels, duration).map_err(|e| e.to_string())
    }

    pub fn run(&self, tracks: &[LocalTrack], log: &(dyn Fn(String) + Sync)) -> FingerprintOutcome {
        let all: Vec<&LocalTrack> = tracks.iter().filter(|t| t.url.is_some()).collect();
        // A SACD image is not something ffmpeg decodes: its tracks are
        // fingerprinted from the extracted DSFs, or not at all.
        let candidates: Vec<&LocalTrack> = all
            .iter()
            .copied()
            .filter(|t| {
                t.url.as_ref().and_then(|u| u.extension()).map(|e| e.to_string_lossy().to_lowercase()).as_deref()
                    != Some("iso")
            })
            .collect();
        if candidates.len() < all.len() {
            log(format!(
                "Fingerprints skipped for {} track(s) inside a SACD image: extract the disc to fingerprint it.",
                all.len() - candidates.len()
            ));
        }
        if candidates.is_empty() {
            return FingerprintOutcome::default();
        }

        let next = Mutex::new(0usize);
        let hits: Mutex<Vec<TrackHit>> = Mutex::new(vec![]);
        let fingerprinted = Mutex::new(0usize);
        std::thread::scope(|s| {
            for _ in 0..self.max_concurrent_decodes.max(1) {
                s.spawn(|| {
                    loop {
                        let track = {
                            let mut n = next.lock().unwrap();
                            let Some(t) = candidates.get(*n) else { break };
                            *n += 1;
                            *t
                        };
                        let result = self.fingerprint(track).and_then(|fp| {
                            self.acoust_id.lookup(&fp.fingerprint, fp.duration_seconds).map_err(|e| e.to_string())
                        });
                        match result {
                            Ok(matches) => {
                                *fingerprinted.lock().unwrap() += 1;
                                hits.lock().unwrap().push(TrackHit { index: track.index, matches });
                            }
                            Err(e) if e.is_empty() => {}
                            Err(e) => log(format!("Fingerprint failed for track {}: {e}", track.index + 1)),
                        }
                    }
                });
            }
        });
        let fingerprinted = fingerprinted.into_inner().unwrap();
        let mut hits = hits.into_inner().unwrap();
        hits.sort_by_key(|h| h.index);

        let mut scores: HashMap<String, f64> = HashMap::new();
        let mut track_hits: HashMap<String, BTreeSet<usize>> = HashMap::new();
        let mut recording_ids = BTreeMap::new();
        let mut acoust_ids = BTreeMap::new();
        let mut tracks_with_hits = 0;
        for hit in hits.iter().filter(|h| !h.matches.is_empty()) {
            tracks_with_hits += 1;
            // The first of the best, as Swift's max(by:) picks.
            let best = hit.matches.iter().fold(None::<&AcoustIDMatch>, |b, m| match b {
                Some(x) if m.score <= x.score => Some(x),
                _ => Some(m),
            });
            if let Some(best) = best {
                recording_ids.insert(hit.index, best.recording_id.clone());
                if let Some(id) = &best.acoust_id {
                    acoust_ids.insert(hit.index, id.clone());
                }
            }
            for m in &hit.matches {
                for r in &m.release_ids {
                    let s = scores.entry(r.clone()).or_insert(0.0);
                    *s = s.max(m.score);
                    track_hits.entry(r.clone()).or_default().insert(hit.index);
                }
            }
        }
        let votes = Self::consensus(&scores, &track_hits, fingerprinted);
        log(format!(
            "AcoustID: {tracks_with_hits} of {fingerprinted} tracks matched; {} release(s) after consensus.",
            votes.len()
        ));
        FingerprintOutcome { fingerprinted_tracks: fingerprinted, tracks_with_hits, votes, recording_ids, acoust_ids }
    }

    /// Keeps releases hit by at least half the maximum hit count (and at
    /// least two tracks) when the album has more than one track. Sorted by
    /// hits, then best score; ties by release ID (Swift leaves them in
    /// dictionary order, which changes from run to run).
    pub fn consensus(
        scores: &HashMap<String, f64>,
        track_hits: &HashMap<String, BTreeSet<usize>>,
        track_count: usize,
    ) -> Vec<ReleaseVote> {
        let max_hits = track_hits.values().map(|h| h.len()).max().unwrap_or(0);
        let eligible = track_count > 1 && max_hits >= 2;
        let min_hits = 2.max(max_hits.div_ceil(2));
        let mut votes: Vec<ReleaseVote> = scores
            .keys()
            .filter_map(|id| {
                let hits = track_hits.get(id).cloned().unwrap_or_default();
                if eligible && hits.len() < min_hits {
                    return None;
                }
                Some(ReleaseVote {
                    release_id: id.clone(),
                    track_indices: hits.into_iter().collect(),
                    best_score: scores[id],
                })
            })
            .collect();
        votes.sort_by(|a, b| {
            (b.hit_count(), b.best_score)
                .partial_cmp(&(a.hit_count(), a.best_score))
                .unwrap()
                .then_with(|| a.release_id.cmp(&b.release_id))
        });
        votes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consensus_filter_drops_compilations() {
        let scores = HashMap::from([("album".to_string(), 0.9), ("compilation".into(), 0.95), ("single".into(), 0.7)]);
        let hits = HashMap::from([
            ("album".to_string(), BTreeSet::from([0, 1, 2, 3, 4, 5, 6, 7])),
            ("compilation".into(), BTreeSet::from([2, 5])),
            ("single".into(), BTreeSet::from([0])),
        ]);
        let votes = FingerprintService::consensus(&scores, &hits, 8);
        assert_eq!(votes.iter().map(|v| v.release_id.as_str()).collect::<Vec<_>>(), ["album"]);
        let sparse = FingerprintService::consensus(
            &HashMap::from([("a".to_string(), 0.9), ("b".into(), 0.8)]),
            &HashMap::from([("a".to_string(), BTreeSet::from([0])), ("b".into(), BTreeSet::from([1]))]),
            2,
        );
        assert_eq!(sparse.len(), 2);
    }
}
