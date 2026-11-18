//! MP4/M4A (ALAC): tags in moov/udta/meta/ilst. A rewrite never moves
//! mdat: an old moov in front of the audio becomes a `free` atom of the
//! same size and the new moov is appended, so chunk offsets stay valid.

use super::{ContainerContents, WriteSeek, copy, finish, u16_be, u32_be, u64_be};
use crate::TagFileError;
use crate::picture::{Picture, PictureKind};
use crate::tag_set::{TagSet, field};
use flackit::{DataSource, text};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug)]
struct Atom {
    kind: String,
    offset: u64,
    size: u64,
    header_size: u64,
}

impl Atom {
    fn payload_offset(&self) -> u64 {
        self.offset + self.header_size
    }
    fn payload_size(&self) -> u64 {
        self.size - self.header_size
    }
}

const SIMPLE: &[(&str, &str)] = &[
    ("\u{A9}nam", field::TITLE),
    ("\u{A9}ART", field::ARTIST),
    ("aART", field::ALBUM_ARTIST),
    ("\u{A9}alb", field::ALBUM),
    ("\u{A9}day", field::DATE),
    ("\u{A9}gen", field::GENRE),
    ("\u{A9}wrt", field::COMPOSER),
    ("\u{A9}cmt", field::COMMENT),
    ("\u{A9}grp", "GROUPING"),
    ("\u{A9}lyr", "LYRICS"),
    ("soar", field::ARTIST_SORT),
    ("soaa", field::ALBUM_ARTIST_SORT),
    ("soal", field::ALBUM_SORT),
    ("soco", field::COMPOSER_SORT),
    ("sonm", "TITLESORT"),
];

const FREEFORM: &[(&str, &str)] = &[
    ("MusicBrainz Track Id", field::MB_TRACK_ID),
    ("MusicBrainz Artist Id", field::MB_ARTIST_ID),
    ("MusicBrainz Album Id", field::MB_ALBUM_ID),
    ("MusicBrainz Album Artist Id", field::MB_ALBUM_ARTIST_ID),
    ("MusicBrainz Release Group Id", field::MB_RELEASE_GROUP_ID),
    ("MusicBrainz Release Track Id", field::MB_RELEASE_TRACK_ID),
    ("MusicBrainz Work Id", field::MB_WORK_ID),
    ("MusicBrainz Disc Id", field::MB_DISC_ID),
    ("MusicBrainz Album Status", field::RELEASE_STATUS),
    ("MusicBrainz Album Type", field::RELEASE_TYPE),
    ("MusicBrainz Album Release Country", field::RELEASE_COUNTRY),
    ("Acoustid Id", field::ACOUSTID),
    ("Acoustid Fingerprint", field::ACOUSTID_FINGERPRINT),
    ("CATALOGNUMBER", field::CATALOG_NUMBER),
    ("BARCODE", field::BARCODE),
    ("LABEL", field::LABEL),
    ("MEDIA", field::MEDIA),
    ("ISRC", field::ISRC),
    ("ORIGINALDATE", field::ORIGINAL_DATE),
    ("ORIGINALYEAR", field::ORIGINAL_YEAR),
    ("SCRIPT", field::SCRIPT),
    ("ARTISTS", field::ARTISTS),
    ("WORK", field::WORK),
    ("LYRICIST", field::LYRICIST),
    ("CONDUCTOR", field::CONDUCTOR),
    ("REMIXER", field::REMIXER),
    ("ENGINEER", field::ENGINEER),
    ("PRODUCER", field::PRODUCER),
    ("MIXER", field::MIXER),
    ("ARRANGER", field::ARRANGER),
    ("PERFORMER", field::PERFORMER),
    ("WRITER", field::WRITER),
    ("DISCSUBTITLE", field::DISC_SUBTITLE),
    ("STYLE", field::STYLE),
    ("DISCID", field::DISC_ID),
];

/// Atom types are four ISO-8859-1 bytes ("©nam").
fn type_bytes(kind: &str) -> Vec<u8> {
    kind.chars().map(|c| c as u32 as u8).collect()
}

