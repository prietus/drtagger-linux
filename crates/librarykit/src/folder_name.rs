//! Pulls artist, title, years, edition, source format and country out of
//! folder names such as
//!   "1974 - Diamond Dogs (1984, W. Germany, RCA PD83889)"
//!   "Linkin Park - 2003 - Meteora 20th Anniversary Edition (MQA)"
//!   "Diana Krall- All For You XRCD Japan"
//! The bracketed notes are kept verbatim so identification can mine them
//! for catalog numbers and barcodes.

use fancy_regex::Regex as FancyRegex;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

/// What the folder name says the audio came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceHint {
    Cd,
    Sacd,
    Vinyl,
    Digital,
    Dvd,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedName {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    /// Core title, edition words removed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// First year seen: usually the original release.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<String>,
    /// A second year (pressing / remaster).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edition_year: Option<String>,
    /// "20th Anniversary Edition", "2011 Remaster", …
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edition: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceHint>,
    /// ISO 3166-1 alpha-2 as MusicBrainz uses it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    /// Bracketed groups, verbatim.
    pub notes: Vec<String>,
}

impl ParsedName {
    /// Title with the edition kept, for lists and sidebars.
    pub fn display_title(&self) -> Option<String> {
        let title = self.title.as_ref()?;
        Some(match &self.edition {
            Some(edition) => format!("{title} ({edition})"),
            None => title.clone(),
        })
    }
}

