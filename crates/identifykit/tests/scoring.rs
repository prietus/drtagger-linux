//! MatchScorer and IdentificationResult, ported from IdentifyKitTests.

use identifykit::*;
use librarykit::SourceHint;
use providerkit::{Candidate, CandidateMedium, CandidateTrack, Source};
use std::collections::{BTreeMap, HashMap};

fn tracks(durations: &[i64]) -> Vec<CandidateTrack> {
    durations
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let mut t = CandidateTrack::new(i as i64 + 1, &format!("T{}", i + 1));
            t.duration_ms = Some(d * 1000);
            t
        })
        .collect()
}

fn medium(position: i64, format: &str, tracks: Vec<CandidateTrack>, disc_ids: Vec<String>) -> CandidateMedium {
    CandidateMedium::new(position, Some(format.into()), None, tracks, disc_ids, None)
}

fn fmt_medium(position: i64, format: &str) -> CandidateMedium {
    CandidateMedium::new(position, Some(format.into()), None, vec![], vec![], None)
}

static NEXT_ID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn candidate(durations: &[i64], barcode: Option<&str>, catno: Option<&str>, disc_id: Option<&str>) -> Candidate {
    let ct = tracks(durations);
    let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut c = Candidate::new(Source::Musicbrainz, &format!("id-{id}"), "Diamond Dogs", "David Bowie");
    c.year = Some("1984".into());
    c.catalog_number = catno.map(String::from);
    c.track_count = Some(ct.len() as i64);
    c.barcode = barcode.map(String::from);
    c.media = vec![medium(1, "CD", ct.clone(), disc_id.map(|d| vec![d.to_string()]).unwrap_or_default())];
    c.tracks = ct;
    c
}

fn signals(durations: &[f64], barcode: Option<&str>, catno: Option<&str>, disc_id: Option<&str>) -> AlbumSignals {
    AlbumSignals {
        tracks: durations.iter().enumerate().map(|(i, d)| LocalTrack::new(i, 1, Some(*d))).collect(),
        barcodes: barcode.map(|b| vec![SignalValue::new(b, SignalOrigin::Artwork)]).unwrap_or_default(),
        catalog_numbers: catno.map(|c| vec![SignalValue::new(c, SignalOrigin::Artwork)]).unwrap_or_default(),
        disc_id: disc_id.map(String::from),
        artist_hint: Some("David Bowie".into()),
        album_hint: Some("Diamond Dogs".into()),
        year_hint: Some("1984".into()),
        ..Default::default()
    }
}

fn score(c: &Candidate, s: &AlbumSignals, origins: &[CandidateOrigin]) -> ScoredCandidate {
    MatchScorer::score(c, s, origins, None, 0)
}

use CandidateOrigin as O;

#[test]
fn barcode_plus_durations_is_confident_only_with_a_second_signal() {
    let c = candidate(&[200, 180, 240], Some("0035628388926"), Some("PD 83889"), None);
    let one = score(&c, &signals(&[201.0, 179.0, 241.0], Some("035628388926"), None, None), &[O::Barcode]);
    assert_eq!(one.confidence, MatchConfidence::Likely);
    assert!(one.reasons.iter().any(|r| r.starts_with("Barcode")));
    assert!(one.duration_fit.unwrap() > 0.8);
    let two = score(
        &c,
        &signals(&[201.0, 179.0, 241.0], Some("035628388926"), Some("pd-83889"), None),
        &[O::Barcode, O::CatalogNumber],
    );
    assert_eq!(two.confidence, MatchConfidence::Confident);
    assert!(two.score > one.score);
}

#[test]
fn track_count_mismatch_is_never_confident() {
    let c = candidate(&[200, 180, 240, 100], Some("0035628388926"), None, None);
    let s =
        score(&c, &signals(&[201.0, 179.0, 241.0], Some("0035628388926"), None, Some("x")), &[O::Barcode, O::DiscId]);
    assert_eq!(s.track_count_matches, Some(false));
    assert!(s.confidence < MatchConfidence::Likely);
}

