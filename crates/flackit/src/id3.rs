//! Minimal ID3v2: reads v2.3 and v2.4 (all four text encodings), writes
//! v2.3 with UTF-16LE-with-BOM text frames — the dialect every player and
//! DAC front-end reads. The tag size is syncsafe in the header and plain
//! big-endian in each v2.3 frame header.

use crate::VorbisComment;
use crate::text;
use std::fmt;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ID3v2Tag {
    pub frames: Vec<ID3v2Frame>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ID3v2Frame {
    /// "TIT2", "TXXX", "COMM", "APIC", …
    pub id: String,
    pub flags: u16,
    /// Raw frame body, encoding byte included for text frames.
    pub payload: Vec<u8>,
}

impl ID3v2Frame {
    pub fn new(id: &str, payload: Vec<u8>) -> Self {
        ID3v2Frame { id: id.to_string(), flags: 0, payload }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ID3v2Error {
    NotAnId3Tag,
    UnsupportedVersion { major: u8, minor: u8 },
    Truncated,
    InvalidFrame { id: String, reason: String },
}

impl fmt::Display for ID3v2Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ID3v2Error::NotAnId3Tag => write!(f, "not an ID3v2 tag"),
            ID3v2Error::UnsupportedVersion { major, minor } => write!(f, "unsupported ID3v2.{major}.{minor}"),
            ID3v2Error::Truncated => write!(f, "ID3v2 tag is truncated"),
            ID3v2Error::InvalidFrame { id, reason } => write!(f, "invalid ID3 frame {id}: {reason}"),
        }
    }
}

impl std::error::Error for ID3v2Error {}

fn syncsafe(b: &[u8]) -> usize {
    (b[0] as usize) << 21 | (b[1] as usize) << 14 | (b[2] as usize) << 7 | b[3] as usize
}

fn encode_syncsafe(v: usize) -> [u8; 4] {
    [(v >> 21) as u8 & 0x7F, (v >> 14) as u8 & 0x7F, (v >> 7) as u8 & 0x7F, v as u8 & 0x7F]
}

fn be32(b: &[u8]) -> usize {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize
}

impl ID3v2Tag {
    /// Parses a tag at the start of `data`; returns it with the total size
    /// (header + frames + padding).
    pub fn parse(data: &[u8]) -> Result<(ID3v2Tag, usize), ID3v2Error> {
        if data.len() < 10 {
            return Err(ID3v2Error::Truncated);
        }
        if &data[..3] != b"ID3" {
            return Err(ID3v2Error::NotAnId3Tag);
        }
        let (major, minor) = (data[3], data[4]);
        if major != 3 && major != 4 {
            return Err(ID3v2Error::UnsupportedVersion { major, minor });
        }
        let flags = data[5];
        let total = 10 + syncsafe(&data[6..10]);
        if data.len() < total {
            return Err(ID3v2Error::Truncated);
        }
        let mut cursor = 10;
        // Extended header: skipped.
        if flags & 0x40 != 0 {
            if cursor + 4 > total {
                return Err(ID3v2Error::Truncated);
            }
            let ext = if major == 4 { syncsafe(&data[cursor..cursor + 4]) } else { be32(&data[cursor..cursor + 4]) };
            cursor += ext;
        }
        let mut frames = vec![];
        while cursor + 10 <= total {
            if data[cursor] == 0 {
                break; // padding
            }
            let id = text::ascii(&data[cursor..cursor + 4])
                .ok_or_else(|| ID3v2Error::InvalidFrame { id: "?".into(), reason: "non-ASCII frame id".into() })?;
            let size =
                if major == 4 { syncsafe(&data[cursor + 4..cursor + 8]) } else { be32(&data[cursor + 4..cursor + 8]) };
            let flags = u16::from_be_bytes([data[cursor + 8], data[cursor + 9]]);
            cursor += 10;
            if cursor + size > total {
                break;
            }
            frames.push(ID3v2Frame { id, flags, payload: data[cursor..cursor + size].to_vec() });
            cursor += size;
        }
        Ok((ID3v2Tag { frames }, total))
    }

    /// The tag as an ID3v2.3 block: 10-byte header with a syncsafe size,
    /// then the frames, no padding.
    pub fn encoded_v23(&self) -> Vec<u8> {
        let mut body = vec![];
        for frame in &self.frames {
            // Frames whose IDs are not legal v2.3 (v2.2 three-letter IDs…).
            if frame.id.chars().count() != 4 || !frame.id.chars().all(|c| c.is_alphabetic() || c.is_numeric()) {
                continue;
            }
            let id = text::ascii(frame.id.as_bytes()).map_or(vec![0u8; 4], String::into_bytes);
            body.extend_from_slice(&id);
            body.extend_from_slice(&(frame.payload.len() as u32).to_be_bytes());
            body.extend_from_slice(&frame.flags.to_be_bytes());
            body.extend_from_slice(&frame.payload);
        }
        let mut out = b"ID3\x03\x00\x00".to_vec();
        out.extend_from_slice(&encode_syncsafe(body.len()));
        out.extend_from_slice(&body);
        out
    }
}

