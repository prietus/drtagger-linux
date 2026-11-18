//! The rip inbox: a local folder the rippers write into. Every top-level
//! entry (an album folder, a box with disc folders, an ISO) is one rip. A
//! rip is finished when
//! - a ripper log is there (EAC, XLD, whipper, CUERipper write it when
//!   they are done) and nothing has changed for a short settle time, or
//! - without a log, nothing has changed for the quiet time; for an ISO,
//!   when it has stopped growing for that long.
//!
//! Finished rips are added to the queue and handed to the batch (import:
//! tag locally, then a verified move into the library). What was handed
//! over is remembered with its fingerprint, so a rip comes back only when
//! it changes (a re-rip into the same folder).

use crate::store::Store;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Where split and extracted tracks are written before they are tagged
/// and moved into the library (hidden: never taken for a rip).
pub const STAGING: &str = ".drtagger-staging";

/// How long a rip with a complete log must stay unchanged.
pub const SETTLE: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ripper {
    Eac,
    Xld,
    Whipper,
    CueRipper,
}

/// Text of a log whatever its encoding: EAC writes UTF-16 LE with a BOM,
/// the others UTF-8; old EAC logs are in the system code page (the ASCII
/// markers survive a lossy read).
fn log_text(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let units: Vec<u16> = bytes[2..].as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        return String::from_utf16_lossy(&units);
    }
    String::from_utf8_lossy(bytes).into_owned()
}

/// The ripper that wrote a log, if it is one.
pub fn ripper(bytes: &[u8]) -> Option<Ripper> {
    let text = log_text(&bytes[..bytes.len().min(4096)]);
    if text.contains("Exact Audio Copy") || text.contains("EAC extraction logfile") {
        Some(Ripper::Eac)
    } else if text.contains("X Lossless Decoder") {
        Some(Ripper::Xld)
    } else if text.contains("whipper") {
        Some(Ripper::Whipper)
    } else if text.contains("CUERipper") {
        Some(Ripper::CueRipper)
    } else {
        None
    }
}

/// A ripper's log with the end of its report. EAC logs in other languages
/// end in translated words; their checksum line, when present, is ASCII.
/// A recognised log without a known end still counts once it is settled:
/// the rippers write their log when the rip is over.
pub fn log_has_end(bytes: &[u8]) -> bool {
    let text = log_text(bytes);
    ["End of status report", "==== Log checksum", "SHA-256 hash:", "Conclusive status report"]
        .iter()
        .any(|m| text.contains(m))
}

/// What a poll sees of one rip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub fingerprint: u64,
    /// A ripper log is there.
    pub has_log: bool,
}

fn hidden(path: &Path) -> bool {
    path.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'))
}

/// Names, sizes and modification times of everything under `entry` (no
/// file is read except logs).
pub fn snapshot(entry: &Path) -> Option<Snapshot> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut has_log = false;
    let mut stack = vec![entry.to_path_buf()];
    let mut any = false;
    while let Some(path) = stack.pop() {
        let meta = std::fs::symlink_metadata(&path).ok()?;
        path.hash(&mut hasher);
        meta.len().hash(&mut hasher);
        meta.mtime().hash(&mut hasher);
        meta.mtime_nsec().hash(&mut hasher);
        if meta.is_dir() {
            let mut children: Vec<PathBuf> =
                std::fs::read_dir(&path).ok()?.flatten().map(|e| e.path()).filter(|p| !hidden(p)).collect();
            children.sort();
            stack.extend(children);
        } else {
            any = true;
            let is_log = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("log"));
            if is_log
                && meta.len() < 4 << 20
                && let Ok(bytes) = std::fs::read(&path)
                && ripper(&bytes).is_some()
            {
                has_log = true;
            }
        }
    }
    any.then(|| Snapshot { fingerprint: hasher.finish(), has_log })
}

#[derive(Clone, Debug)]
struct Watched {
    snapshot: Snapshot,
    since: Instant,
}

