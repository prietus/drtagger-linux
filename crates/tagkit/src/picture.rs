use base64::Engine;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// ID3 APIC picture types (0 other, 3 front cover, 4 back cover, …).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PictureKind(pub u8);

impl PictureKind {
    pub const OTHER: PictureKind = PictureKind(0);
    pub const FRONT: PictureKind = PictureKind(3);
    pub const BACK: PictureKind = PictureKind(4);
    pub const LEAFLET: PictureKind = PictureKind(5);
    pub const MEDIA: PictureKind = PictureKind(6);

    /// Values outside the ID3 table (0…20) read as "other".
    pub fn from_raw(raw: u32) -> PictureKind {
        if raw <= 20 { PictureKind(raw as u8) } else { PictureKind::OTHER }
    }
}

fn b64_ser<S: Serializer>(d: &[u8], s: S) -> Result<S::Ok, S::Error> {
    base64::engine::general_purpose::STANDARD.encode(d).serialize(s)
}

fn b64_de<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
    let s = String::deserialize(d)?;
    base64::engine::general_purpose::STANDARD.decode(s).map_err(serde::de::Error::custom)
}

/// An embedded picture as every container understands it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Picture {
    pub kind: PictureKind,
    pub mime_type: String,
    #[serde(serialize_with = "b64_ser", deserialize_with = "b64_de")]
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub description: String,
}

impl Picture {
    /// A picture with its size and type read from the image header.
    pub fn from_image(kind: PictureKind, data: Vec<u8>, fallback_mime: &str) -> Picture {
        let (width, height) = dimensions(&data).unwrap_or((0, 0));
        let mime_type = mime_type(&data).unwrap_or(fallback_mime).to_string();
        Picture { kind, mime_type, data, width, height, description: String::new() }
    }

    pub fn is_front(&self) -> bool {
        self.kind == PictureKind::FRONT
    }

    pub fn file_extension(&self) -> &'static str {
        if self.mime_type == "image/png" { "png" } else { "jpg" }
    }
}

/// Summary kept in backups and shown in the preview (no pixel data).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PictureInfo {
    pub kind: PictureKind,
    pub mime_type: String,
    pub width: u32,
    pub height: u32,
    pub byte_count: usize,
}

impl From<&Picture> for PictureInfo {
    fn from(p: &Picture) -> Self {
        PictureInfo {
            kind: p.kind,
            mime_type: p.mime_type.clone(),
            width: p.width,
            height: p.height,
            byte_count: p.data.len(),
        }
    }
}

/// Pixel size from the image header (stored orientation, not rotated).
pub fn dimensions(data: &[u8]) -> Option<(u32, u32)> {
    let size = imagesize::blob_size(data).ok()?;
    Some((size.width as u32, size.height as u32))
}

/// The MIME type of the image format, as macOS names it.
pub fn mime_type(data: &[u8]) -> Option<&'static str> {
    use imagesize::ImageType::*;
    Some(match imagesize::image_type(data).ok()? {
        Jpeg => "image/jpeg",
        Png => "image/png",
        Gif => "image/gif",
        Bmp => "image/bmp",
        Tiff => "image/tiff",
        Webp => "image/webp",
        Heif(_) => "image/heic",
        Ico => "image/vnd.microsoft.icon",
        Psd => "image/vnd.adobe.photoshop",
        Jxl => "image/jxl",
        _ => return None,
    })
}
