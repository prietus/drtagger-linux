//! Barcodes, OCR and cover similarity for artwork scans, standing in for
//! Apple Vision: zxing-cpp for EAN/UPC, PaddleOCR PP-OCRv5 and DINOv2 on
//! ONNX Runtime. The models come from `scripts/fetch-models.sh`; without
//! them barcodes still work and OCR and cover checks are skipped.

pub mod barcode;
pub mod cover_check;
pub mod embedding;
pub mod imaging;
pub mod models;
pub mod ocr;
pub mod scanner;

pub use cover_check::CoverCheck;
pub use embedding::{FeaturePrint, FeaturePrinter};
pub use models::models_dir;
pub use ocr::{TextLine, TextRecognizer};
pub use scanner::ArtworkScanner;

/// Frees the ONNX Runtime sessions not in use and hands freed heap back to
/// the system. A resident app calls it when nothing is being identified:
/// the sessions' arenas keep their peak size (hundreds of MB) otherwise.
pub fn release_idle_sessions() {
    if let Some(t) = TextRecognizer::shared() {
        t.release_idle();
    }
    if let Some(f) = FeaturePrinter::shared() {
        f.release_idle();
    }
    #[cfg(target_env = "gnu")]
    {
        unsafe extern "C" {
            fn malloc_trim(pad: usize) -> i32;
        }
        // SAFETY: glibc's malloc_trim only returns free heap pages.
        unsafe {
            malloc_trim(0);
        }
    }
}
