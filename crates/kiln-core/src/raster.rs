//! Raster conversion through the `image` crate.

use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, ExtendedColorType, ImageEncoder, ImageFormat, ImageReader};

use crate::converter::{Context, Converter, Progress, Request};
use crate::error::{Error, Result};

/// Transparent pixels are composited onto this, since JPG has no alpha.
const MATTE: [u8; 3] = [255, 255, 255];

/// The extensions this backend reads and writes.
const FORMATS: [&str; 6] = ["png", "jpg", "webp", "bmp", "gif", "tiff"];

pub struct ImageConverter;

impl Converter for ImageConverter {
    fn id(&self) -> &'static str {
        "image"
    }

    /// Any raster type to any other. Transparent areas are flattened onto
    /// white for JPG, which has no alpha. Grayscale images stay grayscale
    /// where the target allows it. Only the first frame of an animated GIF
    /// is kept.
    fn convert(&self, req: &Request<'_>, ctx: &mut Context<'_>) -> Result<()> {
        let (from, to) = (req.from.ext.as_str(), req.to.ext.as_str());
        if !FORMATS.contains(&from) || !FORMATS.contains(&to) {
            return Err(Error::Registry(format!(
                "The image backend can't convert {} to {}.",
                req.from.code(),
                req.to.code()
            )));
        }
        let quality = req.options.jpeg_quality;
        if !(1..=100).contains(&quality) {
            return Err(Error::InvalidQuality(quality));
        }
        (ctx.progress)(Progress::Indeterminate);
        let img = decode(req.input, from)?;
        if ctx.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        encode(&img, req.output, to, quality)
    }
}

fn image_format(ext: &str) -> Option<ImageFormat> {
    Some(match ext {
        "png" => ImageFormat::Png,
        "jpg" => ImageFormat::Jpeg,
        "webp" => ImageFormat::WebP,
        "bmp" => ImageFormat::Bmp,
        "gif" => ImageFormat::Gif,
        "tiff" => ImageFormat::Tiff,
        _ => return None,
    })
}

/// Decode an image of type `ext`, checking its signature first.
pub(crate) fn decode(input: &Path, ext: &str) -> Result<DynamicImage> {
    let format = image_format(ext).ok_or_else(|| {
        Error::Registry(format!(
            "The image backend can't read {}.",
            ext.to_ascii_uppercase()
        ))
    })?;
    check_signature(input, ext)?;
    let file = File::open(input).map_err(|e| Error::io(input, e))?;
    ImageReader::with_format(BufReader::new(file), format)
        .decode()
        .map_err(|e| Error::Decode {
            path: input.to_path_buf(),
            reason: e.to_string(),
        })
}

/// Write `img` as `ext` to a new file at `output`. `quality` applies to JPG.
pub(crate) fn encode(img: &DynamicImage, output: &Path, ext: &str, quality: u8) -> Result<()> {
    let out = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .map_err(|e| Error::io(output, e))?;
    match ext {
        "jpg" => write_jpeg(out, img, quality),
        "png" => write_png(out, img),
        other => write_other(out, img, other),
    }
}

fn write_png(file: File, img: &DynamicImage) -> Result<()> {
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    let mut out = BufWriter::new(file);
    let encoder =
        PngEncoder::new_with_quality(&mut out, CompressionType::Best, FilterType::Adaptive);
    img.write_with_encoder(encoder)
        .map_err(|e| encode_err("png", e.to_string()))?;
    out.flush().map_err(|e| encode_err("png", e.to_string()))
}

/// WEBP (lossless), BMP, GIF and TIFF. Each gets 8-bit pixels in a layout
/// its encoder takes.
fn write_other(file: File, img: &DynamicImage, ext: &str) -> Result<()> {
    use image::codecs::{bmp::BmpEncoder, gif::GifEncoder, tiff::TiffEncoder, webp::WebPEncoder};
    let img = eight_bit(img, ext);
    let mut out = BufWriter::new(file);
    let result = match ext {
        "webp" => img.write_with_encoder(WebPEncoder::new_lossless(&mut out)),
        "bmp" => img.write_with_encoder(BmpEncoder::new(&mut out)),
        "gif" => img.write_with_encoder(GifEncoder::new(&mut out)),
        "tiff" => img.write_with_encoder(TiffEncoder::new(&mut out)),
        other => {
            return Err(Error::Registry(format!(
                "The image backend can't write {}.",
                other.to_ascii_uppercase()
            )))
        }
    };
    result.map_err(|e| encode_err(ext, e.to_string()))?;
    out.flush().map_err(|e| encode_err(ext, e.to_string()))
}

/// Reduce deeper images to 8 bits per channel, except TIFF, which keeps 16.
/// GIF always gets RGBA; TIFF has no gray-with-alpha layout, so that
/// becomes RGBA too.
fn eight_bit(img: &DynamicImage, ext: &str) -> DynamicImage {
    use image::ColorType as C;
    match (ext, img.color()) {
        ("gif", C::Rgba8) => img.clone(),
        ("gif", _) => DynamicImage::ImageRgba8(img.to_rgba8()),
        ("tiff", C::La8) => DynamicImage::ImageRgba8(img.to_rgba8()),
        ("tiff", C::La16) => DynamicImage::ImageRgba16(img.to_rgba16()),
        ("tiff", C::L16 | C::Rgb16 | C::Rgba16) => img.clone(),
        (_, C::L8 | C::La8 | C::Rgb8 | C::Rgba8) => img.clone(),
        (_, C::L16) => DynamicImage::ImageLuma8(img.to_luma8()),
        (_, C::La16) => DynamicImage::ImageLumaA8(img.to_luma_alpha8()),
        (_, c) if c.has_alpha() => DynamicImage::ImageRgba8(img.to_rgba8()),
        _ => DynamicImage::ImageRgb8(img.to_rgb8()),
    }
}

