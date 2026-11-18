//! Walks folders and turns them into `DetectedAlbum`s. Synchronous; callers
//! run it off the UI thread.

use crate::audio_format::{AudioFormat, FileRules};
use crate::cue::CueSheet;
use crate::folder_name::FolderNameParser;
use crate::natural::naturally_sorted;
use crate::path_serde;
use crate::sacd_probe::{SACDInfo, SACDProbe};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet};
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum AlbumKind {
    /// One Scarletbook image.
    #[serde(rename = "sacdISO")]
    SacdIso,
    /// One audio/raw image file described by a CUE.
    #[serde(rename = "cueImage")]
    CueImage,
    /// A CUE whose FILE entries all exist (one per track).
    #[serde(rename = "cueMultiFile")]
    CueMultiFile,
    /// Loose track files (a CUE may exist as a hint).
    #[serde(rename = "trackFolder")]
    TrackFolder,
}

impl AlbumKind {
    /// The Swift enum's raw value, as stored.
    pub fn raw_value(self) -> &'static str {
        match self {
            AlbumKind::SacdIso => "sacdISO",
            AlbumKind::CueImage => "cueImage",
            AlbumKind::CueMultiFile => "cueMultiFile",
            AlbumKind::TrackFolder => "trackFolder",
        }
    }

    pub fn from_raw(raw: &str) -> Option<AlbumKind> {
        [AlbumKind::SacdIso, AlbumKind::CueImage, AlbumKind::CueMultiFile, AlbumKind::TrackFolder]
            .into_iter()
            .find(|k| k.raw_value() == raw)
    }

    pub fn display_name(self) -> &'static str {
        match self {
            AlbumKind::SacdIso => "SACD ISO",
            AlbumKind::CueImage => "CD image + CUE",
            AlbumKind::CueMultiFile => "Tracks + CUE",
            AlbumKind::TrackFolder => "Track folder",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedTrackFile {
    #[serde(with = "path_serde")]
    pub url: PathBuf,
    pub format: AudioFormat,
    pub file_size: i64,
}

impl DetectedTrackFile {
    pub fn file_name(&self) -> String {
        self.url.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }
}

/// One disc of an album: a folder (or the album folder itself) with either
/// an image + CUE, or track files.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedDisc {
    pub number: u32,
    #[serde(with = "path_serde")]
    pub folder: PathBuf,
    #[serde(rename = "cueURL", with = "path_serde::option", skip_serializing_if = "Option::is_none", default)]
    pub cue_url: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cue: Option<CueSheet>,
    /// Set for cueImage discs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_file: Option<DetectedTrackFile>,
    /// Set for track / multi-file discs.
    pub track_files: Vec<DetectedTrackFile>,
    #[serde(with = "path_serde::vec")]
    pub artwork_files: Vec<PathBuf>,
    #[serde(with = "path_serde::vec")]
    pub log_files: Vec<PathBuf>,
}

impl DetectedDisc {
    pub fn new(number: u32, folder: &Path) -> Self {
        DetectedDisc {
            number,
            folder: folder.to_path_buf(),
            cue_url: None,
            cue: None,
            image_file: None,
            track_files: vec![],
            artwork_files: vec![],
            log_files: vec![],
        }
    }