fn atoms(source: &dyn DataSource, start: u64, end: u64) -> Result<Vec<Atom>, TagFileError> {
    let mut list = vec![];
    let mut cursor = start;
    while cursor + 8 <= end {
        let h = source.read_at(cursor, 8)?;
        let mut size = u32_be(&h, 0) as u64;
        let kind = text::latin1(&h[4..8]);
        let mut header_size = 8;
        if size == 1 {
            if cursor + 16 > end {
                break;
            }
            size = u64_be(&source.read_at(cursor + 8, 8)?, 0);
            header_size = 16;
        } else if size == 0 {
            size = end - cursor;
        }
        if size < header_size || cursor + size > end {
            return Err(TagFileError::Malformed(format!("atom {kind} size")));
        }
        list.push(Atom { kind, offset: cursor, size, header_size });
        cursor += size;
    }
    Ok(list)
}

fn children(atom: &Atom, source: &dyn DataSource, skip: u64) -> Result<Vec<Atom>, TagFileError> {
    atoms(source, atom.payload_offset() + skip, atom.offset + atom.size)
}

fn find<'a>(kind: &str, list: &'a [Atom]) -> Option<&'a Atom> {
    list.iter().find(|a| a.kind == kind)
}

struct Located {
    top: Vec<Atom>,
    moov: Atom,
    udta: Option<Atom>,
    meta: Option<Atom>,
    ilst: Option<Atom>,
}

fn locate(source: &dyn DataSource) -> Result<Located, TagFileError> {
    let top = atoms(source, 0, source.len()?)?;
    let moov = find("moov", &top).cloned().ok_or_else(|| TagFileError::Malformed("no moov atom".into()))?;
    let udta = find("udta", &children(&moov, source, 0)?).cloned();
    let meta = match &udta {
        Some(u) => find("meta", &children(u, source, 0)?).cloned(),
        None => None,
    };
    let ilst = match &meta {
        Some(m) => find("ilst", &children(m, source, 4)?).cloned(),
        None => None,
    };
    Ok(Located { top, moov, udta, meta, ilst })
}

fn read_all(source: &dyn DataSource, a: &Atom) -> Result<Vec<u8>, TagFileError> {
    Ok(source.read_at(a.offset, a.size as usize)?)
}

fn data_text(d: &Atom, source: &dyn DataSource) -> Result<Option<String>, TagFileError> {
    if d.payload_size() < 8 {
        return Ok(None);
    }
    let head = source.read_at(d.payload_offset(), 8)?;
    let kind = u32_be(&head, 0) & 0xFF_FFFF;
    let body = source.read_at(d.payload_offset() + 8, (d.payload_size() - 8) as usize)?;
    Ok(match kind {
        1 => Some(text::utf8_lossy(&body)),
        2 => text::utf16_be(&body),
        0 | 21 | 22 if body.len() <= 8 => {
            Some(body.iter().fold(0i64, |acc, &b| acc.wrapping_shl(8) | b as i64).to_string())
        }
        _ => None,
    })
}

fn named_string(kids: &[Atom], kind: &str, source: &dyn DataSource) -> Result<String, TagFileError> {
    Ok(match kids.iter().find(|a| a.kind == kind) {
        Some(a) => text::utf8_lossy(&source.read_at(a.payload_offset() + 4, (a.payload_size() - 4) as usize)?),
        None => String::new(),
    })
}