/// Rips handed to the queue, by path, with the fingerprint they had.
#[derive(Default, Serialize, Deserialize)]
struct Seen {
    entries: BTreeMap<PathBuf, u64>,
}

fn seen_path() -> PathBuf {
    crate::paths::data_dir().join("inbox-seen.json")
}

pub struct Inbox {
    pub folder: PathBuf,
    pub quiet: Duration,
    watched: HashMap<PathBuf, Watched>,
    seen: Seen,
}

impl Inbox {
    pub fn new(folder: &Path, quiet: Duration) -> Inbox {
        let seen = std::fs::read(seen_path()).ok().and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default();
        Inbox { folder: folder.to_path_buf(), quiet, watched: HashMap::new(), seen }
    }

    /// Looks at every rip; returns those finished since the last poll.
    pub fn poll(&mut self, now: Instant) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(&self.folder) else { return vec![] };
        let entries: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| !hidden(p)).collect();
        self.watched.retain(|p, _| entries.contains(p));
        let mut ready = vec![];
        for entry in entries {
            let Some(snapshot) = snapshot(&entry) else { continue };
            let watched =
                self.watched.entry(entry.clone()).or_insert(Watched { snapshot: snapshot.clone(), since: now });
            if watched.snapshot != snapshot {
                *watched = Watched { snapshot, since: now };
                continue;
            }
            if self.seen.entries.get(&entry) == Some(&watched.snapshot.fingerprint) {
                continue;
            }
            let wait = if watched.snapshot.has_log { SETTLE.min(self.quiet) } else { self.quiet };
            if now.duration_since(watched.since) >= wait {
                ready.push(entry);
            }
        }
        ready.sort();
        ready
    }

    /// Remembers that `entry` went to the queue as it is now.
    pub fn mark_seen(&mut self, entry: &Path) {
        if let Some(w) = self.watched.get(entry) {
            self.seen.entries.insert(entry.to_path_buf(), w.snapshot.fingerprint);
        }
        // Rips no longer in the inbox (moved into the library) are forgotten.
        let present: Vec<PathBuf> = self.seen.entries.keys().filter(|p| p.exists()).cloned().collect();
        self.seen.entries.retain(|p, _| present.contains(p));
        if let Ok(data) = serde_json::to_vec(&self.seen) {
            let path = seen_path();
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(path, data);
        }
    }
}

/// Scans a finished rip into the queue (with release-set grouping) and
/// returns the leaders of what it holds, for the batch.
pub fn add_to_queue(store: &mut Store, entry: &Path) -> Result<Vec<PathBuf>> {
    let result = librarykit::LibraryScanner::new().scan(&[entry.to_path_buf()]);
    store.upsert(&result)?;
    crate::sets::after_scan(store, &result)?;
    let all = store.all()?;
    let mut leaders = vec![];
    for album in &result.albums {
        let Some(r) = all.iter().find(|r| r.path == album.id()) else { continue };
        let first = crate::sets::members_in(&all, r).first().map(|m| m.path.clone());
        if let Some(p) = first
            && !leaders.contains(&p)
        {
            leaders.push(p);
        }
    }
    Ok(leaders)
}

