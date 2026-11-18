//! Finder-like ordering ("2 - b" before "10 - a"), the Rust counterpart of
//! Foundation's `localizedStandardCompare`: locale collation, numeric
//! substrings compared by value, case-insensitive with a deterministic
//! tie-break.

use icu_collator::options::{CollatorOptions, Strength};
use icu_collator::preferences::CollationNumericOrdering;
use icu_collator::{Collator, CollatorBorrowed, CollatorPreferences};
use icu_locale_core::Locale;
use std::cmp::Ordering;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

struct Collators {
    primary: CollatorBorrowed<'static>,
    tertiary: CollatorBorrowed<'static>,
}

/// The POSIX locale of the process (`LC_ALL`, `LC_COLLATE`, `LANG`), e.g.
/// "es_ES.UTF-8" → "es-ES"; root collation when unset or unknown.
fn system_locale() -> Locale {
    let raw = ["LC_ALL", "LC_COLLATE", "LANG"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .find(|v| !v.is_empty())
        .unwrap_or_default();
    let tag = raw.split(['.', '@']).next().unwrap_or("").replace('_', "-");
    if tag.is_empty() || tag == "C" || tag == "POSIX" {
        return Locale::UNKNOWN;
    }
    tag.parse().unwrap_or(Locale::UNKNOWN)
}

static COLLATORS: LazyLock<Collators> = LazyLock::new(|| {
    let locale = system_locale();
    let make = |strength| {
        let mut prefs = CollatorPreferences::from(&locale);
        prefs.numeric_ordering = Some(CollationNumericOrdering::True);
        let mut options = CollatorOptions::default();
        options.strength = Some(strength);
        Collator::try_new(prefs, options)
            .or_else(|_| {
                let mut prefs = CollatorPreferences::default();
                prefs.numeric_ordering = Some(CollationNumericOrdering::True);
                Collator::try_new(prefs, options)
            })
            .expect("root collation data is compiled in")
    };
    Collators { primary: make(Strength::Secondary), tertiary: make(Strength::Tertiary) }
});

/// Compares two names the way the Finder sorts them.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let c = &*COLLATORS;
    c.primary.compare(a, b).then_with(|| c.tertiary.compare(a, b)).then_with(|| a.cmp(b))
}

fn name_of(path: &Path) -> &OsStr {
    path.file_name().unwrap_or(path.as_os_str())
}

fn os_cmp(a: &OsStr, b: &OsStr) -> Ordering {
    match (a.to_str(), b.to_str()) {
        (Some(x), Some(y)) => natural_cmp(x, y),
        _ => natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()).then_with(|| a.as_bytes().cmp(b.as_bytes())),
    }
}

/// Sorts paths by their last component, Finder-like.
pub fn naturally_sorted(mut paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths.sort_by(|a, b| os_cmp(name_of(a), name_of(b)));
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_compare_by_value() {
        let sorted = naturally_sorted(vec!["10 - a.flac".into(), "2 - b.flac".into(), "1 - c.flac".into()]);
        let names: Vec<_> = sorted.iter().map(|p| p.to_str().unwrap()).collect();
        assert_eq!(names, ["1 - c.flac", "2 - b.flac", "10 - a.flac"]);
    }

    #[test]
    fn case_is_secondary() {
        assert_eq!(natural_cmp("abc", "ABD"), Ordering::Less);
        assert_eq!(natural_cmp("Disc 2", "disc 10"), Ordering::Less);
    }
}
