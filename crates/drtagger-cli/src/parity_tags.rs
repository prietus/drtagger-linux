//! Tag parity dump: the same JSON as `MacDump tags` (parity/MacDump/
//! Sources/MacDump/Tags.swift). Every audio file is read; the first file of
//! each folder also goes through write (sample tags + cover), write keeping
//! pictures, write stripping them, and restore, on a scratch copy.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tagkit::{ContainerContents, Picture, PictureKind, TagContainer, TagSet, TaggedFile, field};

fn sha(d: &[u8]) -> String {
    hex::encode(Sha256::digest(d))
}

fn sha_file(p: &Path) -> String {
    std::fs::read(p).map_or_else(|_| "unreadable".to_string(), |d| sha(&d))
}

pub fn sample_tags() -> TagSet {
    let mut t = TagSet::new();
    t.set(field::TITLE, Some("So What"));
    t.set(field::ARTIST, Some("Miles Davis"));
    t.set(field::ALBUM, Some("Kind of Blue"));
    t.set(field::ALBUM_ARTIST, Some("Miles Davis"));
    t.set(field::DATE, Some("1997-03-25"));
    t.set(field::ORIGINAL_DATE, Some("1959-08-17"));
    t.set(field::TRACK_NUMBER, Some("1"));
    t.set(field::TRACK_TOTAL, Some("2"));
    t.set(field::DISC_NUMBER, Some("1"));
    t.set(field::DISC_TOTAL, Some("1"));
    t.put(field::LABEL, ["Columbia", "Legacy"]);
    t.set(field::CATALOG_NUMBER, Some("CK 64935"));
    t.set(field::BARCODE, Some("074646493526"));
    t.set(field::MB_ALBUM_ID, Some("rel-1"));
    t.set(field::MB_TRACK_ID, Some("rec-1"));
    t.set(field::MB_RELEASE_TRACK_ID, Some("trk-1"));
    t.put(field::PERFORMER, ["Miles Davis (trumpet)", "Paul Chambers (bass)"]);
    t.set(field::PRODUCER, Some("Teo Macero"));
    t.set(field::ISRC, Some("USSM15900001"));
    t.set("REPLAYGAIN_TRACK_GAIN", Some("-6.10 dB"));
    t.set(field::COMMENT, Some("Ünïcödé — ok"));
    t.put(field::GENRE, ["Jazz", "Modal"]);
    t.set(field::COMPILATION, Some("1"));
    t.set("ORIGINALYEAR", Some("1959"));
    t.set(field::CONDUCTOR, Some("Nobody"));
    t.set(field::ARTIST_SORT, Some("Davis, Miles"));
    t.set("MOOD", Some("Cool"));
    t
}

fn picture_json(p: &Picture) -> Value {
    json!({
        "kind": p.kind.0, "mimeType": p.mime_type, "width": p.width, "height": p.height,
        "byteCount": p.data.len(), "sha": sha(&p.data), "description": p.description,
    })
}

fn contents_json(c: &ContainerContents) -> Value {
    json!({
        "tags": c.tags.fields(),
        "pictures": c.pictures.iter().map(picture_json).collect::<Vec<_>>(),
        "rawSHA": sha(&c.raw_metadata),
        "rawLength": c.raw_metadata.len(),
        "audioDigest": c.audio_digest,
        "preserved": c.preserved.iter().map(|f| json!({ "id": f.id, "sha": sha(&f.payload) })).collect::<Vec<_>>(),
        "opaqueSHA": sha(&c.opaque),
    })
}

fn audio_files(path: &Path, out: &mut Vec<PathBuf>) {
    let Ok(meta) = std::fs::metadata(path) else { return };
    if !meta.is_dir() {
        if TagContainer::from_path(path).is_some() {
            out.push(path.to_path_buf());
        }
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else { return };
    for e in entries.flatten() {
        if !e.file_name().to_string_lossy().starts_with('.') {
            audio_files(&e.path(), out);
        }
    }
}

fn cycle(url: &Path, scratch: &Path, cover: &Picture) -> Result<Value, tagkit::TagFileError> {
    let copy = scratch.join(url.file_name().unwrap());
    let _ = std::fs::remove_file(&copy);
    std::fs::copy(url, &copy)?;
    let original = sha_file(&copy);
    let backup = TaggedFile::backup(&copy)?;
    TaggedFile::write(&copy, &sample_tags(), Some(std::slice::from_ref(cover)), None)?;
    let write = sha_file(&copy);
    let after = contents_json(&TaggedFile::read(&copy)?);
    let mut edited = sample_tags();
    edited.set(field::TITLE, Some("So What (edit)"));
    TaggedFile::write(&copy, &edited, None, None)?;
    let keep = sha_file(&copy);
    TaggedFile::write(&copy, &edited, Some(&[]), None)?;
    let strip = sha_file(&copy);
    TaggedFile::restore(&copy, &backup)?;
    let restored = sha_file(&copy) == original;
    let _ = std::fs::remove_file(&copy);
    Ok(json!({ "write": write, "afterWrite": after, "keep": keep, "strip": strip, "restoredOriginal": restored }))
}

/// `preview`: tags and pictures only (no audio hashing, no writes), for
/// whole libraries.
pub fn dump(roots: &[PathBuf], cover: &Path, preview: bool) -> Value {
    tagkit::set_vendor("drtagger for Mac");
    let cover = Picture::from_image(PictureKind::FRONT, std::fs::read(cover).expect("cover"), "image/jpeg");
    let mut files = vec![];
    for r in roots {
        audio_files(r, &mut files);
    }
    files.sort();
    let scratch = tempfile::tempdir().expect("scratch dir");
    let mut seen = std::collections::HashSet::new();
    let mut out = vec![];
    for url in files {
        let container = serde_json::to_value(TagContainer::from_path(&url).unwrap()).unwrap();
        let mut entry = json!({ "path": url, "container": container });
        let read = if preview { TaggedFile::read_for_preview(&url) } else { TaggedFile::read(&url) };
        match read {
            Ok(c) => entry["read"] = contents_json(&c),
            Err(e) => entry["readError"] = e.to_string().into(),
        }
        if !preview && seen.insert(url.parent().map(Path::to_path_buf)) {
            match cycle(&url, scratch.path(), &cover) {
                Ok(c) => entry["cycle"] = c,
                Err(e) => entry["cycleError"] = e.to_string().into(),
            }
        }
        out.push(entry);
    }
    json!({ "files": out })
}
