//! Raster conversion through the `image` crate.

use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, ExtendedColorType, ImageEncoder, ImageFormat, ImageReader};

use crate::error::{Error, Result};
use crate::output;

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n'];

/// Transparent pixels are composited onto this, since JPG has no alpha.
const MATTE: [u8; 3] = [255, 255, 255];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JpegOptions {
    /// 1 to 100. Higher is larger and closer to the source.
    pub quality: u8,
    /// Where to write the result. `None` writes next to the input.
    pub out_dir: Option<PathBuf>,
}

impl Default for JpegOptions {
    fn default() -> Self {
        Self {
            quality: 90,
            out_dir: None,
        }
    }
}

/// Convert a PNG to a JPG and return the path that was written.
///
/// The output is named after the input and never replaces an existing
/// file: if `photo.jpg` exists the result is `photo (1).jpg`. Transparent
/// areas are flattened onto white. Grayscale images stay grayscale.
pub fn png_to_jpg(input: &Path, opts: &JpegOptions) -> Result<PathBuf> {
    if !(1..=100).contains(&opts.quality) {
        return Err(Error::InvalidQuality(opts.quality));
    }
    check_png(input)?;

    let out_dir = match &opts.out_dir {
        Some(dir) => dir.clone(),
        None => input
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(".")),
    };
    let out_dir = if out_dir.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out_dir
    };
    if !out_dir.is_dir() {
        return Err(Error::OutputDirNotFound(out_dir));
    }

    // Decode before claiming an output name, so a bad input leaves nothing
    // behind.
    let file = File::open(input).map_err(|e| Error::io(input, e))?;
    let img = ImageReader::with_format(BufReader::new(file), ImageFormat::Png)
        .decode()
        .map_err(|e| Error::Decode {
            path: input.to_path_buf(),
            reason: e.to_string(),
        })?;

    let stem = input
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (file, out_path) = output::create_unique(&out_dir, &stem, "jpg")?;

    if let Err(e) = write_jpeg(file, &img, opts.quality) {
        let _ = fs::remove_file(&out_path);
        return Err(e);
    }
    Ok(out_path)
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
        Ok(()) => Err(Error::NotPng(input.to_path_buf())),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            Err(Error::NotPng(input.to_path_buf()))
        }
        Err(e) => Err(Error::io(input, e)),
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
    result.map_err(|e| Error::Encode(e.to_string()))?;
    out.flush().map_err(|e| Error::Encode(e.to_string()))?;
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

    #[test]
    fn default_quality_is_90() {
        assert_eq!(JpegOptions::default().quality, 90);
    }
}
