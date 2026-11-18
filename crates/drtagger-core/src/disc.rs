//! Disc work on queue records: the TOC from the files' exact lengths,
//! splitting CUE images and extracting SACD ISOs into the library (the Mac
//! app's DiscService). The CUETools DB lookup joins in the providers phase.

use crate::store::{AlbumRecord, AlbumState, Store};
use anyhow::{Context, Result};
use flackit::FLACFile;
use librarykit::{CueTime, DiscTOC};
use splitkit::{AlbumContext, FFmpegTool, ImageSplitter, SplitOptions, SplitOutcome, SplitPhase, SplitProgress};
use std::path::{Path, PathBuf};

/// Where large temporary files go (decoded images): the cache directory,
/// not a RAM-backed /tmp.
pub fn work_dir() -> PathBuf {
    crate::paths::cache_dir().join("work")
}

/// FLAC headers carry the exact sample count; other formats are decoded
/// once (their length is what the splitter will see anyway).
fn exact_sample_count(tool: &FFmpegTool, url: &Path, info: &splitkit::StreamInfo) -> Result<i64> {
    if url.extension().is_some_and(|e| e.eq_ignore_ascii_case("flac"))
        && let Ok(flac) = FLACFile::open(url)
    {
        return Ok(flac.stream_info.total_samples as i64);
    }
    let format = info.pcm_format()?;
    let dir = work_dir();
    std::fs::create_dir_all(&dir)?;
    Ok(tool.raw_checksum(url, format, &dir)?.1)
}

/// Probes the first disc's file(s) for their exact length and derives the TOC.
pub fn compute_toc(record: &AlbumRecord, tool: &FFmpegTool) -> Result<Option<DiscTOC>> {
    match record.detected() {
        Some(detected) => album_toc(&detected, tool),
        None => Ok(None),
    }
}

/// The TOC of an album's first CUE disc, from the exact sample counts of
/// its files (FLAC headers, or one decode).
pub fn album_toc(detected: &librarykit::DetectedAlbum, tool: &FFmpegTool) -> Result<Option<DiscTOC>> {
    let Some(disc) = detected.discs.first() else { return Ok(None) };
    let Some(cue) = &disc.cue else { return Ok(None) };
    let mut frames = vec![];
    for file in &cue.files {
        let path = file.path_in(&disc.folder);
        let url = librarykit::resolve_on_disk(&path).with_context(|| format!("{} is missing", path.display()))?;
        let info = tool.probe(&url)?;
        let samples = exact_sample_count(tool, &url, &info)?;
        let spf = info.sample_rate / CueTime::FRAMES_PER_SECOND;
        if spf <= 0 || samples % spf != 0 {
            anyhow::bail!("{}", librarykit::TocError::NoAudioTracks);
        }
        frames.push(samples / spf);
    }
    Ok(Some(DiscTOC::from_cue(cue, &frames)?))
}

fn describe(phase: &SplitPhase) -> String {
    match phase {
        SplitPhase::Probing => "Reading the image…".into(),
        SplitPhase::Decoding { file, of } if *of > 1 => format!("Decoding file {file} of {of}…"),
        SplitPhase::Decoding { .. } => "Decoding…".into(),
        SplitPhase::Encoding { track, of } => format!("Encoding track {track} of {of}…"),
        SplitPhase::Verifying => "Verifying…".into(),
        SplitPhase::Finished => "Done".into(),
    }
}

/// Progress for the whole album: a message and 0…1.
pub type Progress<'a> = &'a mut dyn FnMut(&str, f64);

/// Splits every CUE disc of `record` into `destination` and stores the
/// outcomes on the record. After a fully verified split the images and
/// their CUEs can go to the trash.
pub fn split(
    store: &Store,
    record: &mut AlbumRecord,
    destination: &Path,
    tool: &FFmpegTool,
    options: &SplitOptions,
    trash_originals: bool,
    progress: Progress,
) -> Result<Vec<SplitOutcome>> {
    let detected = record.detected().context("the album has no scan details")?;
    let mut discs: Vec<_> = detected.discs.iter().filter(|d| d.cue.is_some()).collect();
    discs.sort_by_key(|d| d.number);
    if discs.is_empty() {
        return Ok(vec![]);
    }
    record.state = AlbumState::Applying;
    store.save(record)?;
    let multi = discs.len() > 1;
    let existing_toc = record.toc();
    let mut outcomes = vec![];
    let mut options = options.clone();
    if options.work_dir.is_none() {
        options.work_dir = Some(work_dir());
    }
    let result = (|| -> Result<()> {
        for (i, disc) in discs.iter().enumerate() {
            let cue = disc.cue.as_ref().unwrap();
            let context = AlbumContext::from_cue(
                cue,
                if multi { Some(disc.number) } else { record.set_position.map(|p| p as u32) },
                if multi { Some(discs.len() as u32) } else { record.set_total.map(|t| t as u32) },
                if i == 0 { existing_toc.as_ref() } else { None },
            );
            let mut options = options.clone();
            // Members of a set share the album folder named after the set.
            if record.set_id.is_some()
                && let Some(title) = &record.set_title
            {
                let base = set_folder(
                    &options.album_folder_template,
                    record.artist_hint.clone().or_else(|| cue.performer.clone()),
                    title,
                    record.year_hint.clone(),
                    options.ascii_file_names,
                );
                options.album_folder_override = Some(format!("{base}/Disc {}", record.set_position.unwrap_or(1)));
            }
            let label = if multi { format!("Disc {}: ", disc.number) } else { String::new() };
            let total = discs.len() as f64;
            let outcome = ImageSplitter::new(tool.clone()).split(
                disc,
                &context,
                destination,
                &options,
                &mut |p: SplitProgress| {
                    progress(&format!("{label}{}", describe(&p.phase)), (i as f64 + p.fraction) / total)
                },
            )?;
            outcomes.push(outcome);
        }
        Ok(())
    })();
    if !outcomes.is_empty() {
        record.set_split_outcomes(&outcomes);
        if record.toc().is_none() {
            record.set_toc(outcomes[0].toc.as_ref());
        }
    }
    match result {
        Ok(()) => {
            let verified = outcomes.iter().all(|o| o.verified);
            record.state = if verified { AlbumState::Scanned } else { AlbumState::Error };
            record.error_message =
                (!verified).then(|| outcomes.iter().flat_map(|o| o.warnings.clone()).collect::<Vec<_>>().join("\n"));
            if verified && trash_originals {
                // Only after a byte-verified split.
                let originals: Vec<PathBuf> = discs
                    .iter()
                    .flat_map(|d| [d.image_file.as_ref().map(|f| f.url.clone()), d.cue_url.clone()])
                    .flatten()
                    .collect();
                if let Err(e) = crate::trash::delete_all(&originals) {
                    record.error_message = Some(format!("Could not move originals to the trash: {e}"));
                }
            }
            store.save(record)?;
            Ok(outcomes)
        }
        Err(e) => {
            record.state = AlbumState::Error;
            record.error_message = Some(format!("{e:#}"));
            store.save(record)?;
            Err(e)
        }
    }
}

