//! Barcodes and catalog numbers off album artwork (IdentifyKit's
//! ArtworkScanner): OBI strips, back covers, booklet pages.

use crate::ocr::TextRecognizer;
use crate::{barcode, imaging};
use identifykit::{ArtworkScanResult, ArtworkScanning, CatalogNumberParser};
use image::RgbImage;
use std::collections::BTreeSet;
use std::path::Path;

#[derive(Clone, Copy, Debug)]
pub struct ArtworkScanner {
    /// Longest side the image is read at: OCR is slow on 5000 px booklet
    /// scans and barcodes are found reliably at 2000 px.
    pub max_pixel: u32,
}

impl Default for ArtworkScanner {
    fn default() -> Self {
        ArtworkScanner { max_pixel: 1800 }
    }
}

impl ArtworkScanner {
    /// Whether OCR is available (the models are installed); barcodes are
    /// read either way.
    pub fn has_ocr() -> bool {
        TextRecognizer::shared().is_some()
    }

    fn recognize_text(image: &RgbImage) -> (Vec<String>, f32) {
        let Some(ocr) = TextRecognizer::shared() else { return (vec![], 0.0) };
        let lines = ocr.recognize(image).unwrap_or_default();
        let confidence =
            if lines.is_empty() { 0.0 } else { lines.iter().map(|l| l.confidence).sum::<f32>() / lines.len() as f32 };
        (lines.into_iter().map(|l| l.text).collect(), confidence)
    }

    fn barcodes_turned(image: &RgbImage) -> BTreeSet<String> {
        let mut codes: BTreeSet<String> = barcode::detect(image).into_iter().collect();
        codes.extend(barcode::detect(&imaging::rotated(image, true)));
        codes.extend(barcode::detect(&imaging::rotated(image, false)));
        codes
    }

    pub fn scan_file(&self, path: &Path) -> ArtworkScanResult {
        let empty = || ArtworkScanResult {
            url: path.to_path_buf(),
            barcodes: vec![],
            catalog_numbers: vec![],
            recognized_texts: vec![],
            confidence: 0.0,
        };
        let Some(full) = imaging::load(path, u32::MAX) else { return empty() };
        let (fw, fh) = full.dimensions();
        let image = imaging::downscaled(full.clone(), self.max_pixel);
        let (cw, ccw) = (imaging::rotated(&image, true), imaging::rotated(&image, false));
        // Barcodes: the three orientations, they are cheap. zxing needs
        // more pixels per bar than Vision: when the working size shows
        // none, the full-size image, and small images enlarged.
        let mut barcodes: BTreeSet<String> = [&image, &cw, &ccw].into_iter().flat_map(barcode::detect).collect();
        if barcodes.is_empty() && fw.max(fh) > self.max_pixel {
            barcodes.extend(Self::barcodes_turned(&full));
        }
        for factor in [2, 3] {
            if !barcodes.is_empty() || fw.max(fh) * factor > 2 * self.max_pixel {
                break;
            }
            barcodes.extend(Self::barcodes_turned(&imaging::upscaled(&full, factor)));
        }
        drop(full);
        let (mut texts, mut confidence) = Self::recognize_text(&image);
        let catalogs = |texts: &[String]| -> Vec<String> {
            CatalogNumberParser::extract(texts).iter().map(|c| c.formatted()).collect()
        };
        let mut catalog_numbers = catalogs(&texts);
        // OBI strips are usually photographed rotated; only rescan when the
        // first pass found no catalog number.
        let (w, h) = image.dimensions();
        let elongated = w.max(h) as f64 / w.min(h).max(1) as f64 >= 1.6;
        if catalog_numbers.is_empty() && elongated {
            for rotated in [&ccw, &cw] {
                let (r_texts, r_conf) = Self::recognize_text(rotated);
                let r_cats = catalogs(&r_texts);
                if !r_cats.is_empty() {
                    texts = r_texts;
                    confidence = r_conf;
                    catalog_numbers = r_cats;
                    break;
                }
            }
        }
        ArtworkScanResult {
            url: path.to_path_buf(),
            barcodes: barcodes.into_iter().collect(),
            catalog_numbers,
            recognized_texts: texts,
            confidence,
        }
    }
}

impl ArtworkScanning for ArtworkScanner {
    fn scan(&self, image: &Path) -> Option<ArtworkScanResult> {
        Some(self.scan_file(image))
    }
}
