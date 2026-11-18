//! Release sets: records that are discs of one release (the Mac app's
//! LibraryController grouping). After a scan new records join a set already
//! in the queue or form one with their siblings ("Box (Disc 1)", "Box
//! (Disc 2)", the discs of a SACD album set), and folders whose files name
//! the same release become one too. Grouping by hand, ungrouping and disc
//! positions are here as well.

use crate::store::{AlbumRecord, AlbumState, Store};
use anyhow::Result;
use librarykit::AlbumKind;
use regex::Regex;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use tagkit::{TaggedFile, field};

/// The discs of `record`'s set among `all`, in disc order (itself alone
/// when it is in no set).
pub fn members_in(all: &[AlbumRecord], record: &AlbumRecord) -> Vec<AlbumRecord> {
    let Some(id) = &record.set_id else { return vec![record.clone()] };
    let mut members: Vec<AlbumRecord> = all.iter().filter(|r| r.set_id.as_ref() == Some(id)).cloned().collect();
    members.sort_by_key(|r| (r.set_position.unwrap_or(0), r.added_at));
    members
}

pub fn members(store: &Store, record: &AlbumRecord) -> Result<Vec<AlbumRecord>> {
    if record.set_id.is_none() {
        return Ok(vec![record.clone()]);
    }
    Ok(members_in(&store.all()?, record))
}

/// Records in the same folder that carry a disc number and are not yet in
/// a set: what "Group with folder siblings" would join.
pub fn sibling_candidates(all: &[AlbumRecord], record: &AlbumRecord) -> Vec<AlbumRecord> {
    let Some(key) = set_key(record) else { return vec![] };
    all.iter()
        .filter(|r| r.path != record.path && r.set_id.is_none() && set_key(r).as_ref() == Some(&key))
        .cloned()
        .collect()
}

/// What makes two records discs of one release. SACDs carry the box title
/// and catalog in their master TOC, so their key ignores folders; other
/// albums need the same parent folder and the same name minus its disc
/// token ("Box Set (Disc 1)" / "Box Set (Disc 2)").
fn set_key(record: &AlbumRecord) -> Option<String> {
    let detected = record.detected()?;
    detected.disc_position()?;
    if let Some(sacd) = &detected.sacd
        && sacd.album_set_size > 1
    {
        // The album catalog (often the box barcode) is shared by every
        // disc; titles may carry "Disc N of M", so they come second.
        let catalog = trim_spaces(&sacd.album_catalog_number.to_lowercase()).to_string();
        if !catalog.is_empty() {
            return Some(format!("sacd|cat|{catalog}|{}", sacd.album_set_size));
        }
        let title = trim_spaces(&detected.set_base_name().to_lowercase()).to_string();
        if !title.is_empty() {
            return Some(format!("sacd|title|{title}|{}", sacd.album_set_size));
        }
    }
    let parent = record.path.parent().unwrap_or(Path::new("/"));
    Some(format!("name|{}|{}", parent.display(), detected.set_base_name().to_lowercase()))
}

/// Foundation's `CharacterSet.whitespaces`: tab and the space separators.
fn is_space(c: char) -> bool {
    c == '\t'
        || matches!(c, ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}')
}

fn trim_spaces(s: &str) -> &str {
    s.trim_matches(is_space)
}

fn disc_number(r: &AlbumRecord) -> Option<i64> {
    r.detected().and_then(|d| d.disc_position()).map(|p| p.0 as i64)
}

fn declared_total(r: &AlbumRecord) -> Option<i64> {
    r.detected().and_then(|d| d.disc_position()).and_then(|p| p.1).map(i64::from)
}

fn folder_name(r: &AlbumRecord) -> String {
    r.detected().map_or_else(|| r.display_title.clone(), |d| d.folder_name())
}

/// Clears what the store knew about the release: the set changed shape,
/// so the stored identification was for other discs.
fn reset_identification(r: &mut AlbumRecord) {
    if r.identification_json.is_some() {
        r.identification_json = None;
        r.selected_candidate_id = None;
        if matches!(r.state, AlbumState::Confident | AlbumState::NeedsReview) {
            r.state = AlbumState::Scanned;
        }
    }
}

