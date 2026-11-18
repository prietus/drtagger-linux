//! The library's albums by release (the Mac app's LibraryIndexService),
//! kept on disk between launches and refreshed incrementally: unchanged
//! folders are not read again. A batch looks releases up here so it never
//! imports one the library already holds.

use providerkit::{Candidate, Source};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tagkit::{LibraryIndexEntry, LibraryIndexer, TagContainer, TaggedFile};

#[derive(Default, Serialize, Deserialize)]
struct Stored {
    root: PathBuf,
    entries: BTreeMap<PathBuf, LibraryIndexEntry>,
}

#[derive(Default)]
pub struct LibraryIndex {
    stored: Mutex<Stored>,
    building: Mutex<bool>,
}

fn store_path() -> PathBuf {
    crate::paths::data_dir().join("LibraryIndex.json")
}

/// "mb:<mbid>", "discogs:<id>", else the candidate's own id.
pub fn release_key(c: &Candidate) -> String {
    match c.source {
        Source::Musicbrainz => format!("mb:{}", c.provider_id.to_lowercase()),
        Source::Discogs => format!("discogs:{}", c.provider_id),
        _ => c.id(),
    }
}

impl LibraryIndex {
    /// The index saved by an earlier session, if any.
    pub fn load() -> LibraryIndex {
        let stored = std::fs::read(store_path())
            .ok()
            .and_then(|data| serde_json::from_slice::<Stored>(&data).ok())
            .unwrap_or_default();
        LibraryIndex { stored: Mutex::new(stored), building: Mutex::new(false) }
    }

    pub fn len(&self) -> usize {
        self.stored.lock().unwrap().entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Builds or refreshes the index of `root`. `progress` gets the number
    /// of folders visited so far.
    pub fn ensure(&self, root: &Path, progress: &mut dyn FnMut(usize)) {
        {
            let mut building = self.building.lock().unwrap();
            if *building {
                return;
            }
            *building = true;
        }
        let root = librarykit::standardized(root);
        let previous = {
            let stored = self.stored.lock().unwrap();
            if stored.root == root { stored.entries.clone() } else { BTreeMap::new() }
        };
        let entries = LibraryIndexer::scan(&root, &previous, &mut |count, _| progress(count));
        {
            let mut stored = self.stored.lock().unwrap();
            stored.root = root;
            stored.entries = entries;
        }
        *self.building.lock().unwrap() = false;
        self.save();
    }

    /// Records the albums written by an Apply so the next ones see them.
    pub fn update(&self, folders: &[PathBuf]) {
        let found: Vec<LibraryIndexEntry> = folders
            .iter()
            .filter_map(|folder| {
                let mut files: Vec<PathBuf> = std::fs::read_dir(folder)
                    .ok()?
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
                    .filter(|p| TagContainer::from_path(p).is_some())
                    .collect();
                files.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
                let tags = TaggedFile::read_tags(files.first()?).ok()?;
                let modified = std::fs::metadata(folder)
                    .map(|m| m.mtime() as i128 * 1_000_000_000 + m.mtime_nsec() as i128)
                    .unwrap_or(0);
                Some(LibraryIndexEntry::new(folder, &tags, modified))
            })
            .collect();
        if found.is_empty() {
            return;
        }
        {
            let mut stored = self.stored.lock().unwrap();
            for e in found {
                stored.entries.insert(e.folder.clone(), e);
            }
        }
        self.save();
    }

    /// Folders of the library holding the same release as `c`, outside
    /// `excluding`.
    pub fn duplicates(&self, c: &Candidate, excluding: &HashSet<PathBuf>) -> Vec<LibraryIndexEntry> {
        let key = matches!(c.source, Source::Musicbrainz | Source::Discogs).then(|| release_key(c));
        let stored = self.stored.lock().unwrap();
        LibraryIndexer::duplicates(
            &stored.entries,
            key.as_deref(),
            c.barcode.as_deref(),
            &c.catalog_numbers(),
            excluding,
        )
    }

    fn save(&self) {
        let stored = self.stored.lock().unwrap();
        if stored.root.as_os_str().is_empty() {
            return;
        }
        let path = store_path();
        let Ok(data) = serde_json::to_vec(&*stored) else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, data).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}
