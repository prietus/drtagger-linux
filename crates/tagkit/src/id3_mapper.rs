//! Picard's ID3 conventions: standard text frames where they exist, TXXX
//! with Picard's exact descriptions for MusicBrainz identifiers, UFID for
//! the recording id, IPLS for people, APIC for pictures. Written as v2.3;
//! v2.4 reads fine.

use crate::picture::{Picture, PictureKind};
use crate::tag_set::{TagSet, field};
use flackit::{ID3v2Bridge, ID3v2Frame, ID3v2Tag, text};

const TEXT_FRAMES: &[(&str, &str)] = &[
    ("TIT2", field::TITLE),
    ("TPE1", field::ARTIST),
    ("TPE2", field::ALBUM_ARTIST),
    ("TALB", field::ALBUM),
    ("TSOP", field::ARTIST_SORT),
    ("TSOA", field::ALBUM_SORT),
    ("TSO2", field::ALBUM_ARTIST_SORT),
    ("TSOC", field::COMPOSER_SORT),
    ("TCOM", field::COMPOSER),
    ("TEXT", field::LYRICIST),
    ("TPE3", field::CONDUCTOR),
    ("TPE4", field::REMIXER),
    ("TPUB", field::LABEL),
    ("TMED", field::MEDIA),
    ("TSRC", field::ISRC),
    ("TCON", field::GENRE),
    ("TSST", field::DISC_SUBTITLE),
    ("TCMP", field::COMPILATION),
    ("TIT1", "GROUPING"),
    ("TIT3", "SUBTITLE"),
    ("TBPM", "BPM"),
    ("TLAN", "LANGUAGE"),
    ("TCOP", "COPYRIGHT"),
    ("TENC", "ENCODEDBY"),
    ("TMOO", "MOOD"),
    ("TOPE", "ORIGINALARTIST"),
    ("TOAL", "ORIGINALALBUM"),
    ("TSSE", "ENCODERSETTINGS"),
];

const TXXX: &[(&str, &str)] = &[
    ("MusicBrainz Album Id", field::MB_ALBUM_ID),
    ("MusicBrainz Artist Id", field::MB_ARTIST_ID),
    ("MusicBrainz Album Artist Id", field::MB_ALBUM_ARTIST_ID),
    ("MusicBrainz Release Group Id", field::MB_RELEASE_GROUP_ID),
    ("MusicBrainz Release Track Id", field::MB_RELEASE_TRACK_ID),
    ("MusicBrainz Work Id", field::MB_WORK_ID),
    ("MusicBrainz Disc Id", field::MB_DISC_ID),
    ("MusicBrainz Album Type", field::RELEASE_TYPE),
    ("MusicBrainz Album Status", field::RELEASE_STATUS),
    ("MusicBrainz Album Release Country", field::RELEASE_COUNTRY),
    ("Acoustid Id", field::ACOUSTID),
    ("Acoustid Fingerprint", field::ACOUSTID_FINGERPRINT),
    ("CATALOGNUMBER", field::CATALOG_NUMBER),
    ("BARCODE", field::BARCODE),
    ("SCRIPT", field::SCRIPT),
    ("ARTISTS", field::ARTISTS),
    ("WORK", field::WORK),
    ("Writer", field::WRITER),
    ("originalyear", field::ORIGINAL_YEAR),
    ("DISCID", field::DISC_ID),
    ("STYLE", field::STYLE),
];

/// IPLS role → field.
const PEOPLE_FIELDS: &[(&str, &str)] = &[
    ("producer", field::PRODUCER),
    ("engineer", field::ENGINEER),
    ("mixer", field::MIXER),
    ("mix", field::MIXER),
    ("arranger", field::ARRANGER),
    ("DJ-mix", field::REMIXER),
];

pub const UFID_OWNER: &str = "http://musicbrainz.org";

#[derive(Clone, Debug, Default)]
pub struct Parsed {
    pub tags: TagSet,
    pub pictures: Vec<Picture>,
    /// Frames the schema cannot express, kept raw.
    pub preserved: Vec<ID3v2Frame>,
}

