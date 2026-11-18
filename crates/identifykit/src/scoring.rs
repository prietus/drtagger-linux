//! How well a release candidate fits the local album.

use crate::catalog::CatalogNumberParser;
use crate::fingerprint::ReleaseVote;
use crate::signals::{AlbumSignals, LocalFormat, SignalOrigin};
use crate::text::{fold, is_alphanumeric};
use librarykit::SourceHint;
use providerkit::{Candidate, CandidateMedium, CandidateTrack};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MatchConfidence {
    Unlikely,
    Possible,
    Likely,
    Confident,
}

impl MatchConfidence {
    pub fn raw(self) -> &'static str {
        match self {
            MatchConfidence::Unlikely => "unlikely",
            MatchConfidence::Possible => "possible",
            MatchConfidence::Likely => "likely",
            MatchConfidence::Confident => "confident",
        }
    }
}

/// How a candidate reached the pool; strong origins count as independent
/// evidence when they agree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CandidateOrigin {
    #[serde(rename = "tagMBID")]
    TagMbid,
    #[serde(rename = "ctdbMBID")]
    CtdbMbid,
    #[serde(rename = "discID")]
    DiscId,
    TocMatch,
    Barcode,
    CatalogNumber,
    Fingerprint,
    ReleaseGroup,
    TextSearch,
}

impl CandidateOrigin {
    pub fn raw(self) -> &'static str {
        match self {
            CandidateOrigin::TagMbid => "tagMBID",
            CandidateOrigin::CtdbMbid => "ctdbMBID",
            CandidateOrigin::DiscId => "discID",
            CandidateOrigin::TocMatch => "tocMatch",
            CandidateOrigin::Barcode => "barcode",
            CandidateOrigin::CatalogNumber => "catalogNumber",
            CandidateOrigin::Fingerprint => "fingerprint",
            CandidateOrigin::ReleaseGroup => "releaseGroup",
            CandidateOrigin::TextSearch => "textSearch",
        }
    }

    pub fn is_strong(self) -> bool {
        !matches!(self, CandidateOrigin::CtdbMbid | CandidateOrigin::ReleaseGroup | CandidateOrigin::TextSearch)
    }
}

/// Whether a candidate's medium could be the origin of the local files.
/// `Unknown` covers releases listed without a format, which is common.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FormatCompatibility {
    Fits,
    Unknown,
    Unlikely,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoredCandidate {
    pub candidate: Candidate,
    pub score: f64,
    pub confidence: MatchConfidence,
    pub reasons: Vec<String>,
    pub origins: Vec<CandidateOrigin>,
    /// 0…1, None when unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_fit: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_count_matches: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint_coverage: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format_compatibility: Option<FormatCompatibility>,
}

impl ScoredCandidate {
    pub fn id(&self) -> String {
        self.candidate.id()
    }
}

pub struct MatchScorer;

/// A few seconds per track absorbs pregap conventions and the ±2 s
/// MusicBrainz commonly disagrees with rips by.
const PER_TRACK_TOLERANCE_SECONDS: f64 = 4.0;