fn new_id() -> String {
    let mut bytes = [0u8; 16];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = std::io::Read::read_exact(&mut f, &mut bytes);
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02X}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

// MARK: Numbers in names

static DIGITS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+").unwrap());

pub fn last_number(name: &str) -> Option<i64> {
    DIGITS.find_iter(name).last().and_then(|m| m.as_str().parse().ok())
}

pub fn name_without_number(name: &str) -> String {
    let Some(m) = DIGITS.find_iter(name).last() else { return name.to_string() };
    let t = format!("{}{}", &name[..m.start()], &name[m.end()..]);
    t.trim_matches(|c: char| is_space(c) || matches!(c, '.' | '-' | '_')).to_string()
}

/// DISCNUMBER when every folder has a distinct one; else the number in
/// otherwise identical folder names; None when neither tells discs apart.
pub fn disc_positions(group: &[AlbumRecord]) -> Option<HashMap<PathBuf, i64>> {
    let tagged: Vec<i64> = group.iter().filter_map(|r| r.tagged_disc_number).collect();
    if tagged.len() == group.len() && tagged.iter().collect::<HashSet<_>>().len() == group.len() {
        return Some(group.iter().map(|r| (r.path.clone(), r.tagged_disc_number.unwrap())).collect());
    }
    let names: Vec<String> = group.iter().map(folder_name).collect();
    if names.iter().map(|n| name_without_number(n)).collect::<HashSet<_>>().len() != 1 {
        return None;
    }
    let numbers: Vec<Option<i64>> = names.iter().map(|n| last_number(n)).collect();
    if numbers.iter().any(Option::is_none) || numbers.iter().flatten().collect::<HashSet<_>>().len() != group.len() {
        return None;
    }
    Some(group.iter().map(|r| r.path.clone()).zip(numbers.into_iter().flatten()).collect())
}

// MARK: Grouping

/// The queue in memory while a grouping pass changes it; saved at the end.
struct Queue {
    all: Vec<AlbumRecord>,
    changed: HashSet<usize>,
    /// Records whose identification is cleared (true: the state too).
    reset: HashMap<usize, bool>,
}

impl Queue {
    fn load(store: &Store) -> Result<Queue> {
        let mut all = store.all()?;
        all.sort_by_key(|r| r.added_at);
        Ok(Queue { all, changed: HashSet::new(), reset: HashMap::new() })
    }

    fn index(&self, path: &Path) -> Option<usize> {
        self.all.iter().position(|r| r.path == path)
    }

    fn members(&self, i: usize) -> Vec<usize> {
        let Some(id) = self.all[i].set_id.clone() else { return vec![i] };
        let mut m: Vec<usize> = (0..self.all.len()).filter(|&j| self.all[j].set_id.as_ref() == Some(&id)).collect();
        m.sort_by_key(|&j| (self.all[j].set_position.unwrap_or(0), self.all[j].added_at));
        m
    }

    fn reset(&mut self, i: usize, with_state: bool) {
        self.changed.insert(i);
        self.reset.insert(i, with_state);
        if with_state {
            reset_identification(&mut self.all[i]);
        }
    }

    fn edit(&mut self, i: usize) -> &mut AlbumRecord {
        self.changed.insert(i);
        &mut self.all[i]
    }

    /// Writes only what grouping owns onto the stored record as it is now:
    /// a worker may have saved other columns since the queue was read.
    fn save(&self, store: &Store) -> Result<()> {
        for &i in &self.changed {
            let mine = &self.all[i];
            let Some(mut r) = store.fetch(&mine.path)? else { continue };
            r.set_id = mine.set_id.clone();
            r.set_position = mine.set_position;
            r.set_total = mine.set_total;
            r.set_title = mine.set_title.clone();
            r.tagged_release_key = mine.tagged_release_key.clone();
            r.tagged_disc_number = mine.tagged_disc_number;
            if let Some(&with_state) = self.reset.get(&i) {
                if with_state {
                    reset_identification(&mut r);
                } else if r.identification_json.is_some() {
                    r.identification_json = None;
                    r.selected_candidate_id = None;
                }
            }
            store.save(&r)?;
        }
        Ok(())
    }