#[test]
fn fingerprint_coverage_and_disc_id() {
    let c = candidate(&[200, 180, 240], None, None, Some("abc"));
    let vote = ReleaseVote { release_id: c.provider_id.clone(), track_indices: vec![0, 1, 2], best_score: 0.98 };
    let s = MatchScorer::score(
        &c,
        &signals(&[200.0, 180.0, 240.0], None, None, Some("abc")),
        &[O::DiscId, O::Fingerprint],
        Some(&vote),
        3,
    );
    assert_eq!(s.confidence, MatchConfidence::Confident);
    assert_eq!(s.fingerprint_coverage, Some(1.0));
    assert!(s.reasons.contains(&"Disc ID matches".to_string()));
    let far = candidate(&[300, 300, 300], None, None, None);
    let bad = score(&far, &signals(&[200.0, 180.0, 240.0], None, None, None), &[O::TextSearch]);
    assert_eq!(bad.confidence, MatchConfidence::Unlikely);
}

#[test]
fn hybrid_sacd_layers_and_format_bonus() {
    let layer = tracks(&[200, 180, 240]);
    let mut hybrid = Candidate::new(Source::Musicbrainz, "hybrid", "Aja", "Steely Dan");
    hybrid.year = Some("2024".into());
    hybrid.catalog_number = Some("B0035163-06, CAPP 139 SA".into());
    hybrid.media_format = Some("2×Hybrid SACD".into());
    hybrid.track_count = Some(6);
    hybrid.tracks = layer.clone();
    hybrid.media = vec![
        medium(1, "Hybrid SACD (CD layer)", layer.clone(), vec![]),
        medium(2, "Hybrid SACD (SACD layer, 2 channels)", layer.clone(), vec![]),
    ];
    let mut sacd = signals(&[200.0, 180.0, 240.0], None, Some("CAPP139 SA"), None);
    sacd.is_dsd = true;
    sacd.artist_hint = Some("Steely Dan".into());
    sacd.album_hint = Some("Aja".into());
    sacd.year_hint = None;
    let scored = score(&hybrid, &sacd, &[O::CatalogNumber]);
    assert_eq!(scored.track_count_matches, Some(true), "layers of one disc must not be summed");
    assert!(scored.reasons.contains(&"SACD edition".to_string()));
    assert!(scored.reasons.iter().any(|r| r.starts_with("Catalog number")));
    assert!(scored.confidence >= MatchConfidence::Likely);

    let mut vinyl = Candidate::new(Source::Musicbrainz, "lp", "Aja", "Steely Dan");
    vinyl.media_format = Some("12\" Vinyl".into());
    vinyl.track_count = Some(3);
    vinyl.tracks = layer.clone();
    vinyl.media = vec![medium(1, "12\" Vinyl", layer.clone(), vec![])];
    let lp = score(&vinyl, &sacd, &[O::TextSearch]);
    assert!(lp.score < scored.score - 30.0);
    assert!(lp.reasons.iter().any(|r| r.starts_with("Not a SACD")));

    let mut cd = signals(&[200.0, 180.0, 240.0], None, None, None);
    cd.sample_rate = Some(44100);
    cd.bits_per_sample = Some(16);
    assert_eq!(cd.local_format(), LocalFormat::Cd);
    let lp_for_cd = score(&vinyl, &cd, &[O::TextSearch]);
    assert!(lp_for_cd.reasons.contains(&"Vinyl cannot be the source of a CD rip".to_string()));
    let hybrid_for_cd = score(&hybrid, &cd, &[O::TextSearch]);
    assert_eq!(hybrid_for_cd.track_count_matches, Some(true));
    assert!(hybrid_for_cd.reasons.contains(&"CD edition".to_string()));
}

