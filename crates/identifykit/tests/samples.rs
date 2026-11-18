//! Real samples (port of IdentifyKit's "Real samples" and CDDB live
//! suites). Signal collection runs when the samples and the bundled ffmpeg
//! are there; identification also needs DRTAGGER_NETWORK_TESTS=1 (and
//! ACOUSTID_KEY / DISCOGS_TOKEN for fingerprints and Discogs). Run with
//! `--test-threads=1`.

use identifykit::*;
use librarykit::{AlbumKind, DetectedAlbum, DetectedDisc, DiscTOC, LibraryScanner};
use splitkit::FFmpegTool;
use std::path::PathBuf;

const UA: &str = "drtagger-linux-tests/0.1 (+https://drtagger.priet.us)";

fn samples() -> PathBuf {
    std::env::var("DRTAGGER_SAMPLES")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap()).join("samples/real"))
}

fn network() -> bool {
    std::env::var("DRTAGGER_NETWORK_TESTS").as_deref() == Ok("1")
}

fn tool() -> Option<FFmpegTool> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../Vendor/ffmpeg");
    dir.join("ffmpeg").exists().then(|| FFmpegTool::new(dir.join("ffmpeg"), dir.join("ffprobe")))
}

fn album(relative: &str) -> Option<DetectedAlbum> {
    let url = samples().join(relative);
    url.exists().then(|| LibraryScanner::new().scan(&[url]).albums.into_iter().next()).flatten()
}

fn no_artwork() -> CollectorOptions {
    CollectorOptions { scan_artwork: false, ..Default::default() }
}

const DIAMOND_DOGS: &str = "cue/1974 - Diamond Dogs (1984, W. Germany, RCA PD83889)";
const WALKIN: &str = "cue-images/Miles Davis All Stars - Walkin' XRCD";

fn walkin_toc(album: &DetectedAlbum) -> DiscTOC {
    DiscTOC::from_cue(album.discs[0].cue.as_ref().unwrap(), &[170557]).unwrap()
}

#[test]
fn collects_signals_from_split_folder() {
    let (Some(tool), Some(album)) = (tool(), album(DIAMOND_DOGS)) else { return };
    let s = SignalCollector::new(&tool).collect(&album, None, &[], &[], &no_artwork(), &|_| {});
    assert_eq!(s.track_count(), 8);
    assert!(s.tracks.iter().all(|t| t.duration_seconds.unwrap_or(0.0) > 60.0));
    assert!(s.album_hint.as_deref().unwrap().to_lowercase().contains("diamond dogs"));
    assert!(s.artist_hint.as_deref().unwrap().to_lowercase().contains("bowie"));
    assert_eq!(s.year_hint.as_ref().map(|y| y.len()), Some(4));
    assert!(!s.existing_tags.is_empty());
    assert_eq!(s.local_format(), LocalFormat::Cd);
    // "(1984, W. Germany, RCA PD83889)": catalog, country and edition year.
    assert!(s.unique_catalog_numbers().contains(&"PD-83889".to_string()), "{:?}", s.catalog_numbers);
}

#[test]
fn collects_signals_from_cue_image_with_toc() {
    let (Some(tool), Some(album)) = (tool(), album(WALKIN)) else { return };
    let toc = walkin_toc(&album);
    let s = SignalCollector::new(&tool).collect(&album, Some(&toc), &[], &[], &no_artwork(), &|_| {});
    assert_eq!(s.disc_id.as_deref(), Some("iOSL4j4VX_YutvVxL4QjWVsVEJE-"));
    assert_eq!(s.tracks.len(), 5);
    assert!((s.tracks[0].duration_seconds.unwrap() - (60712.0 - 37.0) / 75.0).abs() < 0.01);
    assert_eq!(s.tracks[0].image_start, Some(37.0 / 75.0));
}

fn config() -> IdentifierConfig {
    let mut c = IdentifierConfig::new(UA);
    c.acoust_id_key = std::env::var("ACOUSTID_KEY").ok();
    c.discogs_token = std::env::var("DISCOGS_TOKEN").ok();
    c.scan_artwork = false;
    c
}

#[test]
fn identifies_walkin_from_toc() {
    if !network() {
        return;
    }
    let (Some(tool), Some(album)) = (tool(), album(WALKIN)) else { return };
    let toc = walkin_toc(&album);
    let result = Identifier::new(config(), tool).identify(&album, Some(&toc), &[], &[], &|_| {});
    println!("{}", result.log.join("\n"));
    let best = result.best().unwrap();
    assert!(best.candidate.title.to_lowercase().contains("walkin"));
    assert_eq!(best.candidate.all_tracks().len(), 5);
    assert!(best.confidence >= MatchConfidence::Likely);
    assert!(best.duration_fit.unwrap_or(0.0) > 0.8);
}

#[test]
fn identifies_meteora_edition_from_folder_name() {
    if !network() {
        return;
    }
    let Some(tool) = tool() else { return };
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("Linkin Park - 2003 - Meteora 20th Anniversary Edition (MQA)");
    std::fs::create_dir_all(&folder).unwrap();
    let album = DetectedAlbum::new(&folder, AlbumKind::TrackFolder, vec![DetectedDisc::new(1, &folder)], vec![]);
    let mut c = config();
    c.use_fingerprints = false;
    let result = Identifier::new(c, tool).identify(&album, None, &[], &[], &|_| {});
    println!("{}", result.log.join("\n"));
    let best = result.best().unwrap();
    assert!(best.candidate.title.to_lowercase().contains("anniversary"), "{}", best.candidate.title);
    assert!(best.candidate.media_format.as_deref().unwrap_or("").to_lowercase().contains("digital"));
}

#[test]
fn identifies_diamond_dogs_from_its_tags() {
    if !network() {
        return;
    }
    let (Some(tool), Some(album)) = (tool(), album(DIAMOND_DOGS)) else { return };
    let result = Identifier::new(config(), tool).identify(&album, None, &[], &[], &|_| {});
    println!("{}", result.log.join("\n"));
    let best = result.best().unwrap();
    assert!(best.candidate.title.to_lowercase().contains("diamond dogs"));
    assert!(best.candidate.artist.to_lowercase().contains("bowie"));
    assert_eq!(best.candidate.all_tracks().len(), 8);
    assert!(best.confidence >= MatchConfidence::Likely);
    // The files carry the MBID of a release that fits them: no fingerprints.
    assert!(best.origins.contains(&CandidateOrigin::TagMbid));
    assert!(result.fingerprints.is_none());
    assert!(result.log.iter().any(|l| l.starts_with("Fingerprints skipped: the release ID in the tags")));
}

#[test]
fn live_fuzzy_toc_lookup() {
    if !network() {
        return;
    }
    let Some(album) = album(WALKIN) else { return };
    let toc = walkin_toc(&album);
    let offsets: Vec<String> = toc.track_offsets.iter().map(|o| o.to_string()).collect();
    let q = format!(
        "cmd=cddb+query+{}+{}+{}+{}&proto=6",
        toc.freedb_disc_id(),
        toc.track_offsets.len(),
        offsets.join("+"),
        toc.lead_out / 75
    );
    let answer = CDDBResponder::live(UA).respond(&q);
    println!("{answer}");
    assert!(answer.starts_with("200 ") || answer.starts_with("211 "));
    assert!(answer.to_lowercase().contains("walkin"));
}
