//! DSF: an ID3v2 trailer referenced from the DSD chunk's metadata pointer.

use super::{ContainerContents, WriteSeek, copy, digest, finish};
use crate::TagFileError;
use crate::id3_mapper;
use crate::picture::Picture;
use crate::tag_set::TagSet;
use flackit::{DSFFile, DataSource};
use sha2::{Digest, Sha256};

pub fn read(source: &dyn DataSource, with_digest: bool) -> Result<ContainerContents, TagFileError> {
    let file = DSFFile::parse(source)?;
    let parsed = file.id3_tag.as_ref().map(id3_mapper::parse).unwrap_or_default();
    let total = source.len()?;
    let raw_metadata = if file.id3_offset > 0 && file.id3_offset < total {
        source.read_at(file.id3_offset, (total - file.id3_offset) as usize)?
    } else {
        vec![]
    };
    let audio_digest =
        if with_digest { digest(source, file.data_chunk_offset + 12, file.audio_byte_count)? } else { String::new() };
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
    source: &dyn DataSource,
    tags: &TagSet,
    pictures: Option<&[Picture]>,
    out: &mut dyn WriteSeek,
) -> Result<String, TagFileError> {
    let contents = read(source, false)?;
    let tag = id3_mapper::tag(tags, pictures.unwrap_or(&contents.pictures), &contents.preserved);
    emit(source, &tag.encoded_v23(), out)
}

pub fn restore(source: &dyn DataSource, raw_metadata: &[u8], out: &mut dyn WriteSeek) -> Result<String, TagFileError> {
    emit(source, raw_metadata, out)
}

fn emit(source: &dyn DataSource, tag: &[u8], out: &mut dyn WriteSeek) -> Result<String, TagFileError> {
    let file = DSFFile::parse(source)?;
    let audio_start = file.data_chunk_offset + 12;
    let pointer = audio_start + file.audio_byte_count;
    let total = pointer + tag.len() as u64;
    let mut head = b"DSD ".to_vec();
    head.extend_from_slice(&28u64.to_le_bytes());
    head.extend_from_slice(&total.to_le_bytes());
    head.extend_from_slice(&(if tag.is_empty() { 0 } else { pointer }).to_le_bytes());
    out.write_all(&head)?;
    out.write_all(&source.read_at(file.fmt_chunk_offset, 52)?)?;
    out.write_all(&source.read_at(file.data_chunk_offset, 12)?)?;
    let mut hasher = Sha256::new();
    copy(source, audio_start, file.audio_byte_count, Some(out), &mut hasher)?;
    out.write_all(tag)?;
    Ok(finish(hasher))
}
