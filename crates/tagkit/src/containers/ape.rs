//! APEv2 (Monkey's Audio, WavPack, TTA): a tag at the end of the file with
//! a header and a footer, possibly followed by an ID3v1 tag that is dropped
//! on write.

use super::{ContainerContents, WriteSeek, ascii, copy, digest, finish, u32_le};
use crate::TagFileError;
use crate::picture::{Picture, PictureKind};
use crate::tag_set::{TagSet, field};
use flackit::DataSource;
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Item {
    key: String,
    flags: u32,
    value: Vec<u8>,
}

impl Item {
    fn is_text(&self) -> bool {
        (self.flags >> 1) & 3 == 0
    }
    fn is_binary(&self) -> bool {
        (self.flags >> 1) & 3 == 1
    }
}

struct Layout {
    items: Vec<Item>,
    audio_length: u64,
    raw: Vec<u8>,
}

const KEY_MAP: &[(&str, &str)] = &[
    ("Title", field::TITLE),
    ("Artist", field::ARTIST),
    ("Artists", field::ARTISTS),
    ("Album", field::ALBUM),
    ("Album Artist", field::ALBUM_ARTIST),
    ("Year", field::DATE),
    ("Original Date", field::ORIGINAL_DATE),
    ("Track", field::TRACK_NUMBER),
    ("Disc", field::DISC_NUMBER),
    ("Genre", field::GENRE),
    ("Composer", field::COMPOSER),
    ("Lyricist", field::LYRICIST),
    ("Conductor", field::CONDUCTOR),
    ("Comment", field::COMMENT),
    ("ISRC", field::ISRC),
    ("CatalogNumber", field::CATALOG_NUMBER),
    ("Barcode", field::BARCODE),
    ("Label", field::LABEL),
    ("Media", field::MEDIA),
    ("DiscSubtitle", field::DISC_SUBTITLE),
    ("MixArtist", field::REMIXER),
    ("Arranger", field::ARRANGER),
    ("Producer", field::PRODUCER),
    ("Engineer", field::ENGINEER),
    ("Mixer", field::MIXER),
    ("Performer", field::PERFORMER),
    ("Writer", field::WRITER),
    ("Work", field::WORK),
    ("Script", field::SCRIPT),
    ("Compilation", field::COMPILATION),
    ("ArtistSort", field::ARTIST_SORT),
    ("AlbumSort", field::ALBUM_SORT),
    ("AlbumArtistSort", field::ALBUM_ARTIST_SORT),
    ("ComposerSort", field::COMPOSER_SORT),
    ("Style", field::STYLE),
    ("ReleaseCountry", field::RELEASE_COUNTRY),
    ("ReleaseStatus", field::RELEASE_STATUS),
    ("ReleaseType", field::RELEASE_TYPE),
];

fn layout(source: &dyn DataSource) -> Result<Layout, TagFileError> {
    let total = source.len()?;
    let mut end = total;
    if total >= 128 && ascii(&source.read_at(total - 128, 3)?) == "TAG" {
        end = total - 128;
    }
    let bare = || -> Result<Layout, TagFileError> {
        let raw = if end < total { source.read_at(end, (total - end) as usize)? } else { vec![] };
        Ok(Layout { items: vec![], audio_length: end, raw })
    };
    if end < 32 {
        return bare();
    }
    let footer = source.read_at(end - 32, 32)?;
    if ascii(&footer[..8]) != "APETAGEX" {
        return bare();
    }
    let size = u32_le(&footer, 12) as u64;
    let count = u32_le(&footer, 16);
    let flags = u32_le(&footer, 20);
    let has_header = flags & 0x8000_0000 != 0;
    let malformed = || TagFileError::Malformed("APE tag size".into());
    let items_start = end.checked_sub(size).ok_or_else(malformed)?;
    let tag_start = if has_header { items_start.checked_sub(32).ok_or_else(malformed)? } else { items_start };
    if tag_start > end || size < 32 {
        return Err(malformed());
    }
    let raw = source.read_at(tag_start, (total - tag_start) as usize)?; // tag plus any ID3v1 after it
    let body = source.read_at(items_start, (size - 32) as usize)?;
    let mut items = vec![];
    let mut c = 0usize;
    for _ in 0..count {
        if c + 8 > body.len() {
            break;
        }
        let len = u32_le(&body, c) as usize;
        let flags = u32_le(&body, c + 4);
        c += 8;
        let Some(nul) = body[c..].iter().position(|&b| b == 0).map(|p| p + c) else { break };
        let key = String::from_utf8_lossy(&body[c..nul]).into_owned();
        c = nul + 1;
        if c + len > body.len() {
            break;
        }
        items.push(Item { key, flags, value: body[c..c + len].to_vec() });
        c += len;
    }
    Ok(Layout { items, audio_length: tag_start, raw })
}

fn split_slash(v: &str) -> Vec<String> {
    v.splitn(2, '/').filter(|s| !s.is_empty()).map(str::to_string).collect()
}

