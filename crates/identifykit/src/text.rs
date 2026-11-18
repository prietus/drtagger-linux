//! Foundation's string folding, as the scorer compares names with it.

use unicode_normalization::UnicodeNormalization;
use unicode_properties::{GeneralCategoryGroup, UnicodeGeneralCategory};

/// Combining marks `.diacriticInsensitive` removes. Kana voicing marks
/// (U+3099, U+309A) and the marks of other scripts stay, as on macOS.
fn is_stripped_mark(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
}

/// `folding(options: [.diacriticInsensitive, .widthInsensitive], locale: nil)`:
/// accents off, full-width ASCII to ASCII and half-width kana to full
/// width; ligatures, fractions and superscripts stay.
pub fn fold(s: &str) -> String {
    let mut widened = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c as u32, 0xFF00..=0xFFEF | 0x3000) {
            widened.extend(std::iter::once(c).nfkc());
        } else {
            widened.push(c);
        }
    }
    widened.nfd().filter(|c| !is_stripped_mark(*c)).nfc().collect()
}

/// `CharacterSet.alphanumerics`: letters, marks and numbers.
pub fn is_alphanumeric(c: char) -> bool {
    matches!(
        c.general_category_group(),
        GeneralCategoryGroup::Letter | GeneralCategoryGroup::Mark | GeneralCategoryGroup::Number
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_like_foundation() {
        // Captured from Foundation on macOS (lowercased, then folded).
        for (input, expected) in [
            ("Leño – ¡Corre, corre!", "leno – ¡corre, corre!"),
            ("Øresund Łódź æther straße", "øresund łodz æther straße"),
            ("ﬁnal Ａｂｃ１２３ ｶﾀｶﾅ ｈｅｌｌｏ", "ﬁnal abc123 カタカナ hello"),
            ("Ünïcödé Ñandú Čeština", "unicode nandu cestina"),
            ("Mötley Crüe & The ½ Ⅻ ²", "motley crue & the ½ ⅻ ²"),
            ("ありがとう ガギ パ", "ありがとう ガギ パ"),
            ("Ελληνικά άέή Россия Ёё", "ελληνικα αεη россия ее"),
            ("cafe\u{0301}\u{0301} x\u{0308}", "cafe x"),
            ("مرحبا 한국어 中文", "مرحبا 한국어 中文"),
        ] {
            assert_eq!(fold(&input.to_lowercase()), expected, "{input}");
        }
    }
}
