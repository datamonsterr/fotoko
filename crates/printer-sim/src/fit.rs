//! Image fit math: what the Gutenprint pipeline does with `-o fit-to-page` on 4x6 media.
//!
//! The page is always 4x6 inch at 300 dpi. Landscape photos print on a 1800x1200 page, portrait
//! photos on a 1200x1800 page (the best-fit orientation), and the photo is scaled to cover the
//! page, then center-cropped.

use image::imageops::{self, FilterType};
use image::{DynamicImage, RgbImage};

/// Long edge of a 4x6 page at 300 dpi.
pub const LONG_EDGE_PX: u32 = 1800;
/// Short edge of a 4x6 page at 300 dpi.
pub const SHORT_EDGE_PX: u32 = 1200;

/// Source-space rectangle to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CropRect {
    /// Left edge.
    pub x: u32,
    /// Top edge.
    pub y: u32,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
}

/// Page size `(width, height)` that fits an image best: landscape for wide or square images,
/// portrait for tall ones.
pub fn page_size(src_width: u32, src_height: u32) -> (u32, u32) {
    if src_width >= src_height {
        (LONG_EDGE_PX, SHORT_EDGE_PX)
    } else {
        (SHORT_EDGE_PX, LONG_EDGE_PX)
    }
}

/// Largest centered rectangle of the source with the destination's aspect ratio.
///
/// A degenerate (zero-sized) source or destination yields the whole source.
pub fn cover_crop(src_width: u32, src_height: u32, dst_width: u32, dst_height: u32) -> CropRect {
    let whole = CropRect {
        x: 0,
        y: 0,
        width: src_width,
        height: src_height,
    };
    if src_width == 0 || src_height == 0 || dst_width == 0 || dst_height == 0 {
        return whole;
    }
    let (sw, sh) = (u64::from(src_width), u64::from(src_height));
    let (dw, dh) = (u64::from(dst_width), u64::from(dst_height));
    if sw * dh > dw * sh {
        // Source is wider than the page: trim left and right.
        let width = ((sh * dw + dh / 2) / dh).clamp(1, sw) as u32;
        CropRect {
            x: (src_width - width) / 2,
            y: 0,
            width,
            height: src_height,
        }
    } else {
        // Source is taller than (or as tall as) the page: trim top and bottom.
        let height = ((sw * dh + dw / 2) / dw).clamp(1, sh) as u32;
        CropRect {
            x: 0,
            y: (src_height - height) / 2,
            width: src_width,
            height,
        }
    }
}

/// Render a decoded image onto a 4x6 @300dpi page.
pub fn fit_to_page(image: &DynamicImage) -> RgbImage {
    let rgb = image.to_rgb8();
    let (page_w, page_h) = page_size(rgb.width(), rgb.height());
    let crop = cover_crop(rgb.width(), rgb.height(), page_w, page_h);
    let cropped = imageops::crop_imm(&rgb, crop.x, crop.y, crop.width, crop.height).to_image();
    imageops::resize(&cropped, page_w, page_h, FilterType::Triangle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    #[test]
    fn page_size_landscape_for_wide() {
        assert_eq!(page_size(4000, 3000), (1800, 1200));
    }

    #[test]
    fn page_size_landscape_for_square() {
        assert_eq!(page_size(500, 500), (1800, 1200));
    }

    #[test]
    fn page_size_portrait_for_tall() {
        assert_eq!(page_size(3000, 4000), (1200, 1800));
    }

    #[test]
    fn crop_wider_source_trims_sides() {
        // 4000x2000 (2:1) onto 3:2 -> width 3000, centered.
        let crop = cover_crop(4000, 2000, 1800, 1200);
        assert_eq!(
            crop,
            CropRect {
                x: 500,
                y: 0,
                width: 3000,
                height: 2000
            }
        );
    }

    #[test]
    fn crop_taller_source_trims_top_and_bottom() {
        // 3000x3000 square onto 3:2 -> height 2000, centered.
        let crop = cover_crop(3000, 3000, 1800, 1200);
        assert_eq!(
            crop,
            CropRect {
                x: 0,
                y: 500,
                width: 3000,
                height: 2000
            }
        );
    }

    #[test]
    fn crop_exact_aspect_keeps_everything() {
        let crop = cover_crop(900, 600, 1800, 1200);
        assert_eq!(
            crop,
            CropRect {
                x: 0,
                y: 0,
                width: 900,
                height: 600
            }
        );
    }

    #[test]
    fn crop_portrait_page() {
        let crop = cover_crop(2000, 4000, 1200, 1800);
        assert_eq!(
            crop,
            CropRect {
                x: 0,
                y: 500,
                width: 2000,
                height: 3000
            }
        );
    }

    #[test]
    fn crop_degenerate_inputs_return_whole_source() {
        let whole = CropRect {
            x: 0,
            y: 0,
            width: 0,
            height: 10,
        };
        assert_eq!(cover_crop(0, 10, 1800, 1200), whole);
        let whole = CropRect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        assert_eq!(cover_crop(10, 10, 0, 1200), whole);
        assert_eq!(cover_crop(10, 10, 1800, 0), whole);
        let whole = CropRect {
            x: 0,
            y: 0,
            width: 10,
            height: 0,
        };
        assert_eq!(cover_crop(10, 0, 1800, 1200), whole);
    }

    #[test]
    fn crop_extreme_aspect_never_collapses_to_zero() {
        let crop = cover_crop(10_000, 1, 1800, 1200);
        assert_eq!(crop.height, 1);
        assert!(crop.width >= 1);
    }

    #[test]
    fn fit_landscape_gives_1800x1200() {
        let img = DynamicImage::ImageRgb8(RgbImage::from_pixel(60, 40, Rgb([200, 10, 10])));
        let page = fit_to_page(&img);
        assert_eq!(page.dimensions(), (1800, 1200));
        assert_eq!(page.get_pixel(900, 600), &Rgb([200, 10, 10]));
    }

    #[test]
    fn fit_portrait_gives_1200x1800() {
        let img = DynamicImage::ImageRgb8(RgbImage::from_pixel(40, 60, Rgb([1, 2, 3])));
        assert_eq!(fit_to_page(&img).dimensions(), (1200, 1800));
    }

    #[test]
    fn fit_center_crops_instead_of_squashing() {
        // 60x60 image: left third red, middle third green, right third blue... cropped to 3:2
        // keeps the full width, so the left edge stays red and the right edge stays blue.
        let mut src = RgbImage::new(60, 60);
        for (x, _, px) in src.enumerate_pixels_mut() {
            *px = match x / 20 {
                0 => Rgb([255, 0, 0]),
                1 => Rgb([0, 255, 0]),
                _ => Rgb([0, 0, 255]),
            };
        }
        let page = fit_to_page(&DynamicImage::ImageRgb8(src));
        assert_eq!(page.get_pixel(2, 600), &Rgb([255, 0, 0]));
        assert_eq!(page.get_pixel(1797, 600), &Rgb([0, 0, 255]));
    }
}