#[derive(Clone, Copy, Debug)]
enum Category {
    Edition,
    Source(SourceHint),
    Country(&'static str),
    Noise,
}

struct Rule {
    category: Category,
    regex: FancyRegex,
    case_sensitive: bool,
}

/// Each entry: a regex fragment (no anchors), what it means, and whether it
/// is case-sensitive.
const VOCABULARY: &[(&str, Category, bool)] = &[
    // Editions.
    (
        r"(?:\d{1,3}(?:st|nd|rd|th)\s+)?anniversary(?:\s+(?:super\s+)?deluxe)?(?:\s+(?:edition|version|box\s*set|reissue))?",
        Category::Edition,
        false,
    ),
    (r"(?:super\s+)?deluxe(?:\s+(?:edition|version|box\s*set))?", Category::Edition, false),
    (r"(?:19|20)\d{2}\s+(?:remaster(?:ed)?|reissue|mix|remix(?:ed)?|edition|version)", Category::Edition, false),
    (
        r"(?:expanded|special|limited|collector'?s|legacy|definitive|complete|ultimate|premium|extended|remastered|remaster|remixed|reissue|mono|stereo|box\s*set|bonus\s+tracks?|japan\s+edition|import)(?:\s+(?:edition|version))?",
        Category::Edition,
        false,
    ),
    // Physical / digital source.
    (
        r"xrcd(?:2|24)?|k2\s?hd|shm-?cd|blu-?spec\s?cd2?|hdcd|u?hqcd|gold\s?cd|platinum\s?shm|target\s?cd|mfsl|\d?cds?",
        Category::Source(SourceHint::Cd),
        false,
    ),
    (r"shm-?sacd|sacd(?:-?r)?|dsd(?:64|128|256|512)?|dsf|dff|dst|iso", Category::Source(SourceHint::Sacd), false),
    (r#"vinyl(?:\s*rip)?|vinylrip|lp|\d{3}\s?g(?:ram)?|12"|7"|10""#, Category::Source(SourceHint::Vinyl), false),
    (r"dvd-?a(?:udio)?|dvd|blu-?ray(?:\s+audio)?|bd-?a|pure\s+audio", Category::Source(SourceHint::Dvd), false),
    (
        r"mqa|web(?:\s*-?\s*(?:flac|dl))?|hi-?res|hires|qobuz|tidal|itunes|bandcamp|hdtracks|digital|(?:16|24|32)\s?-?bits?|(?:16|24|32)[-/](?:44(?:\.1)?|48|88(?:\.2)?|96|176(?:\.4)?|192|352(?:\.8)?|384)(?:\s?khz)?|\d{2,3}(?:\.\d)?\s?khz",
        Category::Source(SourceHint::Digital),
        false,
    ),
    // Containers and rip markers: stripped, mean nothing about the source.
    (r"flac|ape|wav|alac|aiff|wv|tak|m4a|mp3|aac|ogg|lossless|eac|xld|dbpoweramp|cue|log|rip", Category::Noise, false),
    // Countries by name (any case) and by code (upper case only).
    (r"(?:japan(?:ese)?)(?:\s+(?:edition|press(?:ing)?|import|release|version))?", Category::Country("JP"), false),
    (r"w(?:est)?\.?\s*germany|germany|german|deutschland", Category::Country("DE"), false),
    (r"u\.s\.a?\.?|usa|america(?:n)?", Category::Country("US"), false),
    (r"u\.k\.|england|british|britain", Category::Country("GB"), false),
    (r"europe(?:an)?", Category::Country("XE"), false),
    (r"france|french", Category::Country("FR"), false),
    (r"italy|italian", Category::Country("IT"), false),
    (r"spain|spanish|españa", Category::Country("ES"), false),
    (r"holland|netherlands|dutch", Category::Country("NL"), false),
    (r"canada|canadian", Category::Country("CA"), false),
    (r"australia(?:n)?", Category::Country("AU"), false),
    (r"worldwide", Category::Country("XW"), false),
    (r"korea(?:n)?", Category::Country("KR"), false),
    (r"brazil(?:ian)?", Category::Country("BR"), false),
    (r"russia(?:n)?", Category::Country("RU"), false),
    (r"sweden|swedish", Category::Country("SE"), false),
    (r"norway|norwegian", Category::Country("NO"), false),
    (r"denmark|danish", Category::Country("DK"), false),
    (r"austria(?:n)?", Category::Country("AT"), false),
    (r"switzerland|swiss", Category::Country("CH"), false),
    (r"mexico|mexican", Category::Country("MX"), false),
    (r"argentina", Category::Country("AR"), false),
    (r"taiwan", Category::Country("TW"), false),
    (r"hong\s?kong", Category::Country("HK"), false),
    (r"china|chinese", Category::Country("CN"), false),
    (r"JPN?", Category::Country("JP"), true),
    (r"GER", Category::Country("DE"), true),
    (r"US", Category::Country("US"), true),
    (r"UK", Category::Country("GB"), true),
    (r"EU", Category::Country("XE"), true),
];

fn rules(prefix: &str, suffix: &str) -> Vec<Rule> {
    VOCABULARY
        .iter()
        .map(|&(fragment, category, case_sensitive)| {
            let flags = if case_sensitive { "" } else { "(?i)" };
            Rule {
                category,
                regex: FancyRegex::new(&format!("{flags}{prefix}(?:{fragment}){suffix}")).unwrap(),
                case_sensitive,
            }
        })
        .collect()
}

static TAIL_RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| rules(r"(?:^|(?<=[\s,;/\-]))", r"[\s,;.\-]*$"));
/// Same vocabulary, matched anywhere: for bracketed notes.
static ANYWHERE_RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| rules(r"(?:^|(?<=[\s,;/\-]))", r"(?=$|[\s,;.\-/])"));

static BRACKETS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\(\[\{]([^\)\]\}]*)[\)\]\}]").unwrap());
static MULTI_SPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s{2,}").unwrap());
static LEADING_YEAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*((?:19|20)\d{2})\b\s*[-._]?\s*").unwrap());
static MIDDLE_YEAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+[-–]\s+((?:19|20)\d{2})\s+[-–]\s+").unwrap());
static GLUED_DASH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(.+?\S)-\s+(.+)$").unwrap());
static YEAR_AT_END: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^((?:19|20)\d{2})\s+(\S.*)$|^(.*\S)\s+((?:19|20)\d{2})$").unwrap());
static WHOLE_YEAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:19|20)\d{2}$").unwrap());
static NOTE_SPLIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*[,;/]\s*").unwrap());
static ANY_YEAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b((?:19|20)\d{2})\b").unwrap());