impl MatchScorer {
    pub fn score(
        candidate: &Candidate,
        signals: &AlbumSignals,
        origins: &[CandidateOrigin],
        fingerprint_vote: Option<&ReleaseVote>,
        fingerprinted_tracks: usize,
    ) -> ScoredCandidate {
        let mut score = 0.0;
        let mut reasons: Vec<String> = vec![];
        let mut strong = 0;

        // Media: only the layer of a hybrid the files can come from, and in
        // a set only the media at the local disc positions.
        let media = Self::relevant_media(candidate, signals.local_format());
        let paired = Self::paired_media(&media, signals);
        let candidate_tracks: Vec<CandidateTrack> = if media.is_empty() {
            candidate.tracks.clone()
        } else {
            paired.iter().flat_map(|m| m.tracks.iter().cloned()).collect()
        };
        let missing_discs: Vec<i64> = if media.is_empty() {
            vec![]
        } else {
            signals.present_discs.iter().copied().filter(|p| *p > media.len() as i64).collect()
        };

        // Format plausibility.
        let mut plain_cd_for_dsd = false;
        let format_text = media
            .iter()
            .filter_map(|m| m.format.clone())
            .chain(std::iter::once(candidate.media_format.clone().unwrap_or_default()))
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        let has = |s: &str| format_text.contains(s);
        let is_sacd = has("sacd") || has("dsd");
        let is_vinyl = has("vinyl") || has("cassette") || has("shellac") || has("reel");
        let is_cd = has("cd") && !(has("sacd") && !has("cd layer") && !has("hybrid"));
        match signals.local_format() {
            LocalFormat::Sacd => {
                if is_sacd {
                    score += 15.0;
                    reasons.push("SACD edition".into());
                } else if is_cd {
                    score -= 20.0;
                    plain_cd_for_dsd = true;
                    reasons.push("A CD cannot be the source of DSD files".into());
                } else if is_vinyl || has("digital") || has("dvd") || has("blu-ray") {
                    score -= 20.0;
                    reasons.push(format!(
                        "Not a SACD ({})",
                        candidate.media_format.as_deref().unwrap_or("unknown format")
                    ));
                } else if !format_text.is_empty() {
                    score -= 5.0;
                }
            }
            LocalFormat::Cd => {
                if is_vinyl {
                    score -= 20.0;
                    reasons.push("Vinyl cannot be the source of a CD rip".into());
                } else if is_cd {
                    score += 8.0;
                    reasons.push("CD edition".into());
                } else if is_sacd {
                    score -= 10.0;
                    reasons.push("SACD without a CD layer".into());
                }
            }
            LocalFormat::HiRes | LocalFormat::Unknown => {}
        }

        // The medium named in the folder or tags only nudges.
        if let Some(hint) = signals.source_hint {
            let is_digital = has("digital");
            let is_dvd = has("dvd") || has("blu-ray");
            let local = signals.local_format();
            match hint {
                SourceHint::Vinyl => {
                    if is_vinyl {
                        score += 12.0;
                        reasons.push("Vinyl, as the folder says".into());
                    } else if local != LocalFormat::Cd && !format_text.is_empty() {
                        score -= 6.0;
                    }
                }
                SourceHint::Sacd => {
                    if is_sacd && local != LocalFormat::Sacd {
                        score += 10.0;
                        reasons.push("SACD, as the folder says".into());
                    }
                }
                SourceHint::Cd => {
                    if is_cd && local != LocalFormat::Cd {
                        score += 6.0;
                        reasons.push("CD, as the folder says".into());
                    }
                }
                SourceHint::Digital => {
                    if is_digital {
                        score += 8.0;
                        reasons.push("Digital release, as the folder says".into());
                    } else if is_vinyl || is_dvd {
                        score -= 6.0;
                    }
                }
                SourceHint::Dvd => {
                    if is_dvd {
                        score += 10.0;
                        reasons.push("DVD/Blu-ray, as the folder says".into());
                    }
                }
            }
        }

        // Track count.
        let local_count = signals.track_count() as i64;
        let candidate_count: Option<i64> = if !candidate_tracks.is_empty() {
            Some(candidate_tracks.len() as i64)
        } else if signals.is_set() {
            // Search results carry per-medium counts but no tracks.
            let counts: Vec<Option<i64>> = paired.iter().map(|m| m.track_count).collect();
            if !counts.is_empty() && counts.iter().all(|c| c.is_some()) {
                Some(counts.iter().flatten().sum())
            } else {
                None
            }
        } else {
            // Search results: per-medium counts without the video media.
            let counts: Vec<Option<i64>> = media.iter().map(|m| m.track_count).collect();
            if media.len() < candidate.media.len() && !counts.is_empty() && counts.iter().all(|c| c.is_some()) {
                Some(counts.iter().flatten().sum())
            } else {
                candidate.track_count
            }
        };
        let mut track_count_matches = None;
        if local_count > 0
            && let Some(cc) = candidate_count
        {
            track_count_matches = Some(cc == local_count);
            if cc == local_count {
                score += 10.0;
                reasons.push(format!("Track count {local_count} matches"));
            } else {
                score -= 40.0;
                reasons.push(format!("Track count differs ({cc} vs {local_count})"));
            }
        }
        // A set whose total is not declared may be part of a bigger box.
        if signals.disc_count > 1 && !media.is_empty() && (!signals.is_set() || signals.declared_disc_total.is_some()) {
            if media.len() as i64 == signals.disc_count {
                score += 5.0;
                reasons.push(format!("{} discs", signals.disc_count));
            } else {
                score -= 15.0;
                reasons.push(format!("Disc count differs ({} vs {})", media.len(), signals.disc_count));
            }
        }
        if !missing_discs.is_empty() {
            score -= 30.0;
            reasons.push(format!(
                "Release has no disc {}",
                missing_discs.iter().map(|d| d.to_string()).collect::<Vec<_>>().join(", ")
            ));
        }

        // Durations.
        let mut duration_fit = None;
        let local_durations: Vec<f64> = signals.tracks.iter().filter_map(|t| t.duration_seconds).collect();
        let cand_durations: Vec<Option<f64>> =
            candidate_tracks.iter().map(|t| t.duration_ms.map(|ms| ms as f64 / 1000.0)).collect();
        if local_durations.len() == signals.tracks.len()
            && cand_durations.len() == local_durations.len()
            && !local_durations.is_empty()
            && cand_durations.iter().all(|d| d.is_some())
        {
            let diffs: Vec<f64> =
                local_durations.iter().zip(cand_durations.iter().flatten()).map(|(a, b)| (a - b).abs()).collect();
            let mean = diffs.iter().sum::<f64>() / diffs.len() as f64;
            let fit = (1.0 - mean / (PER_TRACK_TOLERANCE_SECONDS * 2.0)).max(0.0);
            duration_fit = Some(fit);
            score += 30.0 * fit;
            reasons.push(format!("Durations differ by {mean:.1} s on average"));
            if mean > PER_TRACK_TOLERANCE_SECONDS * 3.0 {
                score -= 30.0;
            }
        }

        // Strong signals.
        let unique_barcodes = signals.unique_barcodes();
        if let Some(barcode) = candidate.barcode.as_deref().map(providerkit::digits).filter(|b| !b.is_empty())
            && unique_barcodes.iter().any(|b| Self::barcodes_equal(b, &barcode))
        {
            score += 40.0;
            strong += 1;
            reasons.push(format!("Barcode {barcode} matches"));
        }
        let cand_catalogs: Vec<String> =
            candidate.catalog_numbers().iter().map(|c| CatalogNumberParser::normalize(c)).collect();
        if let Some(hit) = signals.unique_catalog_numbers().into_iter().find(|local| {
            let n = CatalogNumberParser::normalize(local);
            !n.is_empty() && cand_catalogs.iter().any(|c| Self::catalogs_meet(c, &n))
        }) {
            score += 30.0;
            strong += 1;
            reasons.push(format!("Catalog number {hit} matches"));
        }
        // A SACD image names its own catalog number in the master TOC (not
        // the box's): when that exact number is a SACD edition's, the disc
        // has identified itself, as a disc ID would.
        if signals.is_sacd_image
            && Self::is_sacd_edition(candidate)
            && signals.catalog_numbers.iter().any(|v| {
                let n = CatalogNumberParser::normalize(&v.value);
                v.origin == SignalOrigin::SacdText && v.detail.is_none() && !n.is_empty() && cand_catalogs.contains(&n)
            })
        {
            score += 15.0;
            strong += 1;
            reasons.push("The disc's own catalog number names this SACD edition".into());
        }
        let disc_id_hits = signals.all_disc_ids().intersection(&candidate.all_disc_ids()).count();
        if disc_id_hits > 0 {
            score += 45.0 + 5.0 * (disc_id_hits as f64 - 1.0);
            strong += 1;
            reasons.push(if disc_id_hits > 1 {
                format!("Disc IDs match for {disc_id_hits} discs")
            } else {
                "Disc ID matches".into()
            });
        } else if origins.contains(&CandidateOrigin::TocMatch) {
            score += 25.0;
            strong += 1;
            reasons.push("CD TOC matches (MusicBrainz)".into());
        }
        if origins.contains(&CandidateOrigin::TagMbid) {
            score += 45.0;
            strong += 1;
            reasons.push("Release MBID from the file tags".into());
        } else if origins.contains(&CandidateOrigin::CtdbMbid) {
            score += 15.0;
            reasons.push("CUETools DB lists this release for the TOC".into());
        }
        let mut coverage = None;
        if let Some(vote) = fingerprint_vote
            && fingerprinted_tracks > 0
        {
            let c = vote.hit_count() as f64 / fingerprinted_tracks as f64;
            coverage = Some(c);
            if c >= 0.8 {
                score += 35.0;
                strong += 1;
            } else if c >= 0.5 {
                score += 20.0;
            } else {
                score += 8.0;
            }
            reasons.push(format!("Fingerprints match {} of {fingerprinted_tracks} tracks", vote.hit_count()));
        }

        // Soft text signals.
        if let Some(a) = &signals.artist_hint
            && Self::similar(a, &candidate.artist)
        {
            score += 10.0;
            reasons.push("Artist matches".into());
        }
        if let Some(t) = &signals.album_hint
            && Self::similar(t, &candidate.title)
        {
            score += 10.0;
            reasons.push("Title matches".into());
        }
        let candidate_words: HashSet<String> =
            Self::normalize(&format!("{} {}", candidate.title, candidate.disambiguation.as_deref().unwrap_or("")))
                .split(' ')
                .filter(|w| !w.is_empty())
                .map(String::from)
                .collect();
        if let Some(e) = &signals.edition_hint {
            let keywords = Self::edition_keywords(e);
            if !keywords.is_empty() {
                if keywords.iter().all(|k| candidate_words.iter().any(|w| w.starts_with(k.as_str()))) {
                    score += 10.0;
                    reasons.push(format!("Edition \"{e}\" matches"));
                } else {
                    score -= 5.0;
                    reasons.push(format!("Edition \"{e}\" not mentioned"));
                }
            }
        }
        // The pressing year pins an edition; the original year is weaker
        // when the folder names an edition.
        if let Some(ey) = &signals.edition_year_hint
            && Some(ey) == candidate.year.as_ref()
        {
            score += 8.0;
            reasons.push(format!("Edition year {ey}"));
        } else if let Some(y) = &signals.year_hint
            && Some(y) == candidate.year.as_ref()
        {
            let weak = signals.edition_hint.is_some() || signals.edition_year_hint.is_some();
            score += if weak { 2.0 } else { 5.0 };
            reasons.push(format!("Year {y}"));
        }
        if let (Some(c), Some(cc)) = (&signals.country_hint, &candidate.country)
            && c == cc
        {
            score += 6.0;
            reasons.push(format!("Country {c} matches"));
        }
        if candidate.status.as_deref() == Some("Bootleg") {
            score -= 10.0;
        }

        // Every track's fingerprint proves the audio; release durations are
        // often off by a few seconds, so they only veto unproven audio.
        let audio_proven = coverage.unwrap_or(0.0) >= 0.999
            && fingerprinted_tracks == signals.tracks.len()
            && track_count_matches == Some(true);
        let fit_ok = audio_proven
            || match duration_fit {
                None => track_count_matches.unwrap_or(true),
                Some(f) => f >= 0.6,
            };
        let count_ok = track_count_matches.unwrap_or(true);
        let confidence = if strong >= 2 && fit_ok && count_ok && !plain_cd_for_dsd {
            MatchConfidence::Confident
        } else if strong >= 1 && fit_ok && count_ok && !plain_cd_for_dsd {
            MatchConfidence::Likely
        } else if score >= 20.0 {
            MatchConfidence::Possible
        } else {
            MatchConfidence::Unlikely
        };

        ScoredCandidate {
            candidate: candidate.clone(),
            score,
            confidence,
            reasons,
            origins: origins.to_vec(),
            duration_fit,
            track_count_matches,
            fingerprint_coverage: coverage,
            format_compatibility: Some(Self::format_compatibility(candidate, signals)),
        }
    }

