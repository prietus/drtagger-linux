//! Split parity dump: the same JSON as `MacDump split` (parity/MacDump/
//! Sources/MacDump/Split.swift): every CUE image album split the way the
//! app does it, with each track's hashes, CRCs and provisional tags.

use librarykit::{AlbumKind, LibraryScanner};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use splitkit::{AlbumContext, FFmpegTool, ImageSplitter, SplitOptions, SplitOutcome};
use std::path::{Path, PathBuf};

fn relative(p: &Path, root: &Path) -> String {
    let p = librarykit::standardized(p);
    p.strip_prefix(root).map_or_else(|_| p.to_string_lossy().into_owned(), |r| r.to_string_lossy().into_owned())
}

fn outcome_json(o: &SplitOutcome, root: &Path) -> Value {
    let tracks: Vec<Value> = o
        .tracks
        .iter()
        .map(|t| {
            let mut tags: Vec<Vec<String>> = vec![];
            if let Ok(f) = flackit::FLACFile::open(&t.url)
                && let Some(vc) = f.vorbis_comment
            {
                tags = vc.fields.iter().map(|(n, v)| vec![n.clone(), v.clone()]).collect();
                tags.push(vec!["(vendor)".into(), vc.vendor.clone()]);
            }
            let sha = std::fs::read(&t.url).map_or_else(|_| "unreadable".into(), |d| hex::encode(Sha256::digest(d)));
            let mut j = json!({
                "number": t.number, "file": relative(&t.url, root), "samples": t.samples, "md5Hex": t.md5_hex,
                "streamInfoMD5Matches": t.stream_info_md5_matches, "crc32": t.crc32, "verified": t.verified,
                "sha": sha, "tags": tags,
            });
            if let Some(title) = &t.title {
                j["title"] = title.clone().into();
            }
            if let Some(c) = t.ctdb_crc32 {
                j["ctdbCRC32"] = c.into();
            }
            j
        })
        .collect();
    let mut d = json!({
        "outputFolder": relative(&o.output_folder, root),
        "format": o.format,
        "plan": o.plan,
        "verified": o.verified,
        "warnings": o.warnings,
        "tracks": tracks,
    });
    if let Some(toc) = &o.toc {
        d["toc"] = json!({
            "trackOffsets": toc.track_offsets, "leadOut": toc.lead_out, "musicBrainzDiscID": toc.musicbrainz_disc_id(),
            "freeDBDiscID": toc.freedb_disc_id(), "ctdbTOCString": toc.ctdb_toc_string(),
        });
    }
    if let Some(c) = o.ctdb_disc_crc32 {
        d["ctdbDiscCRC32"] = c.into();
    }
    d
}

pub fn dump(roots: &[PathBuf], ffmpeg_dir: &Path, into: &Path) -> Value {
    splitkit::provisional_tags::set_vendor("drtagger for Mac");
    let tool = FFmpegTool::new(ffmpeg_dir.join("ffmpeg"), ffmpeg_dir.join("ffprobe"));
    let root = librarykit::standardized(&std::path::absolute(into).unwrap());
    let albums = LibraryScanner::new().scan(roots).albums;
    let mut out = vec![];
    for album in albums.iter().filter(|a| a.kind == AlbumKind::CueImage) {
        let mut discs: Vec<_> = album.discs.iter().filter(|d| d.cue.is_some()).collect();
        discs.sort_by_key(|d| d.number);
        let multi = discs.len() > 1;
        for disc in &discs {
            let context = AlbumContext::from_cue(
                disc.cue.as_ref().unwrap(),
                multi.then_some(disc.number),
                multi.then_some(discs.len() as u32),
                None,
            );
            let options = SplitOptions { overwrite_existing: true, ..Default::default() };
            let mut entry = json!({ "album": album.id(), "disc": disc.number });
            match ImageSplitter::new(tool.clone()).split(disc, &context, &root, &options, &mut |_| {}) {
                Ok(o) => entry["outcome"] = outcome_json(&o, &root),
                Err(e) => entry["error"] = e.to_string().into(),
            }
            out.push(entry);
        }
    }
    json!({ "splits": out })
}
