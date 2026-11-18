//! EAN-13, EAN-8 and UPC-E with zxing-cpp (Vision's symbologies on Mac;
//! UPC-A comes out as its 13-digit EAN form, as Vision reports it).

use image::RgbImage;
use zxingcpp::{BarcodeFormat, ImageFormat, ImageView};

pub fn detect(image: &RgbImage) -> Vec<String> {
    let Ok(view) = ImageView::from_slice(image.as_raw(), image.width(), image.height(), ImageFormat::RGB) else {
        return vec![];
    };
    let reader = zxingcpp::read()
        .formats([BarcodeFormat::EAN13, BarcodeFormat::EAN8, BarcodeFormat::UPCE])
        .try_harder(true)
        .try_rotate(false)
        .try_invert(false)
        .try_downscale(true);
    reader
        .from(view)
        .map(|codes| codes.iter().map(|c| c.text()).filter(|t| !t.is_empty()).collect())
        .unwrap_or_default()
}
