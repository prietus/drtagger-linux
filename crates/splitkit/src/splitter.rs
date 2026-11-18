//! Turns one disc (image + CUE, or per-track files + CUE) into verified FLAC
//! tracks. Work happens in a temporary folder; nothing is written to the
//! destination until that track's encode succeeded.

use crate::ffmpeg::{FFmpegTool, ToolError};
use crate::path_template::{DEFAULT_ALBUM_FOLDER, DEFAULT_TRACK_FILE, PathTemplate};
use crate::pcm::{PCMFormat, crc32_of_file};
use crate::provisional_tags::{self, AlbumContext};
use flackit::FLACFile;
use librarykit::{CueSplitPlan, CueTime, DetectedDisc, DiscTOC, path_serde};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitOptions {
    pub compression_level: i64,
    pub htoa_threshold_frames: i64,
    pub album_folder_template: String,
    pub track_file_template: String,
    pub ascii_file_names: bool,
    /// A rendered relative album folder that replaces the one derived from
    /// the CUE: set members share it.
    pub album_folder_override: Option<String>,
    pub write_provisional_tags: bool,
    pub overwrite_existing: bool,
    /// Where the decoded image is staged (it can be gigabytes: keep it off
    /// a RAM-backed /tmp). Defaults to the system temporary directory.
    pub work_dir: Option<PathBuf>,
}