    fn catalogs_meet(candidate: &str, local: &str) -> bool {
        candidate == local || (local.chars().count() >= 6 && (candidate.ends_with(local) || local.ends_with(candidate)))
    }

    /// Could this release's media be where the local files came from? A
    /// medium named in the folder or tags wins over what the files imply.
    pub fn format_compatibility(candidate: &Candidate, signals: &AlbumSignals) -> FormatCompatibility {
        let formats: Vec<String> = candidate
            .media
            .iter()
            .filter_map(|m| m.format.clone())
            .chain(std::iter::once(candidate.media_format.clone().unwrap_or_default()))
            .chain(candidate.format_descriptions.iter().cloned())
            .map(|f| f.to_lowercase())
            .filter(|f| !f.is_empty() && f != "unknown" && f != "(unknown)" && f != "other")
            .collect();
        if formats.is_empty() {
            return FormatCompatibility::Unknown;
        }
        let text = formats.join(" ");
        let has = |s: &str| text.contains(s);
        let has_sacd = has("sacd") || has("dsd");
        let has_cd_layer = formats.iter().any(|f| {
            (f.contains("cd") && !f.contains("sacd"))
                || f.contains("cd layer")
                || f.contains("hybrid")
                || f.contains("hdcd")
                || f.contains("shm-cd")
        });
        let has_digital = has("digital") || has("file") || has("download");
        let has_vinyl = has("vinyl") || has("lp") || has("flexi") || has("acetate");
        let has_dvd = has("dvd") || has("blu-ray") || has("bluray") || has("hd dvd");
        let has_tape = has("cassette") || has("reel") || has("8-track") || has("shellac");
        if !(has_sacd || has_cd_layer || has_digital || has_vinyl || has_dvd || has_tape) {
            // "Box set", "CD-ROM"-style labels say nothing usable.
            return FormatCompatibility::Unknown;
        }
        let fits = |b: bool| if b { FormatCompatibility::Fits } else { FormatCompatibility::Unlikely };
        if let Some(hint) = signals.source_hint {
            return match hint {
                SourceHint::Vinyl => fits(has_vinyl),
                SourceHint::Sacd => fits(has_sacd),
                SourceHint::Dvd => fits(has_dvd),
                SourceHint::Digital => fits(has_digital || (signals.is_sacd_image && has_sacd)),
                SourceHint::Cd => fits(has_cd_layer),
            };
        }
        match signals.local_format() {
            // Loose DSF files may be a DSD download; an ISO only a disc.
            LocalFormat::Sacd => fits(has_sacd || (!signals.is_sacd_image && has_digital)),
            LocalFormat::Cd => fits(has_cd_layer || has_digital),
            // A plain CD or tape cannot yield 24-bit or > 44.1 kHz.
            LocalFormat::HiRes => fits(has_digital || has_vinyl || has_sacd || has_dvd),
            LocalFormat::Unknown => FormatCompatibility::Unknown,
        }
    }

