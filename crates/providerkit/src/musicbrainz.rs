//! MusicBrainz web service client (https://musicbrainz.org/doc/MusicBrainz_API).
//! Every request carries a descriptive User-Agent; anonymous clients get
//! one request per second, so requests are spaced 1.05 s apart across all
//! threads, and a 503/429 is retried twice with a back-off.

use crate::candidate::*;
use crate::error::{ProviderError, Result};
use crate::http::{RateLimiter, Request, Transport, default_transport, encode_path, with_query};
use crate::{is_number, prefix, trim_whitespaces};
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

const BASE: &str = "https://musicbrainz.org/ws/2/";

pub struct MusicBrainzClient {
    pub user_agent: String,
    transport: Arc<dyn Transport>,
    limiter: RateLimiter,
}

impl MusicBrainzClient {
    pub fn new(user_agent: &str) -> MusicBrainzClient {
        MusicBrainzClient::with_transport(user_agent, default_transport())
    }

    pub fn with_transport(user_agent: &str, transport: Arc<dyn Transport>) -> MusicBrainzClient {
        MusicBrainzClient {
            user_agent: user_agent.into(),
            transport,
            limiter: RateLimiter::new(Duration::from_millis(1050)),
        }
    }

    /// Targeted search by catalog number, ORing the spellings editors use.
    pub fn search_by_catalog(&self, catalog_number: &str, limit: usize) -> Result<Vec<Candidate>> {
        let trimmed = trim_whitespaces(catalog_number);
        if trimmed.is_empty() {
            return Ok(vec![]);
        }
        let query = Self::catalog_query(trimmed);
        self.search(&query, limit)
    }

    /// Exact lookup by EAN-13 / UPC; both spellings of the leading zero.
    pub fn search_by_barcode(&self, barcode: &str, limit: usize) -> Result<Vec<Candidate>> {
        let digits = crate::digits(barcode);
        if digits.chars().count() < 8 {
            return Ok(vec![]);
        }
        let mut forms = vec![digits.clone()];
        let n = digits.chars().count();
        if n == 12 {
            forms.push(format!("0{digits}"));
        }
        if n == 13 && digits.starts_with('0') {
            forms.push(digits.chars().skip(1).collect());
        }
        let query = forms.iter().map(|f| format!("barcode:{f}")).collect::<Vec<_>>().join(" OR ");
        self.search(&query, limit)
    }

    fn search(&self, query: &str, limit: usize) -> Result<Vec<Candidate>> {
        let url =
            with_query(&format!("{BASE}release"), &[("query", query), ("fmt", "json"), ("limit", &limit.to_string())]);
        let data = self.send(&url)?;
        let response: ReleaseList = serde_json::from_slice(&data)?;
        Ok(response.releases.into_iter().map(|r| to_candidate(r, None)).collect())
    }

    /// Releases with a CD of this Disc ID; with `toc`, MusicBrainz falls back
    /// to a fuzzy TOC match. Empty on 404.
    pub fn lookup_disc_id(&self, disc_id: &str, toc: Option<&str>) -> Result<Vec<Candidate>> {
        let toc_plus = toc.map(|t| t.replace(' ', "+"));
        let mut items: Vec<(&str, &str)> =
            vec![("fmt", "json"), ("inc", "recordings+artist-credits+labels+release-groups"), ("cdstubs", "no")];
        if let Some(t) = &toc_plus {
            items.push(("toc", t));
        }
        let url = with_query(&format!("{BASE}discid/{}", encode_path(disc_id)), &items);
        let data = match self.send(&url) {
            Err(ProviderError::Http(404)) => return Ok(vec![]),
            r => r?,
        };
        let response: ReleaseList = serde_json::from_slice(&data)?;
        Ok(response.releases.into_iter().map(|r| to_candidate(r, None)).collect())
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
        let mut parts = vec![];
        if !artist.is_empty() {
            parts.push(format!("artist:\"{}\"", escape(artist)));
        }
        if !album.is_empty() {
            parts.push(format!("release:\"{}\"", escape(album)));
        }
        if let Some(f) = format {
            parts.push(format!("format:\"{}\"", escape(f)));
        }
        self.search(&parts.join(" AND "), limit)
    }

