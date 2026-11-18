//! Sample-accurate track boundaries for splitting an image (or a run of
//! per-track files) described by a CUE sheet.
//!
//! Gap policy: every track runs from its INDEX 01 to the next track's
//! INDEX 01, so a following track's pregap (INDEX 00) stays at the end of
//! the previous track, the way EAC and XLD split. Audio before track 1's
//! INDEX 01 becomes track 00 (HTOA) when it is longer than
//! `htoa_threshold_frames`; a shorter lead-in (the usual 2 s of silence) is
//! dropped.

use crate::cue::{CueSheet, CueTime, CueTrack};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitTrack {
    /// 0 = hidden track one audio.
    pub number: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub performer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub isrc: Option<String>,
    pub start_sample: i64,
    /// Exclusive.
    pub end_sample: i64,
}

impl SplitTrack {
    pub fn sample_count(&self) -> i64 {
        self.end_sample - self.start_sample
    }
    pub fn is_hidden_track(&self) -> bool {
        self.number == 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanError {
    FileCountMismatch { expected: usize, got: usize },
    UnsupportedSampleRate(i64),
    NoAudioTracks,
    MissingIndex01 { track: i64 },
    IndexBeyondFile { track: i64 },
    NotMonotonic { track: i64 },
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlanError::FileCountMismatch { expected, got } => {
                write!(f, "CUE references {expected} files but {got} lengths were supplied.")
            }
            PlanError::UnsupportedSampleRate(r) => write!(f, "{r} Hz is not a multiple of 75 CD frames per second."),
            PlanError::NoAudioTracks => write!(f, "The CUE sheet has no audio tracks."),
            PlanError::MissingIndex01 { track } => write!(f, "Track {track} has no INDEX 01."),
            PlanError::IndexBeyondFile { track } => write!(f, "Track {track} starts beyond the end of its file."),
            PlanError::NotMonotonic { track } => write!(f, "Track {track} starts before the previous track."),
        }
    }
}

impl std::error::Error for PlanError {}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CueSplitPlan {
    pub sample_rate: i64,
    pub total_samples: i64,
    /// Track 00 first when present.
    pub tracks: Vec<SplitTrack>,
    /// Silence before track 1 that was not kept.
    pub dropped_lead_in_samples: i64,
}

impl CueSplitPlan {
    pub const DEFAULT_HTOA_THRESHOLD_FRAMES: i64 = 150;

    pub fn hidden_track(&self) -> Option<&SplitTrack> {
        self.tracks.iter().find(|t| t.is_hidden_track())
    }

    pub fn numbered_tracks(&self) -> Vec<&SplitTrack> {
        self.tracks.iter().filter(|t| !t.is_hidden_track()).collect()
    }

