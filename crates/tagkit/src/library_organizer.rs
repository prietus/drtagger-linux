//! Moves tagged files into the library layout: root / album folder template
//! / "Disc N" for multi-disc releases / "Multichannel" for SACD multichannel
//! outputs / track file template. Sidecar files (scans, cue, logs) follow
//! when a whole folder moves, and emptied folders are removed.

use crate::tag_set::{TagSet, field};
use crate::tagged_file::TagContainer;
use splitkit::{DEFAULT_ALBUM_FOLDER, DEFAULT_TRACK_FILE, PathTemplate};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrganizerOptions {
    pub album_folder_template: String,
    pub track_file_template: String,
    pub ascii_file_names: bool,
    pub multichannel_subfolder: String,
}

impl Default for OrganizerOptions {
    fn default() -> Self {
        OrganizerOptions {
            album_folder_template: DEFAULT_ALBUM_FOLDER.into(),
            track_file_template: DEFAULT_TRACK_FILE.into(),
            ascii_file_names: true,
            multichannel_subfolder: "Multichannel".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Move {
    pub from: PathBuf,
    pub to: PathBuf,
}

pub const SIDECAR_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "tif", "tiff", "webp", "bmp", "pdf", "cue", "log", "txt", "nfo", "md5", "sfv",
    "accurip", "m3u", "m3u8", "xml",
];

pub struct LibraryOrganizer;

fn first<'a>(tags: &'a TagSet, name: &str) -> Option<&'a str> {
    tags.first(name)
}

/// Swift's `Int(String)`.
fn int(s: Option<&str>) -> Option<i64> {
    let s = s?;
    let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
    (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse().ok()).flatten()
}

fn prefix4(s: &str) -> String {
    s.chars().take(4).collect()
}

fn extension(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default()
}

impl LibraryOrganizer {
    /// Template values from a track's final tags.
    pub fn values(tags: &TagSet, file_extension: &str) -> HashMap<String, String> {
        let album_artist =
            first(tags, field::ALBUM_ARTIST).or(first(tags, field::ARTIST)).unwrap_or("Unknown Artist").to_string();
        let track_total = int(first(tags, field::TRACK_TOTAL)).or(int(first(tags, field::TOTAL_TRACKS))).unwrap_or(0);
        let track = int(first(tags, field::TRACK_NUMBER));
        let width = if track_total >= 100 { 3 } else { 2 };
        let original_year = first(tags, field::ORIGINAL_YEAR)
            .map(str::to_string)
            .unwrap_or_else(|| prefix4(first(tags, field::ORIGINAL_DATE).or(first(tags, field::DATE)).unwrap_or("")));
        let pairs = [
            ("albumartist", album_artist.clone()),
            ("album", first(tags, field::ALBUM).unwrap_or("Unknown Album").to_string()),
            ("year", prefix4(first(tags, field::DATE).unwrap_or(""))),
            ("originalyear", original_year),
            ("artist", first(tags, field::ARTIST).map_or(album_artist, str::to_string)),
            ("title", first(tags, field::TITLE).unwrap_or("").to_string()),
            ("track", track.map_or(String::new(), |t| format!("{t:0width$}"))),
            ("tracktotal", if track_total > 0 { track_total.to_string() } else { String::new() }),
            ("disc", first(tags, field::DISC_NUMBER).unwrap_or("").to_string()),
            ("disctotal", first(tags, field::DISC_TOTAL).or(first(tags, field::TOTAL_DISCS)).unwrap_or("").to_string()),
            ("label", first(tags, field::LABEL).unwrap_or("").to_string()),
            ("catalognumber", first(tags, field::CATALOG_NUMBER).unwrap_or("").to_string()),
            ("media", first(tags, field::MEDIA).unwrap_or("").to_string()),
            ("ext", file_extension.to_string()),
        ];
        pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
    }

    pub fn target(path: &Path, tags: &TagSet, root: &Path, options: &OrganizerOptions) -> PathBuf {
        let ext = extension(path);
        let v = Self::values(tags, &ext);
        let mut folder = PathTemplate::render(&options.album_folder_template, &v, options.ascii_file_names);
        if folder.is_empty() {
            folder = PathTemplate::sanitize_component(&v["albumartist"]);
        }
        if int(Some(&v["disctotal"])).is_some_and(|t| t > 1) && !v["disc"].is_empty() {
            folder += &format!("/Disc {}", v["disc"]);
        }
        let parent_name = path.parent().and_then(Path::file_name).map(|n| n.to_string_lossy().into_owned());
        if parent_name.as_deref() == Some(options.multichannel_subfolder.as_str()) {
            folder += &format!("/{}", PathTemplate::sanitize_component(&options.multichannel_subfolder));
        }
        let mut name = PathTemplate::render(&options.track_file_template, &v, options.ascii_file_names);
        if name.is_empty() {
            name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        }
        let file = if ext.is_empty() { name } else { format!("{name}.{ext}") };
        root.join(folder).join(file)
    }