    /// Full tracklist and credits of one release.
    pub fn release_detail(&self, mbid: &str) -> Result<Candidate> {
        let url = with_query(
            &format!("{BASE}release/{}", encode_path(mbid)),
            &[
                ("fmt", "json"),
                (
                    "inc",
                    "recordings+artist-credits+labels+media+release-groups+discids+isrcs+artist-rels+recording-level-rels+work-rels+work-level-rels",
                ),
            ],
        );
        Self::decode_release(&self.send(&url)?)
    }

    /// Discogs release ids linked from a MusicBrainz release.
    pub fn discogs_release_ids(&self, mbid: &str) -> Result<Vec<String>> {
        let url = with_query(&format!("{BASE}release/{}", encode_path(mbid)), &[("fmt", "json"), ("inc", "url-rels")]);
        Ok(Self::discogs_ids_from_url_rels(&self.send(&url)?))
    }

    pub fn discogs_ids_from_url_rels(data: &[u8]) -> Vec<String> {
        let re = regex::Regex::new(r"discogs\.com/(?:[a-z-]+/)?release/(\d+)").unwrap();
        let Ok(root) = serde_json::from_slice::<serde_json::Value>(data) else { return vec![] };
        let Some(rels) = root.get("relations").and_then(|r| r.as_array()) else { return vec![] };
        rels.iter()
            .filter_map(|r| {
                if r.get("type")?.as_str()? != "discogs" {
                    return None;
                }
                let res = r.get("url")?.get("resource")?.as_str()?;
                Some(re.captures(res)?[1].to_string())
            })
            .collect()
    }

    pub fn decode_release(data: &[u8]) -> Result<Candidate> {
        let release: MBRelease = serde_json::from_slice(data)?;
        Ok(to_candidate(release, None))
    }

    /// Every release of a release group (all editions of one album).
    pub fn releases_in_group(&self, release_group_id: &str, limit: usize) -> Result<Vec<Candidate>> {
        let url = with_query(
            &format!("{BASE}release"),
            &[
                ("release-group", release_group_id),
                ("fmt", "json"),
                ("limit", &limit.to_string()),
                ("inc", "artist-credits+labels+media"),
            ],
        );
        let data = self.send(&url)?;
        let response: ReleaseList = serde_json::from_slice(&data)?;
        Ok(response
            .releases
            .into_iter()
            .map(|mut r| {
                r.release_group_id = Some(release_group_id.to_string());
                to_candidate(r, None)
            })
            .collect())
    }

    fn send(&self, url: &str) -> Result<Vec<u8>> {
        let live = self.transport.is_live();
        self.limiter.wait(live);
        let request =
            Request::get(url).header("User-Agent", self.user_agent.clone()).header("Accept", "application/json");
        let mut attempt = 0;
        loop {
            let response = self.transport.send(&request)?;
            if response.status >= 400 {
                if (response.status == 503 || response.status == 429) && attempt < 2 {
                    attempt += 1;
                    if live {
                        std::thread::sleep(Duration::from_secs_f64(1.5 * attempt as f64));
                    }
                    self.limiter.push_back();
                    continue;
                }
                return Err(ProviderError::Http(response.status));
            }
            return Ok(response.body);
        }
    }

    /// Labels print catalog numbers one way and editors enter them another
    /// ("CAPP139 SA", "CAPP 139 SA", "CAPP-139-SA"): the exact spellings
    /// ORed with a prefix wildcard on the letters + first digit run.
    pub fn catalog_query(raw: &str) -> String {
        let alnum: String = raw.to_uppercase().chars().filter(|c| c.is_alphabetic() || is_number(*c)).collect();
        let mut variants = vec![raw.to_string()];
        let mut spaced = String::new();
        let mut previous: Option<char> = None;
        for ch in alnum.chars() {
            if let Some(p) = previous
                && p.is_alphabetic() != ch.is_alphabetic()
            {
                spaced.push(' ');
            }
            spaced.push(ch);
            previous = Some(ch);
        }
        variants.push(spaced.clone());
        variants.push(spaced.replace(' ', "-"));
        variants.push(alnum.clone());
        let mut seen = HashSet::new();
        let mut terms: Vec<String> = variants
            .into_iter()
            .filter(|v| !v.is_empty() && seen.insert(v.to_lowercase()))
            .map(|v| format!("catno:\"{}\"", v.replace('"', "\\\"")))
            .collect();
        let re = regex::Regex::new(r"^([A-Z]{2,10}[0-9]{2,7})").unwrap();
        if let Some(m) = re.captures(&alnum) {
            terms.push(format!("catno:{}*", &m[1]));
        }
        terms.join(" OR ")
    }
}

