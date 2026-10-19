//! Raster conversion through the `image` crate.

use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, ExtendedColorType, ImageEncoder, ImageFormat, ImageReader};

use crate::converter::{Context, Converter, Progress, Request};
use crate::error::{Error, Result};

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n'];

/// Transparent pixels are composited onto this, since JPG has no alpha.
const MATTE: [u8; 3] = [255, 255, 255];

pub struct ImageConverter;

impl Converter for ImageConverter {
    fn id(&self) -> &'static str {
        "image"
    }

    /// PNG to JPG. Transparent areas are flattened onto white, since JPG
    /// has no alpha. Grayscale images stay grayscale.
    fn convert(&self, req: &Request<'_>, ctx: &mut Context<'_>) -> Result<()> {
        if (req.from.ext.as_str(), req.to.ext.as_str()) != ("png", "jpg") {
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
        check_png(req.input)?;
        (ctx.progress)(Progress::Indeterminate);

        let file = File::open(req.input).map_err(|e| Error::io(req.input, e))?;
        let img = ImageReader::with_format(BufReader::new(file), ImageFormat::Png)
            .decode()
            .map_err(|e| Error::Decode {
                path: req.input.to_path_buf(),
                reason: e.to_string(),
            })?;
        if ctx.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }

        let out = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(req.output)
            .map_err(|e| Error::io(req.output, e))?;
        write_jpeg(out, &img, quality)
    }
}

/// Decode a PNG or JPG, checking its signature first.
pub(crate) fn decode(input: &Path, ext: &str) -> Result<DynamicImage> {
    let format = match ext {
        "png" => {
            check_png(input)?;
            ImageFormat::Png
        }
        "jpg" => ImageFormat::Jpeg,
        other => {
            return Err(Error::Registry(format!(
                "The image backend can't read {}.",
                other.to_ascii_uppercase()
            )))
        }
    };
    let file = File::open(input).map_err(|e| Error::io(input, e))?;
    ImageReader::with_format(BufReader::new(file), format)
        .decode()
        .map_err(|e| Error::Decode {
            path: input.to_path_buf(),
            reason: e.to_string(),
        })
}

/// Write `img` as `ext` ("png" or "jpg") to a new file at `output`.
pub(crate) fn encode(img: &DynamicImage, output: &Path, ext: &str, quality: u8) -> Result<()> {
    let out = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .map_err(|e| Error::io(output, e))?;
    match ext {
        "jpg" => write_jpeg(out, img, quality),
        _ => write_png(out, img),
    }
}

fn write_png(file: File, img: &DynamicImage) -> Result<()> {
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    let mut out = BufWriter::new(file);
    let encoder =
        PngEncoder::new_with_quality(&mut out, CompressionType::Best, FilterType::Adaptive);
    img.write_with_encoder(encoder).map_err(|e| Error::Encode {
        code: "PNG".to_owned(),
        reason: e.to_string(),
    })?;
    out.flush().map_err(|e| Error::io("PNG", e))
}

/// Reject anything that isn't an existing file starting with the PNG
/// signature. The extension alone is not trusted.
fn check_png(input: &Path) -> Result<()> {
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

    let mut sig = [0u8; 8];
    let mut file = File::open(input).map_err(|e| Error::io(input, e))?;
    match file.read_exact(&mut sig) {
        Ok(()) if sig == PNG_SIGNATURE => Ok(()),
        Ok(()) => Err(not_png(input)),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Err(not_png(input)),
        Err(e) => Err(Error::io(input, e)),
    }
}

fn not_png(input: &Path) -> Error {
    Error::WrongContent {
        path: input.to_path_buf(),
        code: "PNG".to_owned(),
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
    let encode_err = |reason: String| Error::Encode {
        code: "JPG".to_owned(),
        reason,
    };
    result.map_err(|e| encode_err(e.to_string()))?;
    out.flush().map_err(|e| encode_err(e.to_string()))?;
    Ok(())
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
}
