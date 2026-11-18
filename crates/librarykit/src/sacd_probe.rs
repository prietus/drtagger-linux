//! Minimal Scarletbook reader: enough of the master TOC, master text and
//! area TOCs to recognise a SACD ISO and describe it (areas, track counts,
//! DST, disc title/artist). Track extraction lives in sacdkit.
//!
//! Sectors are 2048 bytes; all integers are big-endian.
//!   sector 510  master TOC   "SACDMTOC"
//!   sector 511  master text  "SACDText"
//!   master TOC  +64 area 1 TOC start, +72 area 2 TOC start (sector numbers)
//!   area TOC    +21 frame format (0 DST, 2/3 DSD), +32 channels,
//!               +64 play time (min, sec, frames), +68 track offset,
//!               +69 track count, +72/+76 track start/end sectors

use crate::text_decoder::TextDecoder;
use encoding_rs::SHIFT_JIS;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FrameFormat {
    Dst,
    Dsd,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SacdArea {
    pub is_multichannel: bool,
    pub frame_format: FrameFormat,
    pub channel_count: i64,
    pub track_offset: i64,
    pub track_count: i64,
    pub play_time_seconds: i64,
    pub toc_sector: u32,
    pub track_start_sector: u32,
    pub track_end_sector: u32,
}

impl SacdArea {
    pub fn is_dst(&self) -> bool {
        self.frame_format == FrameFormat::Dst
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SACDInfo {
    pub album_set_size: i64,
    pub album_sequence_number: i64,
    pub album_catalog_number: String,
    pub disc_catalog_number: String,
    /// "YYYY-MM-DD" when the disc carries one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disc_date: Option<String>,
    /// e.g. "en", "ja".
    pub locale_language: String,
    pub character_set_code: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disc_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disc_artist: Option<String>,
    pub areas: Vec<SacdArea>,
}

fn non_empty(s: &Option<String>) -> Option<String> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

impl SACDInfo {
    pub fn stereo_area(&self) -> Option<&SacdArea> {
        self.areas.iter().find(|a| !a.is_multichannel)
    }
    pub fn multichannel_area(&self) -> Option<&SacdArea> {
        self.areas.iter().find(|a| a.is_multichannel)
    }
    pub fn has_dst(&self) -> bool {
        self.areas.iter().any(SacdArea::is_dst)
    }
    pub fn track_count(&self) -> i64 {
        self.stereo_area().or(self.areas.first()).map_or(0, |a| a.track_count)
    }
    /// Prefer the disc-level text, fall back to the album-level text.
    pub fn title(&self) -> Option<String> {
        non_empty(&self.disc_title).or_else(|| non_empty(&self.album_title))
    }
    pub fn artist(&self) -> Option<String> {
        non_empty(&self.disc_artist).or_else(|| non_empty(&self.album_artist))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum SacdProbeError {
    NotSacd,
    Truncated,
    BadAreaToc(u32),
    Io(String),
}

impl fmt::Display for SacdProbeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SacdProbeError::NotSacd => write!(f, "Not a SACD image (no Scarletbook master TOC)."),
            SacdProbeError::Truncated => write!(f, "The image is shorter than a SACD master TOC."),
            SacdProbeError::BadAreaToc(s) => write!(f, "Area TOC at sector {s} is not readable."),
            SacdProbeError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SacdProbeError {}

impl From<std::io::Error> for SacdProbeError {
    fn from(e: std::io::Error) -> Self {
        SacdProbeError::Io(e.to_string())
    }
}

pub struct SACDProbe;

impl SACDProbe {
    pub const SECTOR_SIZE: usize = 2048;
    pub const MASTER_TOC_SECTOR: u64 = 510;

    /// Cheap check: only the master TOC sector is read.
    pub fn is_sacd(path: &Path) -> bool {
        let Ok(mut file) = File::open(path) else { return false };
        read_sector(&mut file, Self::MASTER_TOC_SECTOR).is_ok_and(|d| d.starts_with(b"SACDMTOC"))
    }

    pub fn probe(path: &Path) -> Result<SACDInfo, SacdProbeError> {
        let mut file = File::open(path)?;
        let master = read_sector(&mut file, Self::MASTER_TOC_SECTOR)?;
        if master.len() != Self::SECTOR_SIZE {
            return Err(SacdProbeError::Truncated);
        }
        if !master.starts_with(b"SACDMTOC") {
            return Err(SacdProbeError::NotSacd);
        }

        let year = be16(&master, 120) as i64;
        let (month, day) = (master[122] as i64, master[123] as i64);
        let disc_date = (year > 0).then(|| format!("{:04}-{:02}-{:02}", year, month.max(1), day.max(1)));
        let char_set = master[138] as i64;

        // Master text (sector 511). Positions are offsets within the sector.
        let (mut album_title, mut album_artist, mut disc_title, mut disc_artist) = (None, None, None, None);
        if let Ok(text) = read_sector(&mut file, Self::MASTER_TOC_SECTOR + 1)
            && text.len() == Self::SECTOR_SIZE
            && text.starts_with(b"SACDText")
        {
            album_title = c_string(&text, be16(&text, 16) as usize, char_set);
            album_artist = c_string(&text, be16(&text, 18) as usize, char_set);
            disc_title = c_string(&text, be16(&text, 32) as usize, char_set);
            disc_artist = c_string(&text, be16(&text, 34) as usize, char_set);
        }

        let mut areas = vec![];
        for offset in [64, 72] {
            let start = be32(&master, offset);
            if start == 0 {
                continue;
            }
            let toc = read_sector(&mut file, start as u64).map_err(|_| SacdProbeError::BadAreaToc(start))?;
            if toc.len() != Self::SECTOR_SIZE {
                return Err(SacdProbeError::BadAreaToc(start));
            }
            let id = &toc[..8];
            if id != b"TWOCHTOC" && id != b"MULCHTOC" {
                return Err(SacdProbeError::BadAreaToc(start));
            }
            let frame_format = match toc[21] {
                0 => FrameFormat::Dst,
                2 | 3 => FrameFormat::Dsd,
                _ => FrameFormat::Unknown,
            };
            areas.push(SacdArea {
                is_multichannel: id == b"MULCHTOC",
                frame_format,
                channel_count: toc[32] as i64,
                track_offset: toc[68] as i64,
                track_count: toc[69] as i64,
                play_time_seconds: toc[64] as i64 * 60 + toc[65] as i64,
                toc_sector: start,
                track_start_sector: be32(&toc, 72),
                track_end_sector: be32(&toc, 76),
            });
        }

        Ok(SACDInfo {
            album_set_size: be16(&master, 16) as i64,
            album_sequence_number: be16(&master, 18) as i64,
            album_catalog_number: ascii_field(&master, 24, 16),
            disc_catalog_number: ascii_field(&master, 88, 16),
            disc_date,
            locale_language: ascii_field(&master, 136, 2),
            character_set_code: char_set,
            album_title,
            album_artist,
            disc_title,
            disc_artist,
            areas,
        })
    }
}

fn read_sector(file: &mut File, sector: u64) -> std::io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(sector * SACDProbe::SECTOR_SIZE as u64))?;
    let mut buf = Vec::with_capacity(SACDProbe::SECTOR_SIZE);
    file.take(SACDProbe::SECTOR_SIZE as u64).read_to_end(&mut buf)?;
    Ok(buf)
}

fn be16(d: &[u8], o: usize) -> u16 {
    u16::from_be_bytes([d[o], d[o + 1]])
}

fn be32(d: &[u8], o: usize) -> u32 {
    u32::from_be_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

fn ascii_field(d: &[u8], o: usize, len: usize) -> String {
    let slice = &d[o..o + len];
    let end = slice.iter().position(|&b| b == 0).unwrap_or(len);
    String::from_utf8_lossy(&slice[..end]).trim_matches(|c: char| c == ' ' || c == '\t').to_string()
}

/// NUL-terminated string at `pos`, decoded per the disc's character set
/// code (1 ISO 646, 2 ISO 8859-1, 3 Shift-JIS, 7 ISO 8859-1). Unknown codes
/// try UTF-8 then Latin-1.
fn c_string(d: &[u8], pos: usize, char_set: i64) -> Option<String> {
    if pos == 0 || pos >= d.len() {
        return None;
    }
    let rest = &d[pos..];
    let bytes = &rest[..rest.iter().position(|&b| b == 0).unwrap_or(rest.len())];
    if bytes.is_empty() {
        return None;
    }
    let latin1 = || bytes.iter().map(|&b| b as char).collect::<String>();
    let primary = match char_set {
        2 | 7 => Some(latin1()),
        3 => SHIFT_JIS.decode_without_bom_handling_and_without_replacement(bytes).map(|s| {
            // Apple's CP932 table, as in CUE decoding.
            TextDecoder::decode_shift_jis_fixups(s.into_owned())
        }),
        _ => std::str::from_utf8(bytes).ok().map(str::to_string),
    };
    primary.or_else(|| std::str::from_utf8(bytes).ok().map(str::to_string)).or_else(|| Some(latin1()))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A tiny synthetic Scarletbook image: master TOC at sector 510, master
    /// text at 511, a stereo area TOC at 544 and a multichannel one at 560.
    pub(crate) fn fake_sacd_bytes(
        stereo_dst: bool,
        multichannel: bool,
        title: &[u8],
        artist: &[u8],
        char_set: u8,
    ) -> Vec<u8> {
        let sector = SACDProbe::SECTOR_SIZE;
        let mut image = vec![0u8; sector * 600];
        let mut put = |bytes: &[u8], at: usize| image[at..at + bytes.len()].copy_from_slice(bytes);
        let be16 = |v: u16| v.to_be_bytes();
        let be32 = |v: u32| v.to_be_bytes();

        let m = 510 * sector;
        put(b"SACDMTOC", m);
        put(&[1, 0x14], m + 8);
        put(&be16(1), m + 16);
        put(&be16(1), m + 18);
        put(b"ALBCAT-1", m + 24);
        put(&be32(544), m + 64);
        if multichannel {
            put(&be32(560), m + 72);
        }
        put(b"CIPJ 77 SA", m + 88);
        put(&be16(2010), m + 120);
        put(&[6, 28], m + 122);
        put(b"en", m + 136);
        put(&[char_set], m + 138);

        let t = 511 * sector;
        put(b"SACDText", t);
        put(&be16(100), t + 16);
        put(&be16(140), t + 18);
        put(&be16(180), t + 32);
        put(&[title, &[0]].concat(), t + 100);
        put(&[artist, &[0]].concat(), t + 140);
        put(b"Disc Title\0", t + 180);

        let mut area = |sector_no: usize, id: &[u8], dst: bool, channels: u8| {
            let a = sector_no * sector;
            put(id, a);
            put(&[1, 0x14], a + 8);
            put(&[if dst { 0 } else { 2 }], a + 21);
            put(&[channels], a + 32);
            put(&[33, 7, 15], a + 64);
            put(&[0, 3], a + 68);
            put(&be32(624), a + 72);
            put(&be32(696144), a + 76);
        };
        area(544, b"TWOCHTOC", stereo_dst, 2);
        if multichannel {
            area(560, b"MULCHTOC", true, 5);
        }
        image
    }

    fn fake_sacd(
        stereo_dst: bool,
        multichannel: bool,
        title: &[u8],
        artist: &[u8],
        char_set: u8,
    ) -> tempfile::NamedTempFile {
        let file = tempfile::Builder::new().suffix(".iso").tempfile().unwrap();
        std::fs::write(file.path(), fake_sacd_bytes(stereo_dst, multichannel, title, artist, char_set)).unwrap();
        file
    }

    #[test]
    fn recognises_and_describes_stereo_dsd_disc() {
        let f = fake_sacd(false, false, b"Fake Album", b"Fake Artist", 1);
        assert!(SACDProbe::is_sacd(f.path()));
        let info = SACDProbe::probe(f.path()).unwrap();
        assert_eq!(info.areas.len(), 1);
        let stereo = info.stereo_area().unwrap();
        assert_eq!(stereo.frame_format, FrameFormat::Dsd);
        assert_eq!(stereo.channel_count, 2);
        assert_eq!(stereo.track_count, 3);
        assert_eq!(stereo.play_time_seconds, 33 * 60 + 7);
        assert_eq!(stereo.track_start_sector, 624);
        assert!(info.multichannel_area().is_none());
        assert!(!info.has_dst());
        assert_eq!(info.track_count(), 3);
        assert_eq!(info.disc_catalog_number, "CIPJ 77 SA");
        assert_eq!(info.album_catalog_number, "ALBCAT-1");
        assert_eq!(info.disc_date.as_deref(), Some("2010-06-28"));
        assert_eq!(info.locale_language, "en");
        assert_eq!(info.album_title.as_deref(), Some("Fake Album"));
        assert_eq!(info.album_artist.as_deref(), Some("Fake Artist"));
        assert_eq!(info.disc_title.as_deref(), Some("Disc Title"));
        assert_eq!(info.title().as_deref(), Some("Disc Title"));
        assert_eq!(info.artist().as_deref(), Some("Fake Artist"));
    }

    #[test]
    fn describes_dst_and_multichannel() {
        let f = fake_sacd(true, true, b"Fake Album", b"Fake Artist", 1);
        let info = SACDProbe::probe(f.path()).unwrap();
        assert_eq!(info.areas.len(), 2);
        assert!(info.stereo_area().unwrap().is_dst());
        assert_eq!(info.multichannel_area().unwrap().channel_count, 5);
        assert!(info.multichannel_area().unwrap().is_multichannel);
        assert!(info.has_dst());
    }

    #[test]
    fn decodes_latin1_disc_text() {
        let f = fake_sacd(false, true, b"Suites f\xFCr Cello", b"J\xE1nos Starker", 2);
        let info = SACDProbe::probe(f.path()).unwrap();
        assert_eq!(info.character_set_code, 2);
        assert_eq!(info.album_title.as_deref(), Some("Suites für Cello"));
        assert_eq!(info.album_artist.as_deref(), Some("János Starker"));
    }

    #[test]
    fn rejects_non_sacd_files() {
        let f = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(f.path(), vec![0u8; SACDProbe::SECTOR_SIZE * 520]).unwrap();
        assert!(!SACDProbe::is_sacd(f.path()));
        assert_eq!(SACDProbe::probe(f.path()), Err(SacdProbeError::NotSacd));
        let short = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(short.path(), vec![0u8; 100]).unwrap();
        assert!(!SACDProbe::is_sacd(short.path()));
    }
}
