//! Full reading of a SACD image: per area, the TOC, SACDTRL1 (track start
//! sectors and lengths), SACDTRL2 (start time codes and durations),
//! SACD_IGL (ISRCs, then genres) and SACDTTxt (track texts).

use crate::scarletbook::{self, MASTER_TOC_SECTOR, SACDTime, SECTOR_SIZE, be16, be32, decode_text, genre_name};
use librarykit::{FrameFormat, SACDInfo, SACDProbe, path_serde};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SACDTrack {
    /// 1-based within the area.
    pub number: i64,
    pub start_sector: u32,
    pub length_sectors: u32,
    pub start_time: SACDTime,
    pub duration: SACDTime,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub performer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub songwriter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub composer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arranger: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub isrc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
}

impl SACDTrack {
    pub fn end_time(&self) -> SACDTime {
        self.start_time + self.duration
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SACDArea {
    pub is_multichannel: bool,
    pub frame_format: FrameFormat,
    pub channel_count: usize,
    pub sample_rate: u32,
    pub toc_sector: u32,
    pub toc_size_sectors: usize,
    pub audio_start_sector: u32,
    /// Last audio sector (inclusive).
    pub audio_end_sector: u32,
    pub track_offset: i64,
    pub play_time: SACDTime,
    pub character_set_code: u8,
    pub tracks: Vec<SACDTrack>,
}

impl SACDArea {
    pub fn is_dst(&self) -> bool {
        self.frame_format == FrameFormat::Dst
    }
    /// Uncompressed DSD frame size for this area's channel count.
    pub fn dsd_frame_bytes(&self) -> usize {
        self.channel_count * scarletbook::BYTES_PER_CHANNEL_PER_FRAME
    }
    pub fn display_name(&self) -> String {
        if self.is_multichannel { format!("Multichannel ({} ch)", self.channel_count) } else { "Stereo".into() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SACDDisc {
    #[serde(with = "path_serde")]
    pub url: PathBuf,
    pub info: SACDInfo,
    pub areas: Vec<SACDArea>,
}

impl SACDDisc {
    pub fn stereo_area(&self) -> Option<&SACDArea> {
        self.areas.iter().find(|a| !a.is_multichannel)
    }
    pub fn multichannel_area(&self) -> Option<&SACDArea> {
        self.areas.iter().find(|a| a.is_multichannel)
    }
    pub fn title(&self) -> Option<String> {
        self.info.title()
    }
    pub fn artist(&self) -> Option<String> {
        self.info.artist()
    }
    pub fn year(&self) -> Option<String> {
        self.info.disc_date.as_ref().map(|d| d.chars().take(4).collect())
    }
    /// The first named genre (the reader puts the disc's on every track
    /// that has none of its own).
    pub fn genre(&self) -> Option<String> {
        self.areas.first()?.tracks.first()?.genre.clone()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadError {
    NotSacd,
    BadAreaToc(u32),
    MissingTrackLists(u32),
    Probe(String),
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReadError::NotSacd => write!(f, "Not a SACD image."),
            ReadError::BadAreaToc(s) => write!(f, "Area TOC at sector {s} is not readable."),
            ReadError::MissingTrackLists(s) => write!(f, "Area at sector {s} has no track list."),
            ReadError::Probe(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ReadError {}

fn sector(file: &File, n: u64) -> Result<Vec<u8>, ReadError> {
    let mut buf = vec![0u8; SECTOR_SIZE];
    file.read_exact_at(&mut buf, n * SECTOR_SIZE as u64).map_err(|_| ReadError::NotSacd)?;
    Ok(buf)
}

pub fn read(path: &Path) -> Result<SACDDisc, ReadError> {
    let info = SACDProbe::probe(path).map_err(|e| ReadError::Probe(e.to_string()))?;
    let file = File::open(path).map_err(|_| ReadError::NotSacd)?;
    let master = sector(&file, MASTER_TOC_SECTOR)?;
    let master_genre = genre_from_table(&master, 104).or_else(|| genre_from_table(&master, 40));
    let mut areas = vec![];
    for probe in &info.areas {
        areas.push(read_area(
            &file,
            probe.toc_sector,
            probe.is_multichannel,
            info.character_set_code as u8,
            master_genre.as_deref(),
        )?);
    }
    Ok(SACDDisc { url: path.to_path_buf(), info, areas })
}

fn read_area(
    file: &File,
    start: u32,
    is_multichannel: bool,
    master_char_set: u8,
    fallback_genre: Option<&str>,
) -> Result<SACDArea, ReadError> {
    let toc = sector(file, start as u64)?;
    let id = &toc[..8];
    if id != b"TWOCHTOC" && id != b"MULCHTOC" {
        return Err(ReadError::BadAreaToc(start));
    }
    let size = be16(&toc, 10) as usize;
    let frame_format = match toc[21] {
        0 => FrameFormat::Dst,
        2 | 3 => FrameFormat::Dsd,
        _ => FrameFormat::Unknown,
    };
    let track_count = toc[69] as usize;
    let char_set = if toc[90] != 0 { toc[90] } else { master_char_set };

    // Sub-blocks by signature.
    let (mut trl1, mut trl2, mut igl, mut text_start) = (None, None, None, None);
    let mut sectors = vec![];
    for i in 1..size.max(1) {
        let s = sector(file, start as u64 + i as u64)?;
        match &s[..8] {
            b"SACDTRL1" => trl1 = Some(s.clone()),
            b"SACDTRL2" => trl2 = Some(s.clone()),
            b"SACD_IGL" => {
                let mut d = s.clone();
                if i + 1 < size {
                    d.extend(sector(file, start as u64 + i as u64 + 1)?);
                }
                igl = Some(d);
            }
            b"SACDTTxt" => text_start = Some(i - 1),
            _ => {}
        }
        sectors.push(s);
    }
    let (Some(trl1), Some(trl2)) = (trl1, trl2) else { return Err(ReadError::MissingTrackLists(start)) };

    let mut tracks = vec![];
    for k in 0..track_count {
        let st = &trl2[8 + 4 * k..12 + 4 * k];
        let du = &trl2[8 + 1020 + 4 * k..12 + 1020 + 4 * k];
        let mut track = SACDTrack {
            number: k as i64 + 1,
            start_sector: be32(&trl1, 8 + 4 * k),
            length_sectors: be32(&trl1, 8 + 1020 + 4 * k),
            start_time: SACDTime::msf(st[0], st[1], st[2]),
            duration: SACDTime::msf(du[0], du[1], du[2]),
            title: None,
            performer: None,
            songwriter: None,
            composer: None,
            arranger: None,
            message: None,
            isrc: None,
            genre: None,
        };
        if let Some(igl) = &igl {
            let mut isrc_bytes = igl[8 + 12 * k..20 + 12 * k].to_vec();
            isrc_bytes.push(0);
            if let Some(isrc) = decode_text(&isrc_bytes, 1).filter(|s| s.chars().count() == 12) {
                track.isrc = Some(isrc);
            }
            let g = 8 + 3060 + 4 * k;
            if g + 4 <= igl.len() {
                track.genre = genre_name(igl[g], be16(igl, g + 2));
            }
        }
        if track.genre.is_none() {
            track.genre = fallback_genre.map(str::to_string);
        }
        tracks.push(track);
    }

    if let Some(ts) = text_start {
        let block: Vec<u8> = sectors[ts..].concat();
        apply_texts(&block, &mut tracks, char_set);
    }

    Ok(SACDArea {
        is_multichannel,
        frame_format,
        channel_count: toc[32] as usize,
        sample_rate: scarletbook::sample_rate(toc[20]),
        toc_sector: start,
        toc_size_sectors: size,
        audio_start_sector: be32(&toc, 72),
        audio_end_sector: be32(&toc, 76),
        track_offset: toc[68] as i64,
        play_time: SACDTime::msf(toc[64], toc[65], toc[66]),
        character_set_code: char_set,
        tracks,
    })
}

/// SACDTTxt: u16 position per track (relative to the block), then at each
/// position an item count (u8) + 3 reserved bytes, then items of
/// {type u8, reserved u8, NUL-terminated text} padded to 4 bytes.
fn apply_texts(block: &[u8], tracks: &mut [SACDTrack], char_set: u8) {
    for (k, track) in tracks.iter_mut().enumerate() {
        if 8 + 2 * k + 2 > block.len() {
            break;
        }
        let pos = be16(block, 8 + 2 * k) as usize;
        if pos < 12 || pos + 4 > block.len() {
            continue;
        }
        let count = block[pos] as usize;
        let mut p = pos + 4;
        for _ in 0..count.min(16) {
            if p + 2 >= block.len() {
                break;
            }
            let kind = block[p];
            let text_start = p + 2;
            let rest = &block[text_start..];
            let text = decode_text(&rest[..rest.len().min(512)], char_set);
            let nul = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
            match kind {
                1 => track.title = text,
                2 => track.performer = text,
                3 => track.songwriter = text,
                4 => track.composer = text,
                5 => track.arranger = text,
                6 => track.message = text,
                _ => {}
            }
            p = (text_start + nul + 1 + 3) & !3;
        }
    }
}

fn genre_from_table(master: &[u8], offset: usize) -> Option<String> {
    (0..4).find_map(|i| genre_name(master[offset + 4 * i], be16(master, offset + 4 * i + 2)))
}
