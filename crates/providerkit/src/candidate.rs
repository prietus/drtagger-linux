//! Provider-agnostic release candidates. MusicBrainz and Discogs answers
//! are normalised into this shape, so the scorer and the UI do not know
//! where a candidate came from. The JSON is the one Swift's Codable writes
//! (camelCase keys, absent optionals left out), so results persist the
//! same way and parity dumps compare directly.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// One name in an artist credit: "Ella Fitzgerald" + " & " + "Oscar Peterson".
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistCredit {
    pub name: String,
    pub join_phrase: String,
    #[serde(rename = "artistID", default, skip_serializing_if = "Option::is_none")]
    pub artist_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_name: Option<String>,
}

impl ArtistCredit {
    pub fn new(name: &str) -> ArtistCredit {
        ArtistCredit { name: name.into(), join_phrase: String::new(), artist_id: None, sort_name: None }
    }

    pub fn joined(credits: &[ArtistCredit]) -> String {
        credits.iter().map(|c| format!("{}{}", c.name, c.join_phrase)).collect()
    }

    pub fn sort_joined(credits: &[ArtistCredit]) -> String {
        credits.iter().map(|c| format!("{}{}", c.sort_name.as_deref().unwrap_or(&c.name), c.join_phrase)).collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorkCredit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub title: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Musicbrainz,
    Discogs,
    Acoustid,
}

impl Source {
    pub fn raw(self) -> &'static str {
        match self {
            Source::Musicbrainz => "musicbrainz",
            Source::Discogs => "discogs",
            Source::Acoustid => "acoustid",
        }
    }
}

/// One per-track credit line: the provider's raw role ("Written-By",
/// "Bass", "producer") and the artist.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TrackCredit {
    pub role: String,
    pub name: String,
}