pub fn read(source: &dyn DataSource, with_digest: bool) -> Result<ContainerContents, TagFileError> {
    let loc = locate(source)?;
    let mut tags = TagSet::new();
    let mut pictures: Vec<Picture> = vec![];
    let mut opaque = vec![];
    // The whole moov is the backup: a rewrite may have moved it, and a
    // restore puts it back where it was.
    let raw_metadata = read_all(source, &loc.moov)?;
    if let Some(ilst) = &loc.ilst {
        for item in children(ilst, source, 0)? {
            let kids = children(&item, source, 0)?;
            let datas: Vec<&Atom> = kids.iter().filter(|a| a.kind == "data").collect();
            match item.kind.as_str() {
                "----" => {
                    let name = named_string(&kids, "name", source)?;
                    let mean = named_string(&kids, "mean", source)?;
                    if mean != "com.apple.iTunes" {
                        opaque.extend(read_all(source, &item)?);
                        continue;
                    }
                    let lower = name.to_lowercase();
                    let f = FREEFORM
                        .iter()
                        .find(|(n, _)| n.to_lowercase() == lower)
                        .map_or_else(|| name.to_uppercase(), |(_, f)| f.to_string());
                    for d in datas {
                        if let Some(s) = data_text(d, source)? {
                            for v in s.split('\0').filter(|v| !v.is_empty()) {
                                tags.add(&f, Some(v));
                            }
                        }
                    }
                }
                "trkn" | "disk" => {
                    let Some(d) = datas.first().filter(|d| d.payload_size() >= 14) else {
                        opaque.extend(read_all(source, &item)?);
                        continue;
                    };
                    let p = source.read_at(d.payload_offset() + 8, (d.payload_size() - 8) as usize)?;
                    let (n, total) = (u16_be(&p, 2), u16_be(&p, 4));
                    let (number, t1, t2) = if item.kind == "trkn" {
                        (field::TRACK_NUMBER, field::TRACK_TOTAL, field::TOTAL_TRACKS)
                    } else {
                        (field::DISC_NUMBER, field::DISC_TOTAL, field::TOTAL_DISCS)
                    };
                    if n > 0 {
                        tags.set(number, Some(&n.to_string()));
                    }
                    if total > 0 {
                        tags.set(t1, Some(&total.to_string()));
                        tags.set(t2, Some(&total.to_string()));
                    }
                }
                "cpil" => {
                    if let Some(d) = datas.first()
                        && d.payload_size() >= 9
                        && source.read_at(d.payload_offset() + 8, 1)?[0] != 0
                    {
                        tags.set(field::COMPILATION, Some("1"));
                    }
                }
                "covr" => {
                    for d in datas {
                        let data = source.read_at(d.payload_offset() + 8, (d.payload_size() - 8) as usize)?;
                        let kind = if pictures.is_empty() { PictureKind::FRONT } else { PictureKind::OTHER };
                        pictures.push(Picture::from_image(kind, data, "image/jpeg"));
                    }
                }
                other => match SIMPLE.iter().find(|(a, _)| *a == other) {
                    Some((_, f)) => {
                        for d in datas {
                            if let Some(s) = data_text(d, source)? {
                                for v in s.split('\0').filter(|v| !v.is_empty()) {
                                    tags.add(f, Some(v));
                                }
                            }
                        }
                    }
                    None => opaque.extend(read_all(source, &item)?),
                },
            }
        }
    }
    let audio_digest = if with_digest {
        let mut h = Sha256::new();
        for a in loc.top.iter().filter(|a| a.kind == "mdat") {
            copy(source, a.payload_offset(), a.payload_size(), None, &mut h)?;
        }
        finish(h)
    } else {
        String::new()
    };
    Ok(ContainerContents { tags, pictures, raw_metadata, audio_digest, preserved: vec![], opaque })
}

pub fn atom(kind: &str, body: &[u8]) -> Vec<u8> {
    let mut d = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    d.extend(type_bytes(kind));
    d.extend_from_slice(body);
    d
}

fn mean_name(mean: &str, name: &str) -> Vec<u8> {
    let mut m = vec![0u8; 4];
    m.extend_from_slice(mean.as_bytes());
    let mut n = vec![0u8; 4];
    n.extend_from_slice(name.as_bytes());
    let mut d = atom("mean", &m);
    d.extend(atom("name", &n));
    d
}

fn text_data(s: &str) -> Vec<u8> {
    let mut d = 1u32.to_be_bytes().to_vec();
    d.extend_from_slice(&0u32.to_be_bytes());
    d.extend_from_slice(s.as_bytes());
    atom("data", &d)
}