pub fn read(source: &dyn DataSource, with_digest: bool) -> Result<ContainerContents, TagFileError> {
    let l = layout(source)?;
    let mut tags = TagSet::new();
    let mut pictures = vec![];
    let mut opaque = vec![];
    let (mut track_total, mut disc_total): (Option<String>, Option<String>) = (None, None);
    for item in &l.items {
        let lower = item.key.to_lowercase();
        if item.is_text() {
            let f = KEY_MAP
                .iter()
                .find(|(k, _)| k.to_lowercase() == lower)
                .map_or_else(|| item.key.to_uppercase(), |(_, f)| f.to_string());
            for v in String::from_utf8_lossy(&item.value).split('\0').filter(|s| !s.is_empty()) {
                if (f == field::TRACK_NUMBER || f == field::DISC_NUMBER) && v.contains('/') {
                    let parts = split_slash(v);
                    tags.add(&f, parts.first().map(String::as_str));
                    let last = parts.last().cloned();
                    if f == field::TRACK_NUMBER {
                        track_total = last;
                    } else {
                        disc_total = last;
                    }
                } else {
                    tags.add(&f, Some(v));
                }
            }
        } else if item.is_binary()
            && lower.starts_with("cover art")
            && let Some(nul) = item.value.iter().position(|&b| b == 0)
        {
            let kind = if lower.contains("back") { PictureKind::BACK } else { PictureKind::FRONT };
            pictures.push(Picture::from_image(kind, item.value[nul + 1..].to_vec(), "image/jpeg"));
        } else {
            opaque.extend(encode(item));
        }
    }
    if let Some(t) = track_total {
        tags.set(field::TRACK_TOTAL, Some(&t));
        tags.set(field::TOTAL_TRACKS, Some(&t));
    }
    if let Some(d) = disc_total {
        tags.set(field::DISC_TOTAL, Some(&d));
        tags.set(field::TOTAL_DISCS, Some(&d));
    }
    let audio_digest = if with_digest { digest(source, 0, l.audio_length)? } else { String::new() };
    Ok(ContainerContents { tags, pictures, raw_metadata: l.raw, audio_digest, preserved: vec![], opaque })
}

pub fn write(
    source: &dyn DataSource,
    tags: &TagSet,
    pictures: Option<&[Picture]>,
    out: &mut dyn WriteSeek,
) -> Result<String, TagFileError> {
    let contents = read(source, false)?;
    let mut items: Vec<Item> = vec![];
    let mut used: Vec<&str> = vec![];
    let text = |items: &mut Vec<Item>, key: &str, values: &[String]| {
        if !values.is_empty() {
            items.push(Item { key: key.to_string(), flags: 0, value: values.join("\0").into_bytes() });
        }
    };
    for (key, f) in KEY_MAP {
        if tags.get(f).is_empty() {
            continue;
        }
        let mut values = tags.get(f).to_vec();
        if *f == field::TRACK_NUMBER
            && let Some(total) = tags.first(field::TRACK_TOTAL).or(tags.first(field::TOTAL_TRACKS))
        {
            values = vec![format!("{}/{total}", values[0])];
        }
        if *f == field::DISC_NUMBER
            && let Some(total) = tags.first(field::DISC_TOTAL).or(tags.first(field::TOTAL_DISCS))
        {
            values = vec![format!("{}/{total}", values[0])];
        }
        text(&mut items, key, &values);
        used.push(f);
    }
    used.extend([field::TRACK_TOTAL, field::TOTAL_TRACKS, field::DISC_TOTAL, field::TOTAL_DISCS]);
    for name in tags.names() {
        if !used.contains(&name.as_str()) {
            text(&mut items, &name, tags.get(&name));
        }
    }
    for p in pictures.unwrap_or(&contents.pictures) {
        let mut v = format!("cover.{}", p.file_extension()).into_bytes();
        v.push(0);
        v.extend_from_slice(&p.data);
        let key = if p.kind == PictureKind::BACK { "Cover Art (Back)" } else { "Cover Art (Front)" };
        items.push(Item { key: key.to_string(), flags: 1 << 1, value: v });
    }
    let mut tag = vec![];
    for item in &items {
        tag.extend(encode(item));
    }
    tag.extend_from_slice(&contents.opaque);
    let count = (items.len() + count_items(&contents.opaque)) as u32;
    let size = (tag.len() + 32) as u32;
    let frame = |is_header: bool| {
        let mut d = b"APETAGEX".to_vec();
        d.extend_from_slice(&2000u32.to_le_bytes());
        d.extend_from_slice(&size.to_le_bytes());
        d.extend_from_slice(&count.to_le_bytes());
        d.extend_from_slice(&(0x8000_0000u32 | if is_header { 0x2000_0000 } else { 0 }).to_le_bytes());
        d.extend_from_slice(&[0u8; 8]);
        d
    };
    let l = layout(source)?;
    let mut hasher = Sha256::new();
    copy(source, 0, l.audio_length, Some(out), &mut hasher)?;
    if !items.is_empty() || !contents.opaque.is_empty() {
        out.write_all(&frame(true))?;
        out.write_all(&tag)?;
        out.write_all(&frame(false))?;
    }
    Ok(finish(hasher))
}

pub fn restore(source: &dyn DataSource, raw_metadata: &[u8], out: &mut dyn WriteSeek) -> Result<String, TagFileError> {
    let l = layout(source)?;
    let mut hasher = Sha256::new();
    copy(source, 0, l.audio_length, Some(out), &mut hasher)?;
    out.write_all(raw_metadata)?;
    Ok(finish(hasher))
}

fn encode(item: &Item) -> Vec<u8> {
    let mut d = vec![];
    d.extend_from_slice(&(item.value.len() as u32).to_le_bytes());
    d.extend_from_slice(&item.flags.to_le_bytes());
    d.extend_from_slice(item.key.as_bytes());
    d.push(0);
    d.extend_from_slice(&item.value);
    d
}

fn count_items(blob: &[u8]) -> usize {
    let (mut c, mut n) = (0usize, 0);
    while c + 8 <= blob.len() {
        let len = u32_le(blob, c) as usize;
        c += 8;
        let Some(nul) = blob[c..].iter().position(|&b| b == 0).map(|p| p + c) else { break };
        c = nul + 1 + len;
        n += 1;
    }
    n
}
