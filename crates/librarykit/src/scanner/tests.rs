//! Port of LibraryScannerTests.swift. Each test builds a throwaway folder
//! tree; audio files are stubs because the scanner only looks at names,
//! existence and sizes.

use super::*;
use crate::cue::CueSheet;
use crate::disc_toc::DiscTOC;
use crate::sacd_probe::tests::fake_sacd_bytes;
use crate::split_plan::CueSplitPlan;

struct TempTree {
    dir: tempfile::TempDir,
}

impl TempTree {
    fn new() -> Self {
        TempTree { dir: tempfile::Builder::new().prefix("libscan-").tempdir().unwrap() }
    }
    fn root(&self) -> PathBuf {
        self.dir.path().to_path_buf()
    }
    fn dir(&self, path: &str) -> PathBuf {
        let p = self.dir.path().join(path);
        std::fs::create_dir_all(&p).unwrap();
        p
    }
    fn file(&self, path: &str, contents: &str) -> PathBuf {
        self.bytes(path, contents.as_bytes())
    }
    fn stub(&self, path: &str, size: usize) -> PathBuf {
        self.bytes(path, &vec![0u8; size])
    }
    fn bytes(&self, path: &str, data: &[u8]) -> PathBuf {
        let p = self.dir.path().join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, data).unwrap();
        p
    }
}

fn cue(image: &str, tracks: usize) -> String {
    let mut s = format!("PERFORMER \"Artist\"\nTITLE \"Album\"\nFILE \"{image}\" WAVE\n");
    for n in 1..=tracks {
        s += &format!("  TRACK {n:02} AUDIO\n    TITLE \"Track {n}\"\n    INDEX 01 {:02}:00:00\n", (n - 1) * 3);
    }
    s
}

fn scan(roots: &[PathBuf]) -> ScanResult {
    LibraryScanner::new().scan(roots)
}

fn name(p: &Path) -> String {
    p.file_name().unwrap().to_string_lossy().into_owned()
}

#[test]
fn several_numbered_images_in_one_folder_are_one_album() {
    let t = TempTree::new();
    let folder = "Led Zeppelin - Box Set 1990 Japan";
    for n in 1..=3 {
        t.stub(&format!("{folder}/Led Zeppelin - Box Set (Disc {n}).flac"), 10);
        t.file(
            &format!("{folder}/Led Zeppelin - Box Set (Disc {n}).cue"),
            &cue(&format!("Led Zeppelin - Box Set (Disc {n}).flac"), 4 + n),
        );
    }
    let result = scan(&[t.root().join(folder)]);
    assert_eq!(result.albums.len(), 1);
    let album = &result.albums[0];
    assert_eq!(album.kind, AlbumKind::CueImage);
    assert_eq!(album.discs.iter().map(|d| d.number).collect::<Vec<_>>(), [1, 2, 3]);
    assert_eq!(album.discs.iter().map(DetectedDisc::track_count).collect::<Vec<_>>(), [5, 6, 7]);
    assert_eq!(album.disc_position(), None, "a whole box in one folder is not a set member");

    // Unrelated images without numbers: keep the first, report the second.
    let other = t.dir("Two Albums");
    t.stub("Two Albums/a.flac", 10);
    t.file("Two Albums/a.cue", &cue("a.flac", 3));
    t.stub("Two Albums/b.flac", 10);
    t.file("Two Albums/b.cue", &cue("b.flac", 3));
    let r2 = scan(&[other]);
    assert_eq!(r2.albums.len(), 1);
    assert!(r2.issues.iter().any(|i| i.message.contains("ignored")));
}

#[test]
fn gaps_appended_to_previous_track_cue() {
    // EAC "gaps appended": track 2's INDEX 00 ends file 1, its INDEX 01
    // follows the FILE line of file 2 (Live At The Fillmore, disc 2).
    let text = "FILE \"01.flac\" WAVE\n  TRACK 01 AUDIO\n    TITLE \"Tell The Truth\"\n    INDEX 01 00:00:00\n  TRACK 02 AUDIO\n    TITLE \"Nobody Knows You\"\n    INDEX 00 11:04:71\nFILE \"02.flac\" WAVE\n    INDEX 01 00:00:00\nFILE \"03.flac\" WAVE\n  TRACK 03 AUDIO\n    TITLE \"Roll It Over\"\n    INDEX 01 00:00:00";
    let cue = CueSheet::parse_text(text).unwrap();
    let t2 = cue.audio_tracks().into_iter().find(|t| t.number == 2).unwrap().clone();
    assert_eq!(t2.title.as_deref(), Some("Nobody Knows You"));
    assert_eq!(t2.start().unwrap().frames, 0);
    assert_eq!(t2.start_file_offset(), 1);
    assert_eq!(t2.index(0).unwrap().frames, (11 * 60 + 4) * 75 + 71);
    assert!(cue.files[1].tracks.is_empty());

    let (f1, f2, f3) = ((11 * 60 + 5) * 75, 3000, 4000);
    let toc = DiscTOC::from_cue(&cue, &[f1, f2, f3]).unwrap();
    assert_eq!(toc.track_offsets, [150, 150 + f1, 150 + f1 + f2]);
    let plan = CueSplitPlan::make(&cue, &[f1 * 588, f2 * 588, f3 * 588], 44100, 150).unwrap();
    assert_eq!(
        plan.numbered_tracks().iter().map(|t| t.start_sample).collect::<Vec<_>>(),
        [0, f1 * 588, (f1 + f2) * 588]
    );
}

