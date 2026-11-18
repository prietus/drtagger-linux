//! Vision benchmark dumps, the same JSON as `MacDump vision-scan` and
//! `MacDump vision-pairs` (parity/MacDump/Sources/MacDump/Vision.swift):
//! the artwork scan of every album's prioritised images, and feature-print
//! distances for pairs of images. Paths are relative to `root`.

use identifykit::SignalCollector;
use librarykit::LibraryScanner;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;
use visionkit::{ArtworkScanner, FeaturePrinter};

fn relative(path: &Path, root: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).to_string_lossy().into_owned()
}

pub fn scan(root: &Path, roots: &[PathBuf], max_files: usize, threads: usize, list_only: bool) -> Value {
    let mut albums = LibraryScanner::new().scan(roots).albums;
    albums.sort_by(|a, b| a.url.cmp(&b.url));
    let files: Vec<(String, PathBuf)> = albums
        .iter()
        .flat_map(|a| {
            let name = relative(&a.url, root);
            SignalCollector::prioritised_artwork(&a.artwork_files, max_files)
                .into_iter()
                .map(move |f| (name.clone(), f))
        })
        .collect();
    if list_only {
        let tsv: Vec<String> = files.iter().map(|(a, f)| format!("{a}\t{}", relative(f, root))).collect();
        return Value::String(tsv.join("\n"));
    }
    eprintln!("{} album(s), {} image(s); OCR {}", albums.len(), files.len(), ArtworkScanner::has_ocr());
    let scanner = ArtworkScanner::default();
    let next = Mutex::new(0usize);
    let results = Mutex::new(vec![Value::Null; files.len()]);
    std::thread::scope(|s| {
        for _ in 0..threads.max(1) {
            s.spawn(|| {
                loop {
                    let i = {
                        let mut n = next.lock().unwrap();
                        let i = *n;
                        *n += 1;
                        i
                    };
                    let Some((album, file)) = files.get(i) else { break };
                    let start = Instant::now();
                    let r = scanner.scan_file(file);
                    let ms = start.elapsed().as_millis() as u64;
                    eprintln!("[{}/{}] {} ms {}", i + 1, files.len(), ms, relative(file, root));
                    results.lock().unwrap()[i] = json!({
                        "album": album,
                        "file": relative(file, root),
                        "barcodes": r.barcodes,
                        "catalogNumbers": r.catalog_numbers,
                        "recognizedTexts": r.recognized_texts,
                        "confidence": r.confidence,
                        "ms": ms,
                    });
                }
            });
        }
    });
    json!({ "albums": albums.len(), "images": results.into_inner().unwrap() })
}

/// `pairs`: lines "a<TAB>b", relative to `root`.
pub fn pairs(root: &Path, pairs: &Path) -> anyhow::Result<Value> {
    let printer = FeaturePrinter::shared().ok_or_else(|| anyhow::anyhow!("the cover model is not installed"))?;
    let text = std::fs::read_to_string(pairs)?;
    let mut cache = std::collections::HashMap::new();
    let mut print =
        |rel: &str| cache.entry(rel.to_string()).or_insert_with(|| printer.feature_print_file(&root.join(rel))).clone();
    let mut out = vec![];
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let Some((a, b)) = line.split_once('\t') else { continue };
        let (pa, pb) = (print(a), print(b));
        let raw = pa.as_ref().zip(pb.as_ref()).map(|(x, y)| x.raw_distance(y));
        out.push(json!({ "a": a, "b": b, "raw": raw, "distance": raw.map(visionkit::embedding::vision_scale) }));
    }
    Ok(json!({ "pairs": out }))
}
