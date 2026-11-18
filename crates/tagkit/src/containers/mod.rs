//! One module per container family. Each reads tags, pictures, the raw
//! metadata bytes (for byte-exact restores) and a SHA-256 of the audio
//! payload, and writes a new file whose audio is copied — and hashed — from
//! the original.

pub mod ape;
pub mod dsf;
pub mod flac;
pub mod iff;
pub mod mp4;

use crate::TagFileError;
use crate::picture::Picture;
use crate::tag_set::TagSet;
use flackit::{DataSource, ID3v2Frame};
use sha2::{Digest, Sha256};
use std::io::{Seek, Write};

/// What a container hands back.
#[derive(Clone, Debug, Default)]
pub struct ContainerContents {
    pub tags: TagSet,
    pub pictures: Vec<Picture>,
    /// Enough to restore the original tags byte-exact.
    pub raw_metadata: Vec<u8>,
    /// SHA-256 of the audio payload, hex; empty when not computed.
    pub audio_digest: String,
    /// ID3 containers only.
    pub preserved: Vec<ID3v2Frame>,
    /// Container-specific leftovers (APE binary items, MP4 unknown ilst atoms).
    pub opaque: Vec<u8>,
}

pub trait WriteSeek: Write + Seek {}
impl<T: Write + Seek> WriteSeek for T {}

const CHUNK: u64 = 1 << 20;

/// Streams `length` bytes from `offset` of `source` into `out` (if any),
/// hashing on the way.
pub(crate) fn copy(
    source: &dyn DataSource,
    offset: u64,
    length: u64,
    mut out: Option<&mut dyn WriteSeek>,
    hasher: &mut Sha256,
) -> Result<(), TagFileError> {
    let (mut remaining, mut cursor) = (length, offset);
    while remaining > 0 {
        let take = remaining.min(CHUNK);
        let data = source.read_at(cursor, take as usize)?;
        hasher.update(&data);
        if let Some(out) = out.as_mut() {
            out.write_all(&data)?;
        }
        cursor += take;
        remaining -= take;
    }
    Ok(())
}

pub(crate) fn digest(source: &dyn DataSource, offset: u64, length: u64) -> Result<String, TagFileError> {
    let mut h = Sha256::new();
    copy(source, offset, length, None, &mut h)?;
    Ok(hex::encode(h.finalize()))
}

pub(crate) fn finish(h: Sha256) -> String {
    hex::encode(h.finalize())
}

pub(crate) fn u16_be(d: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([d[at], d[at + 1]])
}
pub(crate) fn u32_be(d: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(d[at..at + 4].try_into().unwrap())
}
pub(crate) fn u32_le(d: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(d[at..at + 4].try_into().unwrap())
}
pub(crate) fn u64_be(d: &[u8], at: usize) -> u64 {
    u64::from_be_bytes(d[at..at + 8].try_into().unwrap())
}

/// Swift's `String(decoding:as: UTF8.self)` of a 4-byte chunk or atom id.
pub(crate) fn ascii(d: &[u8]) -> String {
    String::from_utf8_lossy(d).into_owned()
}