#[test]
fn area_subfolder_takes_the_parent_name() {
    let t = TempTree::new();
    t.stub("Beethoven_ Piano Sonatas Vol.9/Stereo/01 - I. Allegro.dsf", 10);
    t.stub("Beethoven_ Piano Sonatas Vol.9/Stereo/02 - II. Andante.dsf", 10);
    t.file("Beethoven_ Piano Sonatas Vol.9/Beethoven_ Piano Sonatas Vol.9.xml", "");
    let result = scan(&[t.root()]);
    let album = &result.albums[0];
    assert_eq!(name(&album.url), "Stereo");
    assert_eq!(album.folder_name(), "Beethoven_ Piano Sonatas Vol.9");
    assert_eq!(album.title_hint().as_deref(), Some("Beethoven Piano Sonatas Vol.9"));
    assert_eq!(album.disc_position(), None, "a volume is not a disc");
}

#[test]
fn disc_tokens_in_names() {
    let n = |s| FileRules::disc_number_from_file_name(s);
    assert_eq!(n("Led Zeppelin - Box Set (Disc 2).cue").unwrap().0, 2);
    assert_eq!(n("CANCIONEROS DEL SIGLO DE ORO  Colombina  1451-1595 (disc 1-3).iso"), Some((1, Some(3))));
    assert_eq!(n("Don Giovanni CD2").unwrap().0, 2);
    assert_eq!(n("Live - Disc 3 of 4").unwrap().1, Some(4));
    assert_eq!(n("Bach - Cantatas Vol. 40"), None);
    assert_eq!(n("AC-DC_(Japanese_SICP-1700-18)"), None);
    assert_eq!(n("King Crimson - Red (2024 CD FLAC)"), None);
    let s = FileRules::stripping_disc_token;
    assert_eq!(s("Led Zeppelin - Box Set (Disc 2)"), "Led Zeppelin - Box Set");
    assert_eq!(s("CANCIONEROS  Colombina  1451-1595 (disc 1-3).iso"), "CANCIONEROS Colombina 1451-1595");
    assert_eq!(s("Don Giovanni CD2"), "Don Giovanni");
    assert_eq!(s("DEBUSSY: PIANO WORKS - Disc 1 of 4"), "DEBUSSY: PIANO WORKS");
    assert_eq!(s("Bach - Cantatas Vol. 40"), "Bach - Cantatas Vol. 40", "a volume number is not an extension");
    assert_eq!(n("DEBUSSY_ PIANO WORKS - Disc 3 of 4 (disc 3-4).iso").unwrap().1, Some(4));
    assert_eq!(FileRules::disc_number_from_folder_name("CD1"), Some(1));
    assert_eq!(FileRules::disc_number_from_folder_name("Disc 2 - Live"), Some(2));
    assert_eq!(FileRules::disc_number_from_folder_name("disco_3"), Some(3));
    assert_eq!(FileRules::disc_number_from_folder_name("Scans"), None);
    assert_eq!(FileRules::disc_number_from_folder_name("CD Quality"), None);
}