/// After a CUE-image rip's tracks went into the library: its log, scans
/// and other sidecars follow them into `album_folder`, the CUE is copied
/// there, and when the image is gone (trashed after a verified split) the
/// CUE and the emptied rip folder go too. An image still in the inbox
/// keeps its CUE.
pub fn carry_leftovers(rip_folder: &Path, album_folder: &Path) -> Result<()> {
    let ext = |p: &Path| p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let mut cues = vec![];
    for entry in std::fs::read_dir(rip_folder)?.flatten() {
        let path = entry.path();
        if hidden(&path) {
            continue;
        }
        let to = album_folder.join(entry.file_name());
        let e = ext(&path);
        if e == "cue" {
            if !to.exists() {
                std::fs::copy(&path, &to)?;
            }
            cues.push(path);
        } else if (path.is_dir() || tagkit::SIDECAR_EXTENSIONS.contains(&e.as_str())) && !to.exists() {
            tagkit::move_item(&path, &to)?;
        }
    }
    let audio_left = std::fs::read_dir(rip_folder)?
        .flatten()
        .map(|e| e.path())
        .any(|p| p.is_file() && !hidden(&p) && !tagkit::SIDECAR_EXTENSIONS.contains(&ext(&p).as_str()));
    if !audio_left {
        for cue in cues {
            let _ = std::fs::remove_file(cue);
        }
        if std::fs::read_dir(rip_folder)?.flatten().all(|e| hidden(&e.path())) {
            std::fs::remove_dir_all(rip_folder)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leftovers_follow_the_tracks() {
        let dir = tempfile::tempdir().unwrap();
        let (rip, album) = (dir.path().join("rip"), dir.path().join("lib/Album"));
        std::fs::create_dir_all(rip.join("Scans")).unwrap();
        std::fs::create_dir_all(&album).unwrap();
        for f in ["a.flac", "a.cue", "a.log", "Scans/obi.jpg"] {
            std::fs::write(rip.join(f), f).unwrap();
        }
        carry_leftovers(&rip, &album).unwrap();
        assert!(album.join("a.log").exists() && album.join("a.cue").exists() && album.join("Scans/obi.jpg").exists());
        assert!(rip.join("a.flac").exists() && rip.join("a.cue").exists(), "the image keeps its CUE");
        std::fs::remove_file(rip.join("a.flac")).unwrap();
        carry_leftovers(&rip, &album).unwrap();
        assert!(!rip.exists(), "an emptied rip folder goes");
    }

    #[test]
    fn logs_are_recognised_with_their_end() {
        let xld = b"X Lossless Decoder version 20250302 (157.2)\n\nXLD extraction logfile\n...\nNo errors occurred\nEnd of status report\n";
        assert_eq!(ripper(xld), Some(Ripper::Xld));
        assert!(log_has_end(xld));
        let mut eac: Vec<u8> = vec![0xFF, 0xFE];
        for u in "Exact Audio Copy V1.6\r\n\r\nEAC extraction logfile\r\n==== Log checksum ABC ====\r\n".encode_utf16()
        {
            eac.extend(u.to_le_bytes());
        }
        assert_eq!(ripper(&eac), Some(Ripper::Eac));
        assert!(log_has_end(&eac));
        let whipper = b"Log created by: whipper 0.10.0\nRipping phase information:\n";
        assert_eq!(ripper(whipper), Some(Ripper::Whipper));
        assert!(!log_has_end(whipper));
        assert_eq!(ripper(b"AUDIOCHECKER v2.0 beta"), None);
    }

    #[test]
    fn a_rip_is_ready_after_its_quiet_time_and_once() {
        let dir = tempfile::tempdir().unwrap();
        let rip = dir.path().join("Album");
        std::fs::create_dir(&rip).unwrap();
        std::fs::write(rip.join("01.flac"), b"x").unwrap();
        std::fs::create_dir(dir.path().join(STAGING)).unwrap();
        let mut inbox = Inbox {
            folder: dir.path().into(),
            quiet: Duration::from_secs(600),
            watched: HashMap::new(),
            seen: Seen::default(),
        };
        let t0 = Instant::now();
        assert!(inbox.poll(t0).is_empty());
        assert!(inbox.poll(t0 + Duration::from_secs(300)).is_empty(), "still within the quiet time");
        // A ripper log shortens the wait to the settle time.
        std::fs::write(rip.join("Album.log"), b"X Lossless Decoder\nEnd of status report\n").unwrap();
        let t1 = t0 + Duration::from_secs(301);
        assert!(inbox.poll(t1).is_empty(), "changed: the clock restarts");
        assert_eq!(inbox.poll(t1 + SETTLE), vec![rip.clone()]);
        inbox.seen.entries.insert(rip.clone(), inbox.watched[&rip].snapshot.fingerprint);
        assert!(inbox.poll(t1 + SETTLE * 2).is_empty(), "handed over once");
    }
}
