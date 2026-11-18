//! Checks against the real services, only with DRTAGGER_NETWORK_TESTS=1
//! (ported from ProviderKit's live suites). Run them one at a time
//! (`--test-threads=1`) to keep MusicBrainz's rate limit.

use providerkit::*;
use std::time::Instant;

const UA: &str = "drtagger-linux-tests/0.1 (+https://drtagger.priet.us)";

fn enabled() -> bool {
    std::env::var("DRTAGGER_NETWORK_TESTS").as_deref() == Ok("1")
}

#[test]
fn musicbrainz_disc_id_returns_the_exact_release() {
    if !enabled() {
        return;
    }
    let mb = MusicBrainzClient::new(UA);
    // Miles Davis All Stars – Walkin' (JVC XRCD), disc ID from the real rip.
    let releases =
        mb.lookup_disc_id("iOSL4j4VX_YutvVxL4QjWVsVEJE-", Some("1 5 170707 187 60862 98304 119694 139499")).unwrap();
    let r = &releases[0];
    assert!(r.title.to_lowercase().contains("walkin"));
    assert_eq!(r.media.len(), 1);
    assert_eq!(r.media[0].tracks.len(), 5);
    assert!(releases.len() >= 5, "fuzzy TOC match should list several editions");
    assert!(r.media[0].tracks[0].recording_id.is_some() && r.media[0].tracks[0].duration_ms.is_some());
}

#[test]
fn musicbrainz_barcode_and_catalog_searches() {
    if !enabled() {
        return;
    }
    let mb = MusicBrainzClient::new(UA);
    let releases = mb.search_by_barcode("4988002013944", 10).unwrap();
    assert!(releases.iter().any(|r| r.barcode.as_deref() == Some("4988002013944")));
    let rs = mb.search_by_catalog("CAPP139 SA", 10).unwrap();
    assert!(rs.iter().any(|r| r.catalog_numbers().contains(&"CAPP 139 SA".to_string())));
    assert!(rs.iter().any(|r| r.media_format.as_deref().is_some_and(|f| f.contains("SACD"))));
}

#[test]
fn concurrent_calls_are_spaced() {
    if !enabled() {
        return;
    }
    let mb = MusicBrainzClient::new(UA);
    let start = Instant::now();
    std::thread::scope(|s| {
        for _ in 0..4 {
            s.spawn(|| {
                let _ = mb.search_by_barcode("074646493526", 10);
            });
        }
    });
    let elapsed = start.elapsed().as_secs_f64();
    assert!(elapsed >= 3.1, "4 concurrent requests took {elapsed} s; they must be ≥ 1.05 s apart");
}

#[test]
fn art_sources_answer() {
    if !enabled() {
        return;
    }
    let caa = CoverArtArchiveClient::new(UA);
    assert!(caa.images("00000000-0000-0000-0000-000000000000").unwrap().is_empty());
    let albums = ITunesSearchClient::new(UA).search_albums("Miles Davis", "Kind of Blue", "us", 10).unwrap();
    assert!(albums.iter().any(|a| a.title.to_lowercase().contains("kind of blue")));
    let dz = DeezerClient::new(UA).search_albums("Miles Davis", "Kind of Blue", 10).unwrap();
    assert!(dz.iter().any(|a| a.title.to_lowercase().contains("kind of blue") && a.cover_xl.is_some()));
}

#[test]
fn cuetools_db_lookup() {
    if !enabled() {
        return;
    }
    let r = CUEToolsDBClient::new(UA).lookup("37:60712:98154:119544:139349:170557").unwrap();
    assert!(r.entries.iter().any(|e| e.id == "1310779"));
    assert!(r.metadata.iter().any(|m| m.barcode.as_deref() == Some("4988002013944")));
}

#[test]
fn wrong_keys_are_rejected() {
    if !enabled() {
        return;
    }
    assert!(matches!(AcoustIDClient::new("definitely-not-a-key", UA).validate_key(), KeyCheck::Invalid(_)));
    assert!(matches!(DiscogsClient::new(UA, "definitely-not-a-token").validate_token(), KeyCheck::Invalid(_)));
    assert!(matches!(FanartClient::new("definitely-not-a-key", UA).validate_key(), KeyCheck::Invalid(_)));
    assert_eq!(AcoustIDClient::new("", UA).validate_key(), KeyCheck::Invalid("No key entered".into()));
}

/// Real keys from the environment, when given.
#[test]
fn real_keys_are_accepted() {
    if !enabled() {
        return;
    }
    if let Ok(k) = std::env::var("ACOUSTID_KEY") {
        assert!(matches!(AcoustIDClient::new(&k, UA).validate_key(), KeyCheck::Valid(_)));
    }
    if let Ok(t) = std::env::var("DISCOGS_TOKEN") {
        let check = DiscogsClient::new(UA, &t).validate_token();
        assert!(check.message().starts_with("Signed in as"), "{check:?}");
        let d = DiscogsClient::new(UA, &t);
        let hits = d.search_by_barcode("4988031573259", 10).unwrap();
        assert!(!hits.is_empty());
        let full = d.release_detail(&hits[0].provider_id).unwrap();
        assert!(!full.tracks.is_empty());
    }
    if let Ok(k) = std::env::var("FANART_KEY") {
        assert!(matches!(FanartClient::new(&k, UA).validate_key(), KeyCheck::Valid(_)));
    }
}
