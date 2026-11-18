//! Real artwork from the sample set (DRTAGGER_SAMPLES, default
//! ~/samples/real); skipped when the samples or the models are missing.

use std::path::PathBuf;
use visionkit::{ArtworkScanner, CoverCheck, FeaturePrinter};

fn samples() -> PathBuf {
    std::env::var("DRTAGGER_SAMPLES")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap()).join("samples/real"))
}

#[test]
fn scans_barcode_from_back_cover() {
    let back = samples().join("cue/1974 - Diamond Dogs (1984, W. Germany, RCA PD83889)/Scans/Back.png");
    if !back.exists() || !ArtworkScanner::has_ocr() {
        return;
    }
    let result = ArtworkScanner::default().scan_file(&back);
    assert!(
        !result.barcodes.is_empty(),
        "back cover should carry an EAN/UPC; texts: {:?}",
        &result.recognized_texts[..8]
    );
    for code in &result.barcodes {
        assert!([8, 12, 13].contains(&code.len()), "{code}");
    }
    assert!(result.catalog_numbers.iter().any(|c| c.contains("83889")), "{:?}", result.catalog_numbers);
}

#[test]
fn reads_japanese_catalog_number() {
    let back =
        samples().join("cue/Ella Fitzgerald & Oscar Peterson - Ella and Oscar (1975, JVC-XRCD)/Artwork/back.jpg");
    if !back.exists() || !ArtworkScanner::has_ocr() {
        return;
    }
    let result = ArtworkScanner::default().scan_file(&back);
    assert!(result.catalog_numbers.contains(&"VICJ-60097".to_string()), "{:?}", result.catalog_numbers);
}

#[test]
fn same_cover_is_not_reported_and_another_is() {
    let Some(printer) = FeaturePrinter::shared() else { return };
    let dogs = samples().join("cue/1974 - Diamond Dogs (1984, W. Germany, RCA PD83889)");
    let ella = samples().join("cue/Ella Fitzgerald & Oscar Peterson - Ella and Oscar (1975, JVC-XRCD)/folder.jpg");
    if !dogs.exists() || !ella.exists() {
        return;
    }
    let jpg = printer.feature_print_file(&dogs.join("Folder.jpg")).unwrap();
    let png = printer.feature_print_file(&dogs.join("Folder.png")).unwrap();
    let other = printer.feature_print_file(&ella).unwrap();
    // Vision: 0.77 (a 200 px thumbnail against the 1500 px scan) and 1.31.
    let same = jpg.distance(&png);
    let different = jpg.distance(&other);
    assert!(same < CoverCheck::DIFFERENT_THRESHOLD, "{same}");
    assert!(different >= CoverCheck::DIFFERENT_THRESHOLD, "{different}");
}

/// Live check against Cover Art Archive (as the Mac test):
/// COVERCHECK_IMAGE=<the Brautigam box cover>.
#[test]
fn cover_of_another_edition_is_reported() {
    let Ok(image) = std::env::var("COVERCHECK_IMAGE") else { return };
    let local = std::fs::read(image).unwrap();
    let check = CoverCheck::new("drtagger-tests/0.1 (+https://drtagger.priet.us)", None);
    let mk =
        |id: &str, title: &str| providerkit::Candidate::new(providerkit::Source::Musicbrainz, id, title, "Brautigam");
    let single = mk("757fed12-3193-4bfb-9f82-5abd962fe769", "Vol. 1");
    let boxed = mk("a4ec848c-d59a-4496-81df-1e1df94d8b91", "The Complete Sonatas");
    let d1 = check.distance(&local, &single).unwrap();
    let d2 = check.distance(&local, &boxed).unwrap();
    println!("single {d1} box {d2}");
    assert!(d1 >= CoverCheck::DIFFERENT_THRESHOLD);
    assert!(d2 < 0.3);
}
