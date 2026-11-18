//! Port of TagKitTests' Containers, Library index and Composed file names
//! suites: write sample tags and a cover into every container, read them
//! back, check the audio digest never changes and that a restore is
//! byte-exact.

use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use tagkit::{Picture, PictureKind, TagSet, TaggedFile, field};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn fixture(name: &str) -> Vec<u8> {
    let path = if name == "tone.flac" {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../flackit/tests/fixtures/tone.flac")
    } else {
        fixtures().join(name)
    };
    std::fs::read(path).unwrap()
}

fn sha(path: &Path) -> String {
    hex(&Sha256::digest(std::fs::read(path).unwrap()))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn cover() -> Picture {
    Picture::from_image(PictureKind::FRONT, fixture("cover.jpg"), "image/jpeg")
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
    t
}

const CHECKED: &[&str] = &[
    field::TITLE,
    field::ARTIST,
    field::ALBUM,
    field::ALBUM_ARTIST,
    field::DATE,
    field::TRACK_NUMBER,
    field::TRACK_TOTAL,
    field::DISC_NUMBER,
    field::LABEL,
    field::CATALOG_NUMBER,
    field::BARCODE,
    field::MB_ALBUM_ID,
    field::MB_TRACK_ID,
    field::MB_RELEASE_TRACK_ID,
    field::PERFORMER,
    field::PRODUCER,
    field::ISRC,
    "REPLAYGAIN_TRACK_GAIN",
    field::COMMENT,
];

fn round_trip(path: &Path) {
    let ext = path.extension().unwrap().to_string_lossy().into_owned();
    let original = sha(path);
    let before = TaggedFile::read(path).unwrap();
    let backup = TaggedFile::backup(path).unwrap();
    let cover = cover();
    let tags = sample_tags();
    let report = TaggedFile::write(path, &tags, Some(std::slice::from_ref(&cover)), None).unwrap();
    assert_eq!(report.audio_digest, before.audio_digest);

    let after = TaggedFile::read(path).unwrap();
    assert_eq!(after.audio_digest, before.audio_digest, "{ext}: audio bytes changed");
    for name in CHECKED {
        assert_eq!(after.tags.get(name), tags.get(name), "{ext}: {name}");
    }
    assert_eq!(after.pictures.len(), 1, "{ext}: picture");
    assert_eq!(after.pictures[0].data, cover.data);
    assert!(after.pictures[0].is_front());
    assert_eq!((after.pictures[0].width, after.pictures[0].height), (600, 600));

    // A second write with no pictures given keeps the cover; an empty list drops it.
    let mut edited = tags.clone();
    edited.set(field::TITLE, Some("So What (edit)"));
    TaggedFile::write(path, &edited, None, None).unwrap();
    assert_eq!(TaggedFile::read(path).unwrap().pictures.len(), 1, "{ext}");
    TaggedFile::write(path, &edited, Some(&[]), None).unwrap();
    let stripped = TaggedFile::read(path).unwrap();
    assert!(stripped.pictures.is_empty(), "{ext}");
    assert_eq!(stripped.tags.first(field::TITLE), Some("So What (edit)"));
    assert_eq!(stripped.audio_digest, before.audio_digest);

    // Restore puts the original file back byte for byte.
    TaggedFile::restore(path, &backup).unwrap();
    assert_eq!(sha(path), original, "{ext}: restore is not byte-exact");
}

fn temp_with(name: &str, data: &[u8]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    std::fs::write(&path, data).unwrap();
    (dir, path)
}

#[test]
fn wav() {
    let (_d, p) = temp_with("t.wav", &fixture("tone.wav"));
    round_trip(&p);
}

#[test]
fn flac() {
    let (_d, p) = temp_with("t.flac", &fixture("tone.flac"));
    assert_eq!(TaggedFile::read(&p).unwrap().tags.first(field::ARTIST), Some("drtagger"));
    round_trip(&p);
}

#[test]
fn aiff_and_alac() {
    let (_d, p) = temp_with("t.aiff", &fixture("tone.aiff"));
    round_trip(&p);
    let (_d2, m) = temp_with("t.m4a", &fixture("tone.m4a"));
    round_trip(&m);
}

pub fn dsf() -> Vec<u8> {
    let audio: Vec<u8> = (0..4096 * 2 * 3u32).map(|i| i.wrapping_mul(31) as u8).collect();
    let mut d = b"DSD ".to_vec();
    d.extend(28u64.to_le_bytes());
    d.extend(((28 + 52 + 12 + audio.len()) as u64).to_le_bytes());
    d.extend(0u64.to_le_bytes());
    d.extend(b"fmt ");
    d.extend(52u64.to_le_bytes());
    for v in [1u32, 0, 2, 2, 2_822_400, 1] {
        d.extend(v.to_le_bytes());
    }
    d.extend((audio.len() as u64 / 2 * 8).to_le_bytes());
    d.extend(4096u32.to_le_bytes());
    d.extend(0u32.to_le_bytes());
    d.extend(b"data");
    d.extend(((12 + audio.len()) as u64).to_le_bytes());
    d.extend(audio);
    d
}

pub fn dff() -> Vec<u8> {
    let audio: Vec<u8> = (0..3000u32).map(|i| i.wrapping_mul(7) as u8).collect();
    let mut body = b"DSD ".to_vec();
    body.extend(b"FVER");
    body.extend(4u64.to_be_bytes());
    body.extend(0x0105_0000u32.to_be_bytes());
    let mut prop = b"SND ".to_vec();
    prop.extend(b"FS  ");
    prop.extend(4u64.to_be_bytes());
    prop.extend(2_822_400u32.to_be_bytes());
    body.extend(b"PROP");
    body.extend((prop.len() as u64).to_be_bytes());
    body.extend(prop);
    body.extend(b"DSD ");
    body.extend((audio.len() as u64).to_be_bytes());
    body.extend(audio);
    let mut d = b"FRM8".to_vec();
    d.extend((body.len() as u64).to_be_bytes());
    d.extend(body);
    d
}

#[test]
fn dsf_and_dff() {
    let (_d, p) = temp_with("t.dsf", &dsf());
    round_trip(&p);
    let (_d2, p2) = temp_with("t.dff", &dff());
    round_trip(&p2);
}

#[test]
fn apev2() {
    let mut fake = b"wvpk".to_vec();
    fake.extend((0..5000u32).map(|i| i.wrapping_mul(13) as u8));
    fake.extend(b"TAG");
    fake.extend([0u8; 125]); // a stray ID3v1 that must be dropped
    let (_d, p) = temp_with("t.wv", &fake);
    let before = TaggedFile::read(&p).unwrap();
    let backup = TaggedFile::backup(&p).unwrap();
    let back = Picture::from_image(PictureKind::BACK, fixture("back.png"), "image/png");
    TaggedFile::write(&p, &sample_tags(), Some(&[cover(), back]), None).unwrap();
    let after = TaggedFile::read(&p).unwrap();
    assert_eq!(after.audio_digest, before.audio_digest);
    for name in CHECKED {
        assert_eq!(after.tags.get(name), sample_tags().get(name), "{name}");
    }
    assert_eq!(after.pictures.len(), 2);
    assert_eq!(after.pictures[1].kind, PictureKind::BACK);
    assert_eq!(after.pictures[1].mime_type, "image/png");
    let bytes = std::fs::read(&p).unwrap();
    assert_eq!(&bytes[bytes.len() - 32..bytes.len() - 24], b"APETAGEX");
    TaggedFile::restore(&p, &backup).unwrap();
    assert_eq!(std::fs::read(&p).unwrap(), fake);
}

#[test]
fn a_changed_audio_digest_leaves_the_original() {
    let (dir, p) = temp_with("t.wav", &fixture("tone.wav"));
    let original = sha(&p);
    let err = TaggedFile::write(&p, &sample_tags(), None, Some("0000")).unwrap_err();
    assert!(matches!(err, tagkit::TagFileError::AudioChanged { .. }));
    assert_eq!(sha(&p), original);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "temporary file removed");
}

