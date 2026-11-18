//! Streams DSD audio into a DSF file (Sony DSF 1.01): "DSD " chunk, "fmt "
//! chunk, "data" chunk with per-channel blocks of 4096 bytes, and an
//! optional ID3v2 tag at the end referenced from the DSD chunk. Input is
//! the SACD frame layout (bytes interleaved per channel, MSB first); DSF
//! keeps each channel in its own block, LSB first, so every byte is
//! bit-reversed while de-interleaving.

use std::fmt;
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const BLOCK_SIZE: usize = 4096;
const OUTPUT_BATCH_BYTES: usize = 4 << 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriterError {
    CannotCreate(String),
    BadChannelCount(usize),
    MisalignedInput(usize),
    Io(String),
}

impl fmt::Display for WriterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WriterError::CannotCreate(p) => write!(f, "Cannot create {p}."),
            WriterError::BadChannelCount(n) => write!(f, "{n} channels cannot be stored in DSF."),
            WriterError::MisalignedInput(n) => write!(f, "Input of {n} bytes is not a multiple of the channel count."),
            WriterError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for WriterError {}

impl From<std::io::Error> for WriterError {
    fn from(e: std::io::Error) -> Self {
        WriterError::Io(e.to_string())
    }
}

pub fn bit_reverse(b: u8) -> u8 {
    b.reverse_bits()
}

/// DSF channel type codes: 1 mono, 2 stereo, 3 3ch, 4 quad, 6 5ch, 7 5.1ch.
pub fn channel_type(count: usize) -> u32 {
    match count {
        5 => 6,
        6 => 7,
        n => n as u32,
    }
}

pub struct DSFWriter {
    pub path: PathBuf,
    pub channel_count: usize,
    pub sample_rate: u32,
    file: File,
    /// Channel c's current block lives at c * BLOCK_SIZE.
    block_group: Vec<u8>,
    fill: usize,
    output: Vec<u8>,
    samples_per_channel: u64,
    data_bytes: u64,
}

impl DSFWriter {
    pub fn create(path: &Path, channel_count: usize, sample_rate: u32) -> Result<DSFWriter, WriterError> {
        if !(1..=6).contains(&channel_count) {
            return Err(WriterError::BadChannelCount(channel_count));
        }
        let mut file = File::create(path).map_err(|_| WriterError::CannotCreate(path.display().to_string()))?;
        // The 28 + 52 + 12 header bytes are rewritten in finish().
        file.write_all(&[0u8; 92])?;
        Ok(DSFWriter {
            path: path.to_path_buf(),
            channel_count,
            sample_rate,
            file,
            block_group: vec![0; channel_count * BLOCK_SIZE],
            fill: 0,
            output: Vec::with_capacity(OUTPUT_BATCH_BYTES + channel_count * BLOCK_SIZE),
            samples_per_channel: 0,
            data_bytes: 0,
        })
    }

    pub fn write_frame(&mut self, data: &[u8]) -> Result<(), WriterError> {
        let ch = self.channel_count;
        if !data.len().is_multiple_of(ch) {
            return Err(WriterError::MisalignedInput(data.len()));
        }
        let mut i = 0;
        while i < data.len() {
            let take = ((data.len() - i) / ch).min(BLOCK_SIZE - self.fill);
            for f in 0..take {
                let base = i + f * ch;
                let out = self.fill + f;
                for c in 0..ch {
                    self.block_group[c * BLOCK_SIZE + out] = data[base + c].reverse_bits();
                }
            }
            i += take * ch;
            self.fill += take;
            if self.fill == BLOCK_SIZE {
                self.flush_block_group()?;
            }
        }
        self.samples_per_channel += (data.len() / ch) as u64 * 8;
        Ok(())
    }

    fn flush_block_group(&mut self) -> Result<(), WriterError> {
        self.output.extend_from_slice(&self.block_group);
        self.data_bytes += (self.channel_count * BLOCK_SIZE) as u64;
        self.fill = 0;
        if self.output.len() >= OUTPUT_BATCH_BYTES {
            self.file.write_all(&self.output)?;
            self.output.clear();
        }
        Ok(())
    }

    /// Pads the last block with zeros, writes the headers and the optional
    /// ID3 tag. Returns samples per channel and the file size.
    pub fn finish(mut self, id3: Option<&[u8]>) -> Result<(u64, u64), WriterError> {
        if self.fill > 0 {
            for c in 0..self.channel_count {
                self.block_group[c * BLOCK_SIZE + self.fill..(c + 1) * BLOCK_SIZE].fill(0);
            }
            self.flush_block_group()?;
        }
        if !self.output.is_empty() {
            self.file.write_all(&self.output)?;
            self.output.clear();
        }
        let data_chunk_size = 12 + self.data_bytes;
        let mut id3_offset = 0u64;
        let mut file_size = 92 + self.data_bytes;
        if let Some(tag) = id3.filter(|t| !t.is_empty()) {
            id3_offset = file_size;
            self.file.write_all(tag)?;
            file_size += tag.len() as u64;
        }
        let mut h = Vec::with_capacity(92);
        h.extend_from_slice(b"DSD ");
        h.extend_from_slice(&28u64.to_le_bytes());
        h.extend_from_slice(&file_size.to_le_bytes());
        h.extend_from_slice(&id3_offset.to_le_bytes());
        h.extend_from_slice(b"fmt ");
        h.extend_from_slice(&52u64.to_le_bytes());
        for v in [1u32, 0, channel_type(self.channel_count), self.channel_count as u32, self.sample_rate, 1] {
            h.extend_from_slice(&v.to_le_bytes());
        }
        h.extend_from_slice(&self.samples_per_channel.to_le_bytes());
        h.extend_from_slice(&(BLOCK_SIZE as u32).to_le_bytes());
        h.extend_from_slice(&0u32.to_le_bytes());
        h.extend_from_slice(b"data");
        h.extend_from_slice(&data_chunk_size.to_le_bytes());
        debug_assert_eq!(h.len(), 92);
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&h)?;
        self.file.flush()?;
        Ok((self.samples_per_channel, file_size))
    }
}
