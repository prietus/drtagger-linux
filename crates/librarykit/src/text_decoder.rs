//! CUE sheets have no encoding declaration and come from Windows tools in
//! every locale. UTF-8 is tried first; for 8-bit files the byte patterns
//! decide between Shift-JIS (runs of double-byte characters), CP1251 (runs
//! of Cyrillic letters) and CP1252 / Latin-1 (isolated accents and smart
//! quotes, by far the most common case).
//!
//! Decoders are strict like Foundation's `String(data:encoding:)`: CP1252
//! fails on its five undefined bytes (→ Latin-1), CP1251 on 0x98, and
//! Shift-JIS follows Apple's CP932 table where it differs from WHATWG's.

use encoding_rs::{Encoding, SHIFT_JIS, UTF_16BE, UTF_16LE, WINDOWS_1251, WINDOWS_1252};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decoded {
    pub text: String,
    pub encoding_name: &'static str,
}

pub struct TextDecoder;

fn strict(encoding: &'static Encoding, bytes: &[u8]) -> Option<String> {
    encoding.decode_without_bom_handling_and_without_replacement(bytes).map(|c| c.into_owned())
}

/// Code points where WHATWG's Shift_JIS table differs from Apple's (CP932).
fn cp932(text: String) -> String {
    if !text.chars().any(|c| matches!(c, '\u{301C}' | '\u{2016}' | '\u{2212}' | '\u{00A2}' | '\u{00A3}' | '\u{00AC}')) {
        return text;
    }
    text.chars()
        .map(|c| match c {
            '\u{301C}' => '\u{FF5E}',
            '\u{2016}' => '\u{2225}',
            '\u{2212}' => '\u{FF0D}',
            '\u{00A2}' => '\u{FFE0}',
            '\u{00A3}' => '\u{FFE1}',
            '\u{00AC}' => '\u{FFE2}',
            c => c,
        })
        .collect()
}

impl TextDecoder {
    /// Maps WHATWG Shift_JIS output onto Apple's CP932 code points.
    pub fn decode_shift_jis_fixups(text: String) -> String {
        cp932(text)
    }

    pub fn decode(bytes: &[u8]) -> Decoded {
        let ok = |text: String, encoding_name| Decoded { text, encoding_name };

        if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF])
            && let Ok(s) = std::str::from_utf8(rest)
        {
            return ok(s.to_string(), "utf-8-bom");
        }
        if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE])
            && let Some(s) = strict(UTF_16LE, rest)
        {
            return ok(s, "utf-16");
        }
        if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF])
            && let Some(s) = strict(UTF_16BE, rest)
        {
            return ok(s, "utf-16");
        }
        if let Ok(s) = std::str::from_utf8(bytes) {
            return ok(s.to_string(), if bytes.iter().any(|&b| b >= 0x80) { "utf-8" } else { "ascii" });
        }

        if Self::is_plausible_shift_jis(bytes)
            && let Some(s) = strict(SHIFT_JIS, bytes)
        {
            return ok(cp932(s), "shift_jis");
        }
        if !bytes.contains(&0x98)
            && let Some(s) = strict(WINDOWS_1251, bytes)
            && Self::has_cyrillic_word(&s)
        {
            return ok(s, "cp1251");
        }
        if !bytes.iter().any(|b| matches!(b, 0x81 | 0x8D | 0x8F | 0x90 | 0x9D))
            && let Some(s) = strict(WINDOWS_1252, bytes)
        {
            return ok(s, "cp1252");
        }
        // ISO 8859-1 maps every byte to the code point of the same value.
        ok(bytes.iter().map(|&b| b as char).collect(), "iso-8859-1")
    }

    /// Every high byte must form a valid Shift-JIS sequence, and there must
    /// be at least one run of two consecutive double-byte characters. A
    /// CP1252 apostrophe (0x92 's') happens to be a valid lead+trail pair,
    /// but never two in a row.
    pub fn is_plausible_shift_jis(bytes: &[u8]) -> bool {
        let (mut i, mut run) = (0, 0);
        let (mut saw_run, mut saw_high) = (false, false);
        while i < bytes.len() {
            let b = bytes[i];
            if b < 0x80 {
                run = 0;
                i += 1;
                continue;
            }
            saw_high = true;
            if (0xA1..=0xDF).contains(&b) {
                // half-width katakana
                run += 1;
                saw_run |= run >= 2;
                i += 1;
                continue;
            }
            let is_lead = (0x81..=0x9F).contains(&b) || (0xE0..=0xFC).contains(&b);
            if !is_lead || i + 1 >= bytes.len() {
                return false;
            }
            let t = bytes[i + 1];
            if !((0x40..=0x7E).contains(&t) || (0x80..=0xFC).contains(&t)) {
                return false;
            }
            run += 1;
            saw_run |= run >= 2;
            i += 2;
        }
        saw_high && saw_run
    }

    /// Three or more consecutive Cyrillic letters somewhere in the text.
    pub fn has_cyrillic_word(s: &str) -> bool {
        let mut run = 0;
        for c in s.chars() {
            if ('\u{0400}'..='\u{04FF}').contains(&c) {
                run += 1;
                if run >= 3 {
                    return true;
                }
            } else {
                run = 0;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foundation_strictness() {
        assert_eq!(TextDecoder::decode(&[0x41, 0x81, 0x42]).encoding_name, "iso-8859-1");
        assert_eq!(TextDecoder::decode(&[0x41, 0x92, 0x42]).text, "A\u{2019}B");
        assert_eq!(TextDecoder::decode(&[0xFF, 0xFE, 0x41, 0x00]).text, "A");
    }

    #[test]
    fn shift_jis_follows_cp932() {
        // "〜" in two runs so the sniffer accepts it: 0x8160 0x8160.
        let d = TextDecoder::decode(&[0x81, 0x60, 0x81, 0x60]);
        assert_eq!(d.encoding_name, "shift_jis");
        assert_eq!(d.text, "\u{FF5E}\u{FF5E}");
    }
}