    /// The album carries barcodes or catalog numbers and none of them
    /// belongs to `candidate`: the files may name another edition.
    pub fn contradicts_local_codes(candidate: &Candidate, signals: &AlbumSignals) -> bool {
        let barcodes = signals.unique_barcodes();
        let catalogs: Vec<String> = signals
            .unique_catalog_numbers()
            .iter()
            .map(|c| CatalogNumberParser::normalize(c))
            .filter(|c| c.chars().count() >= 4)
            .collect();
        if barcodes.is_empty() && catalogs.is_empty() {
            return false;
        }
        if let Some(b) = &candidate.barcode
            && barcodes.iter().any(|x| Self::barcodes_equal(x, b))
        {
            return false;
        }
        let mine: Vec<String> = candidate.catalog_numbers().iter().map(|c| CatalogNumberParser::normalize(c)).collect();
        !catalogs.iter().any(|local| mine.iter().any(|m| Self::catalogs_meet(m, local)))
    }

    /// Media at the local disc positions, in local order.
    pub fn paired_media(media: &[CandidateMedium], signals: &AlbumSignals) -> Vec<CandidateMedium> {
        if !signals.is_set() {
            return media.to_vec();
        }
        signals
            .present_discs
            .iter()
            .filter(|p| **p >= 1 && **p as usize <= media.len())
            .map(|p| media[*p as usize - 1].clone())
            .collect()
    }