    /// Moves for every file whose target differs from where it is. Two
    /// tracks rendering to the same name get ".2", ".3"… before the extension.
    pub fn plan(files: &[(PathBuf, TagSet)], root: &Path, options: &OrganizerOptions) -> Vec<Move> {
        let mut used: HashSet<String> = HashSet::new();
        let mut moves = vec![];
        for (path, tags) in files {
            let mut to = Self::target(path, tags, root, options);
            let ext = extension(path);
            let mut n = 2;
            while used.contains(&to.to_string_lossy().to_lowercase()) {
                let stem = Self::target(path, tags, root, options).with_extension("");
                let mut name: OsString = stem.file_name().unwrap_or_default().to_os_string();
                name.push(format!(".{n}"));
                if !ext.is_empty() {
                    name.push(format!(".{ext}"));
                }
                to = stem.with_file_name(name);
                n += 1;
            }
            used.insert(to.to_string_lossy().to_lowercase());
            if librarykit::standardized(&to) != librarykit::standardized(path) {
                moves.push(Move { from: path.clone(), to });
            }
        }
        moves
    }

    /// Performs the moves, then carries the sidecars of every emptied
    /// source folder along and removes it. Returns what actually moved
    /// (audio and sidecars) so callers can update their records.
    pub fn perform(moves: &[Move]) -> std::io::Result<Vec<Move>> {
        let mut done = vec![];
        for m in moves {
            if let Some(parent) = m.to.parent() {
                std::fs::create_dir_all(parent)?;
            }
            if m.to.exists() {
                return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, m.to.display().to_string()));
            }
            move_item(&m.from, &m.to)?;
            done.push(m.clone());
        }
        let sources: BTreeSet<PathBuf> =
            done.iter().filter_map(|m| m.from.parent().map(librarykit::standardized)).collect();
        for source in sources {
            let Ok(entries) = std::fs::read_dir(&source) else { continue };
            let items: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
            if items.iter().any(|p| TagContainer::from_path(p).is_some()) {
                continue;
            }
            let Some(destination) = done
                .iter()
                .find(|m| m.from.parent().map(librarykit::standardized).as_ref() == Some(&source))
                .and_then(|m| m.to.parent().map(Path::to_path_buf))
            else {
                continue;
            };
            for item in items {
                let name = item.file_name().unwrap_or_default().to_os_string();
                if name.to_string_lossy().starts_with('.') {
                    let _ = if item.is_dir() { std::fs::remove_dir_all(&item) } else { std::fs::remove_file(&item) };
                    continue;
                }
                let is_sidecar = SIDECAR_EXTENSIONS.contains(&extension(&item).to_lowercase().as_str());
                if !is_sidecar && !item.is_dir() {
                    continue;
                }
                let to = destination.join(&name);
                if to.exists() {
                    continue;
                }
                if move_item(&item, &to).is_ok() {
                    done.push(Move { from: item, to });
                }
            }
            if std::fs::read_dir(&source).is_ok_and(|mut e| e.next().is_none()) {
                let _ = std::fs::remove_dir(&source);
                // Parent folders emptied by this ("Artist/Album/Disc 1").
                let mut parent = source.parent().map(Path::to_path_buf);
                while let Some(p) = parent {
                    let only_hidden = std::fs::read_dir(&p)
                        .is_ok_and(|e| e.flatten().all(|x| x.file_name().to_string_lossy().starts_with('.')));
                    if !only_hidden || p.components().count() <= 3 {
                        break;
                    }
                    let _ = std::fs::remove_dir_all(&p);
                    parent = p.parent().map(Path::to_path_buf);
                }
            }
        }
        Ok(done)
    }
}

/// rename(2), or copy + delete across file systems (a NAS library).
pub fn move_item(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(libc_exdev()) => {
            copy_all(from, to)?;
            if from.is_dir() { std::fs::remove_dir_all(from) } else { std::fs::remove_file(from) }
        }
        Err(e) => Err(e),
    }
}

const fn libc_exdev() -> i32 {
    18 // EXDEV on Linux
}

