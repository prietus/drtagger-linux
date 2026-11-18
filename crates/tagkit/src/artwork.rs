//! Cover art as it goes into the files: JPEGs up to `max_pixels` and small
//! PNGs stay as they are; anything else is scaled down (EXIF orientation
//! applied) and re-encoded as JPEG.

use crate::picture::{Picture, PictureKind, dimensions, mime_type};
use image::{ImageDecoder, ImageReader};
use std::fmt;
use std::io::Cursor;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArtworkError {
    Undecodable,
    EncodingFailed,
}

impl fmt::Display for ArtworkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ArtworkError::Undecodable => "The image could not be decoded.",
            ArtworkError::EncodingFailed => "The image could not be encoded.",
        })
    }
}

impl std::error::Error for ArtworkError {}

pub struct ArtworkProcessor;

impl ArtworkProcessor {
    pub fn prepared(data: &[u8], kind: PictureKind, max_pixels: u32) -> Result<Picture, ArtworkError> {
        Self::prepared_with(data, kind, max_pixels, 0.9, 600)
    }

    pub fn prepared_with(
        data: &[u8],
        kind: PictureKind,
        max_pixels: u32,
        jpeg_quality: f64,
        keep_png_up_to: u32,
    ) -> Result<Picture, ArtworkError> {
        let (w, h) = dimensions(data).ok_or(ArtworkError::Undecodable)?;
        let mime = mime_type(data).ok_or(ArtworkError::Undecodable)?;
        let longest = w.max(h);
        let is_png = mime == "image/png";
        if longest <= max_pixels && (mime == "image/jpeg" || (is_png && longest <= keep_png_up_to)) {
            return Ok(Picture {
                kind,
                mime_type: mime.into(),
                data: data.to_vec(),
                width: w,
                height: h,
                description: String::new(),
            });
        }
        let mut decoder = ImageReader::new(Cursor::new(data))
            .with_guessed_format()
            .map_err(|_| ArtworkError::Undecodable)?
            .into_decoder()
            .map_err(|_| ArtworkError::Undecodable)?;
        let orientation = decoder.orientation().map_err(|_| ArtworkError::Undecodable)?;
        let mut img = image::DynamicImage::from_decoder(decoder).map_err(|_| ArtworkError::Undecodable)?;
        img.apply_orientation(orientation);
        let target = longest.min(max_pixels);
        let (iw, ih) = (img.width(), img.height());
        let scale = target as f64 / iw.max(ih) as f64;
        let (nw, nh) = if scale < 1.0 {
            (((iw as f64 * scale).round() as u32).max(1), ((ih as f64 * scale).round() as u32).max(1))
        } else {
            (iw, ih)
        };
        let rgb = if (nw, nh) == (iw, ih) {
            img.into_rgb8()
        } else {
            img.resize_exact(nw, nh, image::imageops::FilterType::Lanczos3).into_rgb8()
        };
        let mut out = vec![];
        let quality = (jpeg_quality * 100.0).round().clamp(1.0, 100.0) as u8;
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
            .encode_image(&rgb)
            .map_err(|_| ArtworkError::EncodingFailed)?;
        Ok(Picture {
            kind,
            mime_type: "image/jpeg".into(),
            data: out,
            width: rgb.width(),
            height: rgb.height(),
            description: String::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: u32, height: u32, png: bool) -> Vec<u8> {
        let img = image::RgbImage::from_fn(width, height, |x, y| {
            if x < width / 2 && y >= height / 2 { image::Rgb([26, 77, 230]) } else { image::Rgb([204, 51, 26]) }
        });
        let mut out = Cursor::new(vec![]);
        let format = if png { image::ImageFormat::Png } else { image::ImageFormat::Jpeg };
        img.write_to(&mut out, format).unwrap();
        out.into_inner()
    }

    #[test]
    fn artwork_is_resized_to_jpeg() {
        let p = ArtworkProcessor::prepared(&image(2400, 2000, false), PictureKind::FRONT, 1500).unwrap();
        assert_eq!((p.mime_type.as_str(), p.width, p.height), ("image/jpeg", 1500, 1250));
        assert_eq!(dimensions(&p.data), Some((1500, 1250)));
        let small = ArtworkProcessor::prepared(&image(500, 500, true), PictureKind::FRONT, 1500).unwrap();
        assert_eq!(small.mime_type, "image/png");
        let big_png = ArtworkProcessor::prepared(&image(800, 800, true), PictureKind::FRONT, 1500).unwrap();
        assert_eq!((big_png.mime_type.as_str(), big_png.width), ("image/jpeg", 800));
        assert_eq!(ArtworkProcessor::prepared(b"nope", PictureKind::FRONT, 1500), Err(ArtworkError::Undecodable));
    }
}