fn escape(s: &str) -> String {
    s.replace('"', "\\\"")
}

// MARK: - JSON → Candidate

fn credits(raw: &Option<Vec<MBArtistCredit>>) -> Vec<ArtistCredit> {
    raw.iter()
        .flatten()
        .filter_map(|c| {
            let name = c.name.clone().or_else(|| c.artist.as_ref().and_then(|a| a.name.clone())).unwrap_or_default();
            if name.is_empty() {
                return None;
            }
            Some(ArtistCredit {
                name,
                join_phrase: c.joinphrase.clone().unwrap_or_default(),
                artist_id: c.artist.as_ref().and_then(|a| a.id.clone()),
                sort_name: c.artist.as_ref().and_then(|a| a.sort_name.clone()),
            })
        })
        .collect()
}

/// Recording and work relationships → role/name credits. Instruments and
/// vocals keep their attribute ("guitar", "lead vocals") as the role.
fn track_credits(recording: Option<&MBRecording>) -> (Vec<TrackCredit>, Vec<WorkCredit>) {
    let mut credits = vec![];
    let mut works = vec![];
    let mut seen = HashSet::new();
    let mut add = |role: &str, name: Option<&String>| {
        let Some(name) = name.filter(|n| !n.is_empty()) else { return };
        if seen.insert(format!("{}|{}", role.to_lowercase(), name)) {
            credits.push(TrackCredit::new(role, name));
        }
    };
    for rel in recording.and_then(|r| r.relations.as_ref()).into_iter().flatten() {
        let kind = rel.kind.as_deref().unwrap_or("").to_lowercase();
        let artist_name = rel.artist.as_ref().and_then(|a| a.name.as_ref());
        if rel.target_type.as_deref() == Some("work")
            && let Some(work) = &rel.work
        {
            if kind == "performance" {
                works.push(WorkCredit { id: work.id.clone(), title: work.title.clone().unwrap_or_default() });
            }
            for wrel in work.relations.iter().flatten().filter(|w| w.target_type.as_deref() == Some("artist")) {
                let wkind = wrel.kind.as_deref().unwrap_or("").to_lowercase();
                if matches!(
                    wkind.as_str(),
                    "composer" | "lyricist" | "librettist" | "writer" | "arranger" | "orchestrator" | "translator"
                ) {
                    add(&wkind, wrel.artist.as_ref().and_then(|a| a.name.as_ref()));
                }
            }
            continue;
        }
        if rel.target_type.as_deref() != Some("artist") {
            continue;
        }
        let attrs: Vec<&String> = rel
            .attributes
            .iter()
            .flatten()
            .filter(|a| a.as_str() != "additional" && a.as_str() != "guest" && a.as_str() != "solo")
            .collect();
        let joined = attrs.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ");
        match kind.as_str() {
            "instrument" => add(if attrs.is_empty() { "performer" } else { &joined }, artist_name),
            "vocal" => add(if attrs.is_empty() { "vocals" } else { &joined }, artist_name),
            "performer"
            | "performing orchestra"
            | "orchestra"
            | "conductor"
            | "arranger"
            | "producer"
            | "engineer"
            | "mix"
            | "recording"
            | "mastering"
            | "remixer"
            | "programming"
            | "chorus master"
            | "concertmaster" => {
                let role = match kind.as_str() {
                    "mix" => "mixer",
                    "recording" => "recording engineer",
                    k => k,
                };
                add(role, artist_name)
            }
            _ => {}
        }
    }
    (credits, works)
}

/// Order-preserving unique join, as `NSOrderedSet` + `joined(", ")`.
fn ordered_unique_join(values: Vec<String>) -> Option<String> {
    if values.is_empty() {
        return None;
    }
    let mut seen = HashSet::new();
    Some(values.into_iter().filter(|v| seen.insert(v.clone())).collect::<Vec<_>>().join(", "))
}