/// Swift's `Int(String)`: optional sign, ASCII digits, nothing else.
fn swift_int(s: &str) -> Option<i64> {
    let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

pub fn write(
    source: &dyn DataSource,
    tags: &TagSet,
    pictures: Option<&[Picture]>,
    out: &mut dyn WriteSeek,
) -> Result<String, TagFileError> {
    let contents = read(source, false)?;
    let mut ilst = vec![];
    let mut used: Vec<&str> = vec![];
    let item = |ilst: &mut Vec<u8>, kind: &str, datas: Vec<Vec<u8>>| ilst.extend(atom(kind, &datas.concat()));
    for (kind, f) in SIMPLE {
        if !tags.get(f).is_empty() {
            let sep = if *kind == "\u{A9}gen" { "\0" } else { "; " };
            item(&mut ilst, kind, vec![text_data(&tags.get(f).join(sep))]);
            used.push(f);
        }
    }
    let numbers = |ilst: &mut Vec<u8>, kind: &str, n: Option<&str>, total: Option<&str>| {
        let Some(n) = n.and_then(swift_int) else { return };
        let mut p = vec![0u8; 8];
        p.extend_from_slice(&0u16.to_be_bytes());
        p.extend_from_slice(&(n.clamp(0, u16::MAX as i64) as u16).to_be_bytes());
        let t = total.and_then(swift_int).unwrap_or(0);
        p.extend_from_slice(&(t.clamp(0, u16::MAX as i64) as u16).to_be_bytes());
        p.extend_from_slice(&0u16.to_be_bytes());
        item(ilst, kind, vec![atom("data", &p)]);
    };
    numbers(
        &mut ilst,
        "trkn",
        tags.first(field::TRACK_NUMBER),
        tags.first(field::TRACK_TOTAL).or(tags.first(field::TOTAL_TRACKS)),
    );
    numbers(
        &mut ilst,
        "disk",
        tags.first(field::DISC_NUMBER),
        tags.first(field::DISC_TOTAL).or(tags.first(field::TOTAL_DISCS)),
    );
    used.extend([
        field::TRACK_NUMBER,
        field::TRACK_TOTAL,
        field::TOTAL_TRACKS,
        field::DISC_NUMBER,
        field::DISC_TOTAL,
        field::TOTAL_DISCS,
    ]);
    if tags.first(field::COMPILATION) == Some("1") {
        let mut p = 21u32.to_be_bytes().to_vec();
        p.extend_from_slice(&0u32.to_be_bytes());
        p.push(1);
        item(&mut ilst, "cpil", vec![atom("data", &p)]);
    }
    used.push(field::COMPILATION);
    for (name, f) in FREEFORM {
        if !tags.get(f).is_empty() {
            let mut datas = vec![mean_name("com.apple.iTunes", name)];
            datas.extend(tags.get(f).iter().map(|v| text_data(v)));
            item(&mut ilst, "----", datas);
            used.push(f);
        }
    }
    for name in tags.names() {
        if !used.contains(&name.as_str()) {
            let mut datas = vec![mean_name("com.apple.iTunes", &name)];
            datas.extend(tags.get(&name).iter().map(|v| text_data(v)));
            item(&mut ilst, "----", datas);
        }
    }
    let pics = pictures.unwrap_or(&contents.pictures);
    if !pics.is_empty() {
        let datas = pics
            .iter()
            .map(|p| {
                let mut d = (if p.mime_type == "image/png" { 14u32 } else { 13 }).to_be_bytes().to_vec();
                d.extend_from_slice(&0u32.to_be_bytes());
                d.extend_from_slice(&p.data);
                atom("data", &d)
            })
            .collect();
        item(&mut ilst, "covr", datas);
    }
    ilst.extend_from_slice(&contents.opaque);
    emit(source, &ilst, out)
}

/// `raw_metadata` is the original moov. If a rewrite left a `free` slot of
/// its size in front of the audio, the moov goes back there and the trailing
/// copy is dropped, which makes the file byte-exact again.
pub fn restore(source: &dyn DataSource, raw_metadata: &[u8], out: &mut dyn WriteSeek) -> Result<String, TagFileError> {
    let loc = locate(source)?;
    if raw_metadata.len() < 8 || &raw_metadata[4..8] != b"moov" {
        return Err(TagFileError::Malformed("backup is not a moov atom".into()));
    }
    let slot = loc
        .top
        .iter()
        .find(|a| a.kind == "free" && a.size == raw_metadata.len() as u64 && a.offset < loc.moov.offset)
        .map(|a| a.offset);
    let mut hasher = Sha256::new();
    for a in &loc.top {
        if Some(a.offset) == slot {
            out.write_all(raw_metadata)?;
            continue;
        }
        if a.kind == "moov" {
            if slot.is_none() {
                out.write_all(raw_metadata)?;
            }
            continue;
        }
        copy_atom(source, a, out, &mut hasher)?;
    }
    Ok(finish(hasher))
}

fn copy_atom(
    source: &dyn DataSource,
    a: &Atom,
    out: &mut dyn WriteSeek,
    hasher: &mut Sha256,
) -> Result<(), TagFileError> {
    out.write_all(&source.read_at(a.offset, a.header_size as usize)?)?;
    if a.kind == "mdat" {
        copy(source, a.payload_offset(), a.payload_size(), Some(out), hasher)
    } else {
        copy(source, a.payload_offset(), a.payload_size(), Some(out), &mut Sha256::new())
    }
}

fn emit(source: &dyn DataSource, ilst_payload: &[u8], out: &mut dyn WriteSeek) -> Result<String, TagFileError> {
    let loc = locate(source)?;
    // New meta: version/flags + hdlr (existing or a fresh mdir one) + other
    // children + ilst.
    let mut meta_body = vec![];
    let mut other_meta = vec![];
    let mut has_hdlr = false;
    if let Some(meta) = &loc.meta {
        meta_body.extend(source.read_at(meta.payload_offset(), 4)?);
        for c in children(meta, source, 4)?.iter().filter(|c| c.kind != "ilst") {
            has_hdlr |= c.kind == "hdlr";
            other_meta.extend(read_all(source, c)?);
        }
    } else {
        meta_body.extend_from_slice(&[0u8; 4]);
    }
    if !has_hdlr {
        let mut h = vec![0u8; 8];
        h.extend_from_slice(b"mdirappl");
        h.extend_from_slice(&[0u8; 9]);
        other_meta.extend(atom("hdlr", &h));
    }
    meta_body.extend(other_meta);
    meta_body.extend(atom("ilst", ilst_payload));
    let new_meta = atom("meta", &meta_body);
    let mut udta_body = vec![];
    if let Some(udta) = &loc.udta {
        for c in children(udta, source, 0)?.iter().filter(|c| c.kind != "meta") {
            udta_body.extend(read_all(source, c)?);
        }
    }
    udta_body.extend(new_meta);
    let mut moov_body = vec![];
    for c in children(&loc.moov, source, 0)?.iter().filter(|c| c.kind != "udta") {
        moov_body.extend(read_all(source, c)?);
    }
    moov_body.extend(atom("udta", &udta_body));
    let new_moov = atom("moov", &moov_body);

    let mdat_offset = loc.top.iter().find(|a| a.kind == "mdat").map_or(u64::MAX, |a| a.offset);
    let moov_before_audio = loc.moov.offset < mdat_offset;
    let mut hasher = Sha256::new();
    for a in &loc.top {
        if a.kind == "moov" {
            if moov_before_audio {
                let mut free = (a.size.min(u32::MAX as u64) as u32).to_be_bytes().to_vec();
                free.extend_from_slice(b"free");
                out.write_all(&free)?;
                out.write_all(&vec![0u8; (a.size - 8) as usize])?;
            } else {
                out.write_all(&new_moov)?;
            }
            continue;
        }
        copy_atom(source, a, out, &mut hasher)?;
    }
    if moov_before_audio {
        out.write_all(&new_moov)?;
    }
    Ok(finish(hasher))
}
