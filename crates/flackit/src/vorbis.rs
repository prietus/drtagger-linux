//! Vorbis comment layout (little-endian, unlike the rest of FLAC):
//!   u32 vendor_length, vendor (UTF-8), u32 count, then per comment
//!   u32 length + "NAME=value" (UTF-8).
//! Names are case-insensitive; duplicates are legal (several ARTIST).

use crate::FlacError;
use crate::text;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VorbisComment {
    pub vendor: String,
    pub fields: Vec<(String, String)>,
}

impl VorbisComment {
    pub fn new(vendor: impl Into<String>, fields: Vec<(String, String)>) -> Self {
        VorbisComment { vendor: vendor.into(), fields }
    }

    /// Every value of a field, matched ignoring case.
    pub fn get(&self, name: &str) -> Vec<&str> {
        self.fields.iter().filter(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str()).collect()
    }

    pub fn encoded(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(self.vendor.len() as u32).to_le_bytes());
        out.extend_from_slice(self.vendor.as_bytes());
        out.extend_from_slice(&(self.fields.len() as u32).to_le_bytes());
        for (name, value) in &self.fields {
            let entry = format!("{name}={value}");
            out.extend_from_slice(&(entry.len() as u32).to_le_bytes());
            out.extend_from_slice(entry.as_bytes());
        }
        out
    }

    pub fn parse(data: &[u8]) -> Result<VorbisComment, FlacError> {
        let mut cursor = 0usize;
        let u32_le = |cursor: &mut usize| -> Result<u32, FlacError> {
            let b = data
                .get(*cursor..*cursor + 4)
                .ok_or_else(|| FlacError::InvalidVorbisComment(format!("truncated at offset {cursor}")))?;
            *cursor += 4;
            Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        };
        let bytes = |cursor: &mut usize, n: usize| -> Result<&[u8], FlacError> {
            let slice = data
                .get(*cursor..cursor.saturating_add(n))
                .ok_or_else(|| FlacError::InvalidVorbisComment(format!("truncated string at offset {cursor}")))?;
            *cursor += n;
            Ok(slice)
        };
        let vendor_len = u32_le(&mut cursor)? as usize;
        let vendor = text::utf8(bytes(&mut cursor, vendor_len)?)
            .ok_or_else(|| FlacError::InvalidVorbisComment("vendor is not valid UTF-8".into()))?;
        let count = u32_le(&mut cursor)?;
        let mut fields = Vec::new();
        for _ in 0..count {
            let len = u32_le(&mut cursor)? as usize;
            let entry = text::utf8(bytes(&mut cursor, len)?)
                .ok_or_else(|| FlacError::InvalidVorbisComment("comment is not valid UTF-8".into()))?;
            let Some(eq) = entry.find('=') else {
                return Err(FlacError::InvalidVorbisComment(format!("comment missing '=': {entry}")));
            };
            fields.push((entry[..eq].to_string(), entry[eq + 1..].to_string()));
        }
        Ok(VorbisComment { vendor, fields })
    }
}
