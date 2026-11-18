//! DSF (Sony DSD storage). Little-endian chunks:
//!   "DSD " (28): size 28, total file size, metadata pointer (ID3 offset or 0)
//!   "fmt " (52): version, format, channel type, channels, rate, bits,
//!                sample count, block size per channel, reserved
//!   "data": size including its 12-byte header, then the audio
//!   ID3v2 tag at the metadata pointer (no chunk header).

use crate::source::DataSource;
use crate::{FlacError, ID3v2Frame, ID3v2Tag};

#[derive(Clone, Debug)]
pub struct DSFFile {
    pub sample_rate: u32,
    pub bits_per_sample: u32,
    pub channel_count: u32,
    pub audio_byte_count: u64,
    /// DSD samples per channel.
    pub sample_count: u64,
    pub block_size_per_channel: u32,
    pub id3_tag: Option<ID3v2Tag>,
    /// APIC, MCDI, PRIV and GEOB frames of the original tag.
    pub preserved_frames: Vec<ID3v2Frame>,
    pub fmt_chunk_offset: u64,
    pub data_chunk_offset: u64,
    pub data_chunk_size: u64,
    pub id3_offset: u64,
    pub original_total_size: u64,
}

fn u32_le(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}

fn u64_le(d: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(d[o..o + 8].try_into().unwrap())
}

impl DSFFile {
    pub fn parse(source: &dyn DataSource) -> Result<DSFFile, FlacError> {
        let length = source.len()?;
        if length < 28 {
            return Err(FlacError::Truncated);
        }
        let dsd = source.read_at(0, 28)?;
        if &dsd[..4] != b"DSD " {
            return Err(FlacError::NotAFlacFile);
        }
        if u64_le(&dsd, 4) != 28 {
            return Err(FlacError::InvalidMetadataBlock("DSF DSD chunk size != 28".into()));
        }
        let total_file_size = u64_le(&dsd, 12);
        let metadata_pointer = u64_le(&dsd, 20);

        let fmt_offset = 28u64;
        if length < fmt_offset + 52 {
            return Err(FlacError::Truncated);
        }
        let fmt = source.read_at(fmt_offset, 52)?;
        if &fmt[..4] != b"fmt " {
            return Err(FlacError::InvalidMetadataBlock("DSF fmt chunk missing".into()));
        }
        if u64_le(&fmt, 4) != 52 {
            return Err(FlacError::InvalidMetadataBlock("DSF fmt chunk size != 52".into()));
        }
        let data_offset = fmt_offset + 52;
        if length < data_offset + 12 {
            return Err(FlacError::Truncated);
        }
        let data = source.read_at(data_offset, 12)?;
        if &data[..4] != b"data" {
            return Err(FlacError::InvalidMetadataBlock("DSF data chunk missing".into()));
        }
        let data_chunk_size = u64_le(&data, 4);

        // The tag is read up to 16 MB (covers fat cover art); a tag that
        // does not parse is ignored, as on the Mac.
        let (mut id3_tag, mut preserved_frames) = (None, vec![]);
        if metadata_pointer != 0 && metadata_pointer < length {
            let take = (length - metadata_pointer).min(16 * 1024 * 1024) as usize;
            if take >= 10 {
                let bytes = source.read_at(metadata_pointer, take)?;
                if bytes.starts_with(b"ID3")
                    && let Ok((tag, _)) = ID3v2Tag::parse(&bytes)
                {
                    preserved_frames = tag
                        .frames
                        .iter()
                        .filter(|f| matches!(f.id.as_str(), "APIC" | "MCDI" | "PRIV" | "GEOB"))
                        .cloned()
                        .collect();
                    id3_tag = Some(tag);
                }
            }
        }

        Ok(DSFFile {
            sample_rate: u32_le(&fmt, 28),
            bits_per_sample: u32_le(&fmt, 32),
            channel_count: u32_le(&fmt, 24),
            audio_byte_count: data_chunk_size.saturating_sub(12),
            sample_count: u64_le(&fmt, 36),
            block_size_per_channel: u32_le(&fmt, 44),
            id3_tag,
            preserved_frames,
            fmt_chunk_offset: fmt_offset,
            data_chunk_offset: data_offset,
            data_chunk_size,
            id3_offset: metadata_pointer,
            original_total_size: total_file_size,
        })
    }
}
