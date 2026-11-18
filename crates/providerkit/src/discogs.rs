//! Discogs client (https://www.discogs.com/developers/). A personal access
//! token goes in `Authorization: Discogs token=…`; authenticated calls get
//! 60 requests a minute, so requests are spaced 1.05 s apart.

use crate::candidate::*;
use crate::error::{ProviderError, Result};
use crate::http::{RateLimiter, Request, Transport, default_transport, encode_path, with_query};
use crate::{digits, prefix, trim_whitespaces};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

const BASE: &str = "https://api.discogs.com/";

pub struct DiscogsClient {
    pub user_agent: String,
    pub token: String,
    transport: Arc<dyn Transport>,
    limiter: RateLimiter,
}

/// A photo on a Discogs release page. Discogs does not say which one is
/// the obi or the back: users upload them as they like.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscogsReleaseImage {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
    pub is_primary: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<i64>,
}

impl DiscogsClient {
    pub fn new(user_agent: &str, token: &str) -> DiscogsClient {
        DiscogsClient::with_transport(user_agent, token, default_transport())
    }

    pub fn with_transport(user_agent: &str, token: &str, transport: Arc<dyn Transport>) -> DiscogsClient {
        DiscogsClient {
            user_agent: user_agent.into(),
            token: token.into(),
            transport,
            limiter: RateLimiter::new(Duration::from_millis(1050)),
        }
    }

    pub fn is_configured(&self) -> bool {
        !self.token.is_empty()
    }

