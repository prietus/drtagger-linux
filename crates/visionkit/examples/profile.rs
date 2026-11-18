//! Times each stage of an artwork scan: `cargo run --release --example profile -- <image>…`

use std::time::Instant;
use visionkit::{TextRecognizer, barcode, imaging};

fn main() {
    let ocr = TextRecognizer::shared().expect("models");
    for path in std::env::args().skip(1) {
        let t = Instant::now();
        let image = imaging::load(path.as_ref(), 1800).expect("image");
        let load = t.elapsed();
        let t = Instant::now();
        let (cw, ccw) = (imaging::rotated(&image, true), imaging::rotated(&image, false));
        let codes: Vec<String> = [&image, &cw, &ccw].iter().flat_map(|i| barcode::detect(i)).collect();
        let bars = t.elapsed();
        let t = Instant::now();
        let lines = ocr.recognize(&image).unwrap();
        let text = t.elapsed();
        println!(
            "{}x{} load {:?} barcodes {:?} ({codes:?}) ocr {:?} ({} lines) {path}",
            image.width(),
            image.height(),
            load,
            bars,
            text,
            lines.len()
        );
    }
}