    /// Hybrid SACDs (and CD+DVD sets) list one medium per layer: the layer
    /// matching the local files, otherwise every medium.
    pub fn relevant_media(candidate: &Candidate, local: LocalFormat) -> Vec<CandidateMedium> {
        let media: Vec<CandidateMedium> = if candidate.media.iter().any(|m| !m.is_video()) {
            candidate.media.iter().filter(|m| !m.is_video()).cloned().collect()
        } else {
            candidate.media.clone()
        };
        let lower = |m: &CandidateMedium| m.format.as_deref().unwrap_or("").to_lowercase();
        let layers: Vec<&CandidateMedium> = media.iter().filter(|m| lower(m).contains("layer")).collect();
        if layers.is_empty() {
            return media;
        }
        let wanted = |f: &str| match local {
            LocalFormat::Sacd => f.contains("sacd layer"),
            LocalFormat::Cd => f.contains("cd layer") && !f.contains("sacd layer"),
            _ => false,
        };
        let mut out: Vec<CandidateMedium> = media.iter().filter(|m| !lower(m).contains("layer")).cloned().collect();
        let chosen = layers.iter().find(|m| wanted(&lower(m))).unwrap_or(&layers[0]);
        out.push((*chosen).clone());
        out
    }

    /// "20th Anniversary Edition" → ["20th", "anniversary"]; "Remastered" → ["remaster"].
    pub fn edition_keywords(edition: &str) -> Vec<String> {
        const GENERIC: &[&str] = &["edition", "version", "the", "of", "and", "release", "import", "press", "pressing"];
        Self::normalize(edition)
            .split(' ')
            .filter(|w| !w.is_empty() && !GENERIC.contains(w))
            .map(
                |w| {
                    if w.ends_with("ed") && w.chars().count() > 5 {
                        w[..w.len() - 2].to_string()
                    } else {
                        w.to_string()
                    }
                },
            )
            .collect()
    }

