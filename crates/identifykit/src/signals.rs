//! What is known about an album before the providers are asked.

use librarykit::SourceHint;
use librarykit::path_serde;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SignalOrigin {
    Artwork,
    Tags,
    Cue,
    Ctdb,
    SacdText,
    FolderName,
    Fingerprint,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SignalValue {
    pub value: String,
    pub origin: SignalOrigin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl SignalValue {
    pub fn new(value: impl Into<String>, origin: SignalOrigin) -> SignalValue {
        SignalValue { value: value.into(), origin, detail: None }
    }

    pub fn with_detail(value: impl Into<String>, origin: SignalOrigin, detail: Option<String>) -> SignalValue {
        SignalValue { value: value.into(), origin, detail }
    }
}

/// One local track as the scorer sees it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalTrack {
    /// 0-based across the whole album.
    pub index: usize,
    pub disc_number: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<f64>,
    /// The track file, or the image it lives in.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "path_serde::option")]
    pub url: Option<PathBuf>,
    /// Seconds into the image (CUE images).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_start: Option<f64>,
}

impl LocalTrack {
    pub fn new(index: usize, disc_number: i64, duration_seconds: Option<f64>) -> LocalTrack {
        LocalTrack { index, disc_number, title: None, duration_seconds, url: None, image_start: None }
    }
}

/// Barcodes and catalog numbers read off one image (filled by the vision
/// phase; empty until then).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtworkScanResult {
    #[serde(with = "path_serde")]
    pub url: PathBuf,
    /// EAN-13 / EAN-8 / UPC-E payloads.
    pub barcodes: Vec<String>,
    /// "PREFIX-NUMBER".
    pub catalog_numbers: Vec<String>,
    pub recognized_texts: Vec<String>,
    pub confidence: f32,
}

impl ArtworkScanResult {
    pub fn has_hit(&self) -> bool {
        !self.barcodes.is_empty() || !self.catalog_numbers.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LocalFormat {
    Sacd,
    Cd,
    HiRes,
    Unknown,
}

fn one() -> Vec<i64> {
    vec![1]
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumSignals {
    #[serde(default)]
    pub barcodes: Vec<SignalValue>,
    #[serde(default)]
    pub catalog_numbers: Vec<SignalValue>,
    #[serde(rename = "mbReleaseIDs", default)]
    pub mb_release_ids: Vec<SignalValue>,
    #[serde(rename = "discID", default, skip_serializing_if = "Option::is_none")]
    pub disc_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toc_string: Option<String>,
    /// Release sets: the disc positions the files cover, the total the set
    /// declares ("disc 2 of 3"), and one disc ID per position.
    #[serde(default = "one")]
    pub present_discs: Vec<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_disc_total: Option<i64>,
    #[serde(rename = "discIDs", default)]
    pub disc_ids: BTreeMap<i64, String>,
    #[serde(default)]
    pub toc_strings: BTreeMap<i64, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year_hint: Option<String>,
    /// The folder's title, kept when tags or the CUE override `album_hint`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_album_hint: Option<String>,
    /// "20th Anniversary Edition".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edition_hint: Option<String>,
    /// Year of the pressing / remaster.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edition_year_hint: Option<String>,
    /// ISO code, from the folder name or tags.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country_hint: Option<String>,
    /// Medium named in the folder or the MEDIA tag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_hint: Option<SourceHint>,
    #[serde(default = "one_count")]
    pub disc_count: i64,
    #[serde(default)]
    pub tracks: Vec<LocalTrack>,
    #[serde(default)]
    pub existing_tags: BTreeMap<String, String>,
    #[serde(default)]
    pub artwork_scans: Vec<ArtworkScanResult>,
    #[serde(rename = "isDSD", default)]
    pub is_dsd: bool,
    /// A Scarletbook ISO, not loose DSF files.
    #[serde(rename = "isSACDImage", default)]
    pub is_sacd_image: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bits_per_sample: Option<i64>,
}

fn one_count() -> i64 {
    1
}

impl Default for AlbumSignals {
    fn default() -> Self {
        AlbumSignals {
            barcodes: vec![],
            catalog_numbers: vec![],
            mb_release_ids: vec![],
            disc_id: None,
            toc_string: None,
            present_discs: one(),
            declared_disc_total: None,
            disc_ids: BTreeMap::new(),
            toc_strings: BTreeMap::new(),
            artist_hint: None,
            album_hint: None,
            year_hint: None,
            folder_album_hint: None,
            edition_hint: None,
            edition_year_hint: None,
            country_hint: None,
            source_hint: None,
            disc_count: 1,
            tracks: vec![],
            existing_tags: BTreeMap::new(),
            artwork_scans: vec![],
            is_dsd: false,
            is_sacd_image: false,
            sample_rate: None,
            bits_per_sample: None,
        }
    }
}

fn unique(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    values.into_iter().filter(|v| seen.insert(v.clone())).collect()
}

impl AlbumSignals {
    /// What physical source the local files can come from.
    pub fn local_format(&self) -> LocalFormat {
        if self.is_dsd {
            return LocalFormat::Sacd;
        }
        let (Some(rate), Some(bits)) = (self.sample_rate, self.bits_per_sample) else { return LocalFormat::Unknown };
        if rate == 44100 && bits <= 16 {
            return LocalFormat::Cd;
        }
        if rate > 44100 || bits > 16 {
            return LocalFormat::HiRes;
        }
        LocalFormat::Unknown
    }

    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }

    /// A partial or declared set: candidates are matched medium by medium.
    pub fn is_set(&self) -> bool {
        self.present_discs != [1] || self.declared_disc_total.is_some()
    }

    pub fn all_disc_ids(&self) -> BTreeSet<String> {
        self.disc_ids.values().cloned().chain(self.disc_id.clone()).collect()
    }

    /// An all-digit 12–14 character "catalog number" is an EAN/UPC (SACD
    /// album catalogs often hold the box barcode).
    pub fn looks_like_barcode(s: &str) -> bool {
        let t = providerkit::trim_whitespaces(s);
        (12..=14).contains(&t.chars().count()) && t.chars().all(|c| c.is_numeric())
    }

    pub fn unique_barcodes(&self) -> Vec<String> {
        unique(
            self.barcodes
                .iter()
                .map(|b| b.value.clone())
                .chain(self.catalog_numbers.iter().map(|c| c.value.clone()).filter(|c| Self::looks_like_barcode(c))),
        )
    }

    pub fn unique_catalog_numbers(&self) -> Vec<String> {
        unique(self.catalog_numbers.iter().map(|c| c.value.clone()).filter(|c| !Self::looks_like_barcode(c)))
    }

    pub fn unique_release_ids(&self) -> Vec<String> {
        unique(self.mb_release_ids.iter().map(|m| m.value.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_catalog_is_a_barcode() {
        let s = AlbumSignals {
            catalog_numbers: vec![
                SignalValue::new("4988006888883", SignalOrigin::SacdText),
                SignalValue::new("TOGE-12061", SignalOrigin::SacdText),
            ],
            ..Default::default()
        };
        assert_eq!(s.unique_barcodes(), ["4988006888883"]);
        assert_eq!(s.unique_catalog_numbers(), ["TOGE-12061"]);
    }
}
