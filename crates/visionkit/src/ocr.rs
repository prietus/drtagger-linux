//! Text recognition with PaddleOCR's PP-OCRv5 mobile models on ONNX
//! Runtime: DB text detection, then CTC line recognition (Chinese,
//! Japanese incl. vertical lines, English and Latin digits/punctuation).
//! Pre- and post-processing follow PaddleOCR's predictor defaults.

use crate::models::{SessionPool, model};
use image::RgbImage;
use imageproc::contours::{BorderType, find_contours};
use imageproc::geometric_transformations::{Border, Interpolation, Projection, warp_into};
use imageproc::geometry::min_area_rect;
use imageproc::point::Point;
use ort::value::Tensor;
use std::path::Path;
use std::sync::OnceLock;

const DET_THRESH: f32 = 0.3;
const BOX_THRESH: f32 = 0.6;
const UNCLIP_RATIO: f32 = 1.5;
const MAX_CANDIDATES: usize = 1000;
const REC_HEIGHT: u32 = 48;
const REC_MIN_WIDTH: u32 = 320;
const REC_MAX_WIDTH: u32 = 3200;
const REC_BATCH: usize = 6;

/// One recognised line.
#[derive(Clone, Debug, PartialEq)]
pub struct TextLine {
    pub text: String,
    pub confidence: f32,
    /// Corners in image pixels: top-left, top-right, bottom-right, bottom-left.
    pub quad: [(f32, f32); 4],
}

pub struct TextRecognizer {
    det: SessionPool,
    rec: SessionPool,
    /// CTC classes: blank, the dictionary, then a space.
    characters: Vec<String>,
}