fn is_blank(c: char) -> bool {
    c.is_whitespace() && !matches!(c, '\n' | '\r' | '\u{0B}' | '\u{0C}' | '\u{85}' | '\u{2028}' | '\u{2029}')
}

fn trim_separators(s: &str) -> String {
    s.trim_matches(|c: char| is_blank(c) || ".-–—,;:/".contains(c)).to_string()
}

pub struct FolderNameParser;

impl FolderNameParser {
    pub fn parse(name: &str) -> ParsedName {
        let mut p = ParsedName::default();
        let s = name.replace('_', " ");

        // 1. Bracketed notes anywhere in the name.
        collect_notes(&s, &mut p);
        let s = BRACKETS.replace_all(&s, " ");
        let mut s = MULTI_SPACE.replace_all(&s, " ").into_owned();

        // 2. Years: "1974 - Title", "Artist - 2003 - Title".
        if let Some(c) = LEADING_YEAR.captures(&s) {
            p.year = Some(c[1].to_string());
            let r = c.get(0).unwrap().range();
            s.replace_range(r, "");
        } else if let Some(c) = MIDDLE_YEAR.captures(&s) {
            p.year = Some(c[1].to_string());
            let r = c.get(0).unwrap().range();
            s.replace_range(r, " - ");
        }
        let s = trim_separators(&s);

        // 3. "Artist - Title" (also "Artist – Title" and "Artist- Title").
        let mut parts: Vec<String> = s.split(" - ").map(str::to_string).collect();
        if parts.len() == 1 {
            parts = s.split(" – ").map(str::to_string).collect();
        }
        if parts.len() == 1
            && let Some(c) = GLUED_DASH.captures(&s)
        {
            parts = vec![c[1].to_string(), c[2].to_string()];
        }
        let mut raw_title = if parts.len() >= 2 {
            p.artist = Some(trim_separators(&parts[0]));
            parts[1..].join(" - ")
        } else {
            s.clone()
        };
        raw_title = trim_separators(&raw_title);

        // 4. A year left at either end of the title ("Aja 1977"; "1984" alone stays).
        if p.year.is_none()
            && let Some(c) = YEAR_AT_END.captures(&raw_title)
        {
            let year = c.get(1).or(c.get(4)).map(|m| m.as_str().to_string());
            let rest = c.get(2).or(c.get(3)).map(|m| m.as_str().to_string());
            if let (Some(year), Some(rest)) = (year, rest) {
                p.year = Some(year);
                raw_title = rest;
            }
        }
        if p.year.is_none() && WHOLE_YEAR.is_match(&raw_title) {
            p.year = Some(raw_title.clone());
        }

        // 5. Edition / source / country words at the end of the title.
        p.title = Some(refine_title(&raw_title, &mut p));

        // 6. The notes: whole pieces that are edition/source/country words,
        // plus any further years.
        refine_notes(&mut p);
        if p.artist.as_deref() == Some("") {
            p.artist = None;
        }
        if p.title.as_deref() == Some("") {
            p.title = None;
        }
        p
    }

    /// For an ALBUM tag: "Meteora (20th Anniversary Edition)" → title + edition.
    pub fn parse_album_title(raw: &str) -> ParsedName {
        let mut p = ParsedName::default();
        collect_notes(raw, &mut p);
        let s = BRACKETS.replace_all(raw, " ");
        let s = MULTI_SPACE.replace_all(&s, " ");
        p.title = Some(refine_title(&trim_separators(&s), &mut p));
        refine_notes(&mut p);
        if p.title.as_deref() == Some("") {
            p.title = Some(raw.to_string());
        }
        p
    }
}

fn collect_notes(s: &str, p: &mut ParsedName) {
    for c in BRACKETS.captures_iter(s) {
        let note = c[1].trim_matches(is_blank);
        if !note.is_empty() {
            p.notes.push(note.to_string());
        }
    }
}

fn refine_notes(p: &mut ParsedName) {
    for note in p.notes.clone() {
        for piece in NOTE_SPLIT.split(&note).filter(|s| !s.is_empty()) {
            let leftover = refine_note(piece, p);
            for c in ANY_YEAR.captures_iter(&leftover) {
                note_year(&c[1], p);
            }
        }
    }
}