#[test]
fn track_folder_with_artwork() {
    let t = TempTree::new();
    let album = t.dir("Artist - Album (2001)");
    t.stub("Artist - Album (2001)/01 - One.flac", 10);
    t.stub("Artist - Album (2001)/10 - Ten.flac", 10);
    t.stub("Artist - Album (2001)/02 - Two.flac", 10);
    t.file("Artist - Album (2001)/folder.jpg", "");
    t.file("Artist - Album (2001)/Scans/back.png", "");
    t.file("Artist - Album (2001)/._01 - One.flac", "");
    t.file("Artist - Album (2001)/rip.log", "");

    let result = scan(&[t.root()]);
    assert_eq!(result.albums.len(), 1);
    let a = &result.albums[0];
    assert_eq!(a.kind, AlbumKind::TrackFolder);
    assert_eq!(a.url, album);
    assert_eq!(a.discs.len(), 1);
    assert_eq!(
        a.discs[0].track_files.iter().map(DetectedTrackFile::file_name).collect::<Vec<_>>(),
        ["01 - One.flac", "02 - Two.flac", "10 - Ten.flac"]
    );
    assert_eq!(a.artwork_files.iter().map(|p| name(p)).collect::<Vec<_>>(), ["folder.jpg", "back.png"]);
    assert_eq!(a.discs[0].log_files.len(), 1);
    assert_eq!(a.title_hint().as_deref(), Some("Album"));
    assert_eq!(a.artist_hint().as_deref(), Some("Artist"));
    assert_eq!(a.year_hint().as_deref(), Some("2001"));
    assert_eq!(a.formats().into_iter().collect::<Vec<_>>(), [AudioFormat::Flac]);
    assert!(result.issues.is_empty());
}

#[test]
fn cue_image_album() {
    let t = TempTree::new();
    t.stub("Album/Album.ape", 100);
    t.file("Album/Album.cue", &cue("Album.ape", 4));
    let result = scan(&[t.root()]);
    assert_eq!(result.albums.len(), 1);
    let a = &result.albums[0];
    assert_eq!(a.kind, AlbumKind::CueImage);
    let image = a.discs[0].image_file.as_ref().unwrap();
    assert_eq!(image.format, AudioFormat::Ape);
    assert_eq!(image.file_size, 100);
    assert_eq!(a.discs[0].cue.as_ref().unwrap().audio_tracks().len(), 4);
    assert_eq!(a.track_count(), 4);
    assert_eq!(a.title_hint().as_deref(), Some("Album"));
    assert_eq!(a.artist_hint().as_deref(), Some("Artist"));
}

#[test]
fn cue_with_bin_image() {
    let t = TempTree::new();
    t.stub("Rip/disc.bin", 50);
    t.file("Rip/disc.cue", "FILE \"disc.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n");
    let a = &scan(&[t.root()]).albums[0];
    assert_eq!(a.kind, AlbumKind::CueImage);
    assert_eq!(a.discs[0].image_file.as_ref().unwrap().file_name(), "disc.bin");
}

#[test]
fn cue_multi_file_album() {
    let t = TempTree::new();
    t.stub("Ella/01 - Mean to Me.wav", 5);
    t.stub("Ella/02 - How Long.wav", 5);
    t.file(
        "Ella/Ella.cue",
        "FILE \"01 - Mean to Me.wav\" WAVE\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\nFILE \"02 - How Long.wav\" WAVE\n  TRACK 02 AUDIO\n    INDEX 01 00:00:00",
    );
    let result = scan(&[t.root()]);
    assert_eq!(result.albums.len(), 1);
    let a = &result.albums[0];
    assert_eq!(a.kind, AlbumKind::CueMultiFile);
    assert_eq!(a.discs[0].track_files.len(), 2);
    assert_eq!(a.track_count(), 2);
}

#[test]
fn orphan_cue_becomes_hint_for_split_tracks() {
    let t = TempTree::new();
    t.stub("Split/01 - A.flac", 5);
    t.stub("Split/02 - B.flac", 5);
    t.file("Split/Split.cue", &cue("Split.ape", 2));
    let result = scan(&[t.root()]);
    assert_eq!(result.albums.len(), 1);
    let a = &result.albums[0];
    assert_eq!(a.kind, AlbumKind::TrackFolder);
    assert_eq!(a.discs[0].cue.as_ref().unwrap().title.as_deref(), Some("Album"));
    assert_eq!(a.discs[0].track_files.len(), 2);
    assert_eq!(result.issues.len(), 1);
    assert!(result.issues[0].message.contains("missing"));
}

#[test]
fn multi_disc_folders() {
    let t = TempTree::new();
    t.stub("Box/CD1/01.flac", 1);
    t.stub("Box/CD1/02.flac", 1);
    t.stub("Box/Disc 2/01.flac", 1);
    t.file("Box/cover.jpg", "");
    let result = scan(&[t.root()]);
    assert_eq!(result.albums.len(), 1);
    let a = &result.albums[0];
    assert_eq!(a.kind, AlbumKind::TrackFolder);
    assert_eq!(a.discs.iter().map(|d| d.number).collect::<Vec<_>>(), [1, 2]);
    assert_eq!(a.discs[0].track_files.len(), 2);
    assert_eq!(a.discs[1].track_files.len(), 1);
    assert_eq!(a.track_count(), 3);
    assert_eq!(a.artwork_files.len(), 1);
    assert_eq!(name(&a.url), "Box");
}