#[test]
fn folder_hints_separate_editions() {
    let s = AlbumSignals {
        artist_hint: Some("Linkin Park".into()),
        album_hint: Some("Meteora".into()),
        year_hint: Some("2003".into()),
        edition_hint: Some("20th Anniversary Edition".into()),
        source_hint: Some(SourceHint::Digital),
        ..Default::default()
    };
    let mut anniversary = Candidate::new(Source::Musicbrainz, "a", "Meteora (20th Anniversary Edition)", "Linkin Park");
    anniversary.year = Some("2023".into());
    anniversary.country = Some("XW".into());
    anniversary.media_format = Some("6×Digital Media".into());
    anniversary.track_count = Some(89);
    anniversary.media = (1..=6).map(|p| fmt_medium(p, "Digital Media")).collect();
    let mut original = Candidate::new(Source::Musicbrainz, "o", "Meteora", "Linkin Park");
    original.year = Some("2003".into());
    original.country = Some("US".into());
    original.media_format = Some("CD".into());
    original.track_count = Some(13);
    original.media = vec![fmt_medium(1, "CD")];
    let a = score(&anniversary, &s, &[O::TextSearch]);
    let o = score(&original, &s, &[O::TextSearch]);
    assert!(a.reasons.iter().any(|r| r.starts_with("Edition") && r.ends_with("matches")), "{:?}", a.reasons);
    assert!(a.reasons.contains(&"Digital release, as the folder says".to_string()));
    assert!(o.reasons.iter().any(|r| r.ends_with("not mentioned")));
    assert!(a.score > o.score + 15.0);

    let jp = AlbumSignals {
        artist_hint: Some("Diana Krall".into()),
        album_hint: Some("All for You".into()),
        country_hint: Some("JP".into()),
        source_hint: Some(SourceHint::Cd),
        sample_rate: Some(44100),
        bits_per_sample: Some(16),
        ..Default::default()
    };
    let mk = |id: &str, country: &str| {
        let mut c = Candidate::new(Source::Musicbrainz, id, "All for You", "Diana Krall");
        c.year = Some("1996".into());
        c.country = Some(country.into());
        c.media_format = Some("CD".into());
        c.track_count = Some(12);
        c
    };
    let j = score(&mk("j", "JP"), &jp, &[O::TextSearch]);
    let u = score(&mk("u", "US"), &jp, &[O::TextSearch]);
    assert!(j.reasons.contains(&"Country JP matches".to_string()));
    assert!(j.score > u.score);

    let dd = AlbumSignals {
        album_hint: Some("Diamond Dogs".into()),
        year_hint: Some("1974".into()),
        edition_year_hint: Some("1984".into()),
        country_hint: Some("DE".into()),
        ..Default::default()
    };
    let mut p84 = Candidate::new(Source::Musicbrainz, "84", "Diamond Dogs", "David Bowie");
    p84.year = Some("1984".into());
    p84.country = Some("DE".into());
    p84.media_format = Some("CD".into());
    let mut p74 = Candidate::new(Source::Musicbrainz, "74", "Diamond Dogs", "David Bowie");
    p74.year = Some("1974".into());
    p74.country = Some("GB".into());
    p74.media_format = Some("12\" Vinyl".into());
    let s84 = score(&p84, &dd, &[O::TextSearch]);
    let s74 = score(&p74, &dd, &[O::TextSearch]);
    assert!(s84.reasons.contains(&"Edition year 1984".to_string()));
    assert!(s84.score > s74.score);
    assert_eq!(MatchScorer::edition_keywords("20th Anniversary Edition"), ["20th", "anniversary"]);
    assert_eq!(MatchScorer::edition_keywords("Remastered"), ["remaster"]);
}