/// Classifies every known word in a note piece; returns what is left
/// (catalog numbers, labels, plain years).
fn refine_note(piece: &str, p: &mut ParsedName) -> String {
    let mut text = piece.to_string();
    let mut editions: Vec<String> = vec![];
    let mut sources: Vec<SourceHint> = vec![];
    for rule in ANYWHERE_RULES.iter() {
        let ranges: Vec<_> = rule.regex.find_iter(&text).filter_map(Result::ok).map(|m| m.range()).collect();
        for r in ranges.into_iter().rev() {
            let token = text[r.clone()].to_string();
            match rule.category {
                Category::Edition => editions.insert(0, token),
                Category::Source(hint) => sources.push(hint),
                Category::Country(code) => {
                    if p.country.is_none() {
                        p.country = Some(code.to_string());
                    }
                }
                Category::Noise => {}
            }
            text.replace_range(r, " ");
        }
    }
    record(&editions, &sources, p);
    text.trim_matches(is_blank).to_string()
}

fn record(editions: &[String], sources: &[SourceHint], p: &mut ParsedName) {
    if !editions.is_empty() {
        let joined = editions.join(", ");
        p.edition = Some(match &p.edition {
            Some(e) => format!("{e}, {joined}"),
            None => joined.clone(),
        });
        // A year inside an edition phrase dates the edition, not the album.
        for c in ANY_YEAR.captures_iter(&joined) {
            if p.edition_year.is_none() && Some(&c[1]) != p.year.as_deref() {
                p.edition_year = Some(c[1].to_string());
            }
        }
    }
    // Prefer the physical medium named over a mere bit depth.
    for hint in [SourceHint::Vinyl, SourceHint::Sacd, SourceHint::Dvd, SourceHint::Cd, SourceHint::Digital] {
        if sources.contains(&hint) {
            if p.source.is_none() || (p.source == Some(SourceHint::Digital) && hint != SourceHint::Digital) {
                p.source = Some(hint);
            }
            break;
        }
    }
}

fn refine_title(raw: &str, p: &mut ParsedName) -> String {
    let mut title = trim_separators(raw);
    let mut editions: Vec<String> = vec![];
    let mut sources: Vec<SourceHint> = vec![];
    let (mut changed, mut stripped_something) = (true, false);
    while changed && !title.is_empty() {
        changed = false;
        for rule in TAIL_RULES.iter() {
            if matches!(rule.category, Category::Country(_)) && rule.case_sensitive && !stripped_something {
                continue;
            }
            let Ok(Some(m)) = rule.regex.find(&title) else { continue };
            let token = trim_separators(m.as_str());
            let rest = trim_separators(&title[..m.start()]);
            // Never empty the title: "1984", "LP", "Mono" can be the album.
            if rest.is_empty() {
                continue;
            }
            match rule.category {
                Category::Edition => editions.insert(0, token),
                Category::Source(hint) => sources.push(hint),
                Category::Country(code) => {
                    if p.country.is_none() {
                        p.country = Some(code.to_string());
                    }
                }
                Category::Noise => {}
            }
            title = rest;
            changed = true;
            stripped_something = true;
            break;
        }
    }
    record(&editions, &sources, p);
    title
}

