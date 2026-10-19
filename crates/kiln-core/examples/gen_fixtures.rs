//! Regenerates the PNG fixtures in tests/fixtures.
//!
//! cargo run -p kiln-core --example gen_fixtures

use std::path::PathBuf;

use image::{GrayImage, ImageBuffer, Luma, Rgb, RgbImage, Rgba, RgbaImage};

fn main() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    std::fs::create_dir_all(&dir).unwrap();

    // 64x48 RGB gradient.
    let rgb = RgbImage::from_fn(64, 48, |x, y| Rgb([(x * 4) as u8, (y * 5) as u8, 128]));
    rgb.save(dir.join("rgb_gradient.png")).unwrap();

    // 32x32: left half fully transparent, right half opaque dark blue.
    let rgba = RgbaImage::from_fn(32, 32, |x, _| {
        if x < 16 {
            Rgba([200, 30, 30, 0])
        } else {
            Rgba([20, 40, 120, 255])
        }
    });
    rgba.save(dir.join("rgba_half_transparent.png")).unwrap();

    // 40x20 8-bit grayscale ramp.
    let gray = GrayImage::from_fn(40, 20, |x, _| Luma([(x * 6) as u8]));
    gray.save(dir.join("gray8.png")).unwrap();

    // 24x24 16-bit RGB, flat mid-green.
    let deep: ImageBuffer<Rgb<u16>, Vec<u16>> =
        ImageBuffer::from_pixel(24, 24, Rgb([10_000, 40_000, 20_000]));
    deep.save(dir.join("rgb16.png")).unwrap();

    // A valid PNG signature followed by garbage.
    let bytes = std::fs::read(dir.join("rgb_gradient.png")).unwrap();
    std::fs::write(dir.join("truncated.png"), &bytes[..bytes.len() / 3]).unwrap();

    // Text with a .png extension.
    std::fs::write(dir.join("not_really.png"), b"this is plain text\n").unwrap();
}