#[test]
fn format_compatibility_tiers() {
    use FormatCompatibility::*;
    let cand = |formats: &[&str], descriptions: &[&str]| {
        let mut c = Candidate::new(Source::Musicbrainz, &formats.join(""), "X", "Y");
        c.media_format = formats.first().map(|f| f.to_string());
        c.format_descriptions = descriptions.iter().map(|d| d.to_string()).collect();
        c.media = formats.iter().enumerate().map(|(i, f)| fmt_medium(i as i64 + 1, f)).collect();
        c
    };
    let sacd = cand(&["SACD"], &[]);
    let hybrid = cand(&["Hybrid SACD (CD layer)", "Hybrid SACD (SACD layer, 2 channels)"], &[]);
    let cd = cand(&["CD"], &[]);
    let vinyl = cand(&["12\" Vinyl"], &[]);
    let digital = cand(&["Digital Media"], &[]);
    let unknown = cand(&[], &[]);
    let box_set = cand(&["12\" Vinyl", "CD", "DVD"], &[]);
    let cassette = cand(&["Cassette"], &[]);
    let mut box_only = Candidate::new(Source::Musicbrainz, "b", "X", "Y");
    box_only.media_format = Some("Box set".into());
    let fc = MatchScorer::format_compatibility;

    let iso = AlbumSignals { is_dsd: true, is_sacd_image: true, ..Default::default() };
    assert_eq!(fc(&sacd, &iso), Fits);
    assert_eq!(fc(&hybrid, &iso), Fits);
    assert_eq!(fc(&cd, &iso), Unlikely);
    assert_eq!(fc(&digital, &iso), Unlikely);
    assert_eq!(fc(&unknown, &iso), Unknown);
    assert_eq!(fc(&box_only, &iso), Unknown);
    let sacd_r = AlbumSignals { source_hint: Some(SourceHint::Digital), ..iso.clone() };
    assert_eq!(fc(&digital, &sacd_r), Fits);
    assert_eq!(fc(&sacd, &sacd_r), Fits);
    let dsf = AlbumSignals { is_dsd: true, ..Default::default() };
    assert_eq!(fc(&digital, &dsf), Fits);
    assert_eq!(fc(&vinyl, &dsf), Unlikely);
    let rip = AlbumSignals { sample_rate: Some(44100), bits_per_sample: Some(16), ..Default::default() };
    assert_eq!(fc(&cd, &rip), Fits);
    assert_eq!(fc(&hybrid, &rip), Fits);
    assert_eq!(fc(&sacd, &rip), Unlikely);
    assert_eq!(fc(&digital, &rip), Fits);
    assert_eq!(fc(&vinyl, &rip), Unlikely);
    assert_eq!(fc(&box_set, &rip), Fits);
    assert_eq!(fc(&cassette, &rip), Unlikely);
    let vinyl_rip = AlbumSignals { source_hint: Some(SourceHint::Vinyl), ..rip.clone() };
    assert_eq!(fc(&vinyl, &vinyl_rip), Fits);
    assert_eq!(fc(&cd, &vinyl_rip), Unlikely);
    let hires = AlbumSignals { sample_rate: Some(96000), bits_per_sample: Some(24), ..Default::default() };
    assert_eq!(fc(&digital, &hires), Fits);
    assert_eq!(fc(&vinyl, &hires), Fits);
    assert_eq!(fc(&sacd, &hires), Fits);
    assert_eq!(fc(&cd, &hires), Unlikely);
    let xrcd = AlbumSignals { source_hint: Some(SourceHint::Cd), ..hires.clone() };
    assert_eq!(fc(&cd, &xrcd), Fits);
    assert_eq!(fc(&cd, &AlbumSignals::default()), Unknown);
    let mut discogs = Candidate::new(Source::Discogs, "d", "X", "Y");
    discogs.media_format = Some("SACD".into());
    discogs.format_descriptions = vec!["Hybrid".into(), "Multichannel".into()];
    assert_eq!(fc(&discogs, &rip), Fits);
}

