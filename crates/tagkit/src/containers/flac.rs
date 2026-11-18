//! FLAC: VORBIS_COMMENT + PICTURE blocks. On write: STREAMINFO, other small
//! blocks, VORBIS_COMMENT, 4 KiB PADDING, PICTUREs.

use super::{ContainerContents, WriteSeek, copy, digest, finish, u32_be};
use crate::TagFileError;
use crate::picture::{Picture, PictureKind};
use crate::tag_set::TagSet;
use flackit::{BlockType, DataSource, FLACFile, VorbisComment};
use sha2::{Digest, Sha256};

pub fn read(
    source: &dyn DataSource,
    with_digest: bool,
    with_pictures: bool,
) -> Result<ContainerContents, TagFileError> {
    let file = FLACFile::parse(source, None)?;
    let mut tags = TagSet::new();
    for (name, value) in file.vorbis_comment.iter().flat_map(|v| v.fields.iter()) {
        tags.add(name, Some(value));
    }
    let mut pictures = vec![];
    if with_pictures {
        for block in file.blocks.iter().filter(|b| b.kind == BlockType::Picture) {
            if let Some(p) = parse_picture_block(&block.load_payload(source)?) {
                pictures.push(p);
            }
        }
    }
    let raw_metadata = source.read_at(4, (file.audio_frame_offset - 4) as usize)?;
    let audio_digest =
        if with_digest { digest(source, file.audio_frame_offset, file.audio_frame_length)? } else { String::new() };
    Ok(ContainerContents { tags, pictures, raw_metadata, audio_digest, ..Default::default() })
}

pub fn write(
    source: &dyn DataSource,
    tags: &TagSet,
    pictures: Option<&[Picture]>,
    out: &mut dyn WriteSeek,
) -> Result<String, TagFileError> {
    let file = FLACFile::parse(source, None)?;
    let stream_info = file
        .blocks
        .iter()
        .find(|b| b.kind == BlockType::StreamInfo)
        .ok_or_else(|| TagFileError::Malformed("no STREAMINFO".into()))?;
    let mut blocks = vec![(BlockType::StreamInfo, stream_info.load_payload(source)?)];
    let mut picture_blocks = vec![];
    for block in &file.blocks {
        match block.kind {
            BlockType::StreamInfo | BlockType::Padding | BlockType::VorbisComment => {}
            BlockType::Picture => {
                if pictures.is_none() {
                    picture_blocks.push(block.load_payload(source)?);
                }
            }
            kind => blocks.push((kind, block.load_payload(source)?)),
        }
    }
    if let Some(pictures) = pictures {
        picture_blocks = pictures.iter().map(picture_block).collect();
    }
    let vendor = file.vorbis_comment.as_ref().map_or_else(crate::vendor, |v| v.vendor.clone());
    let comment = VorbisComment::new(vendor, tags.pairs());
    blocks.push((BlockType::VorbisComment, comment.encoded()));
    blocks.push((BlockType::Padding, vec![0u8; 4096]));
    blocks.extend(picture_blocks.into_iter().map(|p| (BlockType::Picture, p)));

    out.write_all(b"fLaC")?;
    let last = blocks.len() - 1;
    for (i, (kind, payload)) in blocks.iter().enumerate() {
        out.write_all(&header(*kind, i == last, payload.len()))?;
        out.write_all(payload)?;
    }
    let mut hasher = Sha256::new();
    copy(source, file.audio_frame_offset, file.audio_frame_length, Some(out), &mut hasher)?;
    Ok(finish(hasher))
}

pub fn restore(source: &dyn DataSource, raw_metadata: &[u8], out: &mut dyn WriteSeek) -> Result<String, TagFileError> {
    let file = FLACFile::parse(source, None)?;
    out.write_all(b"fLaC")?;
    out.write_all(raw_metadata)?;
    let mut hasher = Sha256::new();
    copy(source, file.audio_frame_offset, file.audio_frame_length, Some(out), &mut hasher)?;
    Ok(finish(hasher))
}

fn header(kind: BlockType, is_last: bool, length: usize) -> [u8; 4] {
    [kind.raw() | if is_last { 0x80 } else { 0 }, (length >> 16) as u8, (length >> 8) as u8, length as u8]
}

/// PICTURE: type, mime, description, width, height, depth, colours, data
/// (big-endian, length-prefixed).
pub fn picture_block(p: &Picture) -> Vec<u8> {
    let mut d = vec![];
    let u32be = |d: &mut Vec<u8>, v: u32| d.extend_from_slice(&v.to_be_bytes());
    u32be(&mut d, p.kind.0 as u32);
    u32be(&mut d, p.mime_type.len() as u32);
    d.extend_from_slice(p.mime_type.as_bytes());
    u32be(&mut d, p.description.len() as u32);
    d.extend_from_slice(p.description.as_bytes());
    u32be(&mut d, p.width);
    u32be(&mut d, p.height);
    u32be(&mut d, 24);
    u32be(&mut d, 0);
    u32be(&mut d, p.data.len() as u32);
    d.extend_from_slice(&p.data);
    d
}

pub fn parse_picture_block(d: &[u8]) -> Option<Picture> {
    if d.len() < 32 {
        return None;
    }
    let mut c = 0;
    let kind = PictureKind::from_raw(u32_be(d, c));
    c += 4;
    let mime_len = u32_be(d, c) as usize;
    c += 4;
    if d.len() < c + mime_len + 4 {
        return None;
    }
    let mime_type = String::from_utf8_lossy(&d[c..c + mime_len]).into_owned();
    c += mime_len;
    let desc_len = u32_be(d, c) as usize;
    c += 4;
    if d.len() < c + desc_len + 20 {
        return None;
    }
    let description = String::from_utf8_lossy(&d[c..c + desc_len]).into_owned();
    c += desc_len;
    let (width, height) = (u32_be(d, c), u32_be(d, c + 4));
    c += 16;
    let len = u32_be(d, c) as usize;
    c += 4;
    if d.len() < c + len {
        return None;
    }
    Some(Picture { kind, mime_type, data: d[c..c + len].to_vec(), width, height, description })
}
