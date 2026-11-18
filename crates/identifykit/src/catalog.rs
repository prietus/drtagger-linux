//! Catalog numbers in OCR'd sleeve text and folder names: "UICY-1234",
//! "PD 83889", "VDJ~1541", glued "UICY40164".

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::LazyLock;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CatalogNumber {
    pub prefix: String,
    pub number: String,
}

impl CatalogNumber {
    pub fn formatted(&self) -> String {
        format!("{}-{}", self.prefix, self.number)
    }
}

static PATTERNS: LazyLock<[regex::Regex; 4]> = LazyLock::new(|| {
    [
        // Japanese style: UICY-1234; western: PRESTIGE-7142.
        regex::Regex::new(r"\b([A-Z]{2,10})-(\d{1,6})\b").unwrap(),
        regex::Regex::new(r"\b([A-Z]{2,10})\s+(\d{3,6})\b").unwrap(),
        regex::Regex::new(r"\b([A-Z]{2,10})\s*[-~_]\s*(\d{1,6})\b").unwrap(),
        // Glued: PD83889, UICY40164.
        regex::Regex::new(r"\b([A-Z]{2,6})(\d{4,7})\b").unwrap(),
    ]
});

/// OCR'd words that look like a prefix but never are.
const STOP_WORDS: &[&str] = &[
    "CD",
    "DVD",
    "SACD",
    "LP",
    "TRACK",
    "DISC",
    "SIDE",
    "TOTAL",
    "TIME",
    "PAGE",
    "VOL",
    "NO",
    "STEREO",
    "MONO",
    "ISBN",
    "EAN",
    "UPC",
    "JAN",
    "TEL",
    "FAX",
    "BOX",
    "SUITE",
    "NEW",
    "YORK",
    "LONDON",
    "TOKYO",
    "PARIS",
    "JANUARY",
    "FEBRUARY",
    "MARCH",
    "APRIL",
    "MAY",
    "JUNE",
    "JULY",
    "AUGUST",
    "SEPTEMBER",
    "OCTOBER",
    "NOVEMBER",
    "DECEMBER",
    "ENERO",
    "FEBRERO",
    "MARZO",
    "ABRIL",
    "MAYO",
    "JUNIO",
    "JULIO",
    "AGOSTO",
    "SEPTIEMBRE",
    "OCTUBRE",
    "NOVIEMBRE",
    "DICIEMBRE",
    // Printed next to years: "EUROPE 2006", "MADE IN JAPAN".
    "EUROPE",
    "JAPAN",
    "GERMANY",
    "HOLLAND",
    "FRANCE",
    "ITALY",
    "SPAIN",
    "ENGLAND",
    "USA",
    "UK",
    "EU",
    "MADE",
    "PRINTED",
    "MANUFACTURED",
    "DISTRIBUTED",
    "RECORDED",
    "MIXED",
    "MASTERED",
    "COPYRIGHT",
    "LIVE",
    "TOUR",
    "STUDIO",
    "RECORDS",
    "MUSIC",
    // Running text: "RECORDED IN 1969", "FOR 1954".
    "IN",
    "FOR",
    "THIS",
    "THE",
    "AND",
    "OF",
    "ON",
    "AT",
    "BY",
    "TO",
    "FROM",
    "WITH",
    "SINCE",
    "YEAR",
    "ALL",
    "RIGHTS",
    "RESERVED",
];

/// US state + ZIP in label addresses: "Los Angeles, CA 90068".
const US_STATES: &[&str] = &[
    "AL", "AK", "AZ", "AR", "CA", "CO", "CT", "DE", "FL", "GA", "HI", "ID", "IL", "IN", "IA", "KS", "KY", "LA", "ME",
    "MD", "MA", "MI", "MN", "MS", "MO", "MT", "NE", "NV", "NH", "NJ", "NM", "NY", "NC", "ND", "OH", "OK", "OR", "PA",
    "RI", "SC", "SD", "TN", "TX", "UT", "VT", "VA", "WA", "WV", "WI", "WY",
];

pub struct CatalogNumberParser;

impl CatalogNumberParser {
    pub fn extract<S: AsRef<str>>(lines: &[S]) -> Vec<CatalogNumber> {
        let mut results = vec![];
        let mut seen = HashSet::new();
        for line in lines {
            let cleaned = line.as_ref().to_uppercase();
            for pattern in PATTERNS.iter() {
                for m in pattern.captures_iter(&cleaned) {
                    let prefix = &m[1];
                    let number = &m[2];
                    if STOP_WORDS.contains(&prefix) || (US_STATES.contains(&prefix) && number.chars().count() == 5) {
                        continue;
                    }
                    let cat = CatalogNumber { prefix: prefix.into(), number: number.into() };
                    if seen.insert(cat.formatted()) {
                        results.push(cat);
                    }
                }
            }
        }
        results
    }

    /// "sicp 1700" == "SICP-1700" == "SICP1700".
    pub fn normalize(s: &str) -> String {
        s.to_uppercase().chars().filter(|c| c.is_alphabetic() || c.is_numeric()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_japanese_and_western_codes() {
        let lines = [
            "SICP-1700  ¥2,500",
            "Made in Japan  UICY 40164",
            "PD 83889",
            "CD 12",
            "TOTAL TIME 45:12",
            "prestige-7142",
            "VDJ~1541",
        ];
        let found: Vec<String> = CatalogNumberParser::extract(&lines).iter().map(|c| c.formatted()).collect();
        for want in ["SICP-1700", "UICY-40164", "PD-83889", "PRESTIGE-7142", "VDJ-1541"] {
            assert!(found.contains(&want.to_string()), "{want} in {found:?}");
        }
        assert!(!found.contains(&"CD-12".to_string()) && !found.contains(&"TIME-45".to_string()));
        assert_eq!(CatalogNumberParser::normalize("sicp 1700"), "SICP1700");
        assert_eq!(CatalogNumberParser::normalize("SICP-1700"), "SICP1700");
    }

    #[test]
    fn printed_words_are_not_catalogs() {
        let found: Vec<String> = CatalogNumberParser::extract(&[
            "EUROPE-2006",
            "MADE IN JAPAN 2006",
            "WPZR-30194",
            "RECORDED IN 1969",
            "HOLLYWOOD, CA 90068",
            "FOR 1954",
            "THIS 2011",
        ])
        .iter()
        .map(|c| c.formatted())
        .collect();
        assert_eq!(found, ["WPZR-30194"]);
    }
}