fn note_year(year: &str, p: &mut ParsedName) {
    if p.year.is_none() {
        p.year = Some(year.to_string());
    } else if Some(year) != p.year.as_deref() && p.edition_year.is_none() {
        p.edition_year = Some(year.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &Option<String>) -> Option<&str> {
        v.as_deref()
    }

    #[test]
    fn folder_name_parsing() {
        let p = FolderNameParser::parse("1974 - Diamond Dogs (1984, W. Germany, RCA PD83889)");
        assert_eq!(s(&p.year), Some("1974"));
        assert_eq!(s(&p.title), Some("Diamond Dogs"));
        assert_eq!(p.artist, None);
        assert_eq!(s(&p.edition_year), Some("1984"));
        assert_eq!(s(&p.country), Some("DE"));
        assert_eq!(p.source, None);

        let q = FolderNameParser::parse("1968. Creedence - Bayou Country (Fantasy FCD 8387-2, Germany)");
        assert_eq!(s(&q.year), Some("1968"));
        assert_eq!(s(&q.artist), Some("Creedence"));
        assert_eq!(s(&q.title), Some("Bayou Country"));

        let r = FolderNameParser::parse("(Japanese_SICP-1700).High_Voltage");
        assert_eq!(r.year, None);
        assert_eq!(s(&r.title), Some("High Voltage"));
        assert_eq!(s(&r.country), Some("JP"));
        assert_eq!(r.notes, ["Japanese SICP-1700"]);

        let m = FolderNameParser::parse("Linkin Park - 2003 - Meteora 20th Anniversary Edition (MQA)");
        assert_eq!(s(&m.artist), Some("Linkin Park"));
        assert_eq!(s(&m.year), Some("2003"));
        assert_eq!(s(&m.title), Some("Meteora"));
        assert_eq!(s(&m.edition), Some("20th Anniversary Edition"));
        assert_eq!(m.display_title().as_deref(), Some("Meteora (20th Anniversary Edition)"));
        assert_eq!(m.source, Some(SourceHint::Digital));
        assert_eq!(m.notes, ["MQA"]);

        let k = FolderNameParser::parse("Diana Krall- All For You XRCD Japan");
        assert_eq!(s(&k.artist), Some("Diana Krall"));
        assert_eq!(s(&k.title), Some("All For You"));
        assert_eq!(k.source, Some(SourceHint::Cd));
        assert_eq!(s(&k.country), Some("JP"));

        let w = FolderNameParser::parse("Miles Davis All Stars - Walkin' XRCD");
        assert_eq!(s(&w.title), Some("Walkin'"));
        assert_eq!(w.source, Some(SourceHint::Cd));

        let e = FolderNameParser::parse("Ella Fitzgerald & Oscar Peterson - Ella and Oscar (1975, JVC-XRCD)");
        assert_eq!(s(&e.artist), Some("Ella Fitzgerald & Oscar Peterson"));
        assert_eq!(s(&e.title), Some("Ella and Oscar"));
        assert_eq!(s(&e.year), Some("1975"));
        assert_eq!(e.source, Some(SourceHint::Cd));

        let vh = FolderNameParser::parse("Van Halen - 1984");
        assert_eq!(s(&vh.artist), Some("Van Halen"));
        assert_eq!(s(&vh.title), Some("1984"));
        assert_eq!(s(&vh.year), Some("1984"));

        let aja = FolderNameParser::parse("Steely Dan - Aja 1977");
        assert_eq!(s(&aja.title), Some("Aja"));
        assert_eq!(s(&aja.year), Some("1977"));

        let pf = FolderNameParser::parse("Pink Floyd - The Dark Side of the Moon (2011 Remaster) [24-96]");
        assert_eq!(s(&pf.title), Some("The Dark Side of the Moon"));
        assert_eq!(s(&pf.edition), Some("2011 Remaster"));
        assert_eq!(s(&pf.edition_year), Some("2011"));
        assert_eq!(pf.source, Some(SourceHint::Digital));

        let lp = FolderNameParser::parse("Led Zeppelin - Physical Graffiti (24-96 Vinyl Rip)");
        assert_eq!(lp.source, Some(SourceHint::Vinyl), "the physical medium wins over the bit depth");

        let us = FolderNameParser::parse("Some Band - The Best of Us");
        assert_eq!(s(&us.title), Some("The Best of Us"), "a plain word is never a country code");

        let tag = FolderNameParser::parse_album_title("Meteora (20th Anniversary Edition)");
        assert_eq!(s(&tag.title), Some("Meteora"));
        assert_eq!(s(&tag.edition), Some("20th Anniversary Edition"));
        assert_eq!(s(&FolderNameParser::parse_album_title("Aja").title), Some("Aja"));
        assert_eq!(s(&FolderNameParser::parse_album_title("LP").title), Some("LP"));
    }
}
