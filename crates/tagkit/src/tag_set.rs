use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Picard field names as used in Vorbis comments. Every container maps
/// these names to its own frames/atoms/items; the app only sees this flat
/// schema.
pub mod field {
    pub const TITLE: &str = "TITLE";
    pub const ARTIST: &str = "ARTIST";
    pub const ARTIST_SORT: &str = "ARTISTSORT";
    pub const ARTISTS: &str = "ARTISTS";
    pub const ALBUM: &str = "ALBUM";
    pub const ALBUM_SORT: &str = "ALBUMSORT";
    pub const ALBUM_ARTIST: &str = "ALBUMARTIST";
    pub const ALBUM_ARTIST_SORT: &str = "ALBUMARTISTSORT";
    pub const DATE: &str = "DATE";
    pub const ORIGINAL_DATE: &str = "ORIGINALDATE";
    pub const ORIGINAL_YEAR: &str = "ORIGINALYEAR";
    pub const TRACK_NUMBER: &str = "TRACKNUMBER";
    pub const TRACK_TOTAL: &str = "TRACKTOTAL";
    pub const TOTAL_TRACKS: &str = "TOTALTRACKS";
    pub const DISC_NUMBER: &str = "DISCNUMBER";
    pub const DISC_TOTAL: &str = "DISCTOTAL";
    pub const TOTAL_DISCS: &str = "TOTALDISCS";
    pub const DISC_SUBTITLE: &str = "DISCSUBTITLE";
    pub const LABEL: &str = "LABEL";
    pub const CATALOG_NUMBER: &str = "CATALOGNUMBER";
    pub const BARCODE: &str = "BARCODE";
    pub const MEDIA: &str = "MEDIA";
    pub const RELEASE_COUNTRY: &str = "RELEASECOUNTRY";
    pub const RELEASE_STATUS: &str = "RELEASESTATUS";
    pub const RELEASE_TYPE: &str = "RELEASETYPE";
    pub const SCRIPT: &str = "SCRIPT";
    pub const COMPILATION: &str = "COMPILATION";
    pub const ISRC: &str = "ISRC";
    pub const GENRE: &str = "GENRE";
    pub const STYLE: &str = "STYLE";
    pub const COMPOSER: &str = "COMPOSER";
    pub const COMPOSER_SORT: &str = "COMPOSERSORT";
    pub const LYRICIST: &str = "LYRICIST";
    pub const WRITER: &str = "WRITER";
    pub const ARRANGER: &str = "ARRANGER";
    pub const CONDUCTOR: &str = "CONDUCTOR";
    pub const PERFORMER: &str = "PERFORMER";
    pub const PRODUCER: &str = "PRODUCER";
    pub const ENGINEER: &str = "ENGINEER";
    pub const MIXER: &str = "MIXER";
    pub const REMIXER: &str = "REMIXER";
    pub const WORK: &str = "WORK";
    pub const COMMENT: &str = "COMMENT";
    pub const MB_ALBUM_ID: &str = "MUSICBRAINZ_ALBUMID";
    pub const MB_ALBUM_ARTIST_ID: &str = "MUSICBRAINZ_ALBUMARTISTID";
    pub const MB_ARTIST_ID: &str = "MUSICBRAINZ_ARTISTID";
    /// Recording MBID (Picard naming).
    pub const MB_TRACK_ID: &str = "MUSICBRAINZ_TRACKID";
    pub const MB_RELEASE_TRACK_ID: &str = "MUSICBRAINZ_RELEASETRACKID";
    pub const MB_RELEASE_GROUP_ID: &str = "MUSICBRAINZ_RELEASEGROUPID";
    pub const MB_WORK_ID: &str = "MUSICBRAINZ_WORKID";
    pub const MB_DISC_ID: &str = "MUSICBRAINZ_DISCID";
    /// FreeDB.
    pub const DISC_ID: &str = "DISCID";
    pub const DISCOGS_RELEASE_ID: &str = "DISCOGS_RELEASE_ID";
    pub const ACOUSTID: &str = "ACOUSTID_ID";
    pub const ACOUSTID_FINGERPRINT: &str = "ACOUSTID_FINGERPRINT";

    /// Display / write order: the human fields first, identifiers last.
    pub const CANONICAL_ORDER: &[&str] = &[
        TITLE,
        ARTIST,
        ARTIST_SORT,
        ARTISTS,
        ALBUM,
        ALBUM_SORT,
        ALBUM_ARTIST,
        ALBUM_ARTIST_SORT,
        DATE,
        ORIGINAL_DATE,
        ORIGINAL_YEAR,
        TRACK_NUMBER,
        TRACK_TOTAL,
        TOTAL_TRACKS,
        DISC_NUMBER,
        DISC_TOTAL,
        TOTAL_DISCS,
        DISC_SUBTITLE,
        LABEL,
        CATALOG_NUMBER,
        BARCODE,
        MEDIA,
        RELEASE_COUNTRY,
        RELEASE_STATUS,
        RELEASE_TYPE,
        SCRIPT,
        COMPILATION,
        ISRC,
        GENRE,
        STYLE,
        COMPOSER,
        COMPOSER_SORT,
        LYRICIST,
        WRITER,
        ARRANGER,
        CONDUCTOR,
        PERFORMER,
        PRODUCER,
        ENGINEER,
        MIXER,
        REMIXER,
        WORK,
        COMMENT,
        MB_ALBUM_ID,
        MB_ALBUM_ARTIST_ID,
        MB_ARTIST_ID,
        MB_TRACK_ID,
        MB_RELEASE_TRACK_ID,
        MB_RELEASE_GROUP_ID,
        MB_WORK_ID,
        MB_DISC_ID,
        DISC_ID,
        DISCOGS_RELEASE_ID,
        ACOUSTID,
        ACOUSTID_FINGERPRINT,
    ];

