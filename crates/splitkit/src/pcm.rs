//! Interleaved little-endian signed PCM, the exchange format between ffmpeg
//! and the splitter, and the plain CRC-32 (IEEE, as zlib and CUETools).

use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PCMFormat {
    pub sample_rate: i64,
    pub channels: i64,
    pub bits_per_sample: i64,
}

/// "16-bit / 44.1 kHz / 2 ch", as Swift prints it (a whole kHz keeps ".0").
impl std::fmt::Display for PCMFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let khz = self.sample_rate as f64 / 1000.0;
        let khz = if khz.fract() == 0.0 { format!("{khz:.1}") } else { format!("{khz}") };
        write!(f, "{}-bit / {} kHz / {} ch", self.bits_per_sample, khz, self.channels)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormatError {
    UnsupportedBitDepth(i64),
    Invalid(String),
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FormatError::UnsupportedBitDepth(b) => write!(f, "{b}-bit PCM is not supported (16, 24 or 32 expected)."),
            FormatError::Invalid(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for FormatError {}

impl PCMFormat {
    pub const CD: PCMFormat = PCMFormat { sample_rate: 44100, channels: 2, bits_per_sample: 16 };

    pub fn new(sample_rate: i64, channels: i64, bits_per_sample: i64) -> Result<PCMFormat, FormatError> {
        if sample_rate <= 0 {
            return Err(FormatError::Invalid("Sample rate must be positive.".into()));
        }
        if !(1..=8).contains(&channels) {
            return Err(FormatError::Invalid(format!("Unsupported channel count {channels}.")));
        }
        if ![16, 24, 32].contains(&bits_per_sample) {
            return Err(FormatError::UnsupportedBitDepth(bits_per_sample));
        }
        Ok(PCMFormat { sample_rate, channels, bits_per_sample })
    }

    pub fn bytes_per_sample(self) -> i64 {
        self.bits_per_sample / 8
    }
    pub fn bytes_per_frame(self) -> i64 {
        self.bytes_per_sample() * self.channels
    }
    pub fn is_cd_audio(self) -> bool {
        self == Self::CD
    }
    /// ffmpeg's "-f s24le".
    pub fn ffmpeg_format_name(self) -> String {
        format!("s{}le", self.bits_per_sample)
    }
    /// ffmpeg's "-acodec pcm_s24le".
    pub fn ffmpeg_codec_name(self) -> String {
        format!("pcm_s{}le", self.bits_per_sample)
    }
    pub fn byte_offset(self, sample: i64) -> i64 {
        sample * self.bytes_per_frame()
    }
}

/// CRC-32 of a byte range of a file, read in chunks.
pub fn crc32_of_file(path: &Path, start: i64, end: i64) -> std::io::Result<u32> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(start as u64))?;
    let mut remaining = (end - start).max(0) as u64;
    let mut hasher = crc32fast::Hasher::new();
    let mut buf = vec![0u8; 4 << 20];
    while remaining > 0 {
        let n = {
            let take = remaining.min(buf.len() as u64) as usize;
            file.read(&mut buf[..take])?
        };
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        remaining -= n as u64;
    }
    Ok(hasher.finalize())
}

#[cfg(test)]
mod tests {
    #[test]
    fn crc_matches_zlib() {
        assert_eq!(crc32fast::hash(b"123456789"), 0xCBF4_3926);
    }
}
