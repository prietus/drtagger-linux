//! Extracts one area of a SACD image into per-track DSF files with ID3
//! tags built from the disc's own text (title, performer, ISRC, genre,
//! date). Tracks are cut by frame time code with a pause policy; DST areas
//! are decoded in parallel batches. Synchronous and CPU-bound.

use crate::disc::{SACDArea, SACDDisc, SACDTrack};
use crate::dsf_writer::DSFWriter;
use crate::frame_reader::{SACDFrame, SACDFrameReader};
use dstkit::DSTDecoder;
use flackit::{ID3v2Bridge, VorbisComment};
use librarykit::path_serde;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use splitkit::{Cancel, DEFAULT_ALBUM_FOLDER, DEFAULT_TRACK_FILE, PathTemplate};
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PausePolicy {
    /// Audio between a track's TOC end and the next track's start stays at
    /// the end of the previous track, like CD gaps appended.
    AppendToPrevious,
    /// Only the TOC duration of each track is written (sacd_extract).
    Drop,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SACDExtractOptions {
    pub pause_policy: PausePolicy,
    pub album_folder_template: String,
    pub track_file_template: String,
    pub ascii_file_names: bool,
    /// A rendered relative album folder (with its "Disc N") that replaces
    /// the one derived from the disc text: set members share it.
    pub album_folder_override: Option<String>,
    pub multichannel_subfolder: String,
    pub write_tags: bool,
    pub overwrite_existing: bool,
    pub lead_in_warning_frames: i64,
}

impl Default for SACDExtractOptions {
    fn default() -> Self {
        SACDExtractOptions {
            pause_policy: PausePolicy::AppendToPrevious,
            album_folder_template: DEFAULT_ALBUM_FOLDER.into(),
            track_file_template: DEFAULT_TRACK_FILE.into(),
            ascii_file_names: false,
            album_folder_override: None,
            multichannel_subfolder: "Multichannel".into(),
            write_tags: true,
            overwrite_existing: false,
            lead_in_warning_frames: 150,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SACDTrackOutput {
    pub number: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(with = "path_serde")]
    pub url: PathBuf,
    pub frames: i64,
    /// TOC duration (drop) or gap to the next track (append).
    pub expected_frames: i64,
    /// Per channel.
    pub sample_count: u64,
    pub file_size: u64,
    /// Frame time codes advanced by exactly one.
    pub contiguous: bool,
}

impl SACDTrackOutput {
    pub fn verified(&self) -> bool {
        self.frames == self.expected_frames && self.contiguous
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SACDExtractOutcome {
    #[serde(with = "path_serde")]
    pub output_folder: PathBuf,
    pub is_multichannel: bool,
    pub channel_count: usize,
    pub tracks: Vec<SACDTrackOutput>,
    pub dropped_lead_in_frames: i64,
    pub dropped_pause_frames: i64,
    pub elapsed_seconds: f64,
    pub warnings: Vec<String>,
}

impl SACDExtractOutcome {
    pub fn verified(&self) -> bool {
        !self.tracks.is_empty() && self.tracks.iter().all(SACDTrackOutput::verified)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SACDExtractError {
    DstDecodeFailed { timecode: String, reason: String },
    NoTracks,
    OutputExists(String),
    FrameSizeMismatch { expected: usize, got: usize, timecode: String },
    Cancelled,
    Other(String),
}

impl fmt::Display for SACDExtractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SACDExtractError::DstDecodeFailed { timecode, reason } => {
                write!(f, "DST frame at {timecode} could not be decoded: {reason}")
            }
            SACDExtractError::NoTracks => write!(f, "The area has no tracks."),
            SACDExtractError::OutputExists(n) => write!(f, "{n} already exists. Enable overwrite to replace it."),
            SACDExtractError::FrameSizeMismatch { expected, got, timecode } => {
                write!(f, "Frame at {timecode} has {got} bytes, expected {expected}.")
            }
            SACDExtractError::Cancelled => write!(f, "Cancelled."),
            SACDExtractError::Other(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SACDExtractError {}

macro_rules! other {
    ($t:ty) => {
        impl From<$t> for SACDExtractError {
            fn from(e: $t) -> Self {
                SACDExtractError::Other(e.to_string())
            }
        }
    };
}
other!(std::io::Error);
other!(crate::frame_reader::ReaderError);
other!(crate::dsf_writer::WriterError);
other!(dstkit::DSTError);

#[derive(Clone, Debug, PartialEq)]
pub struct SACDExtractProgress {
    pub fraction: f64,
    pub message: String,
}

/// Tags written into every DSF: what the disc itself says.
pub fn comment(track: &SACDTrack, disc: &SACDDisc, area: &SACDArea) -> VorbisComment {
    let mut fields = vec![];
    let mut add = |name: &str, value: Option<String>| {
        if let Some(v) = value.map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) {
            fields.push((name.to_string(), v));
        }
    };
    add("TITLE", Some(track.title.clone().unwrap_or_else(|| format!("Track {}", track.number))));
    add("ARTIST", track.performer.clone().or_else(|| disc.artist()));
    add("ALBUM", disc.title());
    add("ALBUMARTIST", disc.artist());
    add("TRACKNUMBER", Some(track.number.to_string()));
    add("TRACKTOTAL", Some(area.tracks.len().to_string()));
    if disc.info.album_set_size > 1 {
        add("DISCNUMBER", Some(disc.info.album_sequence_number.to_string()));
        add("DISCTOTAL", Some(disc.info.album_set_size.to_string()));
    }
    add("DATE", disc.year());
    add("GENRE", track.genre.clone());
    add("COMPOSER", track.composer.clone());
    add("ISRC", track.isrc.clone());
    add("CATALOGNUMBER", Some(disc.info.disc_catalog_number.clone()).filter(|c| !c.is_empty()));
    add("MEDIA", Some("SACD".into()));
    VorbisComment::new(splitkit::provisional_tags::vendor(), fields)
}

pub fn id3(track: &SACDTrack, disc: &SACDDisc, area: &SACDArea) -> Vec<u8> {
    ID3v2Bridge::to_id3v23(&comment(track, disc, area), &[]).encoded_v23()
}

struct Window {
    track: SACDTrack,
    start: i64,
    end: i64,
    path: PathBuf,
}

const BATCH: usize = 48;

pub struct SACDExtractor;

impl SACDExtractor {
    pub fn extract(
        disc: &SACDDisc,
        area: &SACDArea,
        destination_root: &Path,
        options: &SACDExtractOptions,
        cancel: Option<&Cancel>,
        progress: &mut dyn FnMut(SACDExtractProgress),
    ) -> Result<SACDExtractOutcome, SACDExtractError> {
        let started = Instant::now();
        if area.tracks.is_empty() {
            return Err(SACDExtractError::NoTracks);
        }
        let mut warnings = vec![];

        // Output folder.
        let album_artist = disc
            .artist()
            .or_else(|| area.tracks.first().and_then(|t| t.performer.clone()))
            .unwrap_or_else(|| "Unknown Artist".into());
        let album = disc
            .title()
            .unwrap_or_else(|| disc.url.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
        let values: HashMap<String, String> = [
            ("albumartist", album_artist.clone()),
            ("album", album.clone()),
            ("year", disc.year().unwrap_or_default()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        let mut relative = PathTemplate::render(&options.album_folder_template, &values, options.ascii_file_names);
        if disc.info.album_set_size > 1 {
            relative += &format!("/Disc {}", disc.info.album_sequence_number);
        }
        if let Some(o) = options.album_folder_override.as_ref().filter(|o| !o.is_empty()) {
            relative = o.clone();
        }
        if area.is_multichannel && !options.multichannel_subfolder.is_empty() {
            relative += &format!("/{}", PathTemplate::sanitize_component(&options.multichannel_subfolder));
        }
        let output_folder =
            if relative.is_empty() { destination_root.to_path_buf() } else { destination_root.join(&relative) };
        std::fs::create_dir_all(&output_folder)?;

        // Track windows in frames.
        let mut windows = vec![];
        for (i, track) in area.tracks.iter().enumerate() {
            let start = track.start_time.total_frames;
            let next_start = area.tracks.get(i + 1).map_or(i64::MAX, |t| t.start_time.total_frames);
            let end = match options.pause_policy {
                PausePolicy::Drop => (start + track.duration.total_frames).min(next_start),
                PausePolicy::AppendToPrevious => next_start,
            };
            let mut v: HashMap<String, String> = HashMap::new();
            v.insert("track".into(), format!("{:02}", track.number));
            v.insert("title".into(), track.title.clone().unwrap_or_else(|| format!("Track {}", track.number)));
            v.insert("artist".into(), track.performer.clone().unwrap_or_else(|| album_artist.clone()));
            v.insert("album".into(), album.clone());
            let name = PathTemplate::render(&options.track_file_template, &v, options.ascii_file_names);
            let name = if name.is_empty() { format!("{:02}", track.number) } else { name };
            let path = output_folder.join(format!("{name}.dsf"));
            if path.exists() {
                if !options.overwrite_existing {
                    return Err(SACDExtractError::OutputExists(
                        path.file_name().unwrap().to_string_lossy().into_owned(),
                    ));
                }
                std::fs::remove_file(&path)?;
            }
            windows.push(Window { track: track.clone(), start, end, path });
        }

        // Stream frames; DST areas decode in parallel batches.
        let mut reader = SACDFrameReader::new(&disc.url, area.audio_start_sector, area.audio_end_sector)?;
        let frame_bytes = area.dsd_frame_bytes();
        let is_dst = area.is_dst();
        let (channels, rate) = (area.channel_count, area.sample_rate);
        if is_dst {
            DSTDecoder::new(channels, rate)?; // validate before any work
        }
        let mut decoded: std::vec::IntoIter<SACDFrame> = Vec::new().into_iter();
        let mut next_frame = |reader: &mut SACDFrameReader| -> Result<Option<SACDFrame>, SACDExtractError> {
            if !is_dst {
                return Ok(reader.next_frame()?);
            }
            if let Some(f) = decoded.next() {
                return Ok(Some(f));
            }
            let mut batch = vec![];
            while batch.len() < BATCH {
                match reader.next_frame()? {
                    Some(f) => batch.push(f),
                    None => break,
                }
            }
            if batch.is_empty() {
                return Ok(None);
            }
            let results: Vec<Result<SACDFrame, SACDExtractError>> = batch
                .into_par_iter()
                .map_init(
                    || DSTDecoder::new(channels, rate).expect("validated"),
                    |d, f| match d.decode(&f.data) {
                        Ok(data) => Ok(SACDFrame { timecode: f.timecode, data }),
                        Err(e) => Err(SACDExtractError::DstDecodeFailed {
                            timecode: f.timecode.to_string(),
                            reason: e.to_string(),
                        }),
                    },
                )
                .collect();
            decoded = results.into_iter().collect::<Result<Vec<_>, _>>()?.into_iter();
            Ok(decoded.next())
        };

        let mut outputs = vec![];
        let mut window_index = 0;
        let mut writer: Option<DSFWriter> = None;
        let (mut writer_frames, mut writer_contiguous) = (0i64, true);
        let mut last_timecode: Option<i64> = None;
        let (mut dropped_lead_in, mut dropped_pause) = (0i64, 0i64);
        let mut frame_counter = 0i64;
        let total_frames = area.play_time.total_frames.max(1);

        let close = |w: &Window,
                     writer: &mut Option<DSFWriter>,
                     frames: &mut i64,
                     contiguous: &mut bool,
                     outputs: &mut Vec<SACDTrackOutput>|
         -> Result<(), SACDExtractError> {
            let Some(current) = writer.take() else { return Ok(()) };
            let path = current.path.clone();
            let tags = options.write_tags.then(|| id3(&w.track, disc, area));
            let (samples, size) = current.finish(tags.as_deref())?;
            let expected = if w.end == i64::MAX { *frames } else { w.end - w.start };
            outputs.push(SACDTrackOutput {
                number: w.track.number,
                title: w.track.title.clone(),
                url: path,
                frames: *frames,
                expected_frames: expected,
                sample_count: samples,
                file_size: size,
                contiguous: *contiguous,
            });
            *frames = 0;
            *contiguous = true;
            Ok(())
        };

        while let Some(frame) = next_frame(&mut reader)? {
            if cancel.is_some_and(Cancel::is_cancelled) {
                return Err(SACDExtractError::Cancelled);
            }
            frame_counter += 1;
            if frame_counter % 750 == 0 {
                progress(SACDExtractProgress {
                    fraction: (frame_counter as f64 / total_frames as f64).min(0.99),
                    message: format!("Track {} of {}", (window_index + 1).min(windows.len()), windows.len()),
                });
            }
            let t = frame.timecode.total_frames;
            if let Some(last) = last_timecode
                && t != last + 1
                && writer.is_some()
            {
                writer_contiguous = false;
            }
            last_timecode = Some(t);

            // Advance past windows that ended.
            while window_index < windows.len() && t >= windows[window_index].end {
                close(&windows[window_index], &mut writer, &mut writer_frames, &mut writer_contiguous, &mut outputs)?;
                window_index += 1;
            }
            if window_index >= windows.len() {
                break;
            }
            let w = &windows[window_index];
            if t < w.start {
                if window_index == 0 {
                    dropped_lead_in += 1;
                } else {
                    dropped_pause += 1;
                }
                continue;
            }
            if frame.data.len() != frame_bytes {
                return Err(SACDExtractError::FrameSizeMismatch {
                    expected: frame_bytes,
                    got: frame.data.len(),
                    timecode: frame.timecode.to_string(),
                });
            }
            if writer.is_none() {
                writer = Some(DSFWriter::create(&w.path, channels, rate)?);
                writer_frames = 0;
                writer_contiguous = true;
            }
            writer.as_mut().unwrap().write_frame(&frame.data)?;
            writer_frames += 1;
        }
        if window_index < windows.len() {
            close(&windows[window_index], &mut writer, &mut writer_frames, &mut writer_contiguous, &mut outputs)?;
        }
        if outputs.len() < windows.len() {
            warnings.push(format!("Only {} of {} tracks had audio.", outputs.len(), windows.len()));
        }
        if dropped_lead_in > options.lead_in_warning_frames {
            warnings.push(format!("{dropped_lead_in} frames before track 1 were skipped (possible hidden audio)."));
        }
        progress(SACDExtractProgress { fraction: 1.0, message: "Done".into() });
        Ok(SACDExtractOutcome {
            output_folder,
            is_multichannel: area.is_multichannel,
            channel_count: channels,
            tracks: outputs,
            dropped_lead_in_frames: dropped_lead_in,
            dropped_pause_frames: dropped_pause,
            elapsed_seconds: started.elapsed().as_secs_f64(),
            warnings,
        })
    }
}
