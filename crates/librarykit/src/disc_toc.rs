//! Table of contents of a CD reconstructed from a CUE sheet plus the real
//! length of the audio file(s). Offsets are absolute CD frames including the
//! 150-frame (2 s) lead-in, as MusicBrainz and FreeDB expect.

use crate::cue::CueSheet;
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TocError {
    NoAudioTracks,
    FileCountMismatch { expected: usize, got: usize },
    MissingIndex01 { track: i64 },
    NotMonotonic { track: i64 },
    TooManyTracks(usize),
}

impl fmt::Display for TocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TocError::NoAudioTracks => write!(f, "The CUE sheet has no audio tracks."),
            TocError::FileCountMismatch { expected, got } => {
                write!(f, "CUE references {expected} files but {got} lengths were supplied.")
            }
            TocError::MissingIndex01 { track } => write!(f, "Track {track} has no INDEX 01."),
            TocError::NotMonotonic { track } => write!(f, "Track {track} starts before the previous track."),
            TocError::TooManyTracks(n) => write!(f, "{n} tracks exceed the 99 a CD can hold."),
        }
    }
}

impl std::error::Error for TocError {}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscTOC {
    pub first_track: i64,
    pub last_track: i64,
    /// Absolute frames, one per audio track.
    pub track_offsets: Vec<i64>,
    /// Absolute frames.
    pub lead_out: i64,
}

impl DiscTOC {
    pub const LEAD_IN_FRAMES: i64 = 150;
    /// 152 s gap before a data track (enhanced CDs).
    pub const DATA_TRACK_GAP_FRAMES: i64 = 11400;

    /// `file_frames` is the length in CD frames of each FILE in the sheet,
    /// in order. Multi-file sheets are laid out back to back, as on the disc.
    pub fn from_cue(cue: &CueSheet, file_frames: &[i64]) -> Result<DiscTOC, TocError> {
        if file_frames.len() != cue.files.len() {
            return Err(TocError::FileCountMismatch { expected: cue.files.len(), got: file_frames.len() });
        }
        let mut file_start = Vec::with_capacity(file_frames.len());
        let mut acc = 0;
        for &frames in file_frames {
            file_start.push(acc);
            acc += frames;
        }
        let total_frames = acc;

        let mut offsets: Vec<i64> = vec![];
        let mut data_track_offset: Option<i64> = None;
        let mut last_number = 0;
        for (file_index, file) in cue.files.iter().enumerate() {
            for track in &file.tracks {
                let slot = file_index + track.start_file_offset();
                let (Some(start), true) = (track.start(), slot < file_start.len()) else {
                    return Err(TocError::MissingIndex01 { track: track.number });
                };
                let absolute = file_start[slot] + start.frames + Self::LEAD_IN_FRAMES;
                if track.is_audio() {
                    if offsets.last().is_some_and(|&last| absolute < last) {
                        return Err(TocError::NotMonotonic { track: track.number });
                    }
                    offsets.push(absolute);
                    last_number = track.number;
                } else if data_track_offset.is_none() {
                    data_track_offset = Some(absolute);
                }
            }
        }
        if offsets.is_empty() {
            return Err(TocError::NoAudioTracks);
        }
        if offsets.len() > 99 {
            return Err(TocError::TooManyTracks(offsets.len()));
        }
        let first = cue.audio_tracks().first().map_or(1, |t| t.number);
        // Enhanced CD: the audio session ends 11400 frames before the data track.
        let lead_out =
            data_track_offset.map_or(total_frames + Self::LEAD_IN_FRAMES, |d| d - Self::DATA_TRACK_GAP_FRAMES);
        Ok(DiscTOC { first_track: first, last_track: last_number, track_offsets: offsets, lead_out })
    }

    pub fn track_count(&self) -> usize {
        self.track_offsets.len()
    }

    /// MusicBrainz Disc ID: SHA-1 over first/last track and 100 offsets in
    /// upper-case hex, base64 with the URL-safe substitutions '+'→'.',
    /// '/'→'_', '='→'-'. https://musicbrainz.org/doc/Disc_ID_Calculation
    pub fn musicbrainz_disc_id(&self) -> String {
        let mut s = format!("{:02X}{:02X}{:08X}", self.first_track, self.last_track, self.lead_out);
        for i in 0..99 {
            s += &format!("{:08X}", self.track_offsets.get(i).copied().unwrap_or(0));
        }
        let digest = Sha1::digest(s.as_bytes());
        base64::engine::general_purpose::STANDARD.encode(digest).replace('+', ".").replace('/', "_").replace('=', "-")
    }

    /// Classic FreeDB / CDDB ID, the value EAC writes as REM DISCID.
    pub fn freedb_disc_id(&self) -> String {
        fn digit_sum(mut n: i64) -> i64 {
            let mut sum = 0;
            while n > 0 {
                sum += n % 10;
                n /= 10;
            }
            sum
        }
        let n: i64 = self.track_offsets.iter().map(|o| digit_sum(o / 75)).sum();
        let total_seconds = self.lead_out / 75 - self.track_offsets[0] / 75;
        let id = ((n % 0xFF) << 24) | (total_seconds << 8) | self.track_offsets.len() as i64;
        format!("{id:08X}")
    }