#[test]
fn set_members_match_their_medium() {
    let m = |pos: i64, d: &[i64], id: Option<&str>| {
        medium(pos, "CD", tracks(d), id.map(|i| vec![i.into()]).unwrap_or_default())
    };
    let mut box_set = Candidate::new(Source::Musicbrainz, "box", "Box Set", "Led Zeppelin");
    box_set.track_count = Some(9);
    box_set.media = vec![
        m(1, &[200, 210, 220], Some("id1")),
        m(2, &[300, 310, 320], Some("id2")),
        m(3, &[400, 410, 420], Some("id3")),
    ];
    let mut single = Candidate::new(Source::Musicbrainz, "single", "Box Set", "Led Zeppelin");
    single.track_count = Some(3);
    single.media = vec![m(1, &[300, 310, 320], None)];

    let two = AlbumSignals {
        tracks: [300.0, 310.0, 320.0].iter().enumerate().map(|(i, d)| LocalTrack::new(i, 2, Some(*d))).collect(),
        present_discs: vec![2],
        declared_disc_total: Some(3),
        disc_count: 3,
        disc_ids: BTreeMap::from([(2, "id2".to_string())]),
        artist_hint: Some("Led Zeppelin".into()),
        album_hint: Some("Box Set".into()),
        ..Default::default()
    };
    let b = score(&box_set, &two, &[O::DiscId]);
    assert!(b.track_count_matches == Some(true) && b.duration_fit.unwrap_or(0.0) > 0.99);
    assert!(b.reasons.contains(&"Disc ID matches".to_string()) && b.reasons.contains(&"3 discs".to_string()));
    let sgl = score(&single, &two, &[O::TextSearch]);
    assert!(sgl.reasons.iter().any(|r| r.starts_with("Release has no disc 2")));
    assert!(b.score > sgl.score + 40.0);

    let partial = AlbumSignals {
        tracks: [(200.0, 1), (210.0, 1), (220.0, 1), (400.0, 3), (410.0, 3), (420.0, 3)]
            .iter()
            .enumerate()
            .map(|(i, (d, disc))| LocalTrack::new(i, *disc, Some(*d)))
            .collect(),
        present_discs: vec![1, 3],
        declared_disc_total: Some(3),
        disc_count: 3,
        disc_ids: BTreeMap::from([(1, "id1".to_string()), (3, "id3".to_string())]),
        ..Default::default()
    };
    let p = score(&box_set, &partial, &[O::DiscId]);
    assert!(p.track_count_matches == Some(true) && p.reasons.contains(&"Disc IDs match for 2 discs".to_string()));
    assert!(p.confidence >= MatchConfidence::Likely);

    let whole = AlbumSignals { tracks: (0..9).map(|i| LocalTrack::new(i, 1, None)).collect(), ..Default::default() };
    assert_eq!(score(&box_set, &whole, &[O::TextSearch]).track_count_matches, Some(true));
}

#[test]
fn video_disc_of_a_cd_plus_dvd_edition_is_ignored() {
    let t: Vec<CandidateTrack> = (1..=12)
        .map(|i| {
            let mut x = CandidateTrack::new(i, &format!("T{i}"));
            x.duration_ms = Some(200_000);
            x
        })
        .collect();
    let mut c = Candidate::new(Source::Musicbrainz, "wpzr", "Black Holes and Revelations", "Muse");
    c.catalog_number = Some("WPZR-30194".into());
    c.track_count = Some(18);
    c.barcode = Some("4943674070084".into());
    c.media = vec![
        medium(1, "CD", t, vec![]),
        CandidateMedium::new(2, Some("DVD-Video".into()), None, vec![], vec![], Some(6)),
    ];
    let s =
        AlbumSignals { tracks: (0..12).map(|i| LocalTrack::new(i, 1, Some(200.0))).collect(), ..Default::default() };
    assert_eq!(score(&c, &s, &[O::Barcode]).track_count_matches, Some(true));
    assert!(c.plausible_track_counts().contains(&12));
}

fn eight_tracks() -> AlbumSignals {
    AlbumSignals { tracks: (0..8).map(|i| LocalTrack::new(i, 1, Some(300.0))).collect(), ..Default::default() }
}

fn result(
    candidates: Vec<ScoredCandidate>,
    signals: AlbumSignals,
    fingerprinted: Option<usize>,
) -> IdentificationResult {
    IdentificationResult {
        signals,
        fingerprints: fingerprinted.map(|n| FingerprintOutcome {
            fingerprinted_tracks: n,
            tracks_with_hits: n,
            ..Default::default()
        }),
        candidates,
        discogs_candidates: vec![],
        log: vec![],
        started_at: 0.0,
        elapsed_seconds: 0.0,
    }
}