    pub fn is_sacd_edition(c: &Candidate) -> bool {
        let text = c
            .media
            .iter()
            .filter_map(|m| m.format.clone())
            .chain(std::iter::once(c.media_format.clone().unwrap_or_default()))
            .chain(c.format_descriptions.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        text.contains("sacd") || text.contains("dsd")
    }

    /// One pressing entered twice: same barcode or catalog number, same medium.
    pub fn are_twins(a: &Candidate, b: &Candidate) -> bool {
        if Self::is_sacd_edition(a) != Self::is_sacd_edition(b) {
            return false;
        }
        if let (Some(x), Some(y)) = (&a.barcode, &b.barcode)
            && Self::barcodes_equal(x, y)
        {
            return true;
        }
        let set = |c: &Candidate| -> HashSet<String> {
            c.catalog_numbers().iter().map(|x| CatalogNumberParser::normalize(x)).filter(|x| !x.is_empty()).collect()
        };
        !set(a).is_disjoint(&set(b))
    }

    pub fn barcodes_equal(a: &str, b: &str) -> bool {
        let x = providerkit::digits(a);
        let y = providerkit::digits(b);
        if x.is_empty() || y.is_empty() {
            return false;
        }
        // UPC-A vs EAN-13 with a leading zero.
        x == y || x.trim_start_matches('0') == y.trim_start_matches('0')
    }

    /// Case, diacritics and punctuation insensitive; "The X" == "X".
    pub fn normalize(s: &str) -> String {
        let t = fold(&s.to_lowercase()).replace('&', "and");
        let t: String = t.chars().filter(|c| is_alphanumeric(*c) || *c == ' ').collect();
        let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
        let t = providerkit::trim_whitespaces(&t).to_string();
        match t.strip_prefix("the ") {
            Some(rest) => rest.to_string(),
            None => t,
        }
    }

    pub fn similar(a: &str, b: &str) -> bool {
        let x = Self::normalize(a);
        let y = Self::normalize(b);
        if x.is_empty() || y.is_empty() {
            return false;
        }
        x == y || (x.chars().count() >= 6 && (x.contains(&y) || y.contains(&x)))
    }
}
