//! Text decoding with Foundation's `String(data:encoding:)` rules, so tags
//! read the same as on the Mac:
//! - UTF-8 is strict and drops a leading BOM;
//! - UTF-16 ("with BOM") honours FF FE / FE FF, consumes it, and assumes
//!   big-endian without one; later BOMs stay as U+FEFF;
//! - UTF-16BE keeps a BOM as U+FEFF;
//! - an odd trailing byte is ignored; an unpaired surrogate fails.

pub fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

pub fn utf8(bytes: &[u8]) -> Option<String> {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    std::str::from_utf8(bytes).ok().map(str::to_string)
}

/// `String(decoding:as: UTF8.self)`: invalid sequences become U+FFFD.
pub fn utf8_lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn units(bytes: &[u8], little_endian: bool) -> impl Iterator<Item = u16> + '_ {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(move |&p| if little_endian { u16::from_le_bytes(p) } else { u16::from_be_bytes(p) })
}

fn decode_units(units: impl Iterator<Item = u16>) -> Option<String> {
    char::decode_utf16(units).collect::<Result<String, _>>().ok()
}

/// Foundation's `.utf16`.
pub fn utf16(bytes: &[u8]) -> Option<String> {
    match bytes {
        [0xFF, 0xFE, rest @ ..] => decode_units(units(rest, true)),
        [0xFE, 0xFF, rest @ ..] => decode_units(units(rest, false)),
        _ => decode_units(units(bytes, false)),
    }
}

/// Foundation's `.utf16BigEndian`.
pub fn utf16_be(bytes: &[u8]) -> Option<String> {
    decode_units(units(bytes, false))
}

/// Foundation's `.ascii`: 7-bit only.
pub fn ascii(bytes: &[u8]) -> Option<String> {
    bytes.is_ascii().then(|| bytes.iter().map(|&b| b as char).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foundation_rules() {
        assert_eq!(utf16(&[0x41, 0x00, 0x42, 0x00]).unwrap(), "\u{4100}\u{4200}");
        assert_eq!(utf16(&[0xFF, 0xFE, 0x41, 0x00, 0xFF, 0xFE, 0x42, 0x00]).unwrap(), "A\u{FEFF}B");
        assert_eq!(utf16(&[0xFE, 0xFF, 0x00, 0x41]).unwrap(), "A");
        assert_eq!(utf16(&[0xFF, 0xFE, 0x00, 0xD8, 0x41, 0x00]), None);
        assert_eq!(utf16(&[0xFF, 0xFE, 0x41, 0x00, 0x42]).unwrap(), "A");
        assert_eq!(utf16(&[0xFF, 0xFE]).unwrap(), "");
        assert_eq!(utf16_be(&[0xFE, 0xFF, 0x00, 0x41]).unwrap(), "\u{FEFF}A");
        assert_eq!(utf8(&[0x41, 0xFF]), None);
        assert_eq!(utf8(&[0xEF, 0xBB, 0xBF, 0x41]).unwrap(), "A");
        assert_eq!(latin1(&[0x41, 0xE9]), "Aé");
        assert_eq!(ascii(&[0x41, 0xE9]), None);
        assert_eq!(utf8_lossy(&[0x41, 0xE2, 0x82, 0x41, 0xFF, 0xC3]), "A\u{FFFD}A\u{FFFD}\u{FFFD}");
    }
}