impl TrackCredit {
    pub fn new(role: &str, name: &str) -> TrackCredit {
        TrackCredit { role: role.into(), name: name.into() }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateTrack {
    /// 1-based number within its medium.
    pub position: i64,
    pub title: String,
    /// Only set when it differs from the release artist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(rename = "durationMS", default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    #[serde(default)]
    pub credits: Vec<TrackCredit>,
    /// MusicBrainz recording MBID.
    #[serde(rename = "recordingID", default, skip_serializing_if = "Option::is_none")]
    pub recording_id: Option<String>,
    /// MusicBrainz track MBID (release-specific).
    #[serde(rename = "trackID", default, skip_serializing_if = "Option::is_none")]
    pub track_id: Option<String>,
    /// Empty when the same as the release's.
    #[serde(default)]
    pub artist_credits: Vec<ArtistCredit>,
    #[serde(default)]
    pub isrcs: Vec<String>,
    #[serde(default)]
    pub works: Vec<WorkCredit>,
}

impl CandidateTrack {
    pub fn new(position: i64, title: &str) -> CandidateTrack {
        CandidateTrack {
            position,
            title: title.into(),
            artist: None,
            duration_ms: None,
            credits: vec![],
            recording_id: None,
            track_id: None,
            artist_credits: vec![],
            isrcs: vec![],
            works: vec![],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateMedium {
    pub position: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub tracks: Vec<CandidateTrack>,
    #[serde(rename = "discIDs", default)]
    pub disc_ids: Vec<String>,
    /// Known even when `tracks` is empty (search results).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_count: Option<i64>,
}

impl CandidateMedium {
    /// `track_count` falls back to the number of tracks, as Swift's init does.
    pub fn new(
        position: i64,
        format: Option<String>,
        title: Option<String>,
        tracks: Vec<CandidateTrack>,
        disc_ids: Vec<String>,
        track_count: Option<i64>,
    ) -> CandidateMedium {
        let track_count = track_count.or(if tracks.is_empty() { None } else { Some(tracks.len() as i64) });
        CandidateMedium { position, format, title, tracks, disc_ids, track_count }
    }

    pub fn with_tracks(position: i64, tracks: Vec<CandidateTrack>) -> CandidateMedium {
        CandidateMedium::new(position, None, None, tracks, vec![], None)
    }

    pub fn is_layer(&self) -> bool {
        self.format.as_deref().unwrap_or("").to_lowercase().contains("layer")
    }

    /// Picture media (the DVD of a CD+DVD edition): never among audio files.
    pub fn is_video(&self) -> bool {
        let f = self.format.as_deref().unwrap_or("").to_lowercase();
        ["dvd-video", "vhs", "laserdisc", "vcd", "svcd", "hd-dvd", "betamax", "umd"].contains(&f.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub source: Source,
    /// Stable ID on the source, e.g. the MBID.
    #[serde(rename = "providerID")]
    pub provider_id: String,
    #[serde(rename = "releaseGroupID", default, skip_serializing_if = "Option::is_none")]
    pub release_group_id: Option<String>,
    pub title: String,
    pub artist: String,
    /// Just the year, for display; the full date is `released_date`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_number: Option<String>,
    /// "CD", "2×CD", "Vinyl", "Digital Media".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_format: Option<String>,
    /// Total across media.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disambiguation: Option<String>,
    #[serde(rename = "coverArtURL", default, skip_serializing_if = "Option::is_none")]
    pub cover_art_url: Option<String>,
    /// First medium (or all, for single-medium releases).
    #[serde(default)]
    pub tracks: Vec<CandidateTrack>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released_date: Option<String>,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub styles: Vec<String>,
    #[serde(default)]
    pub format_descriptions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub barcode: Option<String>,
    #[serde(default)]
    pub media: Vec<CandidateMedium>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_type: Option<String>,
    #[serde(default)]
    pub artist_credits: Vec<ArtistCredit>,
    #[serde(default)]
    pub secondary_types: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_release_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
}

impl Candidate {
    pub fn new(source: Source, provider_id: &str, title: &str, artist: &str) -> Candidate {
        Candidate {
            source,
            provider_id: provider_id.into(),
            release_group_id: None,
            title: title.into(),
            artist: artist.into(),
            year: None,
            country: None,
            label: None,
            catalog_number: None,
            media_format: None,
            track_count: None,
            disambiguation: None,
            cover_art_url: None,
            tracks: vec![],
            released_date: None,
            genres: vec![],
            styles: vec![],
            format_descriptions: vec![],
            barcode: None,
            media: vec![],
            status: None,
            primary_type: None,
            artist_credits: vec![],
            secondary_types: vec![],
            first_release_date: None,
            script: None,
        }
    }

    pub fn id(&self) -> String {
        format!("{}:{}", self.source.raw(), self.provider_id)
    }

    pub fn artist_sort(&self) -> Option<String> {
        if self.artist_credits.is_empty() { None } else { Some(ArtistCredit::sort_joined(&self.artist_credits)) }
    }

    pub fn artist_ids(&self) -> Vec<String> {
        self.artist_credits.iter().filter_map(|c| c.artist_id.clone()).collect()
    }

    /// Tracks of every medium in order; falls back to `tracks`.
    pub fn all_tracks(&self) -> Vec<CandidateTrack> {
        if self.media.is_empty() {
            self.tracks.clone()
        } else {
            self.media.iter().flat_map(|m| m.tracks.iter().cloned()).collect()
        }
    }

    pub fn all_tracks_is_empty(&self) -> bool {
        if self.media.is_empty() { self.tracks.is_empty() } else { self.media.iter().all(|m| m.tracks.is_empty()) }
    }

    pub fn medium_count(&self) -> usize {
        self.media.len().max(1)
    }

    /// Track counts a local rip could plausibly have: the total, or a
    /// single layer of a hybrid disc.
    pub fn plausible_track_counts(&self) -> BTreeSet<i64> {
        let mut set = BTreeSet::new();
        if let Some(t) = self.track_count {
            set.insert(t);
        }
        let video: i64 = self.media.iter().filter(|m| m.is_video()).filter_map(|m| m.track_count).sum();
        if video > 0
            && let Some(t) = self.track_count
            && t > video
        {
            set.insert(t - video);
        }
        let layers: Vec<&CandidateMedium> = self.media.iter().filter(|m| m.is_layer()).collect();
        if !layers.is_empty() {
            for m in &layers {
                if let Some(c) = m.track_count {
                    set.insert(c);
                }
            }
            let others: i64 = self.media.iter().filter(|m| !m.is_layer()).filter_map(|m| m.track_count).sum();
            for m in &layers {
                if let Some(c) = m.track_count {
                    set.insert(c + others);
                }
            }
        }
        set
    }

    pub fn all_disc_ids(&self) -> BTreeSet<String> {
        self.media.iter().flat_map(|m| m.disc_ids.iter().cloned()).collect()
    }

    /// Catalog numbers as MusicBrainz stores them, split on the joiner used
    /// when several labels are credited.
    pub fn catalog_numbers(&self) -> Vec<String> {
        self.catalog_number
            .as_deref()
            .unwrap_or("")
            .split(',')
            .map(|s| crate::trim_whitespaces(s).to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_combines_source_and_provider_id() {
        assert_eq!(Candidate::new(Source::Musicbrainz, "abc", "T", "A").id(), "musicbrainz:abc");
    }

    #[test]
    fn hybrid_layers_count_once() {
        let mut hybrid = Candidate::new(Source::Musicbrainz, "h", "Aja", "Steely Dan");
        hybrid.track_count = Some(14);
        hybrid.media = vec![
            CandidateMedium::new(1, Some("Hybrid SACD (CD layer)".into()), None, vec![], vec![], Some(7)),
            CandidateMedium::new(2, Some("Hybrid SACD (SACD layer, 2 channels)".into()), None, vec![], vec![], Some(7)),
        ];
        assert_eq!(hybrid.plausible_track_counts(), BTreeSet::from([14, 7]));
        let mut two = Candidate::new(Source::Musicbrainz, "d", "X", "Y");
        two.track_count = Some(20);
        two.media = vec![
            CandidateMedium::new(1, Some("CD".into()), None, vec![], vec![], Some(10)),
            CandidateMedium::new(2, Some("CD".into()), None, vec![], vec![], Some(10)),
        ];
        assert_eq!(two.plausible_track_counts(), BTreeSet::from([20]));
    }

    #[test]
    fn old_blobs_decode_with_defaults() {
        let old = r#"{"source":"musicbrainz","providerID":"x","title":"T","artist":"A","tracks":[{"position":1,"title":"a"}]}"#;
        let c: Candidate = serde_json::from_str(old).unwrap();
        assert!(c.artist_credits.is_empty() && c.tracks[0].isrcs.is_empty());
    }
}
