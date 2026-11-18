//! Cover feature prints: DINOv2 ViT-S/14 image embeddings, standing in
//! for Vision's VNFeaturePrint. Distances are mapped onto Vision's scale
//! (see [`vision_scale`]) so the Mac's thresholds keep their meaning.

use crate::imaging;
use crate::models::{SessionPool, model};
use image::RgbImage;
use image::imageops::FilterType;
use ort::value::Tensor;
use std::path::Path;
use std::sync::OnceLock;

/// An L2-normalised embedding.
#[derive(Clone, Debug, PartialEq)]
pub struct FeaturePrint(pub Vec<f32>);

impl FeaturePrint {
    /// Cosine distance, 0 for the same image, about 1 for unrelated ones.
    pub fn raw_distance(&self, other: &FeaturePrint) -> f32 {
        let dot: f32 = self.0.iter().zip(&other.0).map(|(a, b)| a * b).sum();
        (1.0 - dot).max(0.0)
    }

    /// The distance on Vision's feature-print scale.
    pub fn distance(&self, other: &FeaturePrint) -> f32 {
        vision_scale(self.raw_distance(other))
    }
}

/// Maps a raw cosine distance onto Vision's VNFeaturePrint distance
/// (revision 2), piecewise linearly. Fitted on 832 real pairs (an album's
/// own front scan against Cover Art Archive images of its release, of
/// other editions, of the artist's other albums, and other albums' covers;
/// parity/cover_pairs.py, parity/compare_cover.py) so that each threshold
/// the app uses decides as Vision does: 0.3 and 0.35 (identical images)
/// 100% / 99.9% agreement, 0.6 98.0% (Vision drops below it for about one
/// own scan in ten, at random as far as the images show; the knot keeps
/// that rate), 0.95 93.6% — and where they differ at 0.95 DINOv2 is the
/// one that is right more often (own scans warned 11% vs 16%, other
/// albums' covers 99.7% vs 93%).
pub fn vision_scale(raw: f32) -> f32 {
    const POINTS: &[(f32, f32)] =
        &[(0.0, 0.0), (0.010, 0.30), (0.012, 0.35), (0.0195, 0.60), (0.261, 0.95), (0.65, 1.10), (1.0, 1.25)];
    let mut previous = POINTS[0];
    for &p in &POINTS[1..] {
        if raw <= p.0 {
            let t = (raw - previous.0) / (p.0 - previous.0);
            return previous.1 + t * (p.1 - previous.1);
        }
        previous = p;
    }
    previous.1 + (raw - previous.0) * 0.43
}

pub struct FeaturePrinter {
    pool: SessionPool,
}

impl FeaturePrinter {
    pub fn shared() -> Option<&'static FeaturePrinter> {
        static SHARED: OnceLock<Option<FeaturePrinter>> = OnceLock::new();
        SHARED.get_or_init(|| Some(FeaturePrinter::new(&model("cover/dinov2-small.onnx")?))).as_ref()
    }

    pub(crate) fn release_idle(&self) {
        self.pool.release_idle();
    }

    pub fn new(path: &Path) -> FeaturePrinter {
        FeaturePrinter { pool: SessionPool::new(path.into()) }
    }

    /// From encoded image data, read at most 512 px as the Mac does.
    pub fn feature_print_data(&self, data: &[u8]) -> Option<FeaturePrint> {
        self.feature_print(&imaging::load_data(data, 512)?).ok()
    }

    pub fn feature_print_file(&self, path: &Path) -> Option<FeaturePrint> {
        self.feature_print(&imaging::load(path, 512)?).ok()
    }

    /// Shortest side to 256 (bicubic), centre crop 224, ImageNet mean/std;
    /// the class token after the final layer norm.
    pub fn feature_print(&self, image: &RgbImage) -> ort::Result<FeaturePrint> {
        let (w, h) = image.dimensions();
        let scale = 256.0 / w.min(h) as f64;
        let (rw, rh) = (((w as f64 * scale).round() as u32).max(224), ((h as f64 * scale).round() as u32).max(224));
        let resized = image::imageops::resize(image, rw, rh, FilterType::CatmullRom);
        let (ox, oy) = ((rw - 224) / 2, (rh - 224) / 2);
        let mean = [0.485f32, 0.456, 0.406];
        let std = [0.229f32, 0.224, 0.225];
        let plane = 224 * 224;
        let mut data = vec![0f32; 3 * plane];
        for y in 0..224u32 {
            for x in 0..224u32 {
                let p = resized.get_pixel(ox + x, oy + y);
                for c in 0..3 {
                    data[c * plane + (y * 224 + x) as usize] = (p[c] as f32 / 255.0 - mean[c]) / std[c];
                }
            }
        }
        let tensor = Tensor::from_array(([1usize, 3, 224, 224], data))?;
        let mut session = self.pool.lease()?;
        let input = session.inputs()[0].name().to_string();
        let outputs = session.run(ort::inputs![input => tensor])?;
        // last_hidden_state [1, tokens, 384]: the class token is the first;
        // pooler_output [1, 384] is the same token after the layer norm.
        let pooled = outputs.get("pooler_output").unwrap_or(&outputs[0]);
        let (shape, values) = pooled.try_extract_tensor::<f32>()?;
        let dim = shape[shape.len() - 1] as usize;
        let mut v = values[..dim].to_vec();
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
        v.iter_mut().for_each(|x| *x /= norm);
        Ok(FeaturePrint(v))
    }
}

#[cfg(test)]
mod tests {
    use super::vision_scale;

    #[test]
    fn scale_is_monotone_and_hits_the_thresholds() {
        let mut previous = -1.0;
        for i in 0..=1200 {
            let v = vision_scale(i as f32 / 1000.0);
            assert!(v > previous);
            previous = v;
        }
        assert!((vision_scale(0.261) - 0.95).abs() < 1e-6);
        assert!(vision_scale(0.0) == 0.0);
        assert!(vision_scale(1.2) > 1.25);
    }
}