    /// The `toc` parameter CUETools DB expects: track starts and lead-out in
    /// frames relative to the start of the program area (no lead-in).
    pub fn ctdb_toc_string(&self) -> String {
        self.track_offsets
            .iter()
            .chain(std::iter::once(&self.lead_out))
            .map(|o| (o - Self::LEAD_IN_FRAMES).to_string())
            .collect::<Vec<_>>()
            .join(":")
    }

    /// The `toc` query parameter for MusicBrainz's /ws/2/discid lookups:
    /// "first last leadout offset1 offset2 …".
    pub fn musicbrainz_toc_string(&self) -> String {
        [self.first_track, self.last_track, self.lead_out]
            .iter()
            .chain(self.track_offsets.iter())
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn musicbrainz_lookup_url(&self) -> String {
        format!(
            "https://musicbrainz.org/ws/2/discid/{}?toc={}&inc=recordings+artist-credits+labels&fmt=json",
            self.musicbrainz_disc_id(),
            self.musicbrainz_toc_string().replace(' ', "+")
        )
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    // Real rips whose REM DISCID was written by EAC; the MusicBrainz IDs were
    // cross-checked with an independent implementation on 2026-09-20.
    pub(crate) const WALKIN: &str = "REM DISCID 3C08E205
FILE \"Walkin.flac\" WAVE
  TRACK 01 AUDIO
    INDEX 00 00:00:00
    INDEX 01 00:00:37
  TRACK 02 AUDIO
    INDEX 00 13:27:60
    INDEX 01 13:29:37
  TRACK 03 AUDIO
    INDEX 00 21:47:27
    INDEX 01 21:48:54
  TRACK 04 AUDIO
    INDEX 00 26:32:69
    INDEX 01 26:33:69
  TRACK 05 AUDIO
    INDEX 00 30:57:74
    INDEX 01 30:57:74";
    pub(crate) const WALKIN_FRAMES: i64 = 170557; // 100287516 samples / 588

    fn cue(text: &str) -> CueSheet {
        CueSheet::parse_text(text).unwrap()
    }

    #[test]
    fn matches_real_rip_identifiers() {
        let toc = DiscTOC::from_cue(&cue(WALKIN), &[WALKIN_FRAMES]).unwrap();
        assert_eq!(toc.first_track, 1);
        assert_eq!(toc.last_track, 5);
        assert_eq!(toc.track_offsets, [37 + 150, 60712 + 150, 98154 + 150, 119544 + 150, 139349 + 150]);
        assert_eq!(toc.lead_out, 170557 + 150);
        assert_eq!(toc.freedb_disc_id(), "3C08E205");
        assert_eq!(toc.musicbrainz_disc_id(), "iOSL4j4VX_YutvVxL4QjWVsVEJE-");
        assert_eq!(toc.ctdb_toc_string(), "37:60712:98154:119544:139349:170557");
        assert_eq!(toc.musicbrainz_toc_string(), "1 5 170707 187 60862 98304 119694 139499");
    }

    #[test]
    fn second_real_rip() {
        let starts = [0, 13195, 31742, 52522, 75077, 104117, 120325, 139585, 164567, 184517, 200405, 223842, 246112];
        let mut text = "FILE \"a.flac\" WAVE\n".to_string();
        for (i, s) in starts.iter().enumerate() {
            text += &format!(
                "  TRACK {:02} AUDIO\n    INDEX 01 {:02}:{:02}:{:02}\n",
                i + 1,
                s / 4500,
                (s / 75) % 60,
                s % 75
            );
        }
        let toc = DiscTOC::from_cue(&cue(&text), &[266687]).unwrap();
        assert_eq!(toc.freedb_disc_id(), "B10DE30D");
        assert_eq!(toc.musicbrainz_disc_id(), "F76P4DCRiKgvHXm9SsWgm88xMMQ-");
    }

    #[test]
    fn multi_file_sheets_are_laid_out_back_to_back() {
        let text = "FILE \"01.wav\" WAVE\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\nFILE \"02.wav\" WAVE\n  TRACK 02 AUDIO\n    INDEX 00 00:00:00\n    INDEX 01 00:02:00\nFILE \"03.wav\" WAVE\n  TRACK 03 AUDIO\n    INDEX 01 00:00:00";
        let toc = DiscTOC::from_cue(&cue(text), &[1000, 2000, 3000]).unwrap();
        assert_eq!(toc.track_offsets, [150, 1000 + 150 + 150, 3000 + 150]);
        assert_eq!(toc.lead_out, 6150);
    }

    #[test]
    fn enhanced_cd_lead_out_stops_before_data_track() {
        let text = "FILE \"a.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n  TRACK 02 AUDIO\n    INDEX 01 10:00:00\n  TRACK 03 MODE1/2352\n    INDEX 01 30:00:00";
        let toc = DiscTOC::from_cue(&cue(text), &[200_000]).unwrap();
        assert_eq!(toc.last_track, 2);
        assert_eq!(toc.track_offsets.len(), 2);
        assert_eq!(toc.lead_out, 30 * 60 * 75 + 150 - 11400);
    }

    #[test]
    fn rejects_broken_sheets() {
        let missing = cue("FILE \"a.flac\" WAVE\n TRACK 01 AUDIO\n  INDEX 00 00:00:00\n");
        assert_eq!(DiscTOC::from_cue(&missing, &[100]), Err(TocError::MissingIndex01 { track: 1 }));
        assert_eq!(DiscTOC::from_cue(&cue(WALKIN), &[1, 2]), Err(TocError::FileCountMismatch { expected: 1, got: 2 }));
    }
}
