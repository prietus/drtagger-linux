use fancy_regex::Regex as FancyRegex;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::LazyLock;

/// Lossless audio containers the app understands. Detection is by file
/// extension only; codec verification (an .m4a that is really AAC) happens
/// later with ffprobe.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioFormat {
    Flac,
    Ape,
    Wavpack,
    Tta,
    Wav,
    Aiff,
    Alac,
    Dsf,
    Dff,
}

impl AudioFormat {
    pub const ALL: [AudioFormat; 9] = [
        AudioFormat::Flac,
        AudioFormat::Ape,
        AudioFormat::Wavpack,
        AudioFormat::Tta,
        AudioFormat::Wav,
        AudioFormat::Aiff,
        AudioFormat::Alac,
        AudioFormat::Dsf,
        AudioFormat::Dff,
    ];

    pub fn from_path(path: &Path) -> Option<AudioFormat> {
        Self::from_extension(&extension(path))
    }

    pub fn from_extension(ext: &str) -> Option<AudioFormat> {
        Some(match ext.to_lowercase().as_str() {
            "flac" => AudioFormat::Flac,
            "ape" => AudioFormat::Ape,
            "wv" => AudioFormat::Wavpack,
            "tta" => AudioFormat::Tta,
            "wav" | "wave" => AudioFormat::Wav,
            "aiff" | "aif" | "aifc" => AudioFormat::Aiff,
            "m4a" => AudioFormat::Alac,
            "dsf" => AudioFormat::Dsf,
            "dff" => AudioFormat::Dff,
            _ => return None,
        })
    }

    /// The Swift enum's raw value ("flac", "wavpack", …).
    pub fn raw_value(self) -> &'static str {
        match self {
            AudioFormat::Flac => "flac",
            AudioFormat::Ape => "ape",
            AudioFormat::Wavpack => "wavpack",
            AudioFormat::Tta => "tta",
            AudioFormat::Wav => "wav",
            AudioFormat::Aiff => "aiff",
            AudioFormat::Alac => "alac",
            AudioFormat::Dsf => "dsf",
            AudioFormat::Dff => "dff",
        }
    }

    pub fn is_dsd(self) -> bool {
        matches!(self, AudioFormat::Dsf | AudioFormat::Dff)
    }

    pub fn display_name(self) -> &'static str {
        match self {
            AudioFormat::Flac => "FLAC",
            AudioFormat::Ape => "Monkey's Audio",
            AudioFormat::Wavpack => "WavPack",
            AudioFormat::Tta => "TTA",
            AudioFormat::Wav => "WAV",
            AudioFormat::Aiff => "AIFF",
            AudioFormat::Alac => "ALAC",
            AudioFormat::Dsf => "DSF",
            AudioFormat::Dff => "DFF",
        }
    }
}

/// The last component's extension as text (lossy for non-UTF-8 names).
pub(crate) fn extension(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default()
}

/// File-name rules shared by the scanner.
pub struct FileRules;

const IMAGE_EXTENSIONS: [&str; 9] = ["jpg", "jpeg", "png", "gif", "bmp", "tif", "tiff", "webp", "heic"];

/// Folder names that hold scans rather than audio. They are not skipped
/// (artwork inside them is collected) but never form an album.
const ARTWORK_FOLDER_NAMES: [&str; 9] =
    ["scans", "scan", "artwork", "art", "covers", "cover", "booklet", "images", "pics"];

// "Box Set (Disc 2).cue", "Cancioneros (disc 1-3).iso", "CD2", "Disc 2 of 4":
// the disc number anywhere in a file or folder name, with the total when
// the name states it. Volumes are not discs ("Cantatas Vol. 40" is an album).
static DISC_TOKEN: LazyLock<FancyRegex> = LazyLock::new(|| {
    FancyRegex::new(
        r"(?i)(?:^|[\s\(\[_\-.])(?:cd|disc|disco|disk|dvd)\s*[-_.#]?\s*(\d{1,2})(?:\s*(?:of|de|-|/)\s*(\d{1,2}))?(?=$|[\s\)\]_\-.,])",
    )
    .unwrap()
});
static FOLDER_DISC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?:cd|disc|disco|disk|dvd)\s*[-_.#]?\s*(\d{1,2})\b").unwrap());
static EXT_SHAPE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9]{2,5}$").unwrap());
static EMPTY_BRACKETS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\(\s*\)|\[\s*\]").unwrap());
static MULTI_SPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s{2,}").unwrap());

/// NSString's pathExtension / deletingPathExtension on a bare name: the
/// text after the last dot, unless the dot starts the name.
fn split_extension(name: &str) -> (&str, Option<&str>) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], Some(&name[i + 1..])),
        _ => (name, None),
    }
}

/// Swift's `Character.isNumber`, close enough for file names.
pub(crate) fn is_number(c: char) -> bool {
    c.is_numeric()
}

impl FileRules {
    /// AppleDouble resource forks, Finder metadata, Synology/QNAP index
    /// folders and our own NAS markers.
    pub fn is_junk(name: &str) -> bool {
        if name.starts_with('.') {
            return true;
        }
        let lower = name.to_lowercase();
        if lower == "thumbs.db" || lower == "desktop.ini" || lower == "@eadir" {
            return true;
        }
        lower.ends_with(".dr_done") || lower.ends_with(".part") || lower.ends_with(".tmp")
    }