/// Frame-body codecs and the simple Vorbis ⇄ ID3 bridge FLACKit uses for
/// provisional tags (the full Picard mapping lives in tagkit).
pub struct ID3v2Bridge;

const FRAME_TO_VORBIS: &[(&str, &str)] = &[
    ("TALB", "ALBUM"),
    ("TIT2", "TITLE"),
    ("TPE1", "ARTIST"),
    ("TPE2", "ALBUMARTIST"),
    ("TRCK", "TRACKNUMBER"),
    ("TPOS", "DISCNUMBER"),
    ("TYER", "DATE"),
    ("TDRC", "DATE"),
    ("TCON", "GENRE"),
    ("TCOM", "COMPOSER"),
    ("TPUB", "LABEL"),
    ("TMED", "MEDIA"),
    ("TSRC", "ISRC"),
];

/// Names that always go through TXXX in the bridge.
const ALWAYS_TXXX: &[&str] = &["CATALOGNUMBER", "BARCODE", "RELEASECOUNTRY", "STYLE", "PERFORMER", "PRODUCER", "LABEL"];

const PRESERVED: &[&str] = &["APIC", "MCDI", "PRIV", "GEOB"];

impl ID3v2Bridge {
    fn vorbis_name(frame: &str) -> Option<&'static str> {
        FRAME_TO_VORBIS.iter().find(|(f, _)| *f == frame).map(|(_, v)| *v)
    }

    fn frame_for(vorbis: &str) -> Option<&'static str> {
        // DATE goes back as TYER (v2.3).
        FRAME_TO_VORBIS.iter().find(|(f, v)| *v == vorbis && *f != "TDRC").map(|(f, _)| *f)
    }

    /// ID3 frames as Vorbis-style fields; repeated or NUL-separated values
    /// become repeated fields.
    pub fn to_vorbis(tag: &ID3v2Tag, vendor: &str) -> VorbisComment {
        let mut fields = vec![];
        for frame in &tag.frames {
            match frame.id.as_str() {
                "APIC" => {}
                "TXXX" => {
                    if let Some((d, v)) = Self::decode_txxx(&frame.payload) {
                        fields.push((d.to_uppercase(), v));
                    }
                }
                "COMM" => {
                    if let Some(c) = Self::decode_comm(&frame.payload) {
                        fields.push(("COMMENT".to_string(), c));
                    }
                }
                id if id.starts_with('T') => {
                    if let Some(name) = Self::vorbis_name(id) {
                        for v in Self::decode_text_frame(&frame.payload) {
                            fields.push((name.to_string(), v));
                        }
                    }
                }
                _ => {}
            }
        }
        VorbisComment::new(vendor, fields)
    }

    /// A v2.3 tag from Vorbis-style fields; unknown names become TXXX, and
    /// opaque frames from an original tag (pictures, PRIV…) follow.
    pub fn to_id3v23(comment: &VorbisComment, preserved: &[ID3v2Frame]) -> ID3v2Tag {
        let mut frames = vec![];
        for (name, value) in &comment.fields {
            if value.is_empty() {
                continue;
            }
            let upper = name.to_uppercase();
            if ALWAYS_TXXX.contains(&upper.as_str()) {
                frames.push(Self::make_txxx_frame(&upper, value));
            } else if let Some(id) = Self::frame_for(&upper) {
                frames.push(Self::make_text_frame(id, value));
            } else {
                frames.push(Self::make_txxx_frame(&upper, value));
            }
        }
        frames.extend(preserved.iter().filter(|f| PRESERVED.contains(&f.id.as_str())).cloned());
        ID3v2Tag { frames }
    }

    /// TXXX: encoding, description, value.
    pub fn decode_txxx(payload: &[u8]) -> Option<(String, String)> {
        let (&encoding, body) = payload.split_first()?;
        let mut parts = Self::split_null_terminated(body, encoding).into_iter();
        Some((parts.next()?, parts.next()?))
    }

    /// COMM: encoding, 3-byte language, description, text → the text.
    pub fn decode_comm(payload: &[u8]) -> Option<String> {
        if payload.len() < 4 {
            return None;
        }
        Self::split_null_terminated(&payload[4..], payload[0]).pop()
    }

    /// A text frame's values: v2.4 separates them with NUL, and some v2.3
    /// taggers do too, so the text is always split (empty pieces dropped).
    pub fn decode_text_frame(payload: &[u8]) -> Vec<String> {
        let Some((&encoding, body)) = payload.split_first() else { return vec![] };
        Self::decode_text(body, encoding).split('\0').filter(|s| !s.is_empty()).map(str::to_string).collect()
    }

    pub fn decode_text(data: &[u8], encoding: u8) -> String {
        match encoding {
            0 => text::latin1(data),
            1 => text::utf16(data).unwrap_or_default(),
            2 => text::utf16_be(data).unwrap_or_default(),
            3 => text::utf8(data).unwrap_or_default(),
            _ => String::new(),
        }
    }

    /// Splits a body (encoding byte already consumed) on the encoding's NUL
    /// terminator. Every terminator yields a piece, even an empty one; the
    /// text after the last one counts only if not empty.
    pub fn split_null_terminated(data: &[u8], encoding: u8) -> Vec<String> {
        let mut pieces: Vec<Vec<u8>> = vec![];
        let mut current = vec![];
        if encoding == 1 || encoding == 2 {
            let mut i = 0;
            while i + 1 < data.len() {
                if data[i] == 0 && data[i + 1] == 0 {
                    pieces.push(std::mem::take(&mut current));
                } else {
                    current.extend_from_slice(&data[i..i + 2]);
                }
                i += 2;
            }
            if !data.is_empty() && i == data.len() - 1 {
                current.push(data[i]);
            }
        } else {
            for &b in data {
                if b == 0 {
                    pieces.push(std::mem::take(&mut current));
                } else {
                    current.push(b);
                }
            }
        }
        if !current.is_empty() {
            pieces.push(current);
        }
        pieces.iter().map(|p| Self::decode_text(p, encoding)).collect()
    }

    pub fn make_text_frame(id: &str, value: &str) -> ID3v2Frame {
        let mut payload = vec![0x01];
        payload.extend(Self::encode_utf16_with_bom(value));
        ID3v2Frame::new(id, payload)
    }

    pub fn make_txxx_frame(description: &str, value: &str) -> ID3v2Frame {
        let mut payload = vec![0x01];
        payload.extend(Self::encode_utf16_with_bom(description));
        payload.extend_from_slice(&[0, 0]);
        payload.extend(Self::encode_utf16_with_bom(value));
        ID3v2Frame::new("TXXX", payload)
    }

    /// UTF-16LE with a BOM.
    pub fn encode_utf16_with_bom(s: &str) -> Vec<u8> {
        let mut out = vec![0xFF, 0xFE];
        for unit in s.encode_utf16() {
            out.extend_from_slice(&unit.to_le_bytes());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v23_round_trip_and_bridge() {
        let comment = VorbisComment::new(
            "x",
            vec![
                ("TITLE".into(), "Neuköln".into()),
                ("ARTIST".into(), "David Bowie".into()),
                ("DATE".into(), "1977".into()),
                ("CATALOGNUMBER".into(), "PD83889".into()),
                ("EMPTY".into(), "".into()),
            ],
        );
        let pic = ID3v2Frame::new("APIC", vec![0, b'i', 0, 3, 0, 1, 2, 3]);
        let tag = ID3v2Bridge::to_id3v23(&comment, &[pic.clone(), ID3v2Frame::new("TXXX", vec![0])]);
        assert_eq!(
            tag.frames.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
            ["TIT2", "TPE1", "TYER", "TXXX", "APIC"]
        );
        let bytes = tag.encoded_v23();
        let (back, size) = ID3v2Tag::parse(&bytes).unwrap();
        assert_eq!(size, bytes.len());
        assert_eq!(back, tag);
        let v = ID3v2Bridge::to_vorbis(&back, "v");
        assert_eq!(v.get("TITLE"), ["Neuköln"]);
        assert_eq!(v.get("CATALOGNUMBER"), ["PD83889"]);
        assert_eq!(v.get("DATE"), ["1977"]);
    }

    #[test]
    fn split_keeps_empty_pieces_between_terminators() {
        assert_eq!(ID3v2Bridge::split_null_terminated(b"a\0\0b", 0), ["a", "", "b"]);
        assert_eq!(ID3v2Bridge::split_null_terminated(b"a\0", 3), ["a"]);
        let mut utf16 = ID3v2Bridge::encode_utf16_with_bom("desc");
        utf16.extend_from_slice(&[0, 0]);
        utf16.extend(ID3v2Bridge::encode_utf16_with_bom("val"));
        utf16.push(0x41); // odd byte: Foundation ignores it
        assert_eq!(ID3v2Bridge::split_null_terminated(&utf16, 1), ["desc", "val"]);
    }
}