fn encode_err(ext: &str, reason: String) -> Error {
    Error::Encode {
        code: ext.to_ascii_uppercase(),
        reason,
    }
}

/// Whether `head`, the start of a file, looks like an `ext` image.
fn signature_matches(head: &[u8], ext: &str) -> bool {
    match ext {
        "png" => head.starts_with(&[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n']),
        "jpg" => head.starts_with(&[0xFF, 0xD8, 0xFF]),
        "webp" => head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP",
        "bmp" => head.starts_with(b"BM"),
        "gif" => head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a"),
        "tiff" => head.starts_with(b"II*\0") || head.starts_with(b"MM\0*"),
        _ => false,
    }
}

/// Reject anything that isn't an existing file starting with the right
/// signature for `ext`. The extension alone is not trusted.
fn check_signature(input: &Path, ext: &str) -> Result<()> {
    let meta = match fs::metadata(input) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::InputNotFound(input.to_path_buf()))
        }
        Err(e) => return Err(Error::io(input, e)),
    };
    if !meta.is_file() {
        return Err(Error::InputNotAFile(input.to_path_buf()));
    }

    let mut head = Vec::with_capacity(12);
    let file = File::open(input).map_err(|e| Error::io(input, e))?;
    file.take(12)
        .read_to_end(&mut head)
        .map_err(|e| Error::io(input, e))?;
    if signature_matches(&head, ext) {
        Ok(())
    } else {
        Err(Error::WrongContent {
            path: input.to_path_buf(),
            code: ext.to_ascii_uppercase(),
        })
    }
}

fn write_jpeg(file: File, img: &DynamicImage, quality: u8) -> Result<()> {
    let (w, h) = (img.width(), img.height());
    let mut out = BufWriter::new(file);
    let encoder = JpegEncoder::new_with_quality(&mut out, quality);

    let result = if is_gray(img) {
        let pixels = flatten_gray(img);
        encoder.write_image(&pixels, w, h, ExtendedColorType::L8)
    } else {
        let pixels = flatten_rgb(img);
        encoder.write_image(&pixels, w, h, ExtendedColorType::Rgb8)
    };
    result.map_err(|e| encode_err("jpg", e.to_string()))?;
    out.flush().map_err(|e| encode_err("jpg", e.to_string()))
}

fn is_gray(img: &DynamicImage) -> bool {
    !img.color().has_color()
}

fn blend(c: u8, a: u8, matte: u8) -> u8 {
    let (c, a, m) = (c as u32, a as u32, matte as u32);
    ((c * a + m * (255 - a) + 127) / 255) as u8
}

fn flatten_rgb(img: &DynamicImage) -> Vec<u8> {
    if !img.color().has_alpha() {
        return img.to_rgb8().into_raw();
    }
    img.to_rgba8()
        .pixels()
        .flat_map(|p| {
            let [r, g, b, a] = p.0;
            [
                blend(r, a, MATTE[0]),
                blend(g, a, MATTE[1]),
                blend(b, a, MATTE[2]),
            ]
        })
        .collect()
}

fn flatten_gray(img: &DynamicImage) -> Vec<u8> {
    if !img.color().has_alpha() {
        return img.to_luma8().into_raw();
    }
    img.to_luma_alpha8()
        .pixels()
        .map(|p| blend(p.0[0], p.0[1], MATTE[0]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_endpoints() {
        assert_eq!(blend(10, 255, 255), 10);
        assert_eq!(blend(10, 0, 255), 255);
        assert_eq!(blend(0, 128, 255), 127);
    }

    #[test]
    fn signatures_are_checked_per_type() {
        assert!(signature_matches(b"GIF89a....", "gif"));
        assert!(signature_matches(b"RIFF\x10\0\0\0WEBPVP8L", "webp"));
        assert!(!signature_matches(b"RIFF\x10\0\0\0WAVEfmt ", "webp"));
        assert!(signature_matches(b"II*\0", "tiff") && signature_matches(b"MM\0*", "tiff"));
        assert!(!signature_matches(b"BM", "png"));
        assert!(!signature_matches(b"", "bmp"));
    }

    #[test]
    fn deep_images_become_eight_bit() {
        let deep = DynamicImage::new_rgb16(2, 2);
        assert_eq!(eight_bit(&deep, "bmp").color(), image::ColorType::Rgb8);
        assert_eq!(eight_bit(&deep, "tiff").color(), image::ColorType::Rgb16);
        let gray = DynamicImage::new_luma8(2, 2);
        assert_eq!(eight_bit(&gray, "webp").color(), image::ColorType::L8);
        assert_eq!(eight_bit(&gray, "gif").color(), image::ColorType::Rgba8);
        let ga = DynamicImage::new_luma_a8(2, 2);
        assert_eq!(eight_bit(&ga, "tiff").color(), image::ColorType::Rgba8);
    }
}