    pub fn search_releases(
        &self,
        artist: &str,
        album: &str,
        format: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Candidate>> {
        let artist = trim_whitespaces(artist);
        let album = trim_whitespaces(album);
        if artist.is_empty() && album.is_empty() {
            return Ok(vec![]);
        }
        let limit = limit.to_string();
        let mut items: Vec<(&str, &str)> = vec![("type", "release"), ("per_page", &limit)];
        if !artist.is_empty() {
            items.push(("artist", artist));
        }
        if !album.is_empty() {
            items.push(("release_title", album));
        }
        if let Some(f) = format {
            items.push(("format", f));
        }
        self.search(&items)
    }

    /// Discogs indexes the barcode field directly: a hit is that pressing.
    pub fn search_by_barcode(&self, barcode: &str, limit: usize) -> Result<Vec<Candidate>> {
        let trimmed = trim_whitespaces(barcode);
        if trimmed.is_empty() {
            return Ok(vec![]);
        }
        self.search(&[("type", "release"), ("barcode", trimmed), ("per_page", &limit.to_string())])
    }

    pub fn search_by_catalog(&self, catalog_number: &str, limit: usize) -> Result<Vec<Candidate>> {
        let trimmed = trim_whitespaces(catalog_number);
        if trimmed.is_empty() {
            return Ok(vec![]);
        }
        self.search(&[("type", "release"), ("catno", trimmed), ("per_page", &limit.to_string())])
    }

    fn search(&self, items: &[(&str, &str)]) -> Result<Vec<Candidate>> {
        let data = self.send(&with_query(&format!("{BASE}database/search"), items))?;
        let response: SearchResponse = serde_json::from_slice(&data)?;
        Ok(response.results.into_iter().map(search_hit_candidate).collect())
    }

    /// Full release (tracklist): the search answers carry none.
    pub fn release_detail(&self, id: &str) -> Result<Candidate> {
        Self::decode_release(&self.send(&format!("{BASE}releases/{}", encode_path(id)))?)
    }

    pub fn decode_release(data: &[u8]) -> Result<Candidate> {
        Ok(release_candidate(serde_json::from_slice(data)?))
    }

    pub fn images(&self, release_id: &str) -> Result<Vec<DiscogsReleaseImage>> {
        Ok(Self::parse_images(&self.send(&format!("{BASE}releases/{}", encode_path(release_id)))?))
    }

    /// Primary image first.
    pub fn parse_images(data: &[u8]) -> Vec<DiscogsReleaseImage> {
        let images = serde_json::from_slice::<ImagesOnly>(data).ok().and_then(|i| i.images).unwrap_or_default();
        let mapped: Vec<DiscogsReleaseImage> = images
            .into_iter()
            .filter_map(|i| {
                let url = i.uri.filter(|u| !u.is_empty())?;
                Some(DiscogsReleaseImage {
                    url,
                    thumbnail: i.uri150.filter(|u| !u.is_empty()),
                    is_primary: i.kind.as_deref() == Some("primary"),
                    width: i.width,
                    height: i.height,
                })
            })
            .collect();
        let (primary, rest): (Vec<_>, Vec<_>) = mapped.into_iter().partition(|i| i.is_primary);
        primary.into_iter().chain(rest).collect()
    }

    pub(crate) fn request(&self, url: &str) -> Request {
        let mut r =
            Request::get(url).header("User-Agent", self.user_agent.clone()).header("Accept", "application/json");
        if !self.token.is_empty() {
            r = r.header("Authorization", format!("Discogs token={}", self.token));
        }
        r
    }

    pub(crate) fn transport(&self) -> &Arc<dyn Transport> {
        &self.transport
    }

    fn send(&self, url: &str) -> Result<Vec<u8>> {
        self.limiter.wait(self.transport.is_live());
        let response = self.transport.send(&self.request(url))?;
        if response.status >= 400 {
            return Err(ProviderError::Http(response.status));
        }
        Ok(response.body)
    }
}

// MARK: - Mapping

/// Discogs answers "spacer.gif" placeholders instead of null for releases
/// without art.
fn is_real_cover_url(s: &str) -> bool {
    let lower = s.to_lowercase();
    !lower.is_empty() && !lower.contains("spacer.gif") && !lower.contains("/spacer")
}

/// "Artist - Album".
fn split_title(raw: &str) -> (String, String) {
    match raw.split_once(" - ") {
        Some((a, b)) => (a.to_string(), b.to_string()),
        None => (String::new(), raw.to_string()),
    }
}

/// "1", "A1", "1.1": the trailing number.
fn parse_position(s: Option<&str>) -> Option<i64> {
    let s = s.filter(|s| !s.is_empty())?;
    let tail: Vec<char> = s.chars().rev().take_while(|c| crate::is_number(*c)).collect();
    let digits: String = tail.into_iter().rev().collect();
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

fn parse_duration_ms(s: Option<&str>) -> Option<i64> {
    let s = s.filter(|s| !s.is_empty())?;
    let parts: Vec<i64> = s
        .split(':')
        .filter(|p| !p.is_empty())
        .filter_map(|p| {
            let d = p.strip_prefix(['+', '-']).unwrap_or(p);
            if d.is_empty() || !d.bytes().all(|b| b.is_ascii_digit()) { None } else { p.parse().ok() }
        })
        .collect();
    match parts.len() {
        2 => Some((parts[0] * 60 + parts[1]) * 1000),
        3 => Some((parts[0] * 3600 + parts[1] * 60 + parts[2]) * 1000),
        _ => None,
    }
}

fn search_hit_candidate(hit: SearchHit) -> Candidate {
    let (artist, album) = split_title(&hit.title);
    let mut c = Candidate::new(Source::Discogs, &hit.id.to_string(), &album, &artist);
    c.year = hit.year.filter(|y| !y.is_empty());
    c.country = hit.country;
    c.label = hit.label.and_then(|l| l.into_iter().next());
    c.catalog_number = hit.catno;
    c.media_format = hit.format.and_then(|f| f.into_iter().next());
    c.cover_art_url = hit.cover_image.filter(|u| is_real_cover_url(u));
    c.genres = hit.genre.unwrap_or_default();
    c.styles = hit.style.unwrap_or_default();
    c
}

fn release_candidate(r: Release) -> Candidate {
    let artist = r
        .artists
        .iter()
        .flatten()
        .map(|a| a.name.clone().unwrap_or_default())
        .filter(|n| !n.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    let labels = r.labels.as_deref().unwrap_or_default();
    let label_info = labels.first();
    let catalogs: Vec<String> =
        labels.iter().filter_map(|l| l.catno.clone()).filter(|c| !c.is_empty() && c.to_lowercase() != "none").collect();
    let cat_no = if catalogs.is_empty() {
        label_info.and_then(|l| l.catno.clone())
    } else {
        let mut seen = HashSet::new();
        Some(catalogs.into_iter().filter(|c| seen.insert(c.clone())).collect::<Vec<_>>().join(", "))
    };
    let barcode = r
        .identifiers
        .iter()
        .flatten()
        .filter(|i| i.kind.as_deref().unwrap_or("").to_lowercase() == "barcode")
        .filter_map(|i| i.value.as_deref().map(digits))
        .find(|d| (8..=14).contains(&d.chars().count()));
    let formats = r.formats.clone().unwrap_or_default();
    let first_format = formats.first();
    let format = first_format.and_then(|f| f.name.clone());
    let format_descriptions = first_format.and_then(|f| f.descriptions.clone()).unwrap_or_default();
    let year = match r.year {
        Some(y) if y > 0 => Some(y.to_string()),
        _ => r.released.as_deref().filter(|d| !d.is_empty()).map(|d| prefix(d, 4)),
    };
    let released_date = r.released.clone().filter(|d| !d.is_empty());
    let images = r.images.as_deref().unwrap_or_default();
    let primary_uri = images.iter().find(|i| i.kind.as_deref() == Some("primary")).and_then(|i| i.uri.clone());
    let first_uri = images.first().and_then(|i| i.uri.clone());
    let cover = [primary_uri, first_uri].into_iter().flatten().find(|u| is_real_cover_url(u));

    // CD+DVD editions: the DVD's tracks ("DVD-1"…) are video, unless it is a DVD-Audio.
    let is_dvd_audio = |f: &Format| f.descriptions.as_deref().unwrap_or_default().iter().any(|d| d == "DVD-Audio");
    let video_dvd = formats.len() > 1
        && formats.iter().any(|f| f.name.as_deref() == Some("DVD") && !is_dvd_audio(f))
        && formats.iter().any(|f| f.name.as_deref() != Some("DVD"));
    let tracks: Vec<CandidateTrack> = r
        .tracklist
        .iter()
        .flatten()
        .filter(|t| t.kind.as_deref().unwrap_or("track") == "track")
        .filter(|t| !video_dvd || !t.position.as_deref().unwrap_or("").to_uppercase().starts_with("DVD"))
        .enumerate()
        .map(|(idx, t)| {
            let credits = t
                .extraartists
                .iter()
                .flatten()
                .filter_map(|e| {
                    let name = e.name.clone().filter(|n| !n.is_empty())?;
                    Some(TrackCredit { role: e.role.clone().unwrap_or_default(), name })
                })
                .collect();
            let mut track = CandidateTrack::new(
                parse_position(t.position.as_deref()).unwrap_or(idx as i64 + 1),
                t.title.as_deref().unwrap_or(""),
            );
            track.duration_ms = parse_duration_ms(t.duration.as_deref());
            track.credits = credits;
            track
        })
        .collect();

    let mut c = Candidate::new(Source::Discogs, &r.id.to_string(), r.title.as_deref().unwrap_or(""), &artist);
    c.year = year;
    c.country = r.country.clone();
    c.label = label_info.and_then(|l| l.name.clone());
    c.catalog_number = cat_no;
    c.media_format = format.clone();
    c.track_count = if tracks.is_empty() { None } else { Some(tracks.len() as i64) };
    c.cover_art_url = cover;
    c.released_date = released_date;
    c.genres = r.genres.clone().unwrap_or_default();
    c.styles = r.styles.clone().unwrap_or_default();
    c.format_descriptions = format_descriptions;
    c.barcode = barcode;
    c.media = if tracks.is_empty() {
        vec![]
    } else {
        vec![CandidateMedium::new(1, format, None, tracks.clone(), vec![], None)]
    };
    c.tracks = tracks;
    c
}

// MARK: - Wire types

#[derive(Deserialize)]
struct SearchResponse {
    results: Vec<SearchHit>,
}

#[derive(Deserialize)]
struct SearchHit {
    id: i64,
    title: String,
    year: Option<String>,
    country: Option<String>,
    label: Option<Vec<String>>,
    catno: Option<String>,
    format: Option<Vec<String>>,
    cover_image: Option<String>,
    genre: Option<Vec<String>>,
    style: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct Release {
    id: i64,
    title: Option<String>,
    year: Option<i64>,
    released: Option<String>,
    country: Option<String>,
    genres: Option<Vec<String>>,
    styles: Option<Vec<String>>,
    artists: Option<Vec<Artist>>,
    labels: Option<Vec<Label>>,
    formats: Option<Vec<Format>>,
    images: Option<Vec<Image>>,
    tracklist: Option<Vec<Track>>,
    identifiers: Option<Vec<Identifier>>,
}

#[derive(Deserialize)]
struct Identifier {
    #[serde(rename = "type")]
    kind: Option<String>,
    value: Option<String>,
}

#[derive(Deserialize)]
struct Artist {
    name: Option<String>,
}

#[derive(Deserialize)]
struct Label {
    name: Option<String>,
    catno: Option<String>,
}

#[derive(Deserialize, Clone)]
struct Format {
    name: Option<String>,
    descriptions: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct Image {
    #[serde(rename = "type")]
    kind: Option<String>,
    uri: Option<String>,
    uri150: Option<String>,
    width: Option<i64>,
    height: Option<i64>,
}

#[derive(Deserialize)]
struct ImagesOnly {
    images: Option<Vec<Image>>,
}

#[derive(Deserialize)]
struct Track {
    position: Option<String>,
    title: Option<String>,
    duration: Option<String>,
    #[serde(rename = "type_")]
    kind: Option<String>,
    extraartists: Option<Vec<ExtraArtist>>,
}

#[derive(Deserialize)]
struct ExtraArtist {
    name: Option<String>,
    role: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn barcode_catalogs_and_medium() {
        let json = r#"{"id":27786006,"title":"The Power To Believe","year":2023,"country":"Japan","artists":[{"name":"King Crimson"}],"labels":[{"name":"Discipline Global Mobile","catno":"POCS-1950"},{"name":"Universal","catno":"none"}],"formats":[{"name":"CD","descriptions":["Album","Reissue"]}],"identifiers":[{"type":"Barcode","value":"4 988031 573259"},{"type":"Matrix / Runout","value":"POCS-1929 1T"}],"tracklist":[{"type_":"track","position":"1","title":"A Cappella","duration":"0:44"},{"type_":"heading","position":"","title":"Bonus Tracks:","duration":""},{"type_":"track","position":"2","title":"Superslow","duration":"3:38"}]}"#;
        let c = DiscogsClient::decode_release(json.as_bytes()).unwrap();
        assert_eq!(c.barcode.as_deref(), Some("4988031573259"));
        assert_eq!(c.catalog_number.as_deref(), Some("POCS-1950"));
        assert_eq!(c.media.len(), 1);
        assert_eq!(c.media[0].tracks.len(), 2);
        assert_eq!(c.all_tracks().iter().map(|t| t.duration_ms).collect::<Vec<_>>(), [Some(44000), Some(218000)]);
    }

    #[test]
    fn images_primary_first() {
        let data = br#"{"images":[{"type":"secondary","uri":"https://i.discogs.com/b.jpg","uri150":"https://i.discogs.com/b150.jpg","width":600,"height":600},{"type":"primary","uri":"https://i.discogs.com/a.jpg","width":1200,"height":1190}]}"#;
        let images = DiscogsClient::parse_images(data);
        assert_eq!(images[0].url, "https://i.discogs.com/a.jpg");
        assert!(images[0].is_primary);
        assert_eq!(images[1].thumbnail.as_deref(), Some("https://i.discogs.com/b150.jpg"));
    }

    #[test]
    fn positions_and_durations() {
        assert_eq!(parse_position(Some("A12")), Some(12));
        assert_eq!(parse_position(Some("1.1")), Some(1));
        assert_eq!(parse_position(Some("A")), None);
        assert_eq!(parse_duration_ms(Some("1:02:03")), Some(3_723_000));
        assert_eq!(parse_duration_ms(Some("3:x:40")), Some(220_000));
        assert_eq!(parse_duration_ms(Some("45")), None);
    }
}