    fn assign(&mut self, group: &[usize]) {
        let id = new_id();
        let mut ordered = group.to_vec();
        ordered.sort_by(|&a, &b| {
            let (ra, rb) = (&self.all[a], &self.all[b]);
            let (pa, pb) = (disc_number(ra).unwrap_or(i64::MAX), disc_number(rb).unwrap_or(i64::MAX));
            pa.cmp(&pb).then_with(|| librarykit::natural_cmp(&ra.display_title, &rb.display_title))
        });
        let records: Vec<AlbumRecord> = ordered.iter().map(|&i| self.all[i].clone()).collect();
        let declared = records.iter().filter_map(declared_total).max();
        // Disc numbers from the names or tags when there are no disc tokens
        // ("Vol.1" … "Vol.9"), else the order of the names.
        let from_names = if records.iter().all(|r| disc_number(r).is_none()) { disc_positions(&records) } else { None };
        let positions: Vec<i64> = records
            .iter()
            .enumerate()
            .map(|(i, r)| {
                from_names.as_ref().and_then(|f| f.get(&r.path).copied()).or(disc_number(r)).unwrap_or(i as i64 + 1)
            })
            .collect();
        let total = declared.unwrap_or(0).max(positions.iter().copied().max().unwrap_or(0)).max(records.len() as i64);
        let first = records.first();
        let title = if from_names.is_some() {
            name_without_number(&first.map(folder_name).unwrap_or_default())
        } else {
            first
                .map(|r| r.detected().map_or_else(|| r.display_title.clone(), |d| d.set_base_name()))
                .unwrap_or_default()
        };
        for (k, &i) in ordered.iter().enumerate() {
            let r = self.edit(i);
            r.set_id = Some(id.clone());
            r.set_position = Some(positions[k]);
            r.set_total = Some(total);
            r.set_title = Some(title.clone());
        }
    }

    fn join(&mut self, i: usize, existing: usize) {
        let (set_id, set_title) = (self.all[existing].set_id.clone(), self.all[existing].set_title.clone());
        {
            let r = self.edit(i);
            r.set_id = set_id;
            r.set_title = set_title;
        }
        // As on Mac, the members already include the newcomer here.
        let all = self.members(existing);
        let next = all.iter().filter_map(|&j| self.all[j].set_position).max().unwrap_or(0) + 1;
        let position = disc_number(&self.all[i]).unwrap_or(next);
        self.edit(i).set_position = Some(position);
        let all = self.members(existing);
        let declared = all.iter().filter_map(|&j| declared_total(&self.all[j])).max().unwrap_or(0);
        let highest = all.iter().filter_map(|&j| self.all[j].set_position).max().unwrap_or(0);
        let total = declared.max(highest).max(all.len() as i64);
        for &j in &all {
            self.edit(j).set_total = Some(total);
        }
        for &j in &all {
            self.reset(j, true);
        }
    }

    /// A set of one (a lone "disc 2 of 4") is still open to its siblings.
    fn loose(&self, i: usize) -> bool {
        self.all[i].set_id.is_none() || self.members(i).len() == 1
    }
}

/// Evidence-based grouping after a scan of `scanned` (in scan order). New
/// records join a set that already exists in the queue (an ISO added later
/// still finds its box) or form one with ungrouped peers; a lone "disc 2 of
/// 3" becomes a set of one so the missing discs are visible.
pub fn auto_group(store: &Store, scanned: &[PathBuf]) -> Result<()> {
    let mut q = Queue::load(store)?;
    let keys: Vec<Option<String>> = q.all.iter().map(set_key).collect();
    for path in scanned {
        let Some(i) = q.index(path) else { continue };
        if !q.loose(i) {
            continue;
        }
        let Some(key) = keys[i].clone() else { continue };
        let peers: Vec<usize> = (0..q.all.len()).filter(|&j| j != i && keys[j].as_ref() == Some(&key)).collect();
        let mine = disc_number(&q.all[i]);
        if let Some(&existing) = peers.iter().find(|&&j| !q.loose(j)) {
            let taken: Vec<i64> = q.members(existing).iter().filter_map(|&j| q.all[j].set_position).collect();
            if mine.is_some_and(|n| taken.contains(&n)) {
                continue; // same disc twice: leave it alone
            }
            q.join(i, existing);
        } else {
            let mut group = vec![i];
            group.extend(peers.into_iter().filter(|&j| q.loose(j)));
            if group.len() > 1 && group.iter().any(|&j| q.all[j].set_id.is_some()) {
                for &j in &group {
                    q.edit(j).set_id = None;
                }
            }
            let declared = group.iter().filter_map(|&j| declared_total(&q.all[j])).max();
            if group.len() <= 1 && declared.unwrap_or(0) <= 1 {
                continue;
            }
            let positions: Vec<i64> = group.iter().filter_map(|&j| disc_number(&q.all[j])).collect();
            if positions.iter().collect::<HashSet<_>>().len() != positions.len() {
                continue;
            }
            q.assign(&group);
        }
    }
    q.save(store)
}