impl TextRecognizer {
    /// The shared recognizer; None when the models are not installed.
    pub fn shared() -> Option<&'static TextRecognizer> {
        static SHARED: OnceLock<Option<TextRecognizer>> = OnceLock::new();
        SHARED
            .get_or_init(|| {
                let (det, rec, yml) = (model("ocr/det.onnx")?, model("ocr/rec.onnx")?, model("ocr/rec.yml")?);
                TextRecognizer::new(&det, &rec, &yml)
            })
            .as_ref()
    }

    pub(crate) fn release_idle(&self) {
        self.det.release_idle();
        self.rec.release_idle();
    }

    pub fn new(det: &Path, rec: &Path, rec_yml: &Path) -> Option<TextRecognizer> {
        let dict = Self::dictionary(&std::fs::read_to_string(rec_yml).ok()?);
        if dict.is_empty() {
            return None;
        }
        let mut characters = vec![String::new()];
        characters.extend(dict);
        characters.push(" ".into());
        Some(TextRecognizer { det: SessionPool::new(det.into()), rec: SessionPool::new(rec.into()), characters })
    }

    /// The `character_dict` list of a PaddleX inference.yml.
    fn dictionary(yml: &str) -> Vec<String> {
        let mut out = vec![];
        let mut inside = false;
        for line in yml.lines() {
            if line.trim_end() == "  character_dict:" {
                inside = true;
                continue;
            }
            if !inside {
                continue;
            }
            let Some(item) = line.strip_prefix("  - ") else { break };
            let item = match item.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
                Some(quoted) => quoted.replace("''", "'"),
                None => item.to_string(),
            };
            out.push(item);
        }
        out
    }

    /// Every line found, top to bottom, left to right.
    pub fn recognize(&self, image: &RgbImage) -> ort::Result<Vec<TextLine>> {
        let quads = self.detect(image)?;
        // Tall boxes are vertical Japanese or Latin text turned either way:
        // both quarter turns are read and the surer one kept.
        let (mut crops, turned): (Vec<RgbImage>, Vec<Option<RgbImage>>) = quads.iter().map(|q| crop(image, q)).unzip();
        let mut alternate = vec![None; quads.len()];
        for (i, t) in turned.into_iter().enumerate() {
            if let Some(t) = t {
                alternate[i] = Some(crops.len());
                crops.push(t);
            }
        }
        let read = self.read_lines(&crops)?;
        let lines: Vec<(String, f32)> = (0..quads.len())
            .map(|i| match alternate[i] {
                Some(a) if read[a].1 > read[i].1 => read[a].clone(),
                _ => read[i].clone(),
            })
            .collect();
        Ok(quads
            .into_iter()
            .zip(lines)
            .filter(|(_, (text, _))| !text.trim().is_empty())
            .map(|(quad, (text, confidence))| TextLine { text, confidence, quad })
            .collect())
    }

    // MARK: Detection

    fn detect(&self, image: &RgbImage) -> ort::Result<Vec<[(f32, f32); 4]>> {
        let (w, h) = image.dimensions();
        // limit_type "min" with limit_side_len 64, max_side_limit 4000.
        let mut ratio = 1.0f64;
        if w.min(h) < 64 {
            ratio = 64.0 / w.min(h) as f64;
        }
        if (w.max(h) as f64 * ratio) > 4000.0 {
            ratio = 4000.0 / w.max(h) as f64;
        }
        let round32 = |v: f64| (((v / 32.0).round() as u32) * 32).max(32);
        let (dw, dh) = (round32(w as f64 * ratio), round32(h as f64 * ratio));
        let resized = image::imageops::resize(image, dw, dh, image::imageops::FilterType::Triangle);
        // BGR, with the ImageNet mean/std applied in that channel order.
        let mean = [0.485f32, 0.456, 0.406];
        let std = [0.229f32, 0.224, 0.225];
        let plane = (dw * dh) as usize;
        let mut data = vec![0f32; 3 * plane];
        for (i, p) in resized.pixels().enumerate() {
            let bgr = [p[2], p[1], p[0]];
            for c in 0..3 {
                data[c * plane + i] = (bgr[c] as f32 / 255.0 - mean[c]) / std[c];
            }
        }
        let tensor = Tensor::from_array(([1usize, 3, dh as usize, dw as usize], data))?;
        let mut session = self.det.lease()?;
        let input = session.inputs()[0].name().to_string();
        let outputs = session.run(ort::inputs![input => tensor])?;
        let (shape, prob) = outputs[0].try_extract_tensor::<f32>()?;
        let (ph, pw) = (shape[shape.len() - 2] as u32, shape[shape.len() - 1] as u32);
        let prob = prob.to_vec();
        drop(outputs);
        drop(session);

        let mask = image::GrayImage::from_fn(pw, ph, |x, y| {
            image::Luma([if prob[(y * pw + x) as usize] > DET_THRESH { 255 } else { 0 }])
        });
        let (sx, sy) = (w as f32 / pw as f32, h as f32 / ph as f32);
        let mut boxes = vec![];
        for contour in find_contours::<i32>(&mask).into_iter().filter(|c| c.border_type == BorderType::Outer) {
            if boxes.len() >= MAX_CANDIDATES {
                break;
            }
            let Some((rect, short)) = mini_box(&contour.points) else { continue };
            if short < 3.0 {
                continue;
            }
            if box_score(&prob, pw, ph, &rect) < BOX_THRESH {
                continue;
            }
            let Some((rect, short)) = unclip(&rect) else { continue };
            if short < 5.0 {
                continue;
            }
            let quad =
                rect.map(|(x, y)| ((x * sx).round().clamp(0.0, w as f32), (y * sy).round().clamp(0.0, h as f32)));
            let (bw, bh) = (dist(quad[0], quad[1]), dist(quad[0], quad[3]));
            if bw <= 3.0 || bh <= 3.0 {
                continue;
            }
            boxes.push(quad);
        }
        Ok(sorted_boxes(boxes))
    }

    // MARK: Recognition

    fn read_lines(&self, crops: &[RgbImage]) -> ort::Result<Vec<(String, f32)>> {
        let mut out = vec![(String::new(), 0f32); crops.len()];
        // Batches of similar aspect ratio, as PaddleOCR does.
        let mut order: Vec<usize> = (0..crops.len()).collect();
        let ratio = |c: &RgbImage| c.width() as f32 / c.height().max(1) as f32;
        order.sort_by(|&a, &b| ratio(&crops[a]).total_cmp(&ratio(&crops[b])));
        let mut session = None;
        for batch in order.chunks(REC_BATCH) {
            let max_ratio =
                batch.iter().map(|&i| ratio(&crops[i])).fold(REC_MIN_WIDTH as f32 / REC_HEIGHT as f32, f32::max);
            let width = ((REC_HEIGHT as f32 * max_ratio) as u32).min(REC_MAX_WIDTH);
            let plane = (REC_HEIGHT * width) as usize;
            let mut data = vec![0f32; batch.len() * 3 * plane];
            for (b, &i) in batch.iter().enumerate() {
                let crop = &crops[i];
                let rw = ((REC_HEIGHT as f32 * ratio(crop)).ceil() as u32).clamp(1, width);
                let resized = image::imageops::resize(crop, rw, REC_HEIGHT, image::imageops::FilterType::Triangle);
                let base = b * 3 * plane;
                for (x, y, p) in resized.enumerate_pixels() {
                    let bgr = [p[2], p[1], p[0]];
                    for c in 0..3 {
                        data[base + c * plane + (y * width + x) as usize] = (bgr[c] as f32 / 255.0 - 0.5) / 0.5;
                    }
                }
            }
            let tensor = Tensor::from_array(([batch.len(), 3, REC_HEIGHT as usize, width as usize], data))?;
            let session = match &mut session {
                Some(s) => s,
                None => session.insert(self.rec.lease()?),
            };
            let input = session.inputs()[0].name().to_string();
            let outputs = session.run(ort::inputs![input => tensor])?;
            let (shape, probs) = outputs[0].try_extract_tensor::<f32>()?;
            let (steps, classes) = (shape[1] as usize, shape[2] as usize);
            for (b, &i) in batch.iter().enumerate() {
                out[i] = self.ctc_decode(&probs[b * steps * classes..(b + 1) * steps * classes], steps, classes);
            }
        }
        Ok(out)
    }

    fn ctc_decode(&self, probs: &[f32], steps: usize, classes: usize) -> (String, f32) {
        let mut text = String::new();
        let (mut total, mut count) = (0f32, 0usize);
        let mut previous = usize::MAX;
        for t in 0..steps {
            let row = &probs[t * classes..(t + 1) * classes];
            let (best, p) =
                row.iter().copied().enumerate().fold((0, f32::MIN), |m, (i, p)| if p > m.1 { (i, p) } else { m });
            if best != 0
                && best != previous
                && let Some(c) = self.characters.get(best)
            {
                text.push_str(c);
                total += p;
                count += 1;
            }
            previous = best;
        }
        (text, if count == 0 { 0.0 } else { total / count as f32 })
    }
}

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