    pub fn track_count(&self) -> usize {
        match (&self.cue, &self.image_file) {
            (Some(cue), Some(_)) => cue.audio_tracks().len(),
            _ => self.track_files.len(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedAlbum {
    /// Album folder, or the ISO file itself.
    #[serde(with = "path_serde")]
    pub url: PathBuf,
    pub kind: AlbumKind,
    pub discs: Vec<DetectedDisc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sacd: Option<SACDInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iso_file: Option<DetectedTrackFile>,
    /// Album-level scans (folder + Scans/ etc.).
    #[serde(with = "path_serde::vec")]
    pub artwork_files: Vec<PathBuf>,
}

fn last_component(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

const AREA_FOLDER_NAMES: [&str; 13] = [
    "stereo",
    "multichannel",
    "multi-channel",
    "multi channel",
    "mch",
    "2ch",
    "5ch",
    "6ch",
    "2.0",
    "5.1",
    "surround",
    "2ch stereo",
    "5.1ch",
];

impl DetectedAlbum {
    pub fn new(url: &Path, kind: AlbumKind, discs: Vec<DetectedDisc>, artwork_files: Vec<PathBuf>) -> Self {
        DetectedAlbum { url: url.to_path_buf(), kind, discs, sacd: None, iso_file: None, artwork_files }
    }

    /// The store key: the standardised path.
    pub fn id(&self) -> PathBuf {
        standardized(&self.url)
    }

    pub fn track_count(&self) -> usize {
        match &self.sacd {
            Some(sacd) => sacd.track_count().max(0) as usize,
            None => self.discs.iter().map(DetectedDisc::track_count).sum(),
        }
    }

    pub fn folder_name(&self) -> String {
        if self.kind == AlbumKind::SacdIso {
            return self.url.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        }
        // sacd_extract and similar tools write "Album/Stereo/…" and
        // "Album/Multichannel/…": the album is the parent folder.
        if Self::is_area_folder_name(&last_component(&self.url)) {
            return self.url.parent().map(last_component).unwrap_or_default();
        }
        last_component(&self.url)
    }

    pub fn is_area_folder_name(name: &str) -> bool {
        let n = name.to_lowercase();
        let n = n.trim_matches(|c: char| c == ' ' || c == '\t');
        AREA_FOLDER_NAMES.contains(&n)
    }

    /// "Disc 2 of 3" evidence: the SACD master TOC's album set, else a disc
    /// token in the folder or ISO name. Used to group siblings into one release.
    pub fn disc_position(&self) -> Option<(u32, Option<u32>)> {
        if let Some(sacd) = &self.sacd
            && sacd.album_set_size > 1
        {
            return Some((sacd.album_sequence_number as u32, Some(sacd.album_set_size as u32)));
        }
        if self.discs.len() > 1 {
            return None;
        }
        FileRules::disc_number_from_file_name(&self.folder_name())
    }

    /// What siblings of one set share: the name without its disc token, or
    /// the SACD album title.
    pub fn set_base_name(&self) -> String {
        if let Some(sacd) = &self.sacd
            && sacd.album_set_size > 1
            && let Some(title) = sacd.album_title.as_deref().filter(|t| !t.is_empty())
        {
            return FileRules::stripping_disc_token(title);
        }
        FileRules::stripping_disc_token(&self.folder_name())
    }

    /// Best available title without any network lookup.
    pub fn title_hint(&self) -> Option<String> {
        if let Some(t) = self.sacd.as_ref().and_then(SACDInfo::title) {
            return Some(t);
        }
        if let Some(t) = self.first_cue().and_then(|c| c.title.as_deref()).filter(|t| !t.is_empty()) {
            // "Box Set (Disc 1)" names the disc, not the multi-disc album.
            return Some(if self.discs.len() > 1 { FileRules::stripping_disc_token(t) } else { t.to_string() });
        }
        FolderNameParser::parse(&self.folder_name()).display_title()
    }

    pub fn artist_hint(&self) -> Option<String> {
        if let Some(a) = self.sacd.as_ref().and_then(SACDInfo::artist) {
            return Some(a);
        }
        if let Some(a) = self.first_cue().and_then(|c| c.performer.as_deref()).filter(|a| !a.is_empty()) {
            return Some(a.to_string());
        }
        FolderNameParser::parse(&self.folder_name()).artist
    }

    pub fn year_hint(&self) -> Option<String> {
        if let Some(d) = self.sacd.as_ref().and_then(|s| s.disc_date.as_deref()) {
            return Some(d.chars().take(4).collect());
        }
        if let Some(d) = self.first_cue().and_then(CueSheet::date)
            && d.chars().count() >= 4
        {
            return Some(d.chars().take(4).collect());
        }
        FolderNameParser::parse(&self.folder_name()).year
    }

    pub fn formats(&self) -> BTreeSet<AudioFormat> {
        let mut set = BTreeSet::new();
        for disc in &self.discs {
            if let Some(img) = &disc.image_file {
                set.insert(img.format);
            }
            set.extend(disc.track_files.iter().map(|t| t.format));
        }
        set
    }

    fn first_cue(&self) -> Option<&CueSheet> {
        self.discs.first().and_then(|d| d.cue.as_ref())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ScanIssue {
    #[serde(with = "path_serde")]
    pub url: PathBuf,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    pub albums: Vec<DetectedAlbum>,
    pub issues: Vec<ScanIssue>,
    pub folders_visited: usize,
}

/// Foundation's `standardizedFileURL`: "." and ".." removed lexically,
/// symbolic links left alone.
pub fn standardized(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    if out.as_os_str().is_empty() { PathBuf::from(".") } else { out }
}

fn file_size(path: &Path) -> i64 {
    std::fs::metadata(path).map_or(0, |m| m.len() as i64)
}

#[derive(Default)]
struct FolderContents {
    subfolders: Vec<PathBuf>,
    isos: Vec<PathBuf>,
    cues: Vec<PathBuf>,
    audio: Vec<PathBuf>,
    artwork: Vec<PathBuf>,
    logs: Vec<PathBuf>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LibraryScanner;

impl LibraryScanner {
    pub fn new() -> Self {
        LibraryScanner
    }

    pub fn scan(&self, roots: &[PathBuf]) -> ScanResult {
        self.scan_with_progress(roots, &mut |_| {})
    }

    /// `progress` is called with each folder as the walk enters it.
    pub fn scan_with_progress(&self, roots: &[PathBuf], progress: &mut dyn FnMut(&Path)) -> ScanResult {
        let mut result = ScanResult::default();
        for root in roots {
            let std = standardized(root);
            match std::fs::metadata(&std) {
                Err(_) => result.issues.push(ScanIssue { url: std, message: "Path does not exist.".into() }),
                Ok(m) if m.is_dir() => self.walk(&std, &mut result, progress),
                Ok(_) => self.scan_single_file(&std, &mut result),
            }
        }
        let mut seen = HashSet::new();
        result.albums.retain(|a| seen.insert(a.id()));
        result
    }

    /// A dropped file: an ISO, a CUE, or one audio file whose folder is the album.
    fn scan_single_file(&self, path: &Path, result: &mut ScanResult) {
        if FileRules::is_iso(path) {
            if let Some(album) = self.sacd_album(path, result) {
                result.albums.push(album);
            }
            return;
        }
        if FileRules::is_cue(path) || FileRules::is_audio(path) {
            if let Some(parent) = path.parent() {
                self.walk(parent, result, &mut |_| {});
            }
            return;
        }
        result.issues.push(ScanIssue { url: path.to_path_buf(), message: "Unsupported file type.".into() });
    }

    fn list(&self, folder: &Path) -> FolderContents {
        let mut c = FolderContents::default();
        let Ok(entries) = std::fs::read_dir(folder) else { return c };
        for entry in entries.flatten() {
            let name = entry.file_name();
            if FileRules::is_junk(&name.to_string_lossy()) {
                continue;
            }
            // Resource values do not follow symbolic links: a link is
            // neither a folder nor a regular file to the scanner.
            let Ok(kind) = entry.file_type() else { continue };
            let path = folder.join(&name);
            if kind.is_dir() {
                c.subfolders.push(path);
            } else if kind.is_file() {
                if FileRules::is_iso(&path) {
                    c.isos.push(path);
                } else if FileRules::is_cue(&path) {
                    c.cues.push(path);
                } else if FileRules::is_audio(&path) {
                    c.audio.push(path);
                } else if FileRules::is_artwork(&path) {
                    c.artwork.push(path);
                } else if FileRules::is_log(&path) {
                    c.logs.push(path);
                }
            }
        }
        c.subfolders = naturally_sorted(c.subfolders);
        c.isos = naturally_sorted(c.isos);
        c.cues = naturally_sorted(c.cues);
        c.audio = naturally_sorted(c.audio);
        c.artwork = naturally_sorted(c.artwork);
        c.logs = naturally_sorted(c.logs);
        c
    }

    fn walk(&self, folder: &Path, result: &mut ScanResult, progress: &mut dyn FnMut(&Path)) {
        progress(folder);
        result.folders_visited += 1;
        let contents = self.list(folder);

        // SACD images are always their own album.
        for iso in &contents.isos {
            if let Some(album) = self.sacd_album(iso, result) {
                result.albums.push(album);
            }
        }

        let albums = self.albums_in_folder(folder, &contents, result);
        let found_here = !albums.is_empty();
        result.albums.extend(albums);

        // Multi-disc: no album here, but disc subfolders that each hold one.
        let mut consumed = HashSet::new();
        if !found_here && let Some(multi) = self.multi_disc_album(folder, &contents, result) {
            consumed.extend(multi.discs.iter().map(|d| standardized(&d.folder)));
            result.albums.push(multi);
        }

        for sub in &contents.subfolders {
            if !consumed.contains(&standardized(sub)) {
                self.walk(sub, result, progress);
            }
        }
    }

    /// Albums formed by the files directly inside `folder` (not counting ISOs).
    fn albums_in_folder(
        &self,
        folder: &Path,
        contents: &FolderContents,
        result: &mut ScanResult,
    ) -> Vec<DetectedAlbum> {
        let mut albums: Vec<DetectedAlbum> = vec![];
        let mut handled_audio: HashSet<PathBuf> = HashSet::new();
        let mut orphan_cue: Option<(PathBuf, CueSheet)> = None;
        let artwork = self.collect_artwork(contents);

        for cue_url in &contents.cues {
            let sheet = match CueSheet::parse_file(cue_url) {
                Ok(s) => s,
                Err(e) => {
                    result
                        .issues
                        .push(ScanIssue { url: cue_url.clone(), message: format!("CUE could not be parsed: {e}") });
                    continue;
                }
            };
            let refs = sheet.referenced_files(folder);
            let existing: Vec<PathBuf> = refs.iter().filter_map(|r| resolve_on_disk(r)).collect();

            if refs.len() == 1
                && existing.len() == 1
                && let Some(image) = track_file(&existing[0], true)
            {
                let mut disc = DetectedDisc::new(1, folder);
                disc.cue_url = Some(cue_url.clone());
                disc.cue = Some(sheet);
                handled_audio.insert(standardized(&image.url));
                disc.image_file = Some(image);
                disc.artwork_files = artwork.clone();
                disc.log_files = contents.logs.clone();
                albums.push(DetectedAlbum::new(folder, AlbumKind::CueImage, vec![disc], artwork.clone()));
            } else if refs.len() > 1 && existing.len() == refs.len() {
                let mut disc = DetectedDisc::new(1, folder);
                disc.cue_url = Some(cue_url.clone());
                disc.cue = Some(sheet);
                disc.track_files = existing.iter().filter_map(|p| track_file(p, false)).collect();
                disc.artwork_files = artwork.clone();
                disc.log_files = contents.logs.clone();
                handled_audio.extend(disc.track_files.iter().map(|t| standardized(&t.url)));
                albums.push(DetectedAlbum::new(folder, AlbumKind::CueMultiFile, vec![disc], artwork.clone()));
            } else {
                // Image was split or converted after ripping: keep the sheet
                // as metadata for the loose tracks in this folder.
                let missing = refs.len() - existing.len();
                if orphan_cue.is_none() {
                    orphan_cue = Some((cue_url.clone(), sheet));
                }
                result.issues.push(ScanIssue {
                    url: cue_url.clone(),
                    message: format!("CUE references {missing} missing file(s); using it as a hint only."),
                });
            }
        }

        let loose: Vec<&PathBuf> =
            contents.audio.iter().filter(|p| !handled_audio.contains(&standardized(p))).collect();
        if !loose.is_empty() {
            let mut disc = DetectedDisc::new(1, folder);
            disc.track_files = loose.iter().filter_map(|p| track_file(p, false)).collect();
            disc.artwork_files = artwork.clone();
            disc.log_files = contents.logs.clone();
            if let Some((cue_url, sheet)) = orphan_cue {
                disc.cue_url = Some(cue_url);
                disc.cue = Some(sheet);
            }
            albums.push(DetectedAlbum::new(folder, AlbumKind::TrackFolder, vec![disc], artwork.clone()));
        }

        // Several images in one folder: "(Disc 1)", "(Disc 2)"… are one
        // multi-disc album; images without a disc number cannot be told apart
        // from unrelated albums, so only the first is kept and the rest reported.
        let images: Vec<DetectedAlbum> = albums.iter().filter(|a| a.kind == AlbumKind::CueImage).cloned().collect();
        if images.len() > 1 {
            let numbered: Vec<(u32, &DetectedAlbum)> = images
                .iter()
                .filter_map(|a| {
                    let name = a.discs[0].cue_url.as_deref().map(last_component).unwrap_or_default();
                    FileRules::disc_number_from_file_name(&name).map(|(n, _)| (n, a))
                })
                .collect();
            let distinct: HashSet<u32> = numbered.iter().map(|(n, _)| *n).collect();
            let mut rest: Vec<DetectedAlbum> = albums.into_iter().filter(|a| a.kind != AlbumKind::CueImage).collect();
            if numbered.len() == images.len() && distinct.len() == numbered.len() {
                let mut sorted = numbered;
                sorted.sort_by_key(|(n, _)| *n);
                let discs = sorted
                    .into_iter()
                    .map(|(n, a)| {
                        let mut d = a.discs[0].clone();
                        d.number = n;
                        d
                    })
                    .collect();
                rest.push(DetectedAlbum::new(folder, AlbumKind::CueImage, discs, artwork.clone()));
            } else {
                for extra in &images[1..] {
                    result.issues.push(ScanIssue {
                        url: extra.discs[0].cue_url.clone().unwrap_or_else(|| folder.to_path_buf()),
                        message: "Another image in the same folder without a disc number in its name; ignored.".into(),
                    });
                }
                rest.push(images[0].clone());
            }
            albums = rest;
        }
        albums
    }

    /// `folder` holds no audio but has "CD1", "Disc 2"… subfolders that do.
    fn multi_disc_album(
        &self,
        folder: &Path,
        contents: &FolderContents,
        result: &mut ScanResult,
    ) -> Option<DetectedAlbum> {
        if !contents.audio.is_empty() || !contents.cues.is_empty() {
            return None;
        }
        let mut numbered: Vec<(u32, &PathBuf)> = contents
            .subfolders
            .iter()
            .filter_map(|sub| FileRules::disc_number_from_folder_name(&last_component(sub)).map(|n| (n, sub)))
            .collect();
        if !(numbered.len() >= 2 || (numbered.len() == 1 && contents.subfolders.len() == 1)) {
            return None;
        }
        numbered.sort_by_key(|(n, _)| *n);

        let mut discs = vec![];
        let mut kinds = HashSet::new();
        for (n, sub) in numbered {
            let sub_contents = self.list(sub);
            let mut scratch = ScanResult::default();
            let found = self.albums_in_folder(sub, &sub_contents, &mut scratch);
            result.issues.extend(scratch.issues);
            if found.len() != 1 {
                return None;
            }
            let mut disc = found[0].discs.first()?.clone();
            disc.number = n;
            discs.push(disc);
            kinds.insert(found[0].kind);
        }
        if discs.is_empty() {
            return None;
        }
        let kind = if kinds.len() == 1 { *kinds.iter().next().unwrap() } else { AlbumKind::TrackFolder };
        Some(DetectedAlbum::new(folder, kind, discs, self.collect_artwork(contents)))
    }

    fn sacd_album(&self, iso: &Path, result: &mut ScanResult) -> Option<DetectedAlbum> {
        if !SACDProbe::is_sacd(iso) {
            result
                .issues
                .push(ScanIssue { url: iso.to_path_buf(), message: "ISO is not a SACD image; skipped.".into() });
            return None;
        }
        match SACDProbe::probe(iso) {
            Ok(info) => {
                let folder = iso.parent().unwrap_or(Path::new("/"));
                let artwork = self.collect_artwork(&self.list(folder));
                let mut album = DetectedAlbum::new(iso, AlbumKind::SacdIso, vec![], artwork);
                album.sacd = Some(info);
                album.iso_file = Some(DetectedTrackFile {
                    url: iso.to_path_buf(),
                    format: AudioFormat::Dsf,
                    file_size: file_size(iso),
                });
                Some(album)
            }
            Err(e) => {
                result.issues.push(ScanIssue { url: iso.to_path_buf(), message: format!("SACD probe failed: {e}") });
                None
            }
        }
    }

    /// Artwork in the folder plus one level of Scans/Artwork/Covers folders.
    fn collect_artwork(&self, contents: &FolderContents) -> Vec<PathBuf> {
        let mut files = contents.artwork.clone();
        for sub in &contents.subfolders {
            if FileRules::is_artwork_folder(&last_component(sub)) {
                files.extend(self.list(sub).artwork);
            }
        }
        files
    }
}

/// The referenced file as it exists on disk. macOS file systems match names
/// ignoring case and Unicode normalisation, so a CUE written on Windows
/// ("TRACK01.WAV") or with decomposed accents still finds its files there;
/// on Linux that needs a lookup in the folder.
pub fn resolve_on_disk(path: &Path) -> Option<PathBuf> {
    if path.exists() {
        return Some(path.to_path_buf());
    }
    let parent = path.parent()?;
    let parent = if parent.exists() { parent.to_path_buf() } else { resolve_on_disk(parent)? };
    let want = fold(path.file_name()?);
    let found = std::fs::read_dir(&parent).ok()?.flatten().map(|e| e.file_name()).find(|n| fold(n) == want)?;
    Some(parent.join(found))
}

fn fold(name: &std::ffi::OsStr) -> String {
    use unicode_normalization::UnicodeNormalization;
    name.to_string_lossy().nfc().collect::<String>().to_lowercase()
}

fn track_file(path: &Path, allow_raw: bool) -> Option<DetectedTrackFile> {
    let size = file_size(path);
    if let Some(format) = AudioFormat::from_path(path) {
        return Some(DetectedTrackFile { url: path.to_path_buf(), format, file_size: size });
    }
    if allow_raw && FileRules::is_raw_image(path) {
        return Some(DetectedTrackFile { url: path.to_path_buf(), format: AudioFormat::Wav, file_size: size });
    }
    None
}

#[cfg(test)]
mod tests;