/// What the first track of a folder album names: release key and disc.
pub fn tagged_release(record: &AlbumRecord) -> Option<(Option<String>, Option<i64>)> {
    if !matches!(record.kind, AlbumKind::TrackFolder | AlbumKind::CueMultiFile) {
        return None;
    }
    let detected = record.detected()?;
    let file = detected.discs.first()?.track_files.first()?.url.clone();
    let Ok(tags) = TaggedFile::read_tags(&file) else { return Some((None, None)) };
    let key = tags
        .first(field::MB_ALBUM_ID)
        .map(|m| format!("mb:{}", m.to_lowercase()))
        .or_else(|| tags.first(field::DISCOGS_RELEASE_ID).map(|d| format!("discogs:{d}")));
    let disc = tags.first(field::DISC_NUMBER).and_then(|d| d.split('/').next().unwrap_or("").parse().ok());
    Some((key, disc))
}

/// Folders whose tracks name the same release and whose names differ only
/// by a number ("Piano Sonatas Vol.2" … "Vol.8") are discs of that
/// release. Two copies of one album (same name, no number) are not. Reads
/// one file per scanned folder album.
pub fn group_by_tagged_release(store: &Store, scanned: &[PathBuf]) -> Result<()> {
    let read: Vec<(PathBuf, Option<String>, Option<i64>)> = scanned
        .iter()
        .filter_map(|p| store.fetch(p).ok().flatten())
        .filter_map(|r| tagged_release(&r).map(|(k, d)| (r.path.clone(), k, d)))
        .collect();
    let mut q = Queue::load(store)?;
    for (path, key, disc) in read {
        let Some(i) = q.index(&path) else { continue };
        let r = q.edit(i);
        r.tagged_release_key = key;
        r.tagged_disc_number = disc;
    }
    let mut by_release: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, r) in q.all.iter().enumerate() {
        if r.set_id.is_none()
            && let Some(key) = &r.tagged_release_key
        {
            by_release.entry(key.clone()).or_default().push(i);
        }
    }
    for group in by_release.into_values().filter(|g| g.len() > 1) {
        let records: Vec<AlbumRecord> = group.iter().map(|&i| q.all[i].clone()).collect();
        let Some(positions) = disc_positions(&records) else { continue };
        let id = new_id();
        let total = positions.values().copied().max().unwrap_or(0).max(group.len() as i64);
        let title = name_without_number(&folder_name(&records[0]));
        for &i in &group {
            let r = q.edit(i);
            r.set_id = Some(id.clone());
            r.set_position = positions.get(&r.path).copied();
            r.set_total = Some(total);
            r.set_title = Some(title.clone());
        }
        for &i in &group {
            q.reset(i, false);
        }
    }
    q.save(store)
}

/// Both passes for the albums of a scan, as the Mac app runs them.
pub fn after_scan(store: &Store, result: &librarykit::ScanResult) -> Result<()> {
    let scanned: Vec<PathBuf> = result.albums.iter().map(|a| a.id()).collect();
    auto_group(store, &scanned)?;
    group_by_tagged_release(store, &scanned)
}