/// ID3 puts a BOM in front of every UTF-16 string; drop it after decoding.
fn clean(s: String) -> String {
    match s.strip_prefix('\u{FEFF}') {
        Some(rest) => rest.to_string(),
        None => s,
    }
}

fn texts(payload: &[u8]) -> Vec<String> {
    ID3v2Bridge::decode_text_frame(payload).into_iter().map(clean).filter(|s| !s.is_empty()).collect()
}

/// TXXX: encoding, description, then one or more NUL-separated values.
fn decode_txxx(payload: &[u8]) -> Option<(String, Vec<String>)> {
    let (&encoding, body) = payload.split_first()?;
    let parts: Vec<String> = ID3v2Bridge::split_null_terminated(body, encoding).into_iter().map(clean).collect();
    if parts.len() < 2 {
        return None;
    }
    Some((parts[0].clone(), parts[1..].iter().filter(|s| !s.is_empty()).cloned().collect()))
}

/// Swift's `split(separator: "/", maxSplits: 1)`: empty pieces dropped.
fn split_slash(v: &str) -> Vec<String> {
    v.splitn(2, '/').filter(|s| !s.is_empty()).map(str::to_string).collect()
}

pub fn parse(tag: &ID3v2Tag) -> Parsed {
    let mut tags = TagSet::new();
    let mut pictures = vec![];
    let mut preserved = vec![];
    let (mut track_total, mut disc_total): (Option<String>, Option<String>) = (None, None);
    for frame in &tag.frames {
        match frame.id.as_str() {
            "APIC" => match parse_apic(&frame.payload) {
                Some(p) => pictures.push(p),
                None => preserved.push(frame.clone()),
            },
            "TXXX" => {
                let Some((desc, values)) = decode_txxx(&frame.payload) else {
                    preserved.push(frame.clone());
                    continue;
                };
                let lower = desc.to_lowercase();
                let field = TXXX
                    .iter()
                    .find(|(d, _)| d.to_lowercase() == lower)
                    .map_or_else(|| desc.to_uppercase(), |(_, f)| f.to_string());
                for v in values {
                    tags.add(&field, Some(&v));
                }
            }
            "COMM" => {
                if let Some(c) = ID3v2Bridge::decode_comm(&frame.payload) {
                    tags.add(field::COMMENT, Some(&clean(c)));
                }
            }
            "UFID" => match parse_ufid(&frame.payload) {
                Some((owner, id)) if owner == UFID_OWNER => tags.set(field::MB_TRACK_ID, Some(&id)),
                _ => preserved.push(frame.clone()),
            },
            "TRCK" | "TPOS" => {
                let v = texts(&frame.payload).into_iter().next().unwrap_or_default();
                let parts = split_slash(&v);
                let (number, total) = if frame.id == "TRCK" {
                    (field::TRACK_NUMBER, &mut track_total)
                } else {
                    (field::DISC_NUMBER, &mut disc_total)
                };
                tags.set(number, parts.first().map(String::as_str));
                if parts.len() > 1 {
                    *total = Some(parts[1].clone());
                }
            }
            "TYER" | "TDRC" => {
                if let Some(v) = texts(&frame.payload).first()
                    && (tags.get(field::DATE).is_empty() || frame.id == "TDRC")
                {
                    tags.set(field::DATE, Some(v));
                }
            }
            "TDAT" => {
                if let Some(v) = texts(&frame.payload).first()
                    && v.chars().count() == 4
                    && let Some(year) = tags.first(field::DATE).filter(|y| y.chars().count() == 4).map(str::to_string)
                {
                    let c: Vec<char> = v.chars().collect();
                    let date = format!("{year}-{}{}-{}{}", c[2], c[3], c[0], c[1]);
                    tags.set(field::DATE, Some(&date));
                }
            }
            "TORY" | "TDOR" => {
                if let Some(v) = texts(&frame.payload).first() {
                    tags.set(field::ORIGINAL_DATE, Some(v));
                }
            }
            "IPLS" | "TIPL" | "TMCL" => {
                let items: Vec<String> =
                    ID3v2Bridge::decode_text_frame(&frame.payload).into_iter().map(clean).collect();
                for [role, name] in items.as_chunks::<2>().0 {
                    if let Some((_, f)) = PEOPLE_FIELDS.iter().find(|(r, _)| r == role) {
                        tags.add(f, Some(name));
                    } else if role.is_empty() || role.to_lowercase() == "performer" {
                        tags.add(field::PERFORMER, Some(name));
                    } else {
                        tags.add(field::PERFORMER, Some(&format!("{name} ({role})")));
                    }
                }
            }
            id => match TEXT_FRAMES.iter().find(|(f, _)| *f == id) {
                Some((_, f)) => {
                    for v in texts(&frame.payload) {
                        tags.add(f, Some(&v));
                    }
                }
                None => preserved.push(frame.clone()),
            },
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
    Parsed { tags, pictures, preserved }
}

/// The ID3v2.3 tag for `tags`, then pictures, then preserved frames.
pub fn tag(tags: &TagSet, pictures: &[Picture], preserved: &[ID3v2Frame]) -> ID3v2Tag {
    let mut frames = vec![];
    let mut used: Vec<String> = vec![];
    let text = |frames: &mut Vec<ID3v2Frame>, id: &str, values: &[String]| {
        if !values.is_empty() {
            frames.push(ID3v2Bridge::make_text_frame(id, &values.join("\0")));
        }
    };
    // Multi-valued text frames are NUL-separated (the v2.4 rule; Picard and
    // our reader split them, older players show the first).
    for (frame, f) in TEXT_FRAMES {
        if !tags.get(f).is_empty() {
            text(&mut frames, frame, tags.get(f));
            used.push(f.to_string());
        }
    }
    if let Some(n) = tags.first(field::TRACK_NUMBER) {
        let total = tags.first(field::TRACK_TOTAL).or(tags.first(field::TOTAL_TRACKS));
        text(&mut frames, "TRCK", &[total.map_or(n.to_string(), |t| format!("{n}/{t}"))]);
    }
    if let Some(n) = tags.first(field::DISC_NUMBER) {
        let total = tags.first(field::DISC_TOTAL).or(tags.first(field::TOTAL_DISCS));
        text(&mut frames, "TPOS", &[total.map_or(n.to_string(), |t| format!("{n}/{t}"))]);
    }
    for f in [
        field::TRACK_NUMBER,
        field::TRACK_TOTAL,
        field::TOTAL_TRACKS,
        field::DISC_NUMBER,
        field::DISC_TOTAL,
        field::TOTAL_DISCS,
    ] {
        used.push(f.to_string());
    }
    if let Some(date) = tags.first(field::DATE) {
        let c: Vec<char> = date.chars().collect();
        text(&mut frames, "TYER", &[c.iter().take(4).collect::<String>()]);
        if c.len() >= 10 {
            let tdat: String = c[8..10].iter().chain(c[5..7].iter()).collect();
            text(&mut frames, "TDAT", &[tdat]);
        }
        used.push(field::DATE.to_string());
    }
    if let Some(original) = tags.first(field::ORIGINAL_DATE) {
        text(&mut frames, "TORY", &[original.chars().take(4).collect::<String>()]);
        used.push(field::ORIGINAL_DATE.to_string());
    }
    for c in tags.get(field::COMMENT) {
        let mut payload = vec![0x01];
        payload.extend_from_slice(b"eng");
        payload.extend(ID3v2Bridge::encode_utf16_with_bom(""));
        payload.extend_from_slice(&[0, 0]);
        payload.extend(ID3v2Bridge::encode_utf16_with_bom(c));
        frames.push(ID3v2Frame::new("COMM", payload));
    }
    used.push(field::COMMENT.to_string());
    if let Some(rec) = tags.first(field::MB_TRACK_ID) {
        let mut payload = UFID_OWNER.as_bytes().to_vec();
        payload.push(0);
        payload.extend_from_slice(rec.as_bytes());
        frames.push(ID3v2Frame::new("UFID", payload));
        used.push(field::MB_TRACK_ID.to_string());
    }
    // People: producer/engineer/mixer/arranger and performers "Name (role)".
    let mut people: Vec<String> = vec![];
    for (role, f) in [
        ("producer", field::PRODUCER),
        ("engineer", field::ENGINEER),
        ("mixer", field::MIXER),
        ("arranger", field::ARRANGER),
    ] {
        for name in tags.get(f) {
            people.push(role.to_string());
            people.push(name.clone());
        }
        used.push(f.to_string());
    }
    for p in tags.get(field::PERFORMER) {
        match p.rfind('(') {
            Some(open) if p.ends_with(')') && open > 0 => {
                people.push(p[open + 1..p.len() - 1].to_string());
                people.push(p[..open].trim_matches(|c: char| c == ' ' || c == '\t').to_string());
            }
            _ => {
                people.push("performer".to_string());
                people.push(p.clone());
            }
        }
    }
    used.push(field::PERFORMER.to_string());
    if !people.is_empty() {
        let mut payload = vec![0x01];
        for (i, s) in people.iter().enumerate() {
            payload.extend(ID3v2Bridge::encode_utf16_with_bom(s));
            if i < people.len() - 1 {
                payload.extend_from_slice(&[0, 0]);
            }
        }
        frames.push(ID3v2Frame::new("IPLS", payload));
    }
    for (description, f) in TXXX {
        if !tags.get(f).is_empty() {
            frames.push(ID3v2Bridge::make_txxx_frame(description, &tags.get(f).join("\0")));
            used.push(f.to_string());
        }
    }
    for name in tags.names() {
        if !used.contains(&name) {
            frames.push(ID3v2Bridge::make_txxx_frame(&name, &tags.get(&name).join("\0")));
        }
    }
    frames.extend(pictures.iter().map(apic));
    frames.extend(preserved.iter().cloned());
    ID3v2Tag { frames }
}

pub fn apic(p: &Picture) -> ID3v2Frame {
    let mut payload = vec![0x00]; // ISO-8859-1 for the mime/description
    payload.extend_from_slice(p.mime_type.as_bytes());
    payload.push(0);
    payload.push(p.kind.0);
    payload.extend(p.description.as_bytes().iter().take(60));
    payload.push(0);
    payload.extend_from_slice(&p.data);
    ID3v2Frame::new("APIC", payload)
}

pub fn parse_apic(payload: &[u8]) -> Option<Picture> {
    if payload.len() <= 4 {
        return None;
    }
    let b = payload;
    let encoding = b[0];
    let mime_end = 1 + b[1..].iter().position(|&x| x == 0)?;
    let mime_type = text::utf8_lossy(&b[1..mime_end]);
    let mut cursor = mime_end + 1;
    if cursor >= b.len() {
        return None;
    }
    let kind = PictureKind::from_raw(b[cursor] as u32);
    cursor += 1;
    let mut desc_end = cursor;
    let description = if encoding == 1 || encoding == 2 {
        while desc_end + 1 < b.len() && !(b[desc_end] == 0 && b[desc_end + 1] == 0) {
            desc_end += 2;
        }
        let d = ID3v2Bridge::decode_text(&b[cursor.min(b.len())..desc_end.min(b.len())], encoding);
        cursor = (desc_end + 2).min(b.len());
        d
    } else {
        while desc_end < b.len() && b[desc_end] != 0 {
            desc_end += 1;
        }
        let d = text::utf8_lossy(&b[cursor..desc_end]);
        cursor = (desc_end + 1).min(b.len());
        d
    };
    let data = b[cursor..].to_vec();
    let (width, height) = crate::picture::dimensions(&data).unwrap_or((0, 0));
    Some(Picture { kind, mime_type, data, width, height, description })
}

fn parse_ufid(payload: &[u8]) -> Option<(String, String)> {
    let nul = payload.iter().position(|&b| b == 0)?;
    Some((text::utf8_lossy(&payload[..nul]), text::utf8_lossy(&payload[nul + 1..])))
}
