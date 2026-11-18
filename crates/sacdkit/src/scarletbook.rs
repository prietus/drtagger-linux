//! Shared Scarletbook constants and value types. 2048-byte sectors, all
//! integers big-endian; area TOC sub-blocks are whole sectors identified by
//! an 8-byte signature (SACDTRL1, SACDTRL2, SACD_IGL, SACDTTxt…).

use serde::{Deserialize, Serialize};
use std::fmt;

pub const SECTOR_SIZE: usize = 2048;
pub const FRAMES_PER_SECOND: i64 = 75;
pub const DSD64_SAMPLE_RATE: u32 = 2_822_400;
/// 2822400 / 75 / 8.
pub const BYTES_PER_CHANNEL_PER_FRAME: usize = 4704;
pub const MASTER_TOC_SECTOR: u64 = 510;

/// Audio packet data type.
pub const DATA_TYPE_AUDIO: u8 = 2;

pub fn sample_rate(_code: u8) -> u32 {
    DSD64_SAMPLE_RATE
}

/// Genre table for category 1 ("General"), as sacd_extract prints it.
pub const GENRE_NAMES: [&str; 30] = [
    "Not used",
    "Not defined",
    "Adult Contemporary",
    "Alternative Rock",
    "Children's Music",
    "Classical",
    "Contemporary Christian",
    "Country",
    "Dance",
    "Easy Listening",
    "Erotic",
    "Folk",
    "Gospel",
    "Hip Hop",
    "Jazz",
    "Latin",
    "Musical",
    "New Age",
    "Opera",
    "Operetta",
    "Pop Music",
    "RAP",
    "Reggae",
    "Rock Music",
    "Rhythm and Blues",
    "Sound Effects",
    "Sound Track",
    "Spoken Word",
    "World Music",
    "Blues",
];

pub fn genre_name(category: u8, code: u16) -> Option<String> {
    (category == 1 && code > 1 && (code as usize) < GENRE_NAMES.len()).then(|| GENRE_NAMES[code as usize].to_string())
}

/// NUL-terminated text decoded per the disc's character set code (1 ISO
/// 646, 2 ISO 8859-1, 3 Shift-JIS, 7 ISO 8859-1); unknown codes try UTF-8
/// first. Trimmed.
pub fn decode_text(bytes: &[u8], char_set: u8) -> Option<String> {
    let slice = &bytes[..bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len())];
    if slice.is_empty() {
        return None;
    }
    let latin1 = || flackit::text::latin1(slice);
    let primary = match char_set {
        2 | 7 => Some(latin1()),
        3 => encoding_rs::SHIFT_JIS
            .decode_without_bom_handling_and_without_replacement(slice)
            .map(|s| librarykit::TextDecoder::decode_shift_jis_fixups(s.into_owned())),
        _ => flackit::text::utf8(slice),
    };
    let text = primary.or_else(|| flackit::text::utf8(slice)).unwrap_or_else(latin1);
    Some(text.trim().to_string())
}

pub(crate) fn be16(d: &[u8], o: usize) -> u16 {
    u16::from_be_bytes([d[o], d[o + 1]])
}

pub(crate) fn be32(d: &[u8], o: usize) -> u32 {
    u32::from_be_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

/// A position or duration in minutes:seconds:frames at 75 frames per second.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SACDTime {
    pub total_frames: i64,
}

impl SACDTime {
    pub fn new(total_frames: i64) -> Self {
        SACDTime { total_frames: total_frames.max(0) }
    }
    pub fn msf(minutes: u8, seconds: u8, frames: u8) -> Self {
        SACDTime::new((minutes as i64 * 60 + seconds as i64) * FRAMES_PER_SECOND + frames as i64)
    }
    pub fn minutes(self) -> i64 {
        self.total_frames / (60 * FRAMES_PER_SECOND)
    }
    pub fn seconds(self) -> i64 {
        (self.total_frames / FRAMES_PER_SECOND) % 60
    }
    pub fn frames(self) -> i64 {
        self.total_frames % FRAMES_PER_SECOND
    }
    pub fn total_seconds(self) -> f64 {
        self.total_frames as f64 / FRAMES_PER_SECOND as f64
    }
}

impl std::ops::Add for SACDTime {
    type Output = SACDTime;
    fn add(self, rhs: SACDTime) -> SACDTime {
        SACDTime::new(self.total_frames + rhs.total_frames)
    }
}

impl fmt::Display for SACDTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}:{:02}:{:02}", self.minutes(), self.seconds(), self.frames())
    }
}
