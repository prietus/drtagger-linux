//! Renders "{albumartist}/{album} ({year})/{track} {title}" style templates
//! into file-system-safe relative paths. Unknown tokens render empty and
//! decorations left dangling by empty values ("()", " - ") are removed.

use regex::Regex;
use rust_icu_sys::UTransDirection;
use rust_icu_utrans::UTransliterator;
use std::collections::HashMap;
use std::sync::LazyLock;
use unicode_properties::{GeneralCategory, UnicodeGeneralCategory};
use unicode_segmentation::UnicodeSegmentation;

pub struct PathTemplate;

pub const DEFAULT_ALBUM_FOLDER: &str = "{albumartist}/{album} ({year})";
pub const DEFAULT_TRACK_FILE: &str = "{track} {title}";

/// Placeholders every caller can rely on (case-insensitive in templates).
pub const PLACEHOLDERS: &[&str] = &[
    "albumartist",
    "album",
    "year",
    "originalyear",
    "artist",
    "title",
    "track",
    "tracktotal",
    "disc",
    "disctotal",
    "label",
    "catalognumber",
    "media",
];

static WS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
static TRAILING_DASH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\s-\s*)+$").unwrap());
static LEADING_DASH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\s*-\s)+").unwrap());

/// Foundation's `.toLatin`, `.stripDiacritics` and the diacritic/width
/// folding, as ICU transforms.
const TO_LATIN: &str = "Any-Latin";
const STRIP_DIACRITICS: &str = "NFD; [:Nonspacing Mark:] Remove; NFC";
const FOLD: &str = "NFD; [:Nonspacing Mark:] Remove; NFC; Fullwidth-Halfwidth";

const REPLACEMENTS: &[(&str, &str)] = &[
    ("–", "-"),
    ("—", "-"),
    ("‘", "'"),
    ("’", "'"),
    ("‚", "'"),
    ("“", "\""),
    ("”", "\""),
    ("„", "\""),
    ("…", "..."),
    ("×", "x"),
    ("ß", "ss"),
    ("æ", "ae"),
    ("Æ", "AE"),
    ("ø", "o"),
    ("Ø", "O"),
    ("œ", "oe"),
    ("Œ", "OE"),
    ("đ", "d"),
    ("Đ", "D"),
    ("ł", "l"),
    ("Ł", "L"),
    ("ı", "i"),
    ("·", "-"),
    ("／", "-"),
    ("＆", "&"),
];

fn transform(id: &str, s: &str) -> String {
    UTransliterator::new(id, None, UTransDirection::UTRANS_FORWARD)
        .and_then(|t| t.transliterate(s))
        .unwrap_or_else(|_| s.to_string())
}

/// Swift's `CharacterSet.whitespaces` for trimming.
fn is_blank(c: char) -> bool {
    c == '\t' || c.general_category() == GeneralCategory::SpaceSeparator
}

impl PathTemplate {
    pub fn render(template: &str, values: &HashMap<String, String>, ascii: bool) -> String {
        // Split on the template's own slashes first so a value containing
        // "/" can never create an extra folder.
        template
            .split('/')
            .filter(|c| !c.is_empty())
            .map(|c| Self::render_component(c, values, ascii))
            .filter(|c| !c.is_empty())
            .collect::<Vec<_>>()
            .join("/")
    }

    /// "Björk – Vespertine (日本)" → "Bjork - Vespertine (ri ben)": Latin
    /// transliteration, diacritics stripped, typographic punctuation
    /// replaced, anything else outside printable ASCII dropped.
    pub fn ascii_safe(s: &str) -> String {
        let mut t = transform(TO_LATIN, s);
        t = transform(STRIP_DIACRITICS, &t);
        t = transform(FOLD, &t);
        for (from, to) in REPLACEMENTS {
            t = t.replace(from, to);
        }
        let t: String = t.chars().map(|c| if (' '..='~').contains(&c) { c } else { ' ' }).collect();
        WS.replace_all(&t, " ").trim_matches(is_blank).to_string()
    }

    pub fn render_component(template: &str, values: &HashMap<String, String>, ascii: bool) -> String {
        let mut out = String::new();
        let mut rest = template;
        while let Some(open) = rest.find('{') {
            out.push_str(&rest[..open]);
            let Some(close) = rest[open..].find('}').map(|c| c + open) else {
                out.push_str(&rest[open..]);
                rest = "";
                break;
            };
            let key = rest[open + 1..close].to_lowercase();
            out.push_str(&values.get(&key).map(String::as_str).unwrap_or("").replace('/', "-"));
            rest = &rest[close + 1..];
        }
        out.push_str(rest);
        Self::sanitize_component(&if ascii { Self::ascii_safe(&out) } else { out })
    }

    /// One path component: no slashes or colons, no control characters, no
    /// leading dots, collapsed whitespace, empty decorations dropped.
    pub fn sanitize_component(s: &str) -> String {
        let t = s.replace(['/', ':'], "-");
        let t = WS.replace_all(&t, " ");
        // CharacterSet.controlCharacters: general categories Cc and Cf.
        let mut t: String = t
            .chars()
            .filter(|c| !matches!(c.general_category(), GeneralCategory::Control | GeneralCategory::Format))
            .collect();
        for empty in ["()", "[]", "{}", "( )", "[ ]"] {
            t = t.replace(empty, "");
        }
        let t = WS.replace_all(&t, " ");
        let t = TRAILING_DASH.replace_all(&t, "");
        let t = LEADING_DASH.replace_all(&t, "");
        let mut t: Vec<&str> = t.trim_matches(is_blank).graphemes(true).collect();
        while t.first() == Some(&".") {
            t.remove(0);
        }
        while matches!(t.last(), Some(&".") | Some(&" ")) {
            t.pop();
        }
        let mut t: String = t.concat();
        if t.len() > 200 {
            t = t.graphemes(true).take(200).collect::<String>().trim_matches(is_blank).to_string();
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn renders_and_cleans() {
        let v = values(&[
            ("albumartist", "AC/DC"),
            ("album", "High Voltage"),
            ("year", ""),
            ("track", "01"),
            ("title", "It's a Long Way"),
        ]);
        assert_eq!(PathTemplate::render(DEFAULT_ALBUM_FOLDER, &v, false), "AC-DC/High Voltage");
        assert_eq!(PathTemplate::render("{track} - {title}", &v, false), "01 - It's a Long Way");
        assert_eq!(PathTemplate::render("{TRACK} {nope}", &v, false), "01");
        assert_eq!(PathTemplate::sanitize_component("...hidden: name.. "), "hidden- name");
        assert_eq!(PathTemplate::sanitize_component(" - Title - "), "Title");
    }

    #[test]
    fn ascii_transliterates() {
        assert_eq!(PathTemplate::ascii_safe("Björk – Vespertine (日本)"), "Bjork - Vespertine (ri ben)");
        assert_eq!(PathTemplate::ascii_safe("Кино"), "Kino");
    }
}