#[allow(clippy::too_many_arguments)]
fn scored(
    c: Candidate,
    score: f64,
    confidence: MatchConfidence,
    origin: CandidateOrigin,
    fit: Option<f64>,
    coverage: Option<f64>,
    format: Option<FormatCompatibility>,
) -> ScoredCandidate {
    ScoredCandidate {
        candidate: c,
        score,
        confidence,
        reasons: vec![],
        origins: vec![origin],
        duration_fit: fit,
        track_count_matches: Some(true),
        fingerprint_coverage: coverage,
        format_compatibility: format,
    }
}

#[test]
fn clear_fingerprint_match_needs_a_gap_and_no_contradiction() {
    let s = eight_tracks();
    let mk = |id: &str, score: f64, barcode: Option<&str>| {
        let mut c = Candidate::new(Source::Musicbrainz, id, "The Berlin Concert", "John Williams");
        c.barcode = barcode.map(String::from);
        scored(c, score, MatchConfidence::Likely, O::Fingerprint, Some(1.0), Some(1.0), None)
    };
    assert!(result(vec![mk("a", 115.0, None)], s.clone(), Some(8)).is_clear_fingerprint_match());
    assert!(!result(vec![mk("a", 115.0, None), mk("b", 90.0, None)], s.clone(), Some(8)).is_clear_fingerprint_match());
    assert!(!result(vec![mk("a", 115.0, None)], s.clone(), Some(6)).is_clear_fingerprint_match());
    let mut coded = s.clone();
    coded.barcodes = vec![SignalValue::new("4988031321904", SignalOrigin::Artwork)];
    assert!(!result(vec![mk("a", 115.0, Some("724383608829"))], coded, Some(8)).is_clear_fingerprint_match());
}

#[test]
fn fingerprints_outweigh_release_durations() {
    let c = candidate(&[144, 233, 206, 443, 241, 292, 189, 337, 257, 220], None, Some("WPCR-13132"), None);
    let s = signals(
        &[147.1, 236.4, 211.6, 445.2, 248.6, 298.3, 192.4, 339.2, 258.3, 222.6],
        None,
        Some("WPCR-13132"),
        None,
    );
    let all = ReleaseVote { release_id: c.provider_id.clone(), track_indices: (0..10).collect(), best_score: 0.9 };
    assert_eq!(MatchScorer::score(&c, &s, &[O::Fingerprint], Some(&all), 10).confidence, MatchConfidence::Confident);
    let some = ReleaseVote { release_id: c.provider_id.clone(), track_indices: (0..8).collect(), best_score: 0.9 };
    assert_eq!(MatchScorer::score(&c, &s, &[O::Fingerprint], Some(&some), 10).confidence, MatchConfidence::Possible);
}

#[test]
fn a_cd_is_never_the_source_of_dsd_files() {
    let c = candidate(&[200, 180, 240], None, Some("W2-25110"), None);
    let mut s = signals(&[200.0, 180.0, 240.0], None, Some("W2-25110"), None);
    s.is_dsd = true;
    let all = ReleaseVote { release_id: c.provider_id.clone(), track_indices: vec![0, 1, 2], best_score: 0.9 };
    assert_eq!(MatchScorer::score(&c, &s, &[O::Fingerprint], Some(&all), 3).confidence, MatchConfidence::Possible);
}

#[test]
fn one_catalog_number_entered_twice_is_no_rival() {
    let a = candidate(&[300, 300], None, Some("WPCR-13131"), None);
    let b = candidate(&[300, 300], None, Some("WPCR 13131"), None);
    assert!(MatchScorer::are_twins(&a, &b));
    assert!(!MatchScorer::are_twins(&a, &candidate(&[300, 300], None, Some("AMCY-4006"), None)));
}

#[test]
fn the_albums_cover_breaks_a_tie_between_proven_editions() {
    let mk = |id: &str, catno: &str| {
        let mut c = Candidate::new(Source::Musicbrainz, id, "Burn", "Deep Purple");
        c.catalog_number = Some(catno.into());
        scored(c, 90.0, MatchConfidence::Likely, O::Fingerprint, Some(1.0), Some(1.0), None)
    };
    let r = result(vec![mk("jp", "WPCR-13115"), mk("uk", "CDP 7 92611 2")], eight_tracks(), Some(8));
    let (jp, uk) = (r.candidates[0].id(), r.candidates[1].id());
    assert_eq!(
        r.cover_tie_break(&HashMap::from([(jp.clone(), 0.9), (uk.clone(), 0.2)])).map(|c| c.id()),
        Some(uk.clone())
    );
    assert!(r.cover_tie_break(&HashMap::from([(jp, 0.2), (uk, 0.5)])).is_none());
    assert!(r.cover_tie_break(&HashMap::new()).is_none());
}