    /// Fields that describe *which release* the file is: a merge clears
    /// them when the new candidate does not provide them.
    pub const IDENTITY_FIELDS: &[&str] = &[
        DATE,
        ORIGINAL_DATE,
        ORIGINAL_YEAR,
        LABEL,
        CATALOG_NUMBER,
        BARCODE,
        MEDIA,
        RELEASE_COUNTRY,
        RELEASE_STATUS,
        RELEASE_TYPE,
        SCRIPT,
        ISRC,
        MB_ALBUM_ID,
        MB_ALBUM_ARTIST_ID,
        MB_ARTIST_ID,
        MB_TRACK_ID,
        MB_RELEASE_TRACK_ID,
        MB_RELEASE_GROUP_ID,
        MB_WORK_ID,
        MB_DISC_ID,
        DISC_ID,
        DISCOGS_RELEASE_ID,
        DISC_SUBTITLE,
        WORK,
        ARTISTS,
    ];

    /// What only one pressing has: kept as they are when the album is
    /// tagged without an edition.
    pub const EDITION_FIELDS: &[&str] =
        &[LABEL, CATALOG_NUMBER, BARCODE, RELEASE_COUNTRY, RELEASE_STATUS, SCRIPT, ISRC, MB_DISC_ID, DISC_ID];
}

/// A flat, multi-valued tag map keyed by upper-case Picard names. Values
/// are trimmed; empty values are dropped; a name without values is absent.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TagSet {
    fields: BTreeMap<String, Vec<String>>,
}

impl TagSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_pairs<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let mut t = TagSet::new();
        for (n, v) in pairs {
            t.add(n, Some(v));
        }
        t
    }

    /// Every field, keyed by upper-case name.
    pub fn fields(&self) -> &BTreeMap<String, Vec<String>> {
        &self.fields
    }

    pub fn get(&self, name: &str) -> &[String] {
        self.fields.get(&name.to_uppercase()).map_or(&[], Vec::as_slice)
    }

    pub fn first(&self, name: &str) -> Option<&str> {
        self.get(name).first().map(String::as_str)
    }

    /// Replaces every value of `name`.
    pub fn put<S: AsRef<str>>(&mut self, name: &str, values: impl IntoIterator<Item = S>) {
        let key = name.to_uppercase();
        let values: Vec<String> =
            values.into_iter().map(|v| v.as_ref().trim().to_string()).filter(|v| !v.is_empty()).collect();
        if values.is_empty() {
            self.fields.remove(&key);
        } else {
            self.fields.insert(key, values);
        }
    }

    pub fn set(&mut self, name: &str, value: Option<&str>) {
        self.put(name, value);
    }

    /// Appends a value unless it is empty or already present.
    pub fn add(&mut self, name: &str, value: Option<&str>) {
        let Some(v) = value.map(str::trim).filter(|v| !v.is_empty()) else { return };
        let mut values = self.get(name).to_vec();
        if !values.iter().any(|x| x == v) {
            values.push(v.to_string());
        }
        self.put(name, values);
    }

    pub fn remove(&mut self, name: &str) {
        self.fields.remove(&name.to_uppercase());
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// Names in canonical order, unknown ones alphabetically after.
    pub fn names(&self) -> Vec<String> {
        let mut out: Vec<String> =
            field::CANONICAL_ORDER.iter().filter(|n| self.fields.contains_key(**n)).map(|n| n.to_string()).collect();
        out.extend(self.fields.keys().filter(|k| !field::CANONICAL_ORDER.contains(&k.as_str())).cloned());
        out
    }

    /// (name, value) pairs in write order, one per value.
    pub fn pairs(&self) -> Vec<(String, String)> {
        self.names()
            .into_iter()
            .flat_map(|n| self.get(&n).iter().map(move |v| (n.clone(), v.clone())).collect::<Vec<_>>())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_and_orders() {
        let mut t = TagSet::new();
        t.add("title", Some("  So What \n"));
        t.add("TITLE", Some("So What"));
        t.add("zeta", Some("z"));
        t.add("REPLAYGAIN_TRACK_GAIN", Some("-6.1 dB"));
        t.add("artist", Some(""));
        t.put("label", ["Columbia", " ", "Legacy"]);
        assert_eq!(t.get("Title"), ["So What"]);
        assert_eq!(t.names(), ["TITLE", "LABEL", "REPLAYGAIN_TRACK_GAIN", "ZETA"]);
        assert_eq!(t.pairs().len(), 5);
        t.set("label", None);
        assert!(t.get("LABEL").is_empty());
    }
}
