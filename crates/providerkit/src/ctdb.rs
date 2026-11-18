//! CUETools Database (db.cuetools.net): key-less CD rip checksums. One TOC
//! lookup returns every known rip with per-track CRC32s, plus MusicBrainz /
//! FreeDB metadata for the TOC. Track CRCs skip the first 5880 samples after
//! track 1's INDEX 01 and the last 5880 of the disc ("stride").

use crate::error::{ProviderError, Result};
use crate::http::{Request, Transport, default_transport, with_query};
use quick_xml::events::{BytesStart, Event};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

pub const STRIDE_SAMPLES: usize = 5880;
const BASE: &str = "http://db.cuetools.net/lookup2.php";

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CtdbEntry {
    pub id: String,
    pub confidence: i64,
    #[serde(rename = "discCRC32")]
    pub disc_crc32: u32,
    #[serde(rename = "trackCRC32s")]
    pub track_crc32s: Vec<u32>,
    pub toc: String,
    pub has_parity: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CtdbMetadataTrack {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CtdbMetadata {
    /// "musicbrainz", "freedb", …
    pub source: String,
    /// The release MBID when the source is MusicBrainz.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub album: String,
    pub artist: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub barcode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_number: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disc_number: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disc_count: Option<i64>,
    pub relevance: i64,
    pub tracks: Vec<CtdbMetadataTrack>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CtdbResponse {
    pub entries: Vec<CtdbEntry>,
    pub metadata: Vec<CtdbMetadata>,
}

impl CtdbResponse {
    pub fn total_confidence(&self) -> i64 {
        self.entries.iter().map(|e| e.confidence).sum()
    }

    /// The entry with the highest confidence; the first of equals, as
    /// Swift's `max(by:)` picks.
    pub fn best_entry(&self) -> Option<&CtdbEntry> {
        let mut best: Option<&CtdbEntry> = None;
        for e in &self.entries {
            if best.is_none_or(|b| e.confidence > b.confidence) {
                best = Some(e);
            }
        }
        best
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CtdbTrackVerdict {
    /// 0-based position in the TOC.
    pub track_index: usize,
    /// Summed confidence of the entries whose CRC matches.
    pub matched_confidence: i64,
    pub best_entry_matches: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CtdbVerification {
    pub tracks: Vec<CtdbTrackVerdict>,
    pub disc_matches_best_entry: bool,
    pub total_confidence: i64,
}

impl CtdbVerification {
    pub fn matched_track_count(&self) -> usize {
        self.tracks.iter().filter(|t| t.matched_confidence > 0).count()
    }

    pub fn all_tracks_match(&self) -> bool {
        !self.tracks.is_empty() && self.tracks.iter().all(|t| t.matched_confidence > 0)
    }
}

pub struct CUEToolsDBClient {
    pub user_agent: String,
    transport: Arc<dyn Transport>,
}

impl CUEToolsDBClient {
    pub fn new(user_agent: &str) -> CUEToolsDBClient {
        CUEToolsDBClient { user_agent: user_agent.into(), transport: default_transport() }
    }

    pub fn with_transport(user_agent: &str, transport: Arc<dyn Transport>) -> CUEToolsDBClient {
        CUEToolsDBClient { user_agent: user_agent.into(), transport }
    }

    /// `toc` is `DiscTOC::ctdb_toc_string`.
    pub fn lookup(&self, toc: &str) -> Result<CtdbResponse> {
        let url =
            with_query(BASE, &[("version", "3"), ("ctdb", "1"), ("metadata", "fast"), ("fuzzy", "1"), ("toc", toc)]);
        let response = self.transport.send(&Request::get(url).header("User-Agent", self.user_agent.clone()))?;
        if response.status >= 400 {
            return Err(ProviderError::Http(response.status));
        }
        Self::parse(&response.body)
    }

    /// Compares CRCs computed with the stride rule against every entry.
    pub fn verify(track_crc32s: &[u32], disc_crc32: Option<u32>, response: &CtdbResponse) -> CtdbVerification {
        let best = response.best_entry();
        let tracks = track_crc32s
            .iter()
            .enumerate()
            .map(|(i, crc)| {
                let matched_confidence =
                    response.entries.iter().filter(|e| e.track_crc32s.get(i) == Some(crc)).map(|e| e.confidence).sum();
                CtdbTrackVerdict {
                    track_index: i,
                    matched_confidence,
                    best_entry_matches: best.is_some_and(|b| b.track_crc32s.get(i) == Some(crc)),
                }
            })
            .collect();
        let disc_matches_best_entry = matches!((best, disc_crc32), (Some(b), Some(d)) if b.disc_crc32 == d);
        CtdbVerification { tracks, disc_matches_best_entry, total_confidence: response.total_confidence() }
    }

    pub fn parse(data: &[u8]) -> Result<CtdbResponse> {
        let mut reader = quick_xml::Reader::from_reader(data);
        let mut buf = vec![];
        let mut saw_root = false;
        let mut out = CtdbResponse::default();
        let mut current: Option<HashMap<String, String>> = None;
        let mut tracks: Vec<CtdbMetadataTrack> = vec![];
        let mut label: (Option<String>, Option<String>) = (None, None);
        let mut release: (Option<String>, Option<String>) = (None, None);
        loop {
            let event = reader.read_event_into(&mut buf).map_err(|_| ProviderError::CtdbInvalidResponse)?;
            let (start, ends) = match &event {
                Event::Start(e) => (Some(e), false),
                Event::Empty(e) => (Some(e), true),
                Event::End(e) => {
                    if e.name().into_inner() == "metadata" {
                        finish_metadata(&mut out, current.take(), &mut tracks, &label, &release);
                    }
                    (None, false)
                }
                Event::Eof => break,
                _ => (None, false),
            };
            if let Some(e) = start {
                let attrs = attributes(e)?;
                match e.name().into_inner() {
                    "ctdb" => saw_root = true,
                    "entry" => out.entries.push(CtdbEntry {
                        id: attrs.get("id").cloned().unwrap_or_default(),
                        confidence: attrs.get("confidence").and_then(|c| c.parse().ok()).unwrap_or(0),
                        disc_crc32: attrs.get("crc32").and_then(|c| u32::from_str_radix(c, 16).ok()).unwrap_or(0),
                        track_crc32s: attrs
                            .get("trackcrcs")
                            .map(|s| s.split(' ').filter_map(|c| u32::from_str_radix(c, 16).ok()).collect())
                            .unwrap_or_default(),
                        toc: attrs.get("toc").cloned().unwrap_or_default(),
                        has_parity: attrs.contains_key("hasparity"),
                    }),
                    "metadata" => {
                        current = Some(attrs);
                        tracks.clear();
                        label = (None, None);
                        release = (None, None);
                        if ends {
                            finish_metadata(&mut out, current.take(), &mut tracks, &label, &release);
                        }
                    }
                    "track" if current.is_some() => tracks.push(CtdbMetadataTrack {
                        name: attrs.get("name").cloned().unwrap_or_default(),
                        artist: attrs.get("artist").cloned(),
                    }),
                    "label" if current.is_some() => label = (attrs.get("name").cloned(), attrs.get("catno").cloned()),
                    "release" if current.is_some() => {
                        release = (attrs.get("country").cloned(), attrs.get("date").cloned())
                    }
                    _ => {}
                }
            }
            buf.clear();
        }
        if !saw_root {
            return Err(ProviderError::CtdbInvalidResponse);
        }
        Ok(out)
    }
}

fn attributes(e: &BytesStart) -> Result<HashMap<String, String>> {
    let mut map = HashMap::new();
    for a in e.attributes() {
        let a = a.map_err(|_| ProviderError::CtdbInvalidResponse)?;
        let key = a.key.into_inner().to_string();
        let value = a
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map_err(|_| ProviderError::CtdbInvalidResponse)?
            .into_owned();
        map.insert(key, value);
    }
    Ok(map)
}

fn swift_int(s: Option<&String>) -> Option<i64> {
    let s = s?;
    let d = s.strip_prefix(['+', '-']).unwrap_or(s);
    if d.is_empty() || !d.bytes().all(|b| b.is_ascii_digit()) { None } else { s.parse().ok() }
}

fn finish_metadata(
    out: &mut CtdbResponse,
    attrs: Option<HashMap<String, String>>,
    tracks: &mut Vec<CtdbMetadataTrack>,
    label: &(Option<String>, Option<String>),
    release: &(Option<String>, Option<String>),
) {
    let Some(attrs) = attrs else { return };
    out.metadata.push(CtdbMetadata {
        source: attrs.get("source").cloned().unwrap_or_default(),
        id: attrs.get("id").cloned(),
        album: attrs.get("album").cloned().unwrap_or_default(),
        artist: attrs.get("artist").cloned().unwrap_or_default(),
        year: attrs.get("year").cloned(),
        barcode: attrs.get("barcode").cloned().filter(|b| !b.is_empty()),
        label: label.0.clone(),
        catalog_number: label.1.clone(),
        country: release.0.clone(),
        release_date: release.1.clone(),
        disc_number: swift_int(attrs.get("discnumber")),
        disc_count: swift_int(attrs.get("disccount")),
        relevance: swift_int(attrs.get("relevance")).unwrap_or(0),
        tracks: std::mem::take(tracks),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const WALKIN: &str = r#"<ctdb xmlns="http://db.cuetools.net/ns/mmd-1.0#" xmlns:ext="http://db.cuetools.net/ns/ext-1.0#">
 <entry confidence="42" crc32="534054fd" hasparity="http://p.cuetools.net/1310779" id="1310779" npar="8" stride="5880" syndrome="cum06mLJ3PF1no7KpkMu1A==" toc="37:60712:98154:119544:139349:170557" trackcrcs="fe1b0b1d 1b8c2736 30f99024 40265ae9 f52a6619" />
 <entry confidence="3" crc32="deadbeef" id="99" npar="8" stride="5880" toc="37:60712:98154:119544:139349:170557" trackcrcs="00000001 1b8c2736 30f99024 40265ae9 00000005" />
 <metadata album="Walkin’" artist="Miles Davis All Stars" barcode="4988002013944" disccount="1" discname="" discnumber="1" id="86678562-ea2d-4be6-9763-92ac10aec3e1" relevance="81" source="musicbrainz" year="1957">
  <track artist="Miles Davis" name="Walkin&apos;" />
  <track artist="Miles Davis" name="Blue &apos;n&apos; Boogie" />
  <track artist="Miles Davis" name="Solar" />
  <track artist="Miles Davis" name="You Don&apos;t Know What Love Is" />
  <track artist="Miles Davis" name="Love Me or Leave Me" />
  <label catno="VDJ-1541" name="Prestige" />
  <release country="JP" date="1986-03-21" />
 </metadata>
 <metadata album="Walkin&apos;" artist="Miles Davis" disccount="1" discname="" discnumber="1" id="066c73cc-904c-3b10-a6c2-7eaa63f632cd" infourl="https://www.amazon.com/gp/product/B00000K0YB" relevance="81" source="musicbrainz" year="1957">
  <track name="Walkin&apos;" />
  <label catno="VICJ-60264" name="Prestige" />
  <release country="JP" date="1991-02-03" />
 </metadata>
</ctdb>"#;

    #[test]
    fn parses_entries_and_metadata() {
        let r = CUEToolsDBClient::parse(WALKIN.as_bytes()).unwrap();
        assert_eq!(r.entries.len(), 2);
        let e = r.best_entry().unwrap();
        assert_eq!((e.id.as_str(), e.confidence, e.disc_crc32), ("1310779", 42, 0x534054fd));
        assert_eq!(e.track_crc32s, [0xfe1b0b1d, 0x1b8c2736, 0x30f99024, 0x40265ae9, 0xf52a6619]);
        assert!(e.has_parity && !r.entries[1].has_parity);
        assert_eq!(r.total_confidence(), 45);
        let m = &r.metadata[0];
        assert_eq!(m.source, "musicbrainz");
        assert_eq!(m.id.as_deref(), Some("86678562-ea2d-4be6-9763-92ac10aec3e1"));
        assert_eq!((m.album.as_str(), m.artist.as_str()), ("Walkin’", "Miles Davis All Stars"));
        assert_eq!(m.barcode.as_deref(), Some("4988002013944"));
        assert_eq!((m.label.as_deref(), m.catalog_number.as_deref()), (Some("Prestige"), Some("VDJ-1541")));
        assert_eq!(
            (m.country.as_deref(), m.release_date.as_deref(), m.year.as_deref()),
            (Some("JP"), Some("1986-03-21"), Some("1957"))
        );
        assert_eq!(m.tracks.len(), 5);
        assert_eq!(m.tracks[1].name, "Blue 'n' Boogie");
        assert_eq!(m.tracks[1].artist.as_deref(), Some("Miles Davis"));
        assert_eq!(r.metadata[1].barcode, None);
        assert_eq!(r.metadata[1].tracks.len(), 1);
    }

    #[test]
    fn verifies_against_entries() {
        let r = CUEToolsDBClient::parse(WALKIN.as_bytes()).unwrap();
        let exact = CUEToolsDBClient::verify(
            &[0xfe1b0b1d, 0x1b8c2736, 0x30f99024, 0x40265ae9, 0xf52a6619],
            Some(0x534054fd),
            &r,
        );
        assert!(exact.all_tracks_match() && exact.disc_matches_best_entry);
        assert_eq!(exact.tracks[0].matched_confidence, 42);
        assert_eq!(exact.tracks[1].matched_confidence, 45);
        let off = CUEToolsDBClient::verify(&[0x11111111, 0x1b8c2736, 0x30f99024, 0x40265ae9, 0x22222222], Some(0), &r);
        assert!(!off.all_tracks_match());
        assert_eq!(off.matched_track_count(), 3);
        assert!(!off.disc_matches_best_entry);
        assert!(!off.tracks[0].best_entry_matches && off.tracks[1].best_entry_matches);
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(CUEToolsDBClient::parse(b"<html>nope</html>"), Err(ProviderError::CtdbInvalidResponse));
        assert_eq!(CUEToolsDBClient::parse(b"not xml"), Err(ProviderError::CtdbInvalidResponse));
    }
}
