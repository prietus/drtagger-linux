//! FLAC metadata blocks (https://xiph.org/flac/format.html#metadata_block).
//! Small blocks are kept inline; large ones (PICTURE above all) are
//! recorded as offset/length references and loaded only when a rewrite
//! needs them, so a tag-only read touches a few KB of the file.

use crate::source::{DataSource, MemSource};
use crate::{FlacError, VorbisComment};
use std::path::Path;

/// Blocks above this size stay on disk until requested.
const INLINE_PAYLOAD_LIMIT: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BlockType {
    StreamInfo,
    Padding,
    Application,
    SeekTable,
    VorbisComment,
    CueSheet,
    Picture,
    /// Any other code; the raw value is kept so a rewrite preserves it.
    Other(u8),
}

impl BlockType {
    pub fn from_raw(raw: u8) -> BlockType {
        match raw {
            0 => BlockType::StreamInfo,
            1 => BlockType::Padding,
            2 => BlockType::Application,
            3 => BlockType::SeekTable,
            4 => BlockType::VorbisComment,
            5 => BlockType::CueSheet,
            6 => BlockType::Picture,
            // FLACKit maps every unknown code to `invalid` (127), which is
            // what a rewrite then writes back.
            _ => BlockType::Other(127),
        }
    }

    pub fn raw(self) -> u8 {
        match self {
            BlockType::StreamInfo => 0,
            BlockType::Padding => 1,
            BlockType::Application => 2,
            BlockType::SeekTable => 3,
            BlockType::VorbisComment => 4,
            BlockType::CueSheet => 5,
            BlockType::Picture => 6,
            BlockType::Other(r) => r,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Payload {
    Inline(Vec<u8>),
    Reference { offset: u64, length: usize },
}

impl Payload {
    pub fn len(&self) -> usize {
        match self {
            Payload::Inline(d) => d.len(),
            Payload::Reference { length, .. } => *length,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetadataBlock {
    pub kind: BlockType,
    pub is_last: bool,
    pub payload: Payload,
}

impl MetadataBlock {
    /// The payload bytes, read from `source` if the block is a reference.
    pub fn load_payload(&self, source: &dyn DataSource) -> Result<Vec<u8>, FlacError> {
        match &self.payload {
            Payload::Inline(d) => Ok(d.clone()),
            Payload::Reference { offset, length } => source.read_at(*offset, *length),
        }
    }
}

/// STREAMINFO: the mandatory first block. Its MD5 signature of the decoded
/// audio is preserved byte-exact on every write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamInfo {
    pub min_block_size: u16,
    pub max_block_size: u16,
    pub min_frame_size: u32,
    pub max_frame_size: u32,
    pub sample_rate: u32,
    pub channels: u8,
    pub bits_per_sample: u8,
    pub total_samples: u64,
    pub md5_signature: [u8; 16],
}

impl StreamInfo {
    pub fn duration_seconds(&self) -> f64 {
        if self.sample_rate == 0 { 0.0 } else { self.total_samples as f64 / self.sample_rate as f64 }
    }

    pub fn parse(data: &[u8]) -> Result<StreamInfo, FlacError> {
        if data.len() != 34 {
            return Err(FlacError::InvalidMetadataBlock(format!("STREAMINFO must be 34 bytes, got {}", data.len())));
        }
        let mut bit = 0usize;
        let mut read = |count: usize| -> u64 {
            let mut v = 0u64;
            for _ in 0..count {
                v = (v << 1) | ((data[bit >> 3] >> (7 - (bit & 7))) & 1) as u64;
                bit += 1;
            }
            v
        };
        let min_block_size = read(16) as u16;
        let max_block_size = read(16) as u16;
        let min_frame_size = read(24) as u32;
        let max_frame_size = read(24) as u32;
        let sample_rate = read(20) as u32;
        let channels = read(3) as u8 + 1;
        let bits_per_sample = read(5) as u8 + 1;
        let total_samples = read(36);
        let mut md5_signature = [0u8; 16];
        md5_signature.copy_from_slice(&data[18..34]);
        Ok(StreamInfo {
            min_block_size,
            max_block_size,
            min_frame_size,
            max_frame_size,
            sample_rate,
            channels,
            bits_per_sample,
            total_samples,
            md5_signature,
        })
    }
}

#[derive(Clone, Debug)]
pub struct FLACFile {
    pub stream_info: StreamInfo,
    pub vorbis_comment: Option<VorbisComment>,
    pub blocks: Vec<MetadataBlock>,
    pub audio_frame_offset: u64,
    pub audio_frame_length: u64,
}

pub(crate) fn block_header(kind: BlockType, is_last: bool, length: usize) -> [u8; 4] {
    [kind.raw() & 0x7F | if is_last { 0x80 } else { 0 }, (length >> 16) as u8, (length >> 8) as u8, length as u8]
}

impl FLACFile {
    pub fn open(path: &Path) -> Result<FLACFile, FlacError> {
        Self::parse(&crate::FileSource::open(path)?, None)
    }

    pub fn from_bytes(data: Vec<u8>) -> Result<FLACFile, FlacError> {
        Self::parse(&MemSource::new(data), None)
    }

    /// With `total_length`, `source` may cover only a prefix of the file:
    /// parsing stops quietly at the first block header beyond it, and
    /// referenced blocks are recorded without being read.
    pub fn parse(source: &dyn DataSource, total_length: Option<u64>) -> Result<FLACFile, FlacError> {
        let source_length = source.len()?;
        let total = total_length.unwrap_or(source_length);
        if source_length < 4 {
            return Err(FlacError::Truncated);
        }
        if source.read_at(0, 4)? != b"fLaC" {
            return Err(FlacError::NotAFlacFile);
        }

        let mut cursor = 4u64;
        let mut blocks = vec![];
        let mut stream_info = None;
        let mut vorbis = None;
        let mut is_last = false;
        while !is_last {
            if cursor + 4 > source_length {
                if total_length.is_some() {
                    break;
                }
                return Err(FlacError::Truncated);
            }
            let header = source.read_at(cursor, 4)?;
            cursor += 4;
            is_last = header[0] & 0x80 != 0;
            let kind = BlockType::from_raw(header[0] & 0x7F);
            let length = (header[1] as usize) << 16 | (header[2] as usize) << 8 | header[3] as usize;
            let must_load = matches!(kind, BlockType::StreamInfo | BlockType::VorbisComment);
            let payload = if must_load || length <= INLINE_PAYLOAD_LIMIT {
                if cursor + length as u64 > source_length {
                    return Err(FlacError::Truncated);
                }
                let data = source.read_at(cursor, length)?;
                match kind {
                    BlockType::StreamInfo => stream_info = Some(StreamInfo::parse(&data)?),
                    BlockType::VorbisComment => vorbis = Some(VorbisComment::parse(&data)?),
                    _ => {}
                }
                Payload::Inline(data)
            } else {
                Payload::Reference { offset: cursor, length }
            };
            blocks.push(MetadataBlock { kind, is_last, payload });
            cursor += length as u64;
        }
        let stream_info = stream_info.ok_or(FlacError::UnsupportedStreamInfo)?;
        Ok(FLACFile {
            stream_info,
            vorbis_comment: vorbis,
            blocks,
            audio_frame_offset: cursor,
            audio_frame_length: total.saturating_sub(cursor),
        })
    }

    /// The whole file with its metadata section rebuilt around `tags`:
    /// STREAMINFO, other small blocks, VORBIS_COMMENT, 4 KiB of PADDING,
    /// then PICTUREs. Audio frames are copied byte-exact from `source`.
    pub fn rewritten(&self, tags: &VorbisComment, source: &dyn DataSource) -> Result<Vec<u8>, FlacError> {
        let mut out = b"fLaC".to_vec();
        let blocks = self.rebuilt_blocks(tags, source)?;
        for (i, (kind, payload)) in blocks.iter().enumerate() {
            out.extend_from_slice(&block_header(*kind, i == blocks.len() - 1, payload.len()));
            out.extend_from_slice(payload);
        }
        let mut remaining = self.audio_frame_length;
        let mut cursor = self.audio_frame_offset;
        while remaining > 0 {
            let take = remaining.min(1 << 20);
            out.extend_from_slice(&source.read_at(cursor, take as usize)?);
            cursor += take;
            remaining -= take;
        }
        Ok(out)
    }

    fn rebuilt_blocks(
        &self,
        tags: &VorbisComment,
        source: &dyn DataSource,
    ) -> Result<Vec<(BlockType, Vec<u8>)>, FlacError> {
        let stream_info =
            self.blocks.iter().find(|b| b.kind == BlockType::StreamInfo).ok_or(FlacError::UnsupportedStreamInfo)?;
        let mut result = vec![(BlockType::StreamInfo, stream_info.load_payload(source)?)];
        let mut pictures = vec![];
        for block in &self.blocks {
            match block.kind {
                BlockType::StreamInfo | BlockType::Padding | BlockType::VorbisComment => {}
                BlockType::Picture => pictures.push((BlockType::Picture, block.load_payload(source)?)),
                kind => result.push((kind, block.load_payload(source)?)),
            }
        }
        result.push((BlockType::VorbisComment, tags.encoded()));
        result.push((BlockType::Padding, vec![0u8; 4096]));
        result.extend(pictures);
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FileSource;
    use sha2::{Digest, Sha256};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tone.flac")
    }

    #[test]
    fn rejects_non_flac_and_truncated() {
        assert_eq!(FLACFile::from_bytes(vec![0, 1, 2, 3, 4]).unwrap_err(), FlacError::NotAFlacFile);
        assert_eq!(FLACFile::from_bytes(vec![0x66, 0x4C]).unwrap_err(), FlacError::Truncated);
    }

    #[test]
    fn parses_the_fixture() {
        let file = FLACFile::open(&fixture()).unwrap();
        let si = &file.stream_info;
        assert_eq!((si.sample_rate, si.channels, si.bits_per_sample), (44100, 2, 16));
        assert!(si.total_samples > 0);
        assert_eq!(file.vorbis_comment.as_ref().unwrap().get("ARTIST"), ["drtagger"]);
    }

    #[test]
    fn round_trip_preserves_audio_bytes_and_new_tags() {
        let source = FileSource::open(&fixture()).unwrap();
        let file = FLACFile::parse(&source, None).unwrap();
        let old = file.vorbis_comment.clone().unwrap();
        let mut fields = old.fields.clone();
        if let Some(f) = fields.iter_mut().find(|(n, _)| n == "TITLE") {
            f.1 = "Test Tone (drtagger test)".into();
        }
        fields.push(("DRTAGGER_TEST".into(), "ok".into()));
        let rewritten = file.rewritten(&VorbisComment::new(old.vendor.clone(), fields), &source).unwrap();
        let reparsed = FLACFile::from_bytes(rewritten.clone()).unwrap();
        let vc = reparsed.vorbis_comment.as_ref().unwrap();
        assert_eq!(vc.get("TITLE"), ["Test Tone (drtagger test)"]);
        assert_eq!(vc.get("DRTAGGER_TEST"), ["ok"]);
        assert_eq!(vc.get("ARTIST"), ["drtagger"]);
        let orig = source.read_at(file.audio_frame_offset, file.audio_frame_length as usize).unwrap();
        let start = reparsed.audio_frame_offset as usize;
        let new = &rewritten[start..start + reparsed.audio_frame_length as usize];
        assert_eq!(Sha256::digest(&orig), Sha256::digest(new), "audio frames must be byte-identical");
        assert_eq!(reparsed.stream_info.md5_signature, file.stream_info.md5_signature);
    }

    #[test]
    fn picture_is_preserved_through_a_reference() {
        let source = FileSource::open(&fixture()).unwrap();
        let file = FLACFile::parse(&source, None).unwrap();
        let picture = file.blocks.iter().find(|b| b.kind == BlockType::Picture).unwrap();
        assert!(matches!(picture.payload, Payload::Reference { .. }), "PICTURE must not be inlined");
        let original = picture.load_payload(&source).unwrap();
        let rewritten = file.rewritten(file.vorbis_comment.as_ref().unwrap(), &source).unwrap();
        let mem = MemSource::new(rewritten);
        let reparsed = FLACFile::parse(&mem, None).unwrap();
        let again = reparsed.blocks.iter().find(|b| b.kind == BlockType::Picture).unwrap();
        assert_eq!(again.load_payload(&mem).unwrap(), original);
    }

    struct Counting<'a>(&'a dyn DataSource, AtomicUsize);
    impl DataSource for Counting<'_> {
        fn len(&self) -> Result<u64, FlacError> {
            self.0.len()
        }
        fn read_at(&self, offset: u64, length: usize) -> Result<Vec<u8>, FlacError> {
            self.1.fetch_add(length, Ordering::Relaxed);
            self.0.read_at(offset, length)
        }
    }

    #[test]
    fn tag_only_parse_reads_little() {
        let inner = FileSource::open(&fixture()).unwrap();
        let counting = Counting(&inner, AtomicUsize::new(0));
        let file = FLACFile::parse(&counting, None).unwrap();
        assert!(file.vorbis_comment.is_some());
        assert!(counting.1.load(Ordering::Relaxed) < 16 * 1024);
    }
}