    /// `file_sample_counts` are the decoded lengths of each FILE in the sheet.
    pub fn make(
        cue: &CueSheet,
        file_sample_counts: &[i64],
        sample_rate: i64,
        htoa_threshold_frames: i64,
    ) -> Result<CueSplitPlan, PlanError> {
        if file_sample_counts.len() != cue.files.len() {
            return Err(PlanError::FileCountMismatch { expected: cue.files.len(), got: file_sample_counts.len() });
        }
        if sample_rate <= 0 || sample_rate % CueTime::FRAMES_PER_SECOND != 0 {
            return Err(PlanError::UnsupportedSampleRate(sample_rate));
        }
        let samples_per_frame = sample_rate / CueTime::FRAMES_PER_SECOND;

        let mut file_start = Vec::with_capacity(file_sample_counts.len());
        let mut total = 0;
        for &count in file_sample_counts {
            file_start.push(total);
            total += count;
        }

        // Absolute start of every audio track.
        let mut starts: Vec<(&CueTrack, i64)> = vec![];
        for (file_index, file) in cue.files.iter().enumerate() {
            for track in file.tracks.iter().filter(|t| t.is_audio()) {
                let start_file = file_index + track.start_file_offset();
                let (Some(index01), true) = (track.start(), start_file < file_start.len()) else {
                    return Err(PlanError::MissingIndex01 { track: track.number });
                };
                let sample = file_start[start_file] + index01.frames * samples_per_frame;
                if sample > file_start[start_file] + file_sample_counts[start_file] {
                    return Err(PlanError::IndexBeyondFile { track: track.number });
                }
                if starts.last().is_some_and(|&(_, last)| sample < last) {
                    return Err(PlanError::NotMonotonic { track: track.number });
                }
                starts.push((track, sample));
            }
        }
        if starts.is_empty() {
            return Err(PlanError::NoAudioTracks);
        }

        let mut tracks = vec![];
        let mut dropped = 0;
        let lead_in = starts[0].1;
        if lead_in > 0 {
            if lead_in > htoa_threshold_frames * samples_per_frame {
                tracks.push(SplitTrack {
                    number: 0,
                    title: None,
                    performer: None,
                    isrc: None,
                    start_sample: 0,
                    end_sample: lead_in,
                });
            } else {
                dropped = lead_in;
            }
        }
        for (i, &(track, sample)) in starts.iter().enumerate() {
            let end = starts.get(i + 1).map_or(total, |&(_, s)| s);
            tracks.push(SplitTrack {
                number: track.number,
                title: track.title.clone(),
                performer: track.performer.clone(),
                isrc: track.isrc.clone(),
                start_sample: sample,
                end_sample: end,
            });
        }
        Ok(CueSplitPlan { sample_rate, total_samples: total, tracks, dropped_lead_in_samples: dropped })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disc_toc::tests::{WALKIN, WALKIN_FRAMES};

    const T: i64 = CueSplitPlan::DEFAULT_HTOA_THRESHOLD_FRAMES;

    #[test]
    fn gaps_go_to_previous_track_and_short_lead_in_is_dropped() {
        let cue = CueSheet::parse_text(WALKIN).unwrap();
        let total = WALKIN_FRAMES * 588;
        let plan = CueSplitPlan::make(&cue, &[total], 44100, T).unwrap();
        assert!(plan.hidden_track().is_none()); // 37 frames < 150 threshold
        assert_eq!(plan.dropped_lead_in_samples, 37 * 588);
        assert_eq!(plan.tracks.len(), 5);
        assert_eq!(plan.tracks[0].start_sample, 37 * 588);
        assert_eq!(plan.tracks[0].end_sample, 60712 * 588); // track 2's INDEX 01
        assert_eq!(plan.tracks[4].end_sample, total);
        assert_eq!(plan.tracks.iter().map(SplitTrack::sample_count).sum::<i64>() + plan.dropped_lead_in_samples, total);
    }

    #[test]
    fn long_lead_in_becomes_hidden_track() {
        let text = "FILE \"a.flac\" WAVE\n  TRACK 01 AUDIO\n    TITLE \"One\"\n    INDEX 00 00:00:00\n    INDEX 01 00:10:00\n  TRACK 02 AUDIO\n    TITLE \"Two\"\n    INDEX 01 01:00:00";
        let cue = CueSheet::parse_text(text).unwrap();
        let plan = CueSplitPlan::make(&cue, &[5_000_000], 44100, T).unwrap();
        let htoa = plan.hidden_track().unwrap();
        assert_eq!(htoa.start_sample, 0);
        assert_eq!(htoa.end_sample, 10 * 75 * 588);
        let numbered = plan.numbered_tracks();
        assert_eq!(numbered.iter().map(|t| t.number).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(numbered[0].title.as_deref(), Some("One"));
        assert_eq!(numbered[1].end_sample, 5_000_000);
        assert_eq!(plan.dropped_lead_in_samples, 0);

        let strict = CueSplitPlan::make(&cue, &[5_000_000], 44100, 1000).unwrap();
        assert!(strict.hidden_track().is_none());
        assert_eq!(strict.dropped_lead_in_samples, 10 * 75 * 588);
    }

    #[test]
    fn multi_file_with_gaps_prepended() {
        let text = "FILE \"01.wav\" WAVE\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\nFILE \"02.wav\" WAVE\n  TRACK 02 AUDIO\n    INDEX 00 00:00:00\n    INDEX 01 00:02:00\nFILE \"03.wav\" WAVE\n  TRACK 03 AUDIO\n    INDEX 00 00:00:00\n    INDEX 01 00:01:00";
        let cue = CueSheet::parse_text(text).unwrap();
        let (f1, f2, f3) = (1000 * 588, 2000 * 588, 3000 * 588);
        let plan = CueSplitPlan::make(&cue, &[f1, f2, f3], 44100, T).unwrap();
        assert_eq!(plan.tracks.len(), 3);
        assert_eq!(plan.tracks[0].start_sample, 0);
        assert_eq!(plan.tracks[0].end_sample, f1 + 150 * 588);
        assert_eq!(plan.tracks[1].start_sample, f1 + 150 * 588);
        assert_eq!(plan.tracks[1].end_sample, f1 + f2 + 75 * 588);
        assert_eq!(plan.tracks[2].end_sample, f1 + f2 + f3);
        assert_eq!(plan.total_samples, f1 + f2 + f3);
    }

    #[test]
    fn high_res_rates_and_errors() {
        let cue = CueSheet::parse_text(WALKIN).unwrap();
        let plan = CueSplitPlan::make(&cue, &[WALKIN_FRAMES * 1176], 88200, T).unwrap();
        assert_eq!(plan.tracks[0].start_sample, 37 * 1176);
        assert_eq!(CueSplitPlan::make(&cue, &[1000], 44000, T), Err(PlanError::UnsupportedSampleRate(44000)));
        assert_eq!(CueSplitPlan::make(&cue, &[1000 * 588], 44100, T), Err(PlanError::IndexBeyondFile { track: 2 }));
    }
}