    pub fn is_audio(path: &Path) -> bool {
        AudioFormat::from_path(path).is_some()
    }

    pub fn is_artwork(path: &Path) -> bool {
        IMAGE_EXTENSIONS.contains(&extension(path).to_lowercase().as_str())
    }

    pub fn is_artwork_folder(name: &str) -> bool {
        ARTWORK_FOLDER_NAMES.contains(&name.to_lowercase().as_str())
    }

    pub fn is_cue(path: &Path) -> bool {
        extension(path).to_lowercase() == "cue"
    }

    pub fn is_iso(path: &Path) -> bool {
        extension(path).to_lowercase() == "iso"
    }

    pub fn is_log(path: &Path) -> bool {
        matches!(extension(path).to_lowercase().as_str(), "log" | "accurip" | "txt" | "m3u" | "m3u8")
    }

    /// Raw CD images referenced by a CUE (FILE "x.bin" BINARY).
    pub fn is_raw_image(path: &Path) -> bool {
        matches!(extension(path).to_lowercase().as_str(), "bin" | "img" | "iso")
    }

    /// "Box (Disc 2).cue" → "Box (Disc 2)", but "Cantatas Vol. 40" stays whole.
    pub fn without_file_extension(name: &str) -> &str {
        match split_extension(name) {
            (base, Some(ext)) if EXT_SHAPE.is_match(ext) && ext.chars().any(char::is_alphabetic) => base,
            _ => name,
        }
    }

    /// The disc number anywhere in a file or folder name, with the total
    /// when the name states it.
    pub fn disc_number_from_file_name(name: &str) -> Option<(u32, Option<u32>)> {
        let base = Self::without_file_extension(name);
        let caps = DISC_TOKEN.captures(base).ok()??;
        let number = caps.get(1)?.as_str().parse().ok()?;
        let total = caps.get(2).and_then(|m| m.as_str().parse().ok());
        Some((number, total))
    }

    /// The name without its disc token, so siblings of one set compare
    /// equal: "Led Zeppelin - Box Set (Disc 2)" → "Led Zeppelin - Box Set".
    pub fn stripping_disc_token(name: &str) -> String {
        let base = Self::without_file_extension(name);
        let Ok(Some(m)) = DISC_TOKEN.find(base) else { return base.to_string() };
        let (start, mut end) = (m.start(), m.end());
        // The token's own brackets go with it: "(Disc 2)" → "".
        if matches!(base[start..].chars().next(), Some('(' | '[')) {
            let rest = &base[end..];
            let trimmed = rest.trim_start_matches(' ');
            if trimmed.starts_with(')') || trimmed.starts_with(']') {
                end += rest.len() - trimmed.len() + 1;
            }
        }
        let t = format!("{}{}", &base[..start], &base[end..]);
        let t = EMPTY_BRACKETS.replace_all(&t, "");
        let t = MULTI_SPACE.replace_all(&t, " ");
        trim_set(&t, |c| c.is_whitespace() && c != '\n' || "-_.,".contains(c)).to_string()
    }

    /// "CD1", "Disc 2", "Disco 3 - Live", "disk_4" … at the start of a
    /// folder name.
    pub fn disc_number_from_folder_name(name: &str) -> Option<u32> {
        FOLDER_DISC.captures(name)?.get(1)?.as_str().parse().ok()
    }
}

pub(crate) fn trim_set(s: &str, f: impl Fn(char) -> bool + Copy) -> &str {
    s.trim_matches(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disc_tokens() {
        assert_eq!(FileRules::disc_number_from_file_name("Box Set (Disc 2).cue"), Some((2, None)));
        assert_eq!(FileRules::disc_number_from_file_name("Cancioneros (disc 1-3).iso"), Some((1, Some(3))));
        assert_eq!(FileRules::disc_number_from_file_name("CD2"), Some((2, None)));
        assert_eq!(FileRules::disc_number_from_file_name("Disc 2 of 4"), Some((2, Some(4))));
        assert_eq!(FileRules::disc_number_from_file_name("Cantatas Vol. 40"), None);
        assert_eq!(FileRules::stripping_disc_token("Led Zeppelin - Box Set (Disc 2)"), "Led Zeppelin - Box Set");
        assert_eq!(FileRules::stripping_disc_token("Box Set [CD 1].flac"), "Box Set");
        assert_eq!(FileRules::without_file_extension("Cantatas Vol. 40"), "Cantatas Vol. 40");
        assert_eq!(FileRules::disc_number_from_folder_name("Disco 3 - Live"), Some(3));
        assert_eq!(FileRules::disc_number_from_folder_name("disk_4"), Some(4));
        assert_eq!(FileRules::disc_number_from_folder_name("Scans"), None);
    }

    #[test]
    fn junk() {
        assert!(FileRules::is_junk("._01.flac"));
        assert!(FileRules::is_junk("@eaDir"));
        assert!(FileRules::is_junk("album.dr_done"));
        assert!(!FileRules::is_junk("01.flac"));
    }
}