fn swift_int(s: &str) -> Option<i64> {
    // Swift's Int(String) takes an optional sign and ASCII digits only.
    let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

fn to_candidate(release: MBRelease, disambiguation_fallback: Option<String>) -> Candidate {
    let artist_credits = credits(&release.artist_credit);
    let artist = if artist_credits.is_empty() {
        release
            .artist_credit
            .iter()
            .flatten()
            .map(|c| c.name.clone().or_else(|| c.artist.as_ref().and_then(|a| a.name.clone())).unwrap_or_default())
            .filter(|n| !n.is_empty())
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        ArtistCredit::joined(&artist_credits)
    };
    let year = release.date.as_deref().filter(|d| !d.is_empty()).map(|d| prefix(d, 4));
    let label_info = release.label_info.as_deref().unwrap_or_default();
    let label = ordered_unique_join(
        label_info
            .iter()
            .filter_map(|l| l.label.as_ref().and_then(|x| x.name.clone()))
            .filter(|n| !n.is_empty())
            .collect(),
    );
    let catalog_number = ordered_unique_join(
        label_info.iter().filter_map(|l| l.catalog_number.clone()).filter(|n| !n.is_empty()).collect(),
    );

    let media_list: Vec<CandidateMedium> = release
        .media
        .iter()
        .flatten()
        .enumerate()
        .map(|(m_idx, m)| {
            let tracks = m
                .tracks
                .iter()
                .flatten()
                .enumerate()
                .map(|(idx, t)| {
                    let track_credit = credits(&t.artist_credit);
                    let credit = if track_credit.is_empty() { None } else { Some(ArtistCredit::joined(&track_credit)) };
                    let (extra_credits, works) = track_credits(t.recording.as_ref());
                    let number = t.number.clone().unwrap_or_else(|| (idx + 1).to_string());
                    CandidateTrack {
                        position: swift_int(&number).unwrap_or(t.position.unwrap_or(idx as i64 + 1)),
                        title: t
                            .title
                            .clone()
                            .or_else(|| t.recording.as_ref().and_then(|r| r.title.clone()))
                            .unwrap_or_default(),
                        artist: credit.filter(|c| !c.is_empty() && *c != artist),
                        duration_ms: t.length.or_else(|| t.recording.as_ref().and_then(|r| r.length)),
                        credits: extra_credits,
                        recording_id: t.recording.as_ref().and_then(|r| r.id.clone()),
                        track_id: t.id.clone(),
                        artist_credits: if track_credit == artist_credits { vec![] } else { track_credit },
                        isrcs: t.recording.as_ref().and_then(|r| r.isrcs.clone()).unwrap_or_default(),
                        works,
                    }
                })
                .collect();
            CandidateMedium::new(
                m.position.unwrap_or(m_idx as i64 + 1),
                m.format.clone(),
                m.title.clone(),
                tracks,
                m.discs.iter().flatten().filter_map(|d| d.id.clone()).collect(),
                m.track_count,
            )
        })
        .collect();
    let first_medium = release.media.as_ref().and_then(|m| m.first());
    let formats: Vec<String> = media_list.iter().filter_map(|m| m.format.clone()).collect();
    let media_format = match formats.first() {
        None => first_medium.and_then(|m| m.format.clone()),
        Some(f) => {
            let count = media_list.len();
            if count > 1 && formats.iter().all(|x| x == f) {
                Some(format!("{count}×{f}"))
            } else {
                let heads: HashSet<String> =
                    formats.iter().map(|x| x.split(" (").next().unwrap_or(x).to_string()).collect();
                if heads.len() == 1 && formats.iter().any(|x| x.to_lowercase().contains("layer")) {
                    heads.into_iter().next()
                } else {
                    Some(f.clone())
                }
            }
        }
    };
    let track_count = release.track_count.or_else(|| {
        if media_list.is_empty() {
            first_medium.and_then(|m| m.track_count)
        } else {
            Some(media_list.iter().map(|m| m.tracks.len() as i64).sum())
        }
    });
    let tracks = media_list.first().map(|m| m.tracks.clone()).unwrap_or_default();
    // The Cover Art Archive serves front covers at a predictable URL;
    // a release without art simply 404s.
    let cover = format!("https://coverartarchive.org/release/{}/front-250", release.id);
    let release_group = release.release_group.as_ref();
    Candidate {
        source: Source::Musicbrainz,
        provider_id: release.id.clone(),
        release_group_id: release.release_group_id.clone().or_else(|| release_group.map(|g| g.id.clone())),
        title: release.title.clone().unwrap_or_default(),
        artist,
        year,
        country: release.country.clone(),
        label,
        catalog_number,
        media_format,
        track_count,
        disambiguation: release.disambiguation.clone().or(disambiguation_fallback),
        cover_art_url: Some(cover),
        tracks,
        released_date: release.date.clone().filter(|d| d.chars().count() >= 10),
        genres: vec![],
        styles: vec![],
        format_descriptions: vec![],
        barcode: release.barcode.clone().filter(|b| !b.is_empty()),
        media: media_list,
        status: release.status.clone(),
        primary_type: release_group.and_then(|g| g.primary_type.clone()),
        artist_credits,
        secondary_types: release_group.and_then(|g| g.secondary_types.clone()).unwrap_or_default(),
        first_release_date: release_group.and_then(|g| g.first_release_date.clone()).filter(|d| !d.is_empty()),
        script: release.text_representation.as_ref().and_then(|t| t.script.clone()),
    }
}

// MARK: - Wire types

#[derive(Deserialize)]
struct ReleaseList {
    releases: Vec<MBRelease>,
}

#[derive(Deserialize)]
struct MBRelease {
    id: String,
    title: Option<String>,
    date: Option<String>,
    country: Option<String>,
    disambiguation: Option<String>,
    barcode: Option<String>,
    status: Option<String>,
    #[serde(rename = "track-count")]
    track_count: Option<i64>,
    #[serde(rename = "artist-credit")]
    artist_credit: Option<Vec<MBArtistCredit>>,
    #[serde(rename = "label-info")]
    label_info: Option<Vec<MBLabelInfo>>,
    media: Option<Vec<MBMedium>>,
    #[serde(rename = "release-group")]
    release_group: Option<MBReleaseGroup>,
    #[serde(rename = "text-representation")]
    text_representation: Option<MBTextRepresentation>,
    #[serde(skip)]
    release_group_id: Option<String>,
}

#[derive(Deserialize)]
struct MBTextRepresentation {
    script: Option<String>,
}

#[derive(Deserialize)]
struct MBReleaseGroup {
    id: String,
    #[serde(rename = "primary-type")]
    primary_type: Option<String>,
    #[serde(rename = "secondary-types")]
    secondary_types: Option<Vec<String>>,
    #[serde(rename = "first-release-date")]
    first_release_date: Option<String>,
}

#[derive(Deserialize)]
struct MBArtistCredit {
    name: Option<String>,
    joinphrase: Option<String>,
    artist: Option<MBArtist>,
}

#[derive(Deserialize)]
struct MBArtist {
    id: Option<String>,
    name: Option<String>,
    #[serde(rename = "sort-name")]
    sort_name: Option<String>,
}

#[derive(Deserialize)]
struct MBRelation {
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(rename = "target-type")]
    target_type: Option<String>,
    attributes: Option<Vec<String>>,
    artist: Option<MBArtist>,
    work: Option<MBWork>,
}