fn copy_all(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for e in std::fs::read_dir(from)? {
            let e = e?;
            copy_all(&e.path(), &to.join(e.file_name()))?;
        }
        Ok(())
    } else {
        copy_verified(from, to)
    }
}

fn sha256_of(path: &Path) -> std::io::Result<Vec<u8>> {
    use sha2::Digest;
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = sha2::Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_vec())
}

/// Copies through a hidden temporary file next to the destination, flushed
/// to the device and read back: only a copy whose SHA-256 matches the
/// original takes the final name (the original is deleted after that).
fn copy_verified(from: &Path, to: &Path) -> std::io::Result<()> {
    let name = to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let part = to.with_file_name(format!(".{name}.drtagger-part"));
    let result = (|| {
        std::fs::copy(from, &part)?;
        std::fs::File::open(&part)?.sync_all()?;
        if sha256_of(from)? != sha256_of(&part)? {
            return Err(std::io::Error::other(format!("the copy of {} does not match the original", from.display())));
        }
        std::fs::rename(&part, to)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}

#[cfg(test)]
mod copy_tests {
    use super::*;

    #[test]
    fn verified_copy_lands_under_the_final_name_only() {
        let dir = tempfile::tempdir().unwrap();
        let (from, to) = (dir.path().join("a.flac"), dir.path().join("b.flac"));
        std::fs::write(&from, b"audio bytes").unwrap();
        copy_verified(&from, &to).unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), b"audio bytes");
        assert!(!dir.path().join(".b.flac.drtagger-part").exists());
        assert!(copy_verified(&dir.path().join("missing"), &dir.path().join("c.flac")).is_err());
        assert!(!dir.path().join(".c.flac.drtagger-part").exists());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(pairs: &[(&str, &str)]) -> TagSet {
        let mut t = TagSet::new();
        for (k, v) in pairs {
            t.set(k, Some(v));
        }
        t
    }

    #[test]
    fn target_paths_follow_templates_and_ascii() {
        let root = Path::new("/lib");
        let t = tags(&[
            ("ALBUMARTIST", "Björk"),
            ("ALBUM", "Vespertine"),
            ("DATE", "2001-08-27"),
            ("TRACKNUMBER", "3"),
            ("TRACKTOTAL", "12"),
            ("TITLE", "It’s Not Up to You"),
            ("DISCNUMBER", "2"),
            ("DISCTOTAL", "2"),
        ]);
        let opts = OrganizerOptions::default();
        assert_eq!(
            LibraryOrganizer::target(Path::new("/in/x.flac"), &t, root, &opts),
            Path::new("/lib/Bjork/Vespertine (2001)/Disc 2/03 It's Not Up to You.flac")
        );
        let mc = LibraryOrganizer::target(Path::new("/in/Multichannel/x.dsf"), &t, root, &opts);
        assert!(mc.to_string_lossy().contains("/Disc 2/Multichannel/03 "));
        let dup = LibraryOrganizer::plan(
            &[(PathBuf::from("/in/a.flac"), t.clone()), (PathBuf::from("/in/b.flac"), t.clone())],
            root,
            &opts,
        );
        assert!(dup[1].to.to_string_lossy().ends_with("You.2.flac"));
    }

    #[test]
    fn moves_tracks_and_sidecars_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("in/Some Rip");
        std::fs::create_dir_all(src.join("Scans")).unwrap();
        for f in ["01.wav", "02.wav", "cover.jpg", "rip.log", "Scans/back.jpg", ".DS_Store", "notes.doc"] {
            std::fs::write(src.join(f), b"x").unwrap();
        }
        let root = dir.path().join("lib");
        let files: Vec<(PathBuf, TagSet)> = ["01.wav", "02.wav"]
            .iter()
            .enumerate()
            .map(|(i, f)| {
                (
                    src.join(f),
                    tags(&[
                        ("ALBUMARTIST", "A"),
                        ("ALBUM", "B"),
                        ("TRACKNUMBER", &(i + 1).to_string()),
                        ("TITLE", "T"),
                    ]),
                )
            })
            .collect();
        let moves = LibraryOrganizer::plan(&files, &root, &OrganizerOptions::default());
        let done = LibraryOrganizer::perform(&moves).unwrap();
        let album = root.join("A/B");
        for f in ["01 T.wav", "02 T.wav", "cover.jpg", "rip.log", "Scans/back.jpg"] {
            assert!(album.join(f).exists(), "{f}");
        }
        assert!(done.len() >= 5);
        assert!(src.join("notes.doc").exists(), "unknown files stay, so the folder stays");
    }
}