#[test]
fn library_index_and_duplicates() {
    let dir = tempfile::tempdir().unwrap();
    let album = |name: &str, mbid: Option<&str>, discogs: Option<&str>, barcode: Option<&str>, cat: Option<&str>| {
        let folder = dir.path().join(name);
        std::fs::create_dir_all(&folder).unwrap();
        let url = folder.join("01.wav");
        std::fs::write(&url, fixture("tone.wav")).unwrap();
        let mut t = TagSet::new();
        t.set(field::ALBUM, Some(name));
        t.set(field::MB_ALBUM_ID, mbid);
        t.set(field::DISCOGS_RELEASE_ID, discogs);
        t.set(field::BARCODE, barcode);
        t.set(field::CATALOG_NUMBER, cat);
        TaggedFile::write(&url, &t, None, None).unwrap();
    };
    album("A", Some("AAAA-1"), None, None, None);
    album("A copy", Some("aaaa-1"), None, None, None);
    album("B", None, Some("27786006"), Some("4988031573259"), Some("POCS-1950"));
    album("C", None, None, Some("4988031573259"), Some("POCS 1950"));
    let index = tagkit::LibraryIndexer::scan(dir.path(), &BTreeMap::new(), &mut |_, _| {});
    assert_eq!(index.len(), 4);
    assert_eq!(TaggedFile::read_tags(&dir.path().join("A/01.wav")).unwrap().first(field::MB_ALBUM_ID), Some("AAAA-1"));
    let name = |e: &tagkit::LibraryIndexEntry| e.folder.file_name().unwrap().to_string_lossy().into_owned();
    let a: HashSet<PathBuf> = [dir.path().join("A")].into();
    let dup_a = tagkit::LibraryIndexer::duplicates(&index, Some("mb:aaaa-1"), None, &[], &a);
    assert_eq!(dup_a.iter().map(name).collect::<Vec<_>>(), ["A copy"]);
    let dup_b = tagkit::LibraryIndexer::duplicates(
        &index,
        Some("discogs:27786006"),
        Some("4988031573259"),
        &["POCS-1950".to_string()],
        &HashSet::new(),
    );
    let names: HashSet<String> = dup_b.iter().map(name).collect();
    assert_eq!(names, ["B".to_string(), "C".to_string()].into(), "C matches by barcode and catalog");
    let again = tagkit::LibraryIndexer::scan(dir.path(), &index, &mut |_, _| {});
    assert_eq!(again, index);
}

#[test]
fn non_utf8_names_open_and_write() {
    use std::os::unix::ffi::OsStringExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(std::ffi::OsString::from_vec(b"09 Neuk\xF6ln.wav".to_vec())); // Latin-1 name
    std::fs::write(&path, fixture("tone.wav")).unwrap();
    let mut t = TagSet::new();
    t.set(field::TITLE, Some("Neuköln"));
    TaggedFile::write(&path, &t, None, None).unwrap();
    assert_eq!(TaggedFile::read_tags(&path).unwrap().first(field::TITLE), Some("Neuköln"));
}