/// Makes `paths` discs of one release.
pub fn group(store: &Store, paths: &[PathBuf]) -> Result<()> {
    if paths.len() < 2 {
        return Ok(());
    }
    let mut q = Queue::load(store)?;
    let group: Vec<usize> = paths.iter().filter_map(|p| q.index(p)).collect();
    q.assign(&group);
    for &i in &group {
        q.reset(i, true);
    }
    q.save(store)
}

pub fn ungroup(store: &Store, path: &Path) -> Result<()> {
    let mut q = Queue::load(store)?;
    let Some(i) = q.index(path) else { return Ok(()) };
    for j in q.members(i) {
        let r = q.edit(j);
        r.set_id = None;
        r.set_position = None;
        r.set_total = None;
        r.set_title = None;
        q.reset(j, true);
    }
    q.save(store)
}

pub fn set_position(store: &Store, path: &Path, position: i64) -> Result<()> {
    if position < 1 {
        return Ok(());
    }
    let mut q = Queue::load(store)?;
    let Some(i) = q.index(path) else { return Ok(()) };
    q.edit(i).set_position = Some(position);
    let all = q.members(i);
    let highest = all.iter().filter_map(|&j| q.all[j].set_position).max().unwrap_or(0);
    let total = highest.max(q.all[all[0]].set_total.unwrap_or(0));
    for j in all {
        q.edit(j).set_total = Some(total);
    }
    q.save(store)
}

#[cfg(test)]
mod tests {
    use super::*;
    use librarykit::LibraryScanner;

    fn add(store: &mut Store, roots: &[PathBuf]) {
        let result = LibraryScanner::new().scan(roots);
        store.upsert(&result).unwrap();
        after_scan(store, &result).unwrap();
    }

