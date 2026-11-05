//! Photo fixtures: small real JPEG and PNG files the printer simulation can decode.

use std::io::Cursor;

use fotoko_protocol::ContentType;
use image::{ImageFormat, Rgb, RgbImage};

/// A photo to upload.
#[derive(Debug, Clone)]
pub struct Photo {
    /// File content.
    pub bytes: Vec<u8>,
    /// Declared content type.
    pub content_type: ContentType,
    /// Copies to print.
    pub copies: u32,
}

fn encode(width: u32, height: u32, colour: [u8; 3], format: ImageFormat) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    RgbImage::from_pixel(width, height, Rgb(colour))
        .write_to(&mut out, format)
        .expect("encode fixture");
    out.into_inner()
}

/// A landscape JPEG (600x400) printed as a 1800x1200 page.
pub fn landscape_jpeg(copies: u32) -> Photo {
    Photo {
        bytes: encode(600, 400, [30, 140, 220], ImageFormat::Jpeg),
        content_type: ContentType::Jpeg,
        copies,
    }
}

/// A portrait PNG (400x600) printed as a 1200x1800 page.
pub fn portrait_png(copies: u32) -> Photo {
    Photo {
        bytes: encode(400, 600, [220, 90, 40], ImageFormat::Png),
        content_type: ContentType::Png,
        copies,
    }
}