/// The album folder a release set's discs share.
fn set_folder(template: &str, artist: Option<String>, album: &str, year: Option<String>, ascii: bool) -> String {
    let values = std::collections::HashMap::from([
        ("albumartist".to_string(), artist.unwrap_or_else(|| "Unknown Artist".into())),
        ("album".to_string(), album.to_string()),
        ("year".to_string(), year.unwrap_or_default()),
    ]);
    splitkit::PathTemplate::render(template, &values, ascii)
}

/// Which areas to extract and what happens to the ISO afterwards.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SacdJob {
    /// Also the multichannel area, when the disc has one.
    pub multichannel: bool,
    /// The ISO goes to the trash after a verified extraction.
    pub trash_originals: bool,
}

/// Extracts the stereo area (and the multichannel one when asked) of a SACD
/// image into DSF files under `destination`; outcomes are stored on the
/// record.
pub fn extract_sacd(
    store: &Store,
    record: &mut AlbumRecord,
    destination: &Path,
    options: &sacdkit::SACDExtractOptions,
    job: SacdJob,
    cancel: Option<&splitkit::Cancel>,
    progress: Progress,
) -> Result<Vec<sacdkit::SACDExtractOutcome>> {
    let SacdJob { multichannel, trash_originals } = job;
    if record.kind != librarykit::AlbumKind::SacdIso {
        return Ok(vec![]);
    }
    record.state = AlbumState::Applying;
    store.save(record)?;
    let path = record.path.clone();
    let mut options = options.clone();
    // Members of a set share the album folder named after the set.
    if record.set_id.is_some()
        && let Some(sacd) = record.detected().and_then(|d| d.sacd)
    {
        let base = set_folder(
            &options.album_folder_template,
            sacd.album_artist.clone().or_else(|| record.artist_hint.clone()),
            &sacd
                .album_title
                .clone()
                .or_else(|| record.set_title.clone())
                .unwrap_or_else(|| record.display_title.clone()),
            record.year_hint.clone(),
            options.ascii_file_names,
        );
        options.album_folder_override =
            Some(format!("{base}/Disc {}", record.set_position.unwrap_or(sacd.album_sequence_number)));
    }
    let options = &options;
    let result = (|| -> Result<Vec<sacdkit::SACDExtractOutcome>> {
        progress("Reading disc…", 0.0);
        let disc = sacdkit::read(&path)?;
        let mut areas = vec![];
        if let Some(a) = disc.stereo_area() {
            areas.push(a);
        }
        if multichannel && let Some(a) = disc.multichannel_area() {
            areas.push(a);
        }
        let mut outcomes = vec![];
        let total = areas.len() as f64;
        for (i, area) in areas.iter().enumerate() {
            let label = area.display_name();
            outcomes.push(sacdkit::SACDExtractor::extract(&disc, area, destination, options, cancel, &mut |p| {
                progress(&format!("{label}: {}", p.message), (i as f64 + p.fraction) / total)
            })?);
        }
        Ok(outcomes)
    })();
    match result {
        Ok(outcomes) => {
            record.set_sacd_outcomes(&outcomes);
            let ok = outcomes.iter().all(|o| o.verified());
            record.state = if ok { AlbumState::Scanned } else { AlbumState::Error };
            record.error_message =
                (!ok).then(|| outcomes.iter().flat_map(|o| o.warnings.clone()).collect::<Vec<_>>().join("\n"));
            if ok
                && trash_originals
                && let Err(e) = crate::trash::delete(&path)
            {
                record.error_message = Some(format!("Could not move the original to the trash: {e}"));
            }
            store.save(record)?;
            Ok(outcomes)
        }
        Err(e) => {
            record.state = AlbumState::Error;
            record.error_message = Some(format!("{e:#}"));
            store.save(record)?;
            Err(e)
        }
    }
}