#[derive(Deserialize)]
struct MBWork {
    id: Option<String>,
    title: Option<String>,
    relations: Option<Vec<MBRelation>>,
}

#[derive(Deserialize)]
struct MBLabelInfo {
    #[serde(rename = "catalog-number")]
    catalog_number: Option<String>,
    label: Option<MBLabel>,
}

#[derive(Deserialize)]
struct MBLabel {
    name: Option<String>,
}

#[derive(Deserialize)]
struct MBMedium {
    position: Option<i64>,
    format: Option<String>,
    title: Option<String>,
    #[serde(rename = "track-count")]
    track_count: Option<i64>,
    tracks: Option<Vec<MBTrack>>,
    discs: Option<Vec<MBDisc>>,
}

#[derive(Deserialize)]
struct MBDisc {
    id: Option<String>,
}

#[derive(Deserialize)]
struct MBTrack {
    id: Option<String>,
    number: Option<String>,
    position: Option<i64>,
    title: Option<String>,
    length: Option<i64>,
    recording: Option<MBRecording>,
    #[serde(rename = "artist-credit")]
    artist_credit: Option<Vec<MBArtistCredit>>,
}

#[derive(Deserialize)]
struct MBRecording {
    id: Option<String>,
    title: Option<String>,
    length: Option<i64>,
    isrcs: Option<Vec<String>>,
    relations: Option<Vec<MBRelation>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_query_covers_spellings() {
        let q = MusicBrainzClient::catalog_query("CAPP139 SA");
        for t in [
            r#"catno:"CAPP139 SA""#,
            r#"catno:"CAPP 139 SA""#,
            r#"catno:"CAPP-139-SA""#,
            r#"catno:"CAPP139SA""#,
            "catno:CAPP139*",
        ] {
            assert!(q.contains(t), "{q}");
        }
        assert_eq!(q.split(" OR ").count(), 5);
        let simple = MusicBrainzClient::catalog_query("PD 83889");
        assert!(
            simple.contains(r#"catno:"PD 83889""#)
                && simple.contains(r#"catno:"PD-83889""#)
                && simple.contains("catno:PD83889*")
        );
    }

    const JSON: &str = r#"
    {"id":"rel-1","title":"Kind of Blue","date":"1959-08-17","country":"US","status":"Official","barcode":"074646493526",
     "text-representation":{"script":"Latn","language":"eng"},
     "artist-credit":[{"name":"Miles Davis","joinphrase":"","artist":{"id":"art-1","name":"Miles Davis","sort-name":"Davis, Miles"}}],
     "label-info":[{"catalog-number":"CK 64935","label":{"name":"Columbia"}}],
     "release-group":{"id":"rg-1","primary-type":"Album","secondary-types":["Live"],"first-release-date":"1959-08-17"},
     "media":[{"position":1,"format":"CD","track-count":1,"tracks":[
       {"id":"trk-1","number":"1","position":1,"title":"So What","length":562000,
        "artist-credit":[{"name":"Miles Davis","joinphrase":" feat. ","artist":{"id":"art-1","name":"Miles Davis","sort-name":"Davis, Miles"}},
                         {"name":"Bill Evans","joinphrase":"","artist":{"id":"art-2","name":"Bill Evans","sort-name":"Evans, Bill"}}],
        "recording":{"id":"rec-1","title":"So What","length":562000,"isrcs":["USSM15900001"],
          "relations":[
            {"type":"instrument","direction":"backward","target-type":"artist","attributes":["trumpet"],"artist":{"id":"art-1","name":"Miles Davis","sort-name":"Davis, Miles"}},
            {"type":"instrument","direction":"backward","target-type":"artist","attributes":["piano"],"artist":{"id":"art-2","name":"Bill Evans","sort-name":"Evans, Bill"}},
            {"type":"producer","direction":"backward","target-type":"artist","attributes":[],"artist":{"id":"art-3","name":"Teo Macero","sort-name":"Macero, Teo"}},
            {"type":"performance","direction":"forward","target-type":"work","attributes":[],
             "work":{"id":"work-1","title":"So What","relations":[
               {"type":"composer","direction":"backward","target-type":"artist","artist":{"id":"art-1","name":"Miles Davis","sort-name":"Davis, Miles"}}]}}
          ]}}]}]}
    "#;

    #[test]
    fn decodes_credits_isrcs_and_works() {
        let c = MusicBrainzClient::decode_release(JSON.as_bytes()).unwrap();
        assert_eq!(c.artist, "Miles Davis");
        assert_eq!(c.artist_sort().as_deref(), Some("Davis, Miles"));
        assert_eq!(c.artist_ids(), ["art-1"]);
        assert_eq!(c.secondary_types, ["Live"]);
        assert_eq!(c.first_release_date.as_deref(), Some("1959-08-17"));
        assert_eq!(c.script.as_deref(), Some("Latn"));
        let t = &c.all_tracks()[0];
        assert_eq!(t.artist.as_deref(), Some("Miles Davis feat. Bill Evans"));
        assert_eq!(ArtistCredit::sort_joined(&t.artist_credits), "Davis, Miles feat. Evans, Bill");
        assert_eq!(t.isrcs, ["USSM15900001"]);
        assert_eq!(t.works.iter().map(|w| w.title.as_str()).collect::<Vec<_>>(), ["So What"]);
        assert!(t.credits.contains(&TrackCredit::new("trumpet", "Miles Davis")));
        assert!(t.credits.contains(&TrackCredit::new("producer", "Teo Macero")));
        assert!(t.credits.contains(&TrackCredit::new("composer", "Miles Davis")));
        let round: Candidate = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(round, c);
    }

    #[test]
    fn discogs_links() {
        let rels = br#"{"relations":[{"type":"amazon asin","url":{"resource":"https://www.amazon.com/x"}},{"type":"discogs","url":{"resource":"https://www.discogs.com/release/406222"}},{"type":"discogs","url":{"resource":"https://www.discogs.com/es/release/123-Some-Title"}}]}"#;
        assert_eq!(MusicBrainzClient::discogs_ids_from_url_rels(rels), ["406222", "123"]);
    }
}
