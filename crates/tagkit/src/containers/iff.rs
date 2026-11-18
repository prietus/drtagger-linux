//! RIFF/WAVE, FORM/AIFF and FRM8/DSDIFF share one shape: a header, then
//! chunks; the tag lives in an "id3 " / "ID3 " chunk. Only the size width
//! and byte order differ.

use super::{ContainerContents, WriteSeek, ascii, copy, finish, u32_be, u32_le, u64_be};
use crate::TagFileError;
use crate::id3_mapper;
use crate::picture::Picture;
use crate::tag_set::TagSet;
use flackit::{DataSource, ID3v2Tag};
use sha2::{Digest, Sha256};
use std::io::SeekFrom;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flavor {
    Wav,
    Aiff,
    Dff,
}

struct Chunk {
    id: String,
    offset: u64,
    size: u64,
    header_size: u64,
}

impl Chunk {
    fn payload_offset(&self) -> u64 {
        self.offset + self.header_size
    }
    fn padded(&self) -> u64 {
        self.size + self.size % 2
    }
    fn is_tag(&self) -> bool {
        self.id == "id3 " || self.id == "ID3 "
    }
}

impl Flavor {
    fn size_width(self) -> u64 {
        if self == Flavor::Dff { 8 } else { 4 }
    }
    fn little_endian(self) -> bool {
        self == Flavor::Wav
    }
    fn header_length(self) -> u64 {
        4 + self.size_width() + 4
    }
    fn audio_chunk_id(self) -> &'static str {
        match self {
            Flavor::Wav => "data",
            Flavor::Aiff => "SSND",
            Flavor::Dff => "DSD ",
        }
    }
    fn tag_chunk_id(self) -> &'static [u8] {
        if self == Flavor::Wav { b"id3 " } else { b"ID3 " }
    }

    fn size_field(self, value: u64) -> Vec<u8> {
        match (self.size_width(), self.little_endian()) {
            (8, _) => value.to_be_bytes().to_vec(),
            (_, true) => (value as u32).to_le_bytes().to_vec(),
            _ => (value as u32).to_be_bytes().to_vec(),
        }
    }

    fn chunks(self, source: &dyn DataSource) -> Result<Vec<Chunk>, TagFileError> {
        let total = source.len()?;
        if total < self.header_length() {
            return Err(TagFileError::Malformed("file too short".into()));
        }
        let head = source.read_at(0, self.header_length() as usize)?;
        let magic = ascii(&head[..4]);
        let expected = match self {
            Flavor::Wav => "RIFF",
            Flavor::Aiff => "FORM",
            Flavor::Dff => "FRM8",
        };
        if magic != expected {
            return Err(TagFileError::Malformed(format!("expected {expected}, found {magic}")));
        }
        let sw = self.size_width();
        let mut list = vec![];
        let mut cursor = self.header_length();
        while cursor + 4 + sw <= total {
            let h = source.read_at(cursor, (4 + sw) as usize)?;
            let size = match (sw, self.little_endian()) {
                (8, _) => u64_be(&h, 4),
                (_, true) => u32_le(&h, 4) as u64,
                _ => u32_be(&h, 4) as u64,
            };
            list.push(Chunk { id: ascii(&h[..4]), offset: cursor, size, header_size: 4 + sw });
            let next = cursor + 4 + sw + size + size % 2;
            if next <= cursor {
                break;
            }
            cursor = next;
        }
        Ok(list)
    }

    pub fn read(self, source: &dyn DataSource, with_digest: bool) -> Result<ContainerContents, TagFileError> {
        let list = self.chunks(source)?;
        let mut parsed = id3_mapper::Parsed::default();
        let mut raw_metadata = vec![];
        if let Some(tag) = list.iter().find(|c| c.is_tag())
            && tag.size > 10
        {
            raw_metadata = source.read_at(tag.payload_offset(), tag.size as usize)?;
            if let Ok((t, _)) = ID3v2Tag::parse(&raw_metadata) {
                parsed = id3_mapper::parse(&t);
            }
        }
        let audio_digest = if with_digest {
            let mut h = Sha256::new();
            for c in list.iter().filter(|c| c.id == self.audio_chunk_id()) {
                copy(source, c.payload_offset(), c.size, None, &mut h)?;
            }
            finish(h)
        } else {
            String::new()
        };
        Ok(ContainerContents {
            tags: parsed.tags,
            pictures: parsed.pictures,
            raw_metadata,
            audio_digest,
            preserved: parsed.preserved,
            opaque: vec![],
        })
    }

    pub fn write(
        self,
        source: &dyn DataSource,
        tags: &TagSet,
        pictures: Option<&[Picture]>,
        out: &mut dyn WriteSeek,
    ) -> Result<String, TagFileError> {
        let contents = self.read(source, false)?;
        let tag = id3_mapper::tag(tags, pictures.unwrap_or(&contents.pictures), &contents.preserved);
        self.emit(source, &tag.encoded_v23(), out)
    }

    pub fn restore(
        self,
        source: &dyn DataSource,
        raw_metadata: &[u8],
        out: &mut dyn WriteSeek,
    ) -> Result<String, TagFileError> {
        self.emit(source, raw_metadata, out)
    }

    fn emit(self, source: &dyn DataSource, tag: &[u8], out: &mut dyn WriteSeek) -> Result<String, TagFileError> {
        let list = self.chunks(source)?;
        let sw = self.size_width() as usize;
        let mut head = source.read_at(0, self.header_length() as usize)?;
        head[4..4 + sw].fill(0); // patched below
        out.write_all(&head)?;
        let mut hasher = Sha256::new();
        for c in list.iter().filter(|c| !c.is_tag()) {
            out.write_all(&source.read_at(c.offset, c.header_size as usize)?)?;
            if c.id == self.audio_chunk_id() {
                copy(source, c.payload_offset(), c.size, Some(out), &mut hasher)?;
                if c.size % 2 == 1 {
                    out.write_all(&[0])?;
                }
            } else {
                let mut scratch = Sha256::new();
                copy(source, c.payload_offset(), c.padded(), Some(out), &mut scratch)?;
            }
        }
        if !tag.is_empty() {
            out.write_all(self.tag_chunk_id())?;
            out.write_all(&self.size_field(tag.len() as u64))?;
            out.write_all(tag)?;
            if tag.len() % 2 == 1 {
                out.write_all(&[0])?;
            }
        }
        // Top-level size = everything after magic + size field.
        let total = out.stream_position()?;
        out.seek(SeekFrom::Start(4))?;
        out.write_all(&self.size_field(total - 4 - sw as u64))?;
        out.seek(SeekFrom::Start(total))?;
        Ok(finish(hasher))
    }
}
