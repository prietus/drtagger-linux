//! Parity dump: the same JSON that `parity/MacDump` produces from the Swift
//! packages, so `parity/compare.py` can diff the two implementations on
//! real samples. Keep the shape in step with MacDump/Sources/main.swift.

use librarykit::{
    CueSheet, CueSplitPlan, DetectedAlbum, DiscTOC, FileRules, FolderNameParser, LibraryScanner, ParsedName,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub fn dump(roots: &[PathBuf], names: &[String]) -> Value {
    let result = LibraryScanner::new().scan(roots);
    let albums: Vec<Value> = result.albums.iter().map(|a| json!({ "album": a, "derived": derived(a) })).collect();
    let mut cue_paths = vec![];
    for root in roots {
        find_cues(root, &mut cue_paths);
    }
    cue_paths.sort();
    cue_paths.dedup();
    json!({
        "scan": { "albums": albums, "issues": result.issues, "foldersVisited": result.folders_visited },
        "cues": cue_paths.iter().map(|p| cue(p)).collect::<Vec<_>>(),
        "names": names.iter().map(|n| name(n)).collect::<Vec<_>>(),
    })
}

fn derived(a: &DetectedAlbum) -> Value {
    let mut formats: Vec<&str> = a.formats().iter().map(|f| f.raw_value()).collect();
    formats.sort();
    let mut v = json!({
        "id": a.id(),
        "trackCount": a.track_count(),
        "folderName": a.folder_name(),
        "setBaseName": a.set_base_name(),
        "formats": formats,
    });
    let o = v.as_object_mut().unwrap();
    if let Some(t) = a.title_hint() {
        o.insert("titleHint".into(), t.into());
    }
    if let Some(t) = a.artist_hint() {
        o.insert("artistHint".into(), t.into());
    }
    if let Some(t) = a.year_hint() {
        o.insert("yearHint".into(), t.into());
    }
    if let Some(p) = a.disc_position() {
        o.insert("discPosition".into(), position(p));
    }
    v
}

fn position((number, total): (u32, Option<u32>)) -> Value {
    let mut v = json!({ "number": number });
    if let Some(t) = total {
        v["total"] = t.into();
    }
    v
}

fn find_cues(path: &Path, out: &mut Vec<PathBuf>) {
    let Ok(meta) = std::fs::metadata(path) else { return };
    if meta.is_file() {
        if FileRules::is_cue(path) {
            out.push(path.to_path_buf());
        }
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else { return };
    for e in entries.flatten() {
        if FileRules::is_junk(&e.file_name().to_string_lossy()) {
            continue;
        }
        match e.file_type() {
            Ok(t) if t.is_dir() => find_cues(&e.path(), out),
            Ok(t) if t.is_file() && FileRules::is_cue(&e.path()) => out.push(e.path()),
            _ => {}
        }
    }
}

/// Synthetic file lengths shared with MacDump: every FILE is as long as the
/// last index anywhere in the sheet plus 10000 frames.
fn synthetic_frames(sheet: &CueSheet) -> Vec<i64> {
    let max = sheet.tracks().flat_map(|t| t.indexes.iter().map(|i| i.time.frames)).max().unwrap_or(0);
    vec![max + 10_000; sheet.files.len()]
}

fn cue(path: &Path) -> Value {
    let sheet = match CueSheet::parse_file(path) {
        Ok(s) => s,
        Err(e) => return json!({ "path": path, "error": e.to_string() }),
    };
    let frames = synthetic_frames(&sheet);
    let toc = match DiscTOC::from_cue(&sheet, &frames) {
        Ok(t) => json!({
            "firstTrack": t.first_track, "lastTrack": t.last_track, "trackOffsets": t.track_offsets,
            "leadOut": t.lead_out, "musicBrainzDiscID": t.musicbrainz_disc_id(),
            "freeDBDiscID": t.freedb_disc_id(), "ctdbTOCString": t.ctdb_toc_string(),
            "musicBrainzTOCString": t.musicbrainz_toc_string(),
        }),
        Err(e) => json!({ "error": e.to_string() }),
    };
    let samples: Vec<i64> = frames.iter().map(|f| f * 588).collect();
    let plan = match CueSplitPlan::make(&sheet, &samples, 44100, CueSplitPlan::DEFAULT_HTOA_THRESHOLD_FRAMES) {
        Ok(p) => serde_json::to_value(p).unwrap(),
        Err(e) => json!({ "error": e.to_string() }),
    };
    let mut v = json!({
        "path": path, "sheet": sheet, "catalogNumberHints": sheet.catalog_number_hints(), "toc": toc, "plan": plan,
    });
    if let Some(b) = sheet.barcode_hint() {
        v["barcodeHint"] = b.into();
    }
    if let Some(p) = sheet.track_one_pregap() {
        v["trackOnePregap"] = json!({ "frames": p.frames });
    }
    v
}

fn parsed(p: &ParsedName) -> Value {
    let mut v = serde_json::to_value(p).unwrap();
    if let Some(d) = p.display_title() {
        v["displayTitle"] = d.into();
    }
    v
}

fn name(n: &str) -> Value {
    let mut v = json!({
        "name": n,
        "parse": parsed(&FolderNameParser::parse(n)),
        "albumTitle": parsed(&FolderNameParser::parse_album_title(n)),
        "stripped": FileRules::stripping_disc_token(n),
        "ascii": splitkit::PathTemplate::ascii_safe(n),
        "sanitized": splitkit::PathTemplate::sanitize_component(n),
        "rendered": splitkit::PathTemplate::render(
            "{albumartist}/{album} ({year})/{track} - {title}",
            &[("albumartist", n), ("album", n), ("year", ""), ("track", "07"), ("title", n)]
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            true,
        ),
    });
    if let Some(p) = FileRules::disc_number_from_file_name(n) {
        v["discNumber"] = position(p);
    }
    if let Some(d) = FileRules::disc_number_from_folder_name(n) {
        v["folderDisc"] = d.into();
    }
    v
}