#[test]
fn multi_disc_with_images_per_disc() {
    let t = TempTree::new();
    t.stub("Set/Disc 1/d1.flac", 1);
    t.file("Set/Disc 1/d1.cue", &cue("d1.flac", 2));
    t.stub("Set/Disc 2/d2.flac", 1);
    t.file("Set/Disc 2/d2.cue", &cue("d2.flac", 5));
    let result = scan(&[t.root()]);
    assert_eq!(result.albums.len(), 1);
    let a = &result.albums[0];
    assert_eq!(a.kind, AlbumKind::CueImage);
    assert_eq!(a.discs.len(), 2);
    assert_eq!(a.track_count(), 7);
}

#[test]
fn nested_library_finds_every_album() {
    let t = TempTree::new();
    t.stub("Music/Rock/A/01.flac", 1);
    t.stub("Music/Rock/B/01.wv", 1);
    t.stub("Music/Jazz/C/CD1/01.dsf", 1);
    t.stub("Music/Jazz/C/CD2/01.dsf", 1);
    t.file("Music/notes.txt", "");
    let result = scan(&[t.root()]);
    let mut names: Vec<String> = result.albums.iter().map(|a| name(&a.url)).collect();
    names.sort();
    assert_eq!(names, ["A", "B", "C"]);
    assert!(result.folders_visited >= 6);
}

#[test]
fn sacd_iso_album_and_data_iso() {
    let t = TempTree::new();
    t.bytes("ISOs/Album.iso", &fake_sacd_bytes(false, true, b"Fake Album", b"Fake Artist", 1));
    t.stub("ISOs/linux.iso", SACDProbe::SECTOR_SIZE * 520);
    t.file("ISOs/Album.xml", "<x/>");
    t.file("ISOs/front.jpg", "");
    let result = scan(&[t.root()]);
    assert_eq!(result.albums.len(), 1);
    let a = &result.albums[0];
    assert_eq!(a.kind, AlbumKind::SacdIso);
    assert_eq!(name(&a.url), "Album.iso");
    assert_eq!(a.sacd.as_ref().unwrap().track_count(), 3);
    assert_eq!(a.track_count(), 3);
    assert_eq!(a.title_hint().as_deref(), Some("Disc Title"));
    assert_eq!(a.artist_hint().as_deref(), Some("Fake Artist"));
    assert_eq!(a.artwork_files.len(), 1);
    assert!(result.issues.iter().any(|i| name(&i.url) == "linux.iso"));
}

#[test]
fn single_files_as_roots() {
    let t = TempTree::new();
    let iso = t.bytes("Album.iso", &fake_sacd_bytes(false, true, b"Fake Album", b"Fake Artist", 1));
    let flac = t.stub("Loose/01.flac", 1);
    let result = scan(&[iso, flac]);
    let mut kinds: Vec<&str> = result.albums.iter().map(|a| a.kind.raw_value()).collect();
    kinds.sort();
    assert_eq!(kinds, ["sacdISO", "trackFolder"]);
}

#[test]
fn duplicate_roots_are_collapsed() {
    let t = TempTree::new();
    t.stub("A/01.flac", 1);
    let result = scan(&[t.root(), t.root().join("A")]);
    assert_eq!(result.albums.len(), 1);
}

#[test]
fn cue_names_resolve_ignoring_case_and_normalisation() {
    // Windows rippers write "TRACK01.WAV" for "Track01.wav"; macOS file
    // systems accept that, Linux ones need the lookup.
    let t = TempTree::new();
    t.stub("Rip/Disc Image.flac", 20);
    t.file("Rip/rip.cue", "FILE \"disc image.FLAC\" WAVE\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n");
    t.stub("Acc/Leño.flac", 20); // NFC on disk
    t.file("Acc/a.cue", "FILE \"Len\u{0303}o.flac\" WAVE\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n"); // NFD in the sheet
    let result = scan(&[t.root()]);
    assert_eq!(result.albums.len(), 2);
    for a in &result.albums {
        assert_eq!(a.kind, AlbumKind::CueImage, "{:?}", a.url);
    }
    let rip = result.albums.iter().find(|a| name(&a.url) == "Rip").unwrap();
    assert_eq!(rip.discs[0].image_file.as_ref().unwrap().file_name(), "Disc Image.flac");
    assert!(result.issues.is_empty());
}