    fn find(store: &Store, suffix: &str) -> AlbumRecord {
        store.all().unwrap().into_iter().find(|r| r.path.to_string_lossy().ends_with(suffix)).unwrap()
    }

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, [0u8; 4]).unwrap();
    }

    // LibraryControllerTests.siblingDiscFoldersBecomeAReleaseSet
    #[test]
    fn sibling_disc_folders_become_a_release_set() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        for (folder, count) in [
            ("Box/Led Zeppelin - Box Set (Disc 1)", 1),
            ("Box/Led Zeppelin - Box Set (Disc 2)", 2),
            ("Box/Something Else", 1),
        ] {
            for n in 1..=count {
                touch(&root.join(format!("{folder}/0{n}.flac")));
            }
        }
        let mut store = Store::open_in_memory().unwrap();
        add(&mut store, std::slice::from_ref(&root));
        assert_eq!(store.all().unwrap().len(), 3);
        let disc1 = find(&store, "(Disc 1)");
        let disc2 = find(&store, "(Disc 2)");
        let other = find(&store, "Something Else");
        assert!(disc1.set_id.is_some() && disc1.set_id == disc2.set_id, "siblings with disc numbers are one set");
        assert_eq!((disc1.set_position, disc2.set_position, disc2.set_total), (Some(1), Some(2), Some(2)));
        assert_eq!(disc1.set_title.as_deref(), Some("Led Zeppelin - Box Set"));
        assert!(other.set_id.is_none());
        let paths: Vec<PathBuf> = members(&store, &disc2).unwrap().into_iter().map(|m| m.path).collect();
        assert_eq!(paths, [disc1.path.clone(), disc2.path.clone()]);
        assert_eq!(disc2.disc_position(), 2);

        set_position(&store, &disc2.path, 3).unwrap();
        assert_eq!(find(&store, "(Disc 1)").set_total, Some(3));
        ungroup(&store, &disc1.path).unwrap();
        let (disc1, disc2) = (find(&store, "(Disc 1)"), find(&store, "(Disc 2)"));
        assert!(disc1.set_id.is_none() && disc2.set_id.is_none());
        let siblings: Vec<PathBuf> =
            sibling_candidates(&store.all().unwrap(), &disc1).into_iter().map(|m| m.path).collect();
        assert_eq!(siblings, std::slice::from_ref(&disc2.path), "regroupable by hand");
        group(&store, &[disc1.path.clone(), disc2.path.clone()]).unwrap();
        let (disc1, disc2) = (find(&store, "(Disc 1)"), find(&store, "(Disc 2)"));
        assert!(disc1.set_id.is_some() && disc2.set_position == Some(2));

        // A disc added in a later scan joins the existing set.
        touch(&root.join("Box/Led Zeppelin - Box Set (Disc 3)/01.flac"));
        add(&mut store, &[root.join("Box/Led Zeppelin - Box Set (Disc 3)")]);
        let disc3 = find(&store, "(Disc 3)");
        assert!(disc3.set_id == disc1.set_id && disc3.set_position == Some(3));
        assert_eq!(find(&store, "(Disc 1)").set_total, Some(3));

        // A lone disc that formed a set of one is regrouped when its sibling arrives.
        touch(&root.join("Lone/Opera - Disc 1 of 2/01.flac"));
        add(&mut store, &[root.join("Lone/Opera - Disc 1 of 2")]);
        let lone = find(&store, "Disc 1 of 2");
        assert!(lone.set_id.is_some());
        assert_eq!(members(&store, &lone).unwrap().len(), 1);
        touch(&root.join("Lone/Opera - Disc 2 of 2/01.flac"));
        add(&mut store, &[root.join("Lone/Opera - Disc 2 of 2")]);
        let lone = find(&store, "Disc 1 of 2");
        assert_eq!(members(&store, &lone).unwrap().len(), 2);
    }

    fn wav() -> Vec<u8> {
        let mut d = b"RIFF".to_vec();
        d.extend((36u32 + 400).to_le_bytes());
        d.extend(b"WAVEfmt ");
        d.extend([16, 0, 0, 0, 1, 0, 2, 0, 0x44, 0xAC, 0, 0, 0x10, 0xB1, 2, 0, 4, 0, 16, 0]);
        d.extend(b"data");
        d.extend(400u32.to_le_bytes());
        d.extend([0u8; 400]);
        d
    }

    // LibraryControllerTests.foldersTaggedWithOneReleaseBecomeASet
    #[test]
    fn folders_tagged_with_one_release_become_a_set() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let album = |folder: &str, mbid: &str| {
            let path = root.join(folder).join("01.wav");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, wav()).unwrap();
            let mut t = tagkit::TagSet::default();
            t.set(field::MB_ALBUM_ID, Some(mbid));
            TaggedFile::write(&path, &t, None, None).unwrap();
        };
        for n in [2, 3, 4] {
            album(&format!("Piano Sonatas Vol.{n}"), "box-0001");
        }
        album("Kind of Blue", "kob-1");
        album("Kind of Blue copy", "kob-1");
        let mut store = Store::open_in_memory().unwrap();
        add(&mut store, std::slice::from_ref(&root));
        let records = store.all().unwrap();
        let mut vols: Vec<&AlbumRecord> = records.iter().filter(|r| r.display_title.contains("Vol")).collect();
        vols.sort_by_key(|r| r.set_position.unwrap_or(0));
        assert_eq!(vols.len(), 3);
        assert_eq!(vols.iter().filter_map(|r| r.set_id.clone()).collect::<HashSet<_>>().len(), 1);
        assert_eq!(vols.iter().map(|r| r.set_position).collect::<Vec<_>>(), [Some(2), Some(3), Some(4)]);
        assert!(
            records.iter().filter(|r| r.display_title.starts_with("Kind of Blue")).all(|r| r.set_id.is_none()),
            "two copies are not a set"
        );
    }

    #[test]
    fn numbers_in_names() {
        assert_eq!(last_number("Piano Sonatas Vol.12"), Some(12));
        assert_eq!(last_number("No number"), None);
        assert_eq!(name_without_number("Piano Sonatas Vol.2"), "Piano Sonatas Vol");
        assert_eq!(name_without_number("Box - 3"), "Box");
        assert_eq!(name_without_number("1999 Live 2"), "1999 Live");
        assert_eq!(name_without_number("Plain"), "Plain");
    }

    #[test]
    fn ids_look_like_foundation_uuids() {
        let id = new_id();
        assert_eq!(id.len(), 36);
        assert_eq!(id.as_bytes()[14], b'4');
        assert!(id.chars().all(|c| c == '-' || c.is_ascii_hexdigit() && !c.is_ascii_lowercase()));
    }
}