/// The minimum-area rectangle of `points`, corners ordered top-left,
/// top-right, bottom-right, bottom-left, with its shorter side.
fn mini_box(points: &[Point<i32>]) -> Option<([(f32, f32); 4], f32)> {
    if points.len() < 3 {
        return None;
    }
    let rect = min_area_rect(points);
    Some(order_corners(rect.map(|p| (p.x as f32, p.y as f32))))
}

fn order_corners(mut pts: [(f32, f32); 4]) -> ([(f32, f32); 4], f32) {
    pts.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (tl, bl) = if pts[1].1 > pts[0].1 { (pts[0], pts[1]) } else { (pts[1], pts[0]) };
    let (tr, br) = if pts[3].1 > pts[2].1 { (pts[2], pts[3]) } else { (pts[3], pts[2]) };
    let ordered = [tl, tr, br, bl];
    let short = dist(tl, tr).min(dist(tl, bl));
    (ordered, short)
}

/// Mean probability inside the box.
fn box_score(prob: &[f32], w: u32, h: u32, quad: &[(f32, f32); 4]) -> f32 {
    let xmin = quad.iter().map(|p| p.0.floor()).fold(f32::MAX, f32::min).clamp(0.0, w as f32 - 1.0) as u32;
    let xmax = quad.iter().map(|p| p.0.ceil()).fold(f32::MIN, f32::max).clamp(0.0, w as f32 - 1.0) as u32;
    let ymin = quad.iter().map(|p| p.1.floor()).fold(f32::MAX, f32::min).clamp(0.0, h as f32 - 1.0) as u32;
    let ymax = quad.iter().map(|p| p.1.ceil()).fold(f32::MIN, f32::max).clamp(0.0, h as f32 - 1.0) as u32;
    let (mut sum, mut n) = (0f32, 0usize);
    for y in ymin..=ymax {
        for x in xmin..=xmax {
            if inside(quad, (x as f32, y as f32)) {
                sum += prob[(y * w + x) as usize];
                n += 1;
            }
        }
    }
    if n == 0 { 0.0 } else { sum / n as f32 }
}

/// Inside or on the edge of a convex quadrilateral.
fn inside(quad: &[(f32, f32); 4], p: (f32, f32)) -> bool {
    let mut sign = 0f32;
    for i in 0..4 {
        let (a, b) = (quad[i], quad[(i + 1) % 4]);
        let cross = (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0);
        if cross.abs() < 1e-3 {
            continue;
        }
        if sign == 0.0 {
            sign = cross.signum();
        } else if cross.signum() != sign {
            return false;
        }
    }
    true
}

