//! What the library already holds, one entry per album folder, read from
//! the tags of one track per folder (no audio is read). Used to flag
//! releases that are already in the library.

use crate::tag_set::{TagSet, field};
use crate::tagged_file::TaggedFile;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryIndexEntry {
    pub folder: PathBuf,
    /// "mb:<mbid>", "discogs:<id>".
    pub release_ids: Vec<String>,
    pub catalog_numbers: Vec<String>,
    pub barcode: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    /// Folder modification time, nanoseconds since the epoch.
    pub modified: i128,
}

impl LibraryIndexEntry {
    pub fn new(folder: &Path, tags: &TagSet, modified: i128) -> Self {
        let mut ids = vec![];
        if let Some(mb) = tags.first(field::MB_ALBUM_ID).map(str::to_lowercase).filter(|s| !s.is_empty()) {
            ids.push(format!("mb:{mb}"));
        }
        if let Some(d) = tags.first(field::DISCOGS_RELEASE_ID).filter(|s| !s.is_empty()) {
            ids.push(format!("discogs:{d}"));
        }
        LibraryIndexEntry {
            folder: librarykit::standardized(folder),
            release_ids: ids,
            catalog_numbers: tags.get(field::CATALOG_NUMBER).to_vec(),
            barcode: tags.first(field::BARCODE).map(|b| b.chars().filter(|c| c.is_numeric()).collect()),
            artist: tags.first(field::ALBUM_ARTIST).or(tags.first(field::ARTIST)).map(str::to_string),
            album: tags.first(field::ALBUM).map(str::to_string),
            modified,
        }
    }
}

const AUDIO_EXTENSIONS: &[&str] = &["flac", "dsf", "dff", "wav", "aif", "aiff", "ape", "wv", "tta", "m4a"];

fn modified(path: &Path) -> i128 {
    std::fs::metadata(path).map_or(i128::MIN, |m| m.mtime() as i128 * 1_000_000_000 + m.mtime_nsec() as i128)
}

fn hidden(path: &Path) -> bool {
    path.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'))
}

fn folders(root: &Path, out: &mut Vec<PathBuf>) {
    out.push(root.to_path_buf());
    let Ok(entries) = std::fs::read_dir(root) else { return };
    let mut subs: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .filter(|p| !hidden(p))
        .collect();
    subs.sort();
    for sub in subs {
        folders(&sub, out);
    }
}

pub struct LibraryIndexer;

impl LibraryIndexer {
    /// Walks `root`; folders unchanged since `previous` (same modification
    /// time) are reused without reading anything.
    pub fn scan(
        root: &Path,
        previous: &BTreeMap<PathBuf, LibraryIndexEntry>,
        progress: &mut dyn FnMut(usize, &str),
    ) -> BTreeMap<PathBuf, LibraryIndexEntry> {
        let mut list = vec![];
        folders(root, &mut list);
        let mut result = BTreeMap::new();
        let mut visited = 0;
        for folder in list {
            visited += 1;
            let key = librarykit::standardized(&folder);
            let modified = modified(&folder);
            if let Some(old) = previous.get(&key)
                && old.modified == modified
            {
                result.insert(key, old.clone());
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&folder) else { continue };
            let mut audio: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| !hidden(p))
                .filter(|p| {
                    p.extension()
                        .is_some_and(|e| AUDIO_EXTENSIONS.contains(&e.to_string_lossy().to_lowercase().as_str()))
                })
                .collect();
            audio.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
            let Some(first) = audio.first() else { continue };
            let Ok(tags) = TaggedFile::read_tags(first) else { continue };
            result.insert(key, LibraryIndexEntry::new(&folder, &tags, modified));
            if visited % 25 == 0 {
                progress(visited, &folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            }
        }
        progress(visited, "");
        result
    }

    /// Entries holding the same release, outside `excluding` folders: by
    /// release ID, else by barcode plus catalog number.
    pub fn duplicates(
        index: &BTreeMap<PathBuf, LibraryIndexEntry>,
        release_key: Option<&str>,
        barcode: Option<&str>,
        catalog_numbers: &[String],
        excluding: &HashSet<PathBuf>,
    ) -> Vec<LibraryIndexEntry> {
        let normalize = |s: &str| s.to_uppercase().chars().filter(|c| c.is_alphanumeric()).collect::<String>();
        let cats: HashSet<String> = catalog_numbers.iter().map(|c| normalize(c)).filter(|c| !c.is_empty()).collect();
        let code: Option<String> = barcode.map(|b| b.chars().filter(|c| c.is_numeric()).collect());
        let mut out: Vec<LibraryIndexEntry> = index
            .values()
            .filter(|e| {
                if excluding.contains(&e.folder) {
                    return false;
                }
                if let Some(key) = release_key
                    && e.release_ids.iter().any(|r| r == key)
                {
                    return true;
                }
                (e.release_ids.is_empty() || release_key.is_none())
                    && code.as_ref().is_some_and(|c| !c.is_empty() && e.barcode.as_ref() == Some(c))
                    && !cats.is_empty()
                    && e.catalog_numbers.iter().any(|c| cats.contains(&normalize(c)))
            })
            .cloned()
            .collect();
        out.sort_by(|a, b| a.folder.cmp(&b.folder));
        out
    }
}