impl Default for SplitOptions {
    fn default() -> Self {
        SplitOptions {
            compression_level: 8,
            htoa_threshold_frames: CueSplitPlan::DEFAULT_HTOA_THRESHOLD_FRAMES,
            album_folder_template: DEFAULT_ALBUM_FOLDER.into(),
            track_file_template: DEFAULT_TRACK_FILE.into(),
            ascii_file_names: false,
            album_folder_override: None,
            write_provisional_tags: true,
            overwrite_existing: false,
            work_dir: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SplitPhase {
    Probing,
    Decoding { file: usize, of: usize },
    Encoding { track: usize, of: usize },
    Verifying,
    Finished,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SplitProgress {
    pub phase: SplitPhase,
    /// 0…1 overall estimate.
    pub fraction: f64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitTrackOutput {
    pub number: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(with = "path_serde")]
    pub url: PathBuf,
    pub samples: i64,
    /// MD5 of the PCM fed to the encoder.
    pub md5_hex: String,
    pub stream_info_md5_matches: bool,
    /// Plain CRC of the whole track.
    pub crc32: u32,
    /// CUETools DB stride-adjusted CRC (CD audio only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ctdb_crc32: Option<u32>,
    pub verified: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitOutcome {
    #[serde(with = "path_serde")]
    pub output_folder: PathBuf,
    pub format: PCMFormat,
    pub plan: CueSplitPlan,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub toc: Option<DiscTOC>,
    pub tracks: Vec<SplitTrackOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ctdb_disc_crc32: Option<u32>,
    pub verified: bool,
    pub elapsed_seconds: f64,
    pub warnings: Vec<String>,
}

impl SplitOutcome {
    /// The CTDB CRCs of the numbered tracks, when every one has one.
    pub fn ctdb_track_crc32s(&self) -> Option<Vec<u32>> {
        let numbered: Vec<&SplitTrackOutput> = self.tracks.iter().filter(|t| t.number > 0).collect();
        let crcs: Vec<u32> = numbered.iter().filter_map(|t| t.ctdb_crc32).collect();
        (crcs.len() == numbered.len()).then_some(crcs)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SplitError {
    NoCueSheet,
    MissingFile(String),
    MixedFormats(String),
    OutputExists(String),
    NotCdFrameAligned,
    Tool(ToolError),
    Other(String),
}

impl fmt::Display for SplitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SplitError::NoCueSheet => write!(f, "This disc has no CUE sheet to split with."),
            SplitError::MissingFile(n) => write!(f, "The CUE references {n}, which does not exist."),
            SplitError::MixedFormats(n) => write!(f, "{n} has a different sample format than the first file."),
            SplitError::OutputExists(n) => write!(f, "{n} already exists. Enable overwrite to replace it."),
            SplitError::NotCdFrameAligned => write!(f, "The image length is not a whole number of CD frames."),
            SplitError::Tool(e) => write!(f, "{e}"),
            SplitError::Other(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SplitError {}

impl From<ToolError> for SplitError {
    fn from(e: ToolError) -> Self {
        SplitError::Tool(e)
    }
}
impl From<std::io::Error> for SplitError {
    fn from(e: std::io::Error) -> Self {
        SplitError::Other(e.to_string())
    }
}
impl From<flackit::FlacError> for SplitError {
    fn from(e: flackit::FlacError) -> Self {
        SplitError::Other(e.to_string())
    }
}

fn name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// CUETools DB leaves out 5880 samples at each end of the disc.
const CTDB_STRIDE: i64 = 5880;

pub struct ImageSplitter {
    pub tool: FFmpegTool,
}

impl ImageSplitter {
    pub fn new(tool: FFmpegTool) -> Self {
        ImageSplitter { tool }
    }

    pub fn split(
        &self,
        disc: &DetectedDisc,
        album: &AlbumContext,
        destination_root: &Path,
        options: &SplitOptions,
        progress: &mut dyn FnMut(SplitProgress),
    ) -> Result<SplitOutcome, SplitError> {
        let started = Instant::now();
        let cue = disc.cue.as_ref().ok_or(SplitError::NoCueSheet)?;
        let mut warnings = vec![];

        // 1. Source files, in CUE order (resolved like the scanner does).
        let mut sources = vec![];
        for f in &cue.files {
            let p = f.path_in(&disc.folder);
            let resolved = librarykit::resolve_on_disk(&p).ok_or_else(|| SplitError::MissingFile(name(&p)))?;
            sources.push(resolved);
        }

        // 2. Probe: every file must share one PCM format.
        progress(SplitProgress { phase: SplitPhase::Probing, fraction: 0.0 });
        let first = self.tool.probe(&sources[0])?;
        let format = first.pcm_format().map_err(|e| SplitError::Other(e.to_string()))?;
        for s in &sources[1..] {
            let info = self.tool.probe(s)?;
            let same = info.sample_rate == first.sample_rate
                && info.channels == first.channels
                && info.pcm_format().ok() == Some(format);
            if !same {
                return Err(SplitError::MixedFormats(name(s)));
            }
        }

        // 3. Decode everything into one raw file.
        let work = match &options.work_dir {
            Some(dir) => {
                std::fs::create_dir_all(dir)?;
                tempfile::Builder::new().prefix("drtagger-split-").tempdir_in(dir)?
            }
            None => tempfile::Builder::new().prefix("drtagger-split-").tempdir()?,
        };
        let raw = work.path().join("image.raw");
        let mut file_sample_counts = vec![];
        if sources.len() == 1 {
            progress(SplitProgress { phase: SplitPhase::Decoding { file: 1, of: 1 }, fraction: 0.05 });
            file_sample_counts.push(self.tool.decode_to_raw(&sources[0], format, &raw)?);
        } else {
            let mut sink = File::create(&raw)?;
            for (i, s) in sources.iter().enumerate() {
                progress(SplitProgress {
                    phase: SplitPhase::Decoding { file: i + 1, of: sources.len() },
                    fraction: 0.05 + 0.35 * i as f64 / sources.len() as f64,
                });
                let part = work.path().join(format!("part{i}.raw"));
                file_sample_counts.push(self.tool.decode_to_raw(s, format, &part)?);
                std::io::copy(&mut File::open(&part)?, &mut sink)?;
                std::fs::remove_file(&part)?;
            }
            sink.flush()?;
        }
        let total_samples: i64 = file_sample_counts.iter().sum();

        // 4. Plan and TOC.
        let plan = CueSplitPlan::make(cue, &file_sample_counts, format.sample_rate, options.htoa_threshold_frames)
            .map_err(|e| SplitError::Other(e.to_string()))?;
        let spf = format.sample_rate / CueTime::FRAMES_PER_SECOND;
        let toc = if file_sample_counts.iter().all(|c| c % spf == 0) {
            DiscTOC::from_cue(cue, &file_sample_counts.iter().map(|c| c / spf).collect::<Vec<_>>()).ok()
        } else {
            warnings.push("Image length is not frame aligned; no disc ID computed.".to_string());
            None
        };

        // Disc IDs derived here complete the caller's context.
        let mut album = album.clone();
        if album.musicbrainz_disc_id.is_none() {
            album.musicbrainz_disc_id = toc.as_ref().map(DiscTOC::musicbrainz_disc_id);
        }
        if album.disc_id.is_none() {
            album.disc_id = cue.disc_id().map(str::to_string).or_else(|| toc.as_ref().map(DiscTOC::freedb_disc_id));
        }

        // 5. Destination folder.
        let mut album_values: HashMap<String, String> = HashMap::new();
        album_values.insert(
            "albumartist".into(),
            album.album_artist.clone().or(cue.performer.clone()).unwrap_or_else(|| "Unknown Artist".into()),
        );
        album_values
            .insert("album".into(), album.album.clone().or(cue.title.clone()).unwrap_or_else(|| name(&disc.folder)));
        album_values
            .insert("year".into(), album.date.as_deref().map(|d| d.chars().take(4).collect()).unwrap_or_default());
        if let Some(n) = album.disc_number {
            album_values.insert("disc".into(), n.to_string());
        }
        let mut relative =
            PathTemplate::render(&options.album_folder_template, &album_values, options.ascii_file_names);
        if let (Some(total), Some(n)) = (album.disc_total, album.disc_number)
            && total > 1
        {
            relative += &format!("/Disc {n}");
        }
        if let Some(o) = options.album_folder_override.as_ref().filter(|o| !o.is_empty()) {
            relative = o.clone();
        }
        let output_folder =
            if relative.is_empty() { destination_root.to_path_buf() } else { destination_root.join(&relative) };
        std::fs::create_dir_all(&output_folder)?;

        // 6. Encode and verify each track.
        let mut outputs = vec![];
        let numbered = plan.numbered_tracks();
        let first_start = numbered.first().map_or(0, |t| t.start_sample);
        let disc_end = numbered.last().map_or(total_samples, |t| t.end_sample);
        let ctdb_applies = format.is_cd_audio() && toc.is_some();
        let count = plan.tracks.len();

        for (i, track) in plan.tracks.iter().enumerate() {
            progress(SplitProgress {
                phase: SplitPhase::Encoding { track: i + 1, of: count },
                fraction: 0.4 + 0.55 * i as f64 / count as f64,
            });
            let title = if track.is_hidden_track() {
                "Hidden Track".to_string()
            } else {
                track.title.clone().unwrap_or_else(|| format!("Track {}", track.number))
            };
            let mut values = HashMap::new();
            values.insert("track".to_string(), format!("{:02}", track.number));
            values.insert("title".to_string(), title);
            values.insert(
                "artist".to_string(),
                track.performer.clone().or(album.album_artist.clone()).unwrap_or_default(),
            );
            values.insert("album".to_string(), album_values["album"].clone());
            let file_name = PathTemplate::render(&options.track_file_template, &values, options.ascii_file_names);
            let file_name = if file_name.is_empty() { format!("{:02}", track.number) } else { file_name };
            let url = output_folder.join(format!("{file_name}.flac"));
            if url.exists() {
                if !options.overwrite_existing {
                    return Err(SplitError::OutputExists(name(&url)));
                }
                std::fs::remove_file(&url)?;
            }

            let digest = self.tool.encode_flac(
                &raw,
                format.byte_offset(track.start_sample),
                format.byte_offset(track.end_sample),
                format,
                options.compression_level,
                &url,
            )?;

            // Verify: STREAMINFO MD5 must equal the MD5 of the bytes fed; if
            // the encoder's MD5 differs, decode the track and compare CRCs.
            let flac = FLACFile::open(&url)?;
            let samples_ok = flac.stream_info.total_samples as i64 == track.sample_count();
            let mut md5_ok = flac.stream_info.md5_signature == digest.md5;
            let mut verified = samples_ok && md5_ok;
            if samples_ok && !md5_ok {
                let (crc, samples) = self.tool.raw_checksum(&url, format, work.path())?;
                verified = crc == digest.crc32 && samples == track.sample_count();
                if verified {
                    warnings.push(format!(
                        "{}: STREAMINFO MD5 differs from the fed PCM; verified by decode instead.",
                        name(&url)
                    ));
                }
                md5_ok = false;
            }
            if !verified {
                warnings.push(format!(
                    "{}: verification FAILED (samples {} vs {}).",
                    name(&url),
                    flac.stream_info.total_samples,
                    track.sample_count()
                ));
            }

            let mut ctdb = None;
            if ctdb_applies && !track.is_hidden_track() {
                let (mut s, mut e) = (track.start_sample, track.end_sample);
                if track.start_sample == first_start {
                    s += CTDB_STRIDE;
                }
                if track.end_sample == disc_end {
                    e -= CTDB_STRIDE;
                }
                if e > s {
                    ctdb = Some(crc32_of_file(&raw, format.byte_offset(s), format.byte_offset(e))?);
                }
            }

            if options.write_provisional_tags {
                provisional_tags::write(&provisional_tags::comment(track, numbered.len(), &album), &url)?;
            }

            outputs.push(SplitTrackOutput {
                number: track.number,
                title: if track.is_hidden_track() { None } else { track.title.clone() },
                url,
                samples: track.sample_count(),
                md5_hex: digest.md5_hex(),
                stream_info_md5_matches: md5_ok,
                crc32: digest.crc32,
                ctdb_crc32: ctdb,
                verified,
            });
        }

        progress(SplitProgress { phase: SplitPhase::Verifying, fraction: 0.97 });
        let mut disc_crc = None;
        if ctdb_applies && disc_end - CTDB_STRIDE > first_start + CTDB_STRIDE {
            disc_crc = Some(crc32_of_file(
                &raw,
                format.byte_offset(first_start + CTDB_STRIDE),
                format.byte_offset(disc_end - CTDB_STRIDE),
            )?);
        }
        progress(SplitProgress { phase: SplitPhase::Finished, fraction: 1.0 });
        let verified = outputs.iter().all(|t| t.verified);
        Ok(SplitOutcome {
            output_folder,
            format,
            plan,
            toc,
            tracks: outputs,
            ctdb_disc_crc32: disc_crc,
            verified,
            elapsed_seconds: started.elapsed().as_secs_f64(),
            warnings,
        })
    }
}