#[test]
fn unofficial_sacd_is_tagged_as_the_album() {
    let mut s = eight_tracks();
    s.is_dsd = true;
    let mk = |id: &str, group: &str, format: &str, score: f64| {
        let mut c = Candidate::new(Source::Musicbrainz, id, "Burn", "Deep Purple");
        c.release_group_id = Some(group.into());
        c.media_format = Some(format.into());
        scored(
            c,
            score,
            MatchConfidence::Possible,
            O::Fingerprint,
            Some(0.5),
            Some(1.0),
            Some(FormatCompatibility::Unlikely),
        )
    };
    let pick = |c| result(c, s.clone(), Some(8)).album_only_pick().map(|p| p.candidate.provider_id.clone());
    assert_eq!(pick(vec![mk("lp", "burn", "12\" Vinyl", 75.0), mk("cd", "burn", "CD", 75.0)]).as_deref(), Some("cd"));
    assert_eq!(pick(vec![mk("cd", "burn", "CD", 75.0), mk("best-of", "hits", "CD", 75.0)]), None);
    assert_eq!(pick(vec![mk("cd", "burn", "CD", 75.0), mk("sacd", "burn", "SACD", 40.0)]), None);
}

#[test]
fn a_barcode_that_fits_the_tracks_is_clear() {
    let mut s = eight_tracks();
    s.barcodes = vec![SignalValue::new("4943674083770", SignalOrigin::Artwork)];
    let mk = |id: &str,
              barcode: &str,
              source: Source,
              score: f64,
              catno: Option<&str>,
              format: FormatCompatibility,
              fit: f64| {
        let mut c = Candidate::new(source, id, "Houses of the Holy", "Led Zeppelin");
        c.catalog_number = catno.map(String::from);
        c.barcode = Some(barcode.into());
        scored(c, score, MatchConfidence::Likely, O::Barcode, Some(fit), None, Some(format))
    };
    use FormatCompatibility::*;
    let jp = mk("jp", "4943674083770", Source::Musicbrainz, 100.0, None, Fits, 1.0);
    let clear = |c: Vec<ScoredCandidate>, s: &AlbumSignals| result(c, s.clone(), None).is_clear_barcode_match();
    assert!(clear(vec![jp.clone()], &s));
    assert!(clear(vec![jp.clone(), mk("d", "4943674083770", Source::Discogs, 80.0, None, Fits, 1.0)], &s));
    assert!(!clear(vec![jp.clone(), mk("us", "075678265327", Source::Musicbrainz, 85.0, None, Fits, 1.0)], &s));
    assert!(clear(vec![jp.clone(), mk("us", "075678265327", Source::Musicbrainz, 60.0, None, Fits, 1.0)], &s));
    assert!(!clear(vec![mk("jp", "4943674083770", Source::Musicbrainz, 100.0, None, Fits, 0.5)], &s));
    assert!(!clear(vec![mk("jp", "4943674083770", Source::Musicbrainz, 100.0, None, Unlikely, 1.0)], &s));
    assert!(!clear(vec![mk("us", "075678265327", Source::Musicbrainz, 100.0, None, Fits, 1.0)], &s));
    let mut coded = s.clone();
    coded.catalog_numbers = vec![SignalValue::new("WPCR-13134", SignalOrigin::Artwork)];
    assert!(clear(vec![mk("jp", "4943674083770", Source::Musicbrainz, 100.0, Some("WPCR-11615"), Fits, 1.0)], &coded));
    let mut other = s.clone();
    other.barcodes.push(SignalValue::new("075678265327", SignalOrigin::FolderName));
    assert!(clear(vec![jp], &other));
}

