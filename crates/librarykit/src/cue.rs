use crate::audio_format::is_number;
use crate::text_decoder::TextDecoder;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// A position on a CD in 1/75 s frames, as CUE sheets express it (mm:ss:ff).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CueTime {
    pub frames: i64,
}

impl CueTime {
    pub const FRAMES_PER_SECOND: i64 = 75;

    pub fn new(frames: i64) -> Self {
        CueTime { frames }
    }

    /// "mm:ss:ff" — minutes may exceed 99 on long images.
    pub fn from_msf(msf: &str) -> Option<CueTime> {
        let parts: Vec<&str> = msf.split(':').collect();
        if parts.len() != 3 {
            return None;
        }
        let m: i64 = parts[0].parse().ok()?;
        let s: i64 = parts[1].parse().ok()?;
        let f: i64 = parts[2].parse().ok()?;
        if m < 0 || !(0..60).contains(&s) || !(0..Self::FRAMES_PER_SECOND).contains(&f) {
            return None;
        }
        Some(CueTime { frames: (m * 60 + s) * Self::FRAMES_PER_SECOND + f })
    }

    pub fn minutes(self) -> i64 {
        self.frames / (60 * Self::FRAMES_PER_SECOND)
    }
    pub fn seconds(self) -> i64 {
        (self.frames / Self::FRAMES_PER_SECOND) % 60
    }
    pub fn frame(self) -> i64 {
        self.frames % Self::FRAMES_PER_SECOND
    }
    pub fn total_seconds(self) -> f64 {
        self.frames as f64 / Self::FRAMES_PER_SECOND as f64
    }
    pub fn msf_string(self) -> String {
        format!("{:02}:{:02}:{:02}", self.minutes(), self.seconds(), self.frame())
    }
    /// Sample offset at the given rate (588 samples per frame at 44.1 kHz).
    pub fn samples(self, sample_rate: i64) -> i64 {
        self.frames * sample_rate / Self::FRAMES_PER_SECOND
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CueIndex {
    pub number: i64,
    pub time: CueTime,
    /// 0 when the time is relative to the track's own FILE. EAC's "gaps
    /// appended to previous track" sheets put a track's INDEX 00 at the end
    /// of one file and its INDEX 01 after the next FILE line: that index is
    /// relative to a later file, `file_offset` files after the track's.
    #[serde(default)]
    pub file_offset: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CueRem {
    /// Upper-cased, e.g. "DISCID", "COMMENT", "DATE".
    pub key: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CueTrack {
    pub number: i64,
    /// "AUDIO", "MODE1/2352", …
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub performer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub songwriter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub isrc: Option<String>,
    pub flags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pregap: Option<CueTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub postgap: Option<CueTime>,
    pub indexes: Vec<CueIndex>,
    pub rems: Vec<CueRem>,
}

impl CueTrack {
    pub fn new(number: i64, kind: &str) -> Self {
        CueTrack {
            number,
            kind: kind.to_string(),
            title: None,
            performer: None,
            songwriter: None,
            isrc: None,
            flags: vec![],
            pregap: None,
            postgap: None,
            indexes: vec![],
            rems: vec![],
        }
    }

    pub fn is_audio(&self) -> bool {
        self.kind.to_uppercase() == "AUDIO"
    }

    pub fn index(&self, n: i64) -> Option<CueTime> {
        self.indexes.iter().find(|i| i.number == n).map(|i| i.time)
    }

    /// INDEX 01 is where players start the track.
    pub fn start(&self) -> Option<CueTime> {
        self.index(1)
    }

    /// How many FILEs after the track's own FILE its INDEX 01 is measured in.
    pub fn start_file_offset(&self) -> usize {
        self.indexes.iter().find(|i| i.number == 1).map_or(0, |i| i.file_offset)
    }

    /// INDEX 00 marks the track's pregap.
    pub fn pregap_start(&self) -> Option<CueTime> {
        self.index(0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CueFile {
    pub name: String,
    /// "WAVE", "BINARY", "MP3", …
    #[serde(rename = "type")]
    pub kind: String,
    pub tracks: Vec<CueTrack>,
}

impl CueFile {
    /// The referenced file next to the sheet. A leading "/" is appended, not
    /// absolute, as Foundation's `appending(path:)` does.
    pub fn path_in(&self, folder: &Path) -> PathBuf {
        folder.join(self.name.trim_start_matches('/'))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CueSheet {
    /// UPC/EAN from the CATALOG command.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cd_text_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub performer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub songwriter: Option<String>,
    pub rems: Vec<CueRem>,
    pub files: Vec<CueFile>,
    /// How the bytes were decoded (diagnostics).
    pub encoding_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CueParseError {
    Empty,
    NoFiles,
    TrackOutsideFile { line: usize },
    Io(String),
}

impl fmt::Display for CueParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CueParseError::Empty => write!(f, "The CUE sheet is empty."),
            CueParseError::NoFiles => write!(f, "The CUE sheet has no FILE entries."),
            CueParseError::TrackOutsideFile { line } => write!(f, "TRACK before any FILE at line {line}."),
            CueParseError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for CueParseError {}

static BARCODE_IN_TEXT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)barcode\s*[:=]?\s*([0-9][0-9 ]{7,}[0-9])").unwrap());
static CATALOG_IN_TEXT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b([A-Z]{2,10})[- ]?(\d{3,7}(?:-\d{1,2})?)\b").unwrap());

/// Swift's `CharacterSet.whitespaces`: spaces and tabs, not line breaks.
fn is_blank(c: char) -> bool {
    c.is_whitespace() && !matches!(c, '\n' | '\r' | '\u{0B}' | '\u{0C}' | '\u{85}' | '\u{2028}' | '\u{2029}')
}

impl Default for CueSheet {
    fn default() -> Self {
        CueSheet {
            catalog: None,
            cd_text_file: None,
            title: None,
            performer: None,
            songwriter: None,
            rems: vec![],
            files: vec![],
            encoding_name: "utf-8".to_string(),
        }
    }
}

impl CueSheet {
    pub fn tracks(&self) -> impl Iterator<Item = &CueTrack> {
        self.files.iter().flat_map(|f| f.tracks.iter())
    }

    pub fn audio_tracks(&self) -> Vec<&CueTrack> {
        self.tracks().filter(|t| t.is_audio()).collect()
    }

    pub fn is_single_file(&self) -> bool {
        self.files.len() == 1
    }

    pub fn rem(&self, key: &str) -> Option<&str> {
        let key = key.to_uppercase();
        self.rems.iter().find(|r| r.key == key).map(|r| r.value.as_str())
    }

    pub fn referenced_files(&self, folder: &Path) -> Vec<PathBuf> {
        self.files.iter().map(|f| f.path_in(folder)).collect()
    }

    /// Pregap of track 1 in a single-image rip: audio before INDEX 01 of
    /// track 1. Whether it is a real hidden track (HTOA) or two seconds of
    /// silence is decided at split time.
    pub fn track_one_pregap(&self) -> Option<CueTime> {
        let start = self.audio_tracks().first()?.start()?;
        (start.frames > 0).then_some(start)
    }

    pub fn disc_id(&self) -> Option<&str> {
        self.rem("DISCID")
    }
    pub fn date(&self) -> Option<&str> {
        self.rem("DATE")
    }
    pub fn genre(&self) -> Option<&str> {
        self.rem("GENRE")
    }

    /// Barcode from CATALOG, or from a REM COMMENT such as
    /// "Universal Music – UICY-40164\Barcode: 4988031277102\Reissue…".
    pub fn barcode_hint(&self) -> Option<String> {
        if let Some(catalog) = &self.catalog
            && catalog.chars().count() >= 8
            && catalog.chars().all(is_number)
            && !catalog.chars().all(|c| c == '0')
        {
            return Some(catalog.clone());
        }
        for rem in self.rems.iter().filter(|r| r.key == "COMMENT" || r.key == "BARCODE") {
            if let Some(c) = BARCODE_IN_TEXT.captures(&rem.value) {
                return Some(c[1].replace(' ', ""));
            }
            if rem.key == "BARCODE" {
                let digits: String = rem.value.chars().filter(|&c| is_number(c)).collect();
                if digits.chars().count() >= 8 {
                    return Some(digits);
                }
            }
        }
        None
    }

    /// Catalog-number-looking tokens in comments, e.g. "UICY-40164", "FCD 8410-2".
    pub fn catalog_number_hints(&self) -> Vec<String> {
        let mut out: Vec<String> = vec![];
        for rem in self.rems.iter().filter(|r| matches!(r.key.as_str(), "COMMENT" | "CATALOGNUMBER" | "CATNO")) {
            for c in CATALOG_IN_TEXT.captures_iter(&rem.value) {
                let hint = format!("{}-{}", &c[1], &c[2]);
                if !out.contains(&hint) {
                    out.push(hint);
                }
            }
        }
        out
    }

    pub fn parse_file(path: &Path) -> Result<CueSheet, CueParseError> {
        let data = std::fs::read(path).map_err(|e| CueParseError::Io(e.to_string()))?;
        Self::parse_bytes(&data)
    }

    pub fn parse_bytes(data: &[u8]) -> Result<CueSheet, CueParseError> {
        let decoded = TextDecoder::decode(data);
        let mut sheet = Self::parse_text(&decoded.text)?;
        sheet.encoding_name = decoded.encoding_name.to_string();
        Ok(sheet)
    }

    pub fn parse_text(text: &str) -> Result<CueSheet, CueParseError> {
        let mut sheet = CueSheet::default();
        let mut file_index: Option<usize> = None;
        let mut track_index: Option<usize> = None;
        let mut last_track: Option<(usize, usize)> = None; // survives a FILE line
        let mut saw_anything = false;

        for (n, raw) in text.split('\n').enumerate() {
            let line_number = n + 1;
            let line = raw.trim();
            if line.is_empty() {
                continue;
            }
            saw_anything = true;
            let tokens = Self::tokenize(line);
            let Some(command) = tokens.first().map(|t| t.to_uppercase()) else { continue };
            let args = &tokens[1..];

            match command.as_str() {
                "REM" => {
                    let Some(key) = args.first() else { continue };
                    let rem = CueRem { key: key.to_uppercase(), value: unquoted(&remainder(line, 2)) };
                    match (file_index, track_index) {
                        (Some(fi), Some(ti)) => sheet.files[fi].tracks[ti].rems.push(rem),
                        _ => sheet.rems.push(rem),
                    }
                }
                "CATALOG" => sheet.catalog = args.first().cloned(),
                "CDTEXTFILE" => sheet.cd_text_file = Some(unquoted(&remainder(line, 1))),
                "FILE" => {
                    // FILE "name" TYPE — the type is the last token; unquoted
                    // names with spaces put everything in between.
                    if args.is_empty() {
                        continue;
                    }
                    let (name, kind) = if args.len() >= 2 {
                        (args[..args.len() - 1].join(" "), args[args.len() - 1].to_uppercase())
                    } else {
                        (args[0].clone(), "WAVE".to_string())
                    };
                    sheet.files.push(CueFile { name, kind, tracks: vec![] });
                    file_index = Some(sheet.files.len() - 1);
                    track_index = None;
                }
                "TRACK" => {
                    let Some(fi) = file_index else {
                        return Err(CueParseError::TrackOutsideFile { line: line_number });
                    };
                    let number = args
                        .first()
                        .and_then(|a| a.parse::<i64>().ok())
                        .unwrap_or_else(|| sheet.tracks().count() as i64 + 1);
                    let kind = args.get(1).map_or("AUDIO".to_string(), |k| k.to_uppercase());
                    sheet.files[fi].tracks.push(CueTrack::new(number, &kind));
                    let ti = sheet.files[fi].tracks.len() - 1;
                    track_index = Some(ti);
                    last_track = Some((fi, ti));
                }
                "INDEX" => {
                    let Some(fi) = file_index else { continue };
                    if args.len() < 2 {
                        continue;
                    }
                    let (Ok(n), Some(time)) = (args[0].parse::<i64>(), CueTime::from_msf(&args[1])) else { continue };
                    if let Some(ti) = track_index {
                        sheet.files[fi].tracks[ti].indexes.push(CueIndex { number: n, time, file_offset: 0 });
                    } else if let Some((lf, lt)) = last_track {
                        // INDEX after a new FILE and before any TRACK: it still
                        // belongs to the previous track, measured in this file.
                        sheet.files[lf].tracks[lt].indexes.push(CueIndex { number: n, time, file_offset: fi - lf });
                    }
                }
                "PREGAP" | "POSTGAP" => {
                    let (Some(fi), Some(ti)) = (file_index, track_index) else { continue };
                    let Some(time) = args.first().and_then(|a| CueTime::from_msf(a)) else { continue };
                    let track = &mut sheet.files[fi].tracks[ti];
                    if command == "PREGAP" {
                        track.pregap = Some(time);
                    } else {
                        track.postgap = Some(time);
                    }
                }
                "TITLE" | "PERFORMER" | "SONGWRITER" | "ISRC" | "FLAGS" => {
                    let value = unquoted(&remainder(line, 1));
                    if let (Some(fi), Some(ti)) = (file_index, track_index) {
                        let track = &mut sheet.files[fi].tracks[ti];
                        match command.as_str() {
                            "TITLE" => track.title = Some(value),
                            "PERFORMER" => track.performer = Some(value),
                            "SONGWRITER" => track.songwriter = Some(value),
                            "ISRC" => track.isrc = Some(value),
                            _ => track.flags = args.iter().map(|a| a.to_uppercase()).collect(),
                        }
                    } else {
                        match command.as_str() {
                            "TITLE" => sheet.title = Some(value),
                            "PERFORMER" => sheet.performer = Some(value),
                            "SONGWRITER" => sheet.songwriter = Some(value),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }

        if !saw_anything {
            return Err(CueParseError::Empty);
        }
        if sheet.files.is_empty() {
            return Err(CueParseError::NoFiles);
        }
        Ok(sheet)
    }

    /// Splits on spaces and tabs, keeping quoted strings together (quotes
    /// removed, empty quotes kept as an empty token).
    pub fn tokenize(line: &str) -> Vec<String> {
        let mut tokens = vec![];
        let mut current = String::new();
        let (mut in_quotes, mut had_quotes) = (false, false);
        for ch in line.chars() {
            if ch == '"' {
                in_quotes = !in_quotes;
                had_quotes = true;
                continue;
            }
            if (ch == ' ' || ch == '\t') && !in_quotes {
                if !current.is_empty() || had_quotes {
                    tokens.push(std::mem::take(&mut current));
                    had_quotes = false;
                }
            } else {
                current.push(ch);
            }
        }
        if !current.is_empty() || had_quotes {
            tokens.push(current);
        }
        tokens
    }
}

/// Everything after the first `count` whitespace-separated tokens, so
/// TITLE values keep their internal spacing and quotes can be stripped.
fn remainder(line: &str, count: usize) -> String {
    let sep = |c: char| c == ' ' || c == '\t';
    let mut rest = line;
    for _ in 0..count {
        if rest.is_empty() {
            break;
        }
        rest = rest.trim_start_matches(sep);
        rest = rest.find(sep).map_or("", |i| &rest[i..]);
    }
    rest.trim_matches(is_blank).to_string()
}

fn unquoted(s: &str) -> String {
    let t = s.trim_matches(is_blank);
    if t.chars().count() >= 2 && t.starts_with('"') && t.ends_with('"') {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) const SINGLE_IMAGE: &str = r#"REM GENRE Rock
REM DATE 1971
REM DISCID 8A0B5F0A
REM COMMENT "Universal Music – UICY-40164\Barcode: 4988031277102\Reissue"
CATALOG 0000000000000
PERFORMER "The Rolling Stones"
TITLE "Sticky Fingers"
FILE "Sticky Fingers.flac" WAVE
  TRACK 01 AUDIO
    TITLE "Brown Sugar"
    PERFORMER "The Rolling Stones"
    INDEX 00 00:00:00
    INDEX 01 00:00:33
  TRACK 02 AUDIO
    TITLE "Sway"
    ISRC GBUM71029604
    INDEX 00 03:48:20
    INDEX 01 03:50:12
  TRACK 03 AUDIO
    TITLE "Wild Horses"
    INDEX 01 07:41:60"#;

    const TAIL: &[u8] = b"\"\nFILE \"a.flac\" WAVE\n TRACK 01 AUDIO\n INDEX 01 00:00:00\n";

    fn with_title(encoded: &[u8]) -> Vec<u8> {
        let mut data = b"TITLE \"".to_vec();
        data.extend_from_slice(encoded);
        data.extend_from_slice(TAIL);
        data
    }

    #[test]
    fn parses_single_image_sheet() {
        let sheet = CueSheet::parse_text(SINGLE_IMAGE).unwrap();
        assert_eq!(sheet.title.as_deref(), Some("Sticky Fingers"));
        assert_eq!(sheet.performer.as_deref(), Some("The Rolling Stones"));
        assert!(sheet.is_single_file());
        assert_eq!(sheet.files[0].name, "Sticky Fingers.flac");
        assert_eq!(sheet.files[0].kind, "WAVE");
        let audio = sheet.audio_tracks();
        assert_eq!(audio.len(), 3);
        let t2 = audio[1];
        assert_eq!(t2.number, 2);
        assert_eq!(t2.title.as_deref(), Some("Sway"));
        assert_eq!(t2.isrc.as_deref(), Some("GBUM71029604"));
        assert_eq!(t2.pregap_start(), CueTime::from_msf("03:48:20"));
        assert_eq!(t2.start(), CueTime::from_msf("03:50:12"));
        assert_eq!(sheet.date(), Some("1971"));
        assert_eq!(sheet.genre(), Some("Rock"));
        assert_eq!(sheet.disc_id(), Some("8A0B5F0A"));
        assert!(sheet.rem("COMMENT").unwrap().starts_with("Universal Music"), "REM values lose their quotes");
        assert_eq!(sheet.track_one_pregap().unwrap().frames, 33);
    }

    #[test]
    fn extracts_identification_hints() {
        let sheet = CueSheet::parse_text(SINGLE_IMAGE).unwrap();
        // CATALOG of all zeros is a placeholder; the REM COMMENT carries the real barcode.
        assert_eq!(sheet.barcode_hint().as_deref(), Some("4988031277102"));
        assert_eq!(sheet.catalog_number_hints(), ["UICY-40164"]);
        let mut with_catalog = sheet.clone();
        with_catalog.catalog = Some("0602547858917".into());
        assert_eq!(with_catalog.barcode_hint().as_deref(), Some("0602547858917"));
    }

    #[test]
    fn parses_multi_file_sheet_and_unquoted_names() {
        let text = "TITLE Ella and Oscar\nFILE 01 - Mean to Me.wav WAVE\n  TRACK 01 AUDIO\n    TITLE \"Mean to Me\"\n    INDEX 00 00:00:00\n    INDEX 01 00:00:32\nFILE \"02 - How Long.wav\" WAVE\n  TRACK 02 AUDIO\n    INDEX 01 00:00:00";
        let sheet = CueSheet::parse_text(text).unwrap();
        assert_eq!(sheet.title.as_deref(), Some("Ella and Oscar"));
        assert_eq!(sheet.files.len(), 2);
        assert_eq!(sheet.files[0].name, "01 - Mean to Me.wav");
        assert_eq!(sheet.files[1].name, "02 - How Long.wav");
        assert!(!sheet.is_single_file());
        assert_eq!(sheet.tracks().map(|t| t.number).collect::<Vec<_>>(), [1, 2]);
    }

    #[test]
    fn rejects_sheets_without_files() {
        assert_eq!(CueSheet::parse_text("TITLE \"x\"\nPERFORMER \"y\"\n"), Err(CueParseError::NoFiles));
        assert_eq!(CueSheet::parse_text("\n\n"), Err(CueParseError::Empty));
    }

    #[test]
    fn cue_time_arithmetic() {
        let t = CueTime::from_msf("03:50:12").unwrap();
        assert_eq!(t.frames, (3 * 60 + 50) * 75 + 12);
        assert_eq!(t.msf_string(), "03:50:12");
        assert_eq!(t.samples(44100), t.frames * 588);
        assert_eq!(CueTime::from_msf("00:00:75"), None);
        assert_eq!(CueTime::from_msf("1:2"), None);
        assert_eq!(CueTime::from_msf("100:00:00").unwrap().minutes(), 100);
    }

    #[test]
    fn tokenizer_keeps_quoted_strings_and_empty_quotes() {
        assert_eq!(CueSheet::tokenize("FILE \"a b.flac\" WAVE"), ["FILE", "a b.flac", "WAVE"]);
        assert_eq!(CueSheet::tokenize("TITLE \"\""), ["TITLE", ""]);
        assert_eq!(CueSheet::tokenize("  INDEX 01   00:02:00 "), ["INDEX", "01", "00:02:00"]);
    }

    #[test]
    fn decodes_utf8_with_bom() {
        let mut data = vec![0xEF, 0xBB, 0xBF];
        data.extend_from_slice(
            "TITLE \"Leño – ¡Corre!\"\nFILE \"a.flac\" WAVE\n TRACK 01 AUDIO\n INDEX 01 00:00:00\n".as_bytes(),
        );
        let sheet = CueSheet::parse_bytes(&data).unwrap();
        assert_eq!(sheet.encoding_name, "utf-8-bom");
        assert_eq!(sheet.title.as_deref(), Some("Leño – ¡Corre!"));
    }

    #[test]
    fn decodes_cp1252_smart_quotes() {
        let mut bytes = b"Sailor".to_vec();
        bytes.push(0x92);
        bytes.extend_from_slice(b"s Lament ");
        bytes.push(0x96);
        bytes.extend_from_slice(b" Live");
        let sheet = CueSheet::parse_bytes(&with_title(&bytes)).unwrap();
        assert_eq!(sheet.encoding_name, "cp1252");
        assert_eq!(sheet.title.as_deref(), Some("Sailor’s Lament – Live"));
    }

    #[test]
    fn decodes_shift_jis() {
        let title = "坂本龍一 - 戦場のメリークリスマス";
        let (sjis, _, _) = encoding_rs::SHIFT_JIS.encode(title);
        let sheet = CueSheet::parse_bytes(&with_title(&sjis)).unwrap();
        assert_eq!(sheet.encoding_name, "shift_jis");
        assert_eq!(sheet.title.as_deref(), Some(title));
    }

    #[test]
    fn decodes_cp1251() {
        let title = "Кино - Группа крови";
        let (cp, _, _) = encoding_rs::WINDOWS_1251.encode(title);
        let sheet = CueSheet::parse_bytes(&with_title(&cp)).unwrap();
        assert_eq!(sheet.encoding_name, "cp1251");
        assert_eq!(sheet.title.as_deref(), Some(title));
    }

    #[test]
    fn spanish_accents_stay_latin() {
        // "¡Qué desilusión!" in Latin-1: bytes A1, E9, F3 must not be mistaken for Cyrillic.
        let title = "¡Qué desilusión!";
        let (latin, _, _) = encoding_rs::WINDOWS_1252.encode(title);
        let sheet = CueSheet::parse_bytes(&with_title(&latin)).unwrap();
        assert_eq!(sheet.encoding_name, "cp1252");
        assert_eq!(sheet.title.as_deref(), Some(title));
    }
}
