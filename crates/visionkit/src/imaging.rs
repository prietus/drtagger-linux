//! Image loading as the Mac's CGImageSource thumbnails: EXIF orientation
//! applied, downscaled (never upscaled) to `max_pixel` on the longest side.

use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, ImageReader, RgbImage};
use std::io::{BufRead, Cursor, Seek};
use std::path::Path;

fn decode<R: BufRead + Seek>(reader: ImageReader<R>, max_pixel: u32) -> Option<RgbImage> {
    let mut decoder = reader.with_guessed_format().ok()?.into_decoder().ok()?;
    let orientation = decoder.orientation().ok();
    let mut image = DynamicImage::from_decoder(decoder).ok()?;
    if let Some(o) = orientation {
        image.apply_orientation(o);
    }
    if image.width() == 0 || image.height() == 0 {
        return None;
    }
    Some(downscaled(image.into_rgb8(), max_pixel))
}

/// At most `max_pixel` on the longest side.
pub fn downscaled(image: RgbImage, max_pixel: u32) -> RgbImage {
    let (w, h) = image.dimensions();
    if w.max(h) <= max_pixel {
        return image;
    }
    let scale = max_pixel as f64 / w.max(h) as f64;
    let (nw, nh) = (((w as f64 * scale).round() as u32).max(1), ((h as f64 * scale).round() as u32).max(1));
    image::imageops::resize(&image, nw, nh, FilterType::CatmullRom)
}

/// Enlarged `factor` times.
pub fn upscaled(image: &RgbImage, factor: u32) -> RgbImage {
    image::imageops::resize(image, image.width() * factor, image.height() * factor, FilterType::CatmullRom)
}

pub fn load(path: &Path, max_pixel: u32) -> Option<RgbImage> {
    let mut reader = ImageReader::open(path).ok()?;
    reader.no_limits();
    decode(reader, max_pixel)
}

pub fn load_data(data: &[u8], max_pixel: u32) -> Option<RgbImage> {
    let mut reader = ImageReader::new(Cursor::new(data));
    reader.no_limits();
    decode(reader, max_pixel)
}

/// A quarter turn, clockwise or counter-clockwise.
pub fn rotated(image: &RgbImage, clockwise: bool) -> RgbImage {
    if clockwise { image::imageops::rotate90(image) } else { image::imageops::rotate270(image) }
}