#[test]
fn scans_contradicting_the_tagged_edition() {
    let mut tagged = Candidate::new(Source::Musicbrainz, "wpcr", "Black Holes and Revelations", "Muse");
    tagged.catalog_number = Some("WPCR-12306".into());
    tagged.barcode = Some("4943674064373".into());
    let s = AlbumSignals {
        barcodes: vec![SignalValue::new("4943674070084", SignalOrigin::Artwork)],
        catalog_numbers: vec![SignalValue::new("WPZR-30194", SignalOrigin::FolderName)],
        ..Default::default()
    };
    assert!(MatchScorer::contradicts_local_codes(&tagged, &s));
    let mut right = Candidate::new(Source::Musicbrainz, "wpzr", "Black Holes and Revelations", "Muse");
    right.catalog_number = Some("WPZR-30194~5".into());
    right.barcode = Some("4943674070084".into());
    assert!(!MatchScorer::contradicts_local_codes(&right, &s));
    assert!(!MatchScorer::contradicts_local_codes(&tagged, &AlbumSignals::default()));
}

#[test]
fn normalisation() {
    assert_eq!(MatchScorer::normalize("The Rolling Stones"), "rolling stones");
    assert_eq!(MatchScorer::normalize("Leño – ¡Corre, corre!"), "leno corre corre");
    assert_eq!(MatchScorer::normalize("Björk’s “Homogenic” — 1997…"), "bjorks homogenic 1997");
    assert_eq!(MatchScorer::normalize("Mötley Crüe & The ½ Ⅻ ²"), "motley crue and the ½ ⅻ ²");
    assert!(MatchScorer::similar("Walkin'", "Walkin’"));
    assert!(MatchScorer::barcodes_equal("035628388926", "0035628388926"));
    assert!(!MatchScorer::barcodes_equal("035628388926", "035628388927"));
}

// IdentifyKitTests.sacdDiscCatalogIdentifiesItsEdition
#[test]
fn sacd_disc_catalog_identifies_its_edition() {
    // An Esoteric SACD: its master TOC names ESSJ-90329 (the disc) and the
    // original label's barcode (the album set).
    let layer = tracks(&[157, 200, 180]);
    let edition = |id: &str, format: &str| {
        let mut c = Candidate::new(Source::Discogs, id, "I Just Dropped By To Say Hello", "Johnny Hartman");
        c.catalog_number = Some("ESSJ-90329".into());
        c.media_format = Some(format.into());
        c.track_count = Some(3);
        c.tracks = layer.clone();
        c.media = vec![medium(1, format, layer.clone(), vec![])];
        c
    };
    let esoteric = edition("38985767", "SACD");
    let mut iso = signals(&[157.0, 200.0, 180.0], None, None, None);
    iso.is_dsd = true;
    iso.is_sacd_image = true;
    iso.artist_hint = Some("Johnny Hartman".into());
    iso.album_hint = Some("I JUST DROPPED BY TO SAY HELLO".into());
    iso.year_hint = None;
    let mut box_catalog = SignalValue::new("0602488540667", SignalOrigin::SacdText);
    box_catalog.detail = Some("album set".into());
    iso.catalog_numbers = vec![SignalValue::new("ESSJ-90329      ", SignalOrigin::SacdText), box_catalog];
    let reason = "The disc's own catalog number names this SACD edition".to_string();
    let scored = score(&esoteric, &iso, &[O::CatalogNumber]);
    assert!(scored.reasons.contains(&reason));
    assert_eq!(scored.confidence, MatchConfidence::Confident);

    // Only the disc's own number counts, and only for a SACD edition.
    let mut box_only = iso.clone();
    let mut only = SignalValue::new("ESSJ-90329", SignalOrigin::SacdText);
    only.detail = Some("album set".into());
    box_only.catalog_numbers = vec![only];
    assert_eq!(score(&esoteric, &box_only, &[O::CatalogNumber]).confidence, MatchConfidence::Likely);
    assert!(!score(&edition("cd", "CD"), &iso, &[O::CatalogNumber]).reasons.contains(&reason));
}