/// Grows the rectangle by area × ratio / perimeter on every side (the
/// Vatti offset of a rectangle, whose min-area box is the grown rectangle).
fn unclip(rect: &[(f32, f32); 4]) -> Option<([(f32, f32); 4], f32)> {
    let (w, h) = (dist(rect[0], rect[1]), dist(rect[0], rect[3]));
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let d = w * h * UNCLIP_RATIO / (2.0 * (w + h));
    let ux = ((rect[1].0 - rect[0].0) / w, (rect[1].1 - rect[0].1) / w);
    let uy = ((rect[3].0 - rect[0].0) / h, (rect[3].1 - rect[0].1) / h);
    let grow = |p: (f32, f32), sx: f32, sy: f32| (p.0 + d * (sx * ux.0 + sy * uy.0), p.1 + d * (sx * ux.1 + sy * uy.1));
    let grown =
        [grow(rect[0], -1.0, -1.0), grow(rect[1], 1.0, -1.0), grow(rect[2], 1.0, 1.0), grow(rect[3], -1.0, 1.0)];
    Some(order_corners(grown))
}

/// Top to bottom; boxes within 10 px vertically go left to right.
fn sorted_boxes(mut boxes: Vec<[(f32, f32); 4]>) -> Vec<[(f32, f32); 4]> {
    boxes.sort_by(|a, b| a[0].1.total_cmp(&b[0].1).then(a[0].0.total_cmp(&b[0].0)));
    for i in 0..boxes.len().saturating_sub(1) {
        for j in (0..=i).rev() {
            if (boxes[j + 1][0].1 - boxes[j][0].1).abs() < 10.0 && boxes[j + 1][0].0 < boxes[j][0].0 {
                boxes.swap(j, j + 1);
            } else {
                break;
            }
        }
    }
    boxes
}

/// The box straightened to a rectangle; tall boxes (vertical text) are
/// turned a quarter counter-clockwise, as PaddleOCR does, with the
/// clockwise turn as the alternate.
fn crop(image: &RgbImage, quad: &[(f32, f32); 4]) -> (RgbImage, Option<RgbImage>) {
    let w = dist(quad[0], quad[1]).max(dist(quad[2], quad[3])).max(1.0) as u32;
    let h = dist(quad[0], quad[3]).max(dist(quad[1], quad[2])).max(1.0) as u32;
    let to = [(0.0, 0.0), (w as f32, 0.0), (w as f32, h as f32), (0.0, h as f32)];
    let mut out = RgbImage::new(w, h);
    match Projection::from_control_points(*quad, to) {
        Some(projection) => warp_into(image, projection, Interpolation::Bicubic, Border::Replicate, &mut out),
        None => return (out, None),
    }
    if h as f32 / w as f32 >= 1.5 {
        (image::imageops::rotate270(&out), Some(image::imageops::rotate90(&out)))
    } else {
        (out, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dictionary_from_inference_yml() {
        let yml =
            "PostProcess:\n  name: CTCLabelDecode\n  character_dict:\n  - '!'\n  - ''''\n  - a\n  - 日\nOther: 1\n";
        assert_eq!(TextRecognizer::dictionary(yml), vec!["!", "'", "a", "日"]);
    }

    #[test]
    fn boxes_read_in_lines() {
        let b = |x: f32, y: f32| [(x, y), (x + 10.0, y), (x + 10.0, y + 5.0), (x, y + 5.0)];
        let sorted = sorted_boxes(vec![b(50.0, 3.0), b(0.0, 40.0), b(0.0, 0.0)]);
        assert_eq!(sorted.iter().map(|q| q[0]).collect::<Vec<_>>(), vec![(0.0, 0.0), (50.0, 3.0), (0.0, 40.0)]);
    }

    #[test]
    fn unclip_grows_every_side() {
        let (grown, short) = unclip(&[(0.0, 0.0), (100.0, 0.0), (100.0, 10.0), (0.0, 10.0)]).unwrap();
        // d = 100·10·1.5 / (2·110) ≈ 6.82
        assert!((short - (10.0 + 2.0 * 6.818)).abs() < 0.01, "{short}");
        assert!((grown[0].0 + 6.818).abs() < 0.01 && (grown[0].1 + 6.818).abs() < 0.01, "{grown:?}");
    }
}
