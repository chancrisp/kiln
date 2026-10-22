//! Raster conversion through the `image` crate. SVG is drawn by resvg,
//! and camera RAW is decoded by Windows (WIC, with the Raw Image
//! Extension); both are read only.

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
const FORMATS: [&str; 10] = [
    "png", "jpg", "webp", "bmp", "gif", "tiff", "ico", "tga", "qoi", "ppm",
];

/// Camera RAW types, read through WIC.
pub(crate) const RAW: [&str; 10] = [
    "dng", "cr2", "cr3", "nef", "arw", "orf", "rw2", "raf", "pef", "srw",
];

/// Icon sizes written into an ICO, largest first. Sizes above the
/// image's own are left out; images are never scaled up.
const ICON_SIZES: [u32; 7] = [256, 128, 64, 48, 32, 24, 16];

pub struct ImageConverter;

impl Converter for ImageConverter {
    fn id(&self) -> &'static str {
        "image"
    }

    /// Any raster type to any other. Transparent areas are flattened onto
    /// white for JPG and PPM, which have no alpha. Grayscale images stay
    /// grayscale where the target allows it. Only the first frame of an
    /// animated GIF is kept. ICO gets square icons from 256 pixels down.
    fn convert(&self, req: &Request<'_>, ctx: &mut Context<'_>) -> Result<()> {
        let (from, to) = (req.from.ext.as_str(), req.to.ext.as_str());
        if !reads(from) || !FORMATS.contains(&to) {
            return Err(Error::Registry(format!(
                "Kiln can't convert {} to {}.",
                req.from.code(),
                req.to.code()
            )));
        }
        let quality = req.options.jpeg_quality;
        if !(1..=100).contains(&quality) {
            return Err(Error::InvalidQuality(quality));
        }
        (ctx.progress)(Progress::Indeterminate);
        let cap = req.options.quality.size.resolve(None);
        let img = if from == "svg" {
            draw_svg(req.input, cap)?
        } else {
            fit(decode(req.input, from)?, cap)
        };
        if ctx.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        encode(&img, req.output, to, quality)
    }
}

/// Whether this backend can read `ext`.
pub(crate) fn reads(ext: &str) -> bool {
    FORMATS.contains(&ext) || RAW.contains(&ext) || ext == "svg"
}

/// `img` with its long side at most `cap`, scaled down with Lanczos.
pub(crate) fn fit(img: DynamicImage, cap: Option<u32>) -> DynamicImage {
    let (w, h) = (img.width(), img.height());
    match cap {
        Some(cap) if w.max(h) > cap => {
            let (nw, nh) = crate::quality::scaled(w, h, Some(cap), false);
            img.resize_exact(nw, nh, image::imageops::FilterType::Lanczos3)
        }
        _ => img,
    }
}

/// Draw an SVG at its own size, or with its long side at `size` when one
/// is asked for. Vector art scales up without loss, so unlike other
/// images it may grow. Text uses the fonts installed in Windows.
fn draw_svg(input: &Path, size: Option<u32>) -> Result<DynamicImage> {
    render_svg(input, &svg_tree(input)?, size)
}

/// An SVG's own size, rounded, and a drawing of it with its long side at
/// most `cap`. Unlike a conversion, the drawing never grows.
pub(crate) fn svg_preview(input: &Path, cap: u32) -> Result<((u32, u32), DynamicImage)> {
    let tree = svg_tree(input)?;
    let natural = tree.size();
    let (w, h) = (natural.width(), natural.height());
    let own = ((w.round() as u32).max(1), (h.round() as u32).max(1));
    let size = (w.max(h) > cap as f32).then_some(cap);
    Ok((own, render_svg(input, &tree, size)?))
}

/// Parse an SVG, with the fonts installed in Windows for its text.
fn svg_tree(input: &Path) -> Result<resvg::usvg::Tree> {
    let data = fs::read(input).map_err(|e| Error::io(input, e))?;
    let bad = |reason: String| Error::Decode {
        path: input.to_path_buf(),
        reason,
    };
    let head = String::from_utf8_lossy(&data[..data.len().min(4096)]).to_ascii_lowercase();
    // SVGZ starts with the gzip magic number; usvg unpacks it.
    if !head.contains("<svg") && !data.starts_with(&[0x1F, 0x8B]) {
        return Err(Error::WrongContent {
            path: input.to_path_buf(),
            code: "SVG".to_owned(),
        });
    }
    // External files and URLs are never fetched; only data: images load.
    let mut opt = resvg::usvg::Options {
        image_href_resolver: resvg::usvg::ImageHrefResolver {
            resolve_data: resvg::usvg::ImageHrefResolver::default_data_resolver(),
            resolve_string: Box::new(|_, _| None),
        },
        ..Default::default()
    };
    opt.fontdb_mut().load_system_fonts();
    resvg::usvg::Tree::from_data(&data, &opt).map_err(|e| bad(e.to_string()))
}

/// Draw a parsed SVG at its own size, or with its long side at `size`.
fn render_svg(input: &Path, tree: &resvg::usvg::Tree, size: Option<u32>) -> Result<DynamicImage> {
    let bad = |reason: String| Error::Decode {
        path: input.to_path_buf(),
        reason,
    };
    let natural = tree.size();
    let (nw, nh) = (natural.width(), natural.height());
    let scale = match size {
        Some(cap) => cap as f32 / nw.max(nh),
        None => 1.0,
    };
    let (w, h) = (
        ((nw * scale).round() as u32).clamp(1, 16384),
        ((nh * scale).round() as u32).clamp(1, 16384),
    );
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)
        .ok_or_else(|| bad("the drawing has no size.".to_owned()))?;
    let transform = resvg::tiny_skia::Transform::from_scale(w as f32 / nw, h as f32 / nh);
    resvg::render(tree, transform, &mut pixmap.as_mut());
    // tiny-skia keeps colours premultiplied by alpha.
    let mut rgba = pixmap.take();
    for px in rgba.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a != 0 && a != 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    let img = image::RgbaImage::from_raw(w, h, rgba).expect("the buffer matches the pixmap");
    Ok(DynamicImage::ImageRgba8(img))
}

fn image_format(ext: &str) -> Option<ImageFormat> {
    Some(match ext {
        "png" => ImageFormat::Png,
        "jpg" => ImageFormat::Jpeg,
        "webp" => ImageFormat::WebP,
        "bmp" => ImageFormat::Bmp,
        "gif" => ImageFormat::Gif,
        "tiff" => ImageFormat::Tiff,
        "ico" => ImageFormat::Ico,
        "tga" => ImageFormat::Tga,
        "qoi" => ImageFormat::Qoi,
        "ppm" => ImageFormat::Pnm,
        _ => return None,
    })
}

/// Decode an image of type `ext`, checking its signature first.
pub(crate) fn decode(input: &Path, ext: &str) -> Result<DynamicImage> {
    if ext == "svg" {
        return draw_svg(input, None);
    }
    if RAW.contains(&ext) {
        return decode_raw(input);
    }
    let format = image_format(ext).ok_or_else(|| {
        Error::Registry(format!(
            "Kiln can't read {} images.",
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
        "ico" => write_ico(out, img),
        "ppm" => write_ppm(out, img),
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

/// Square icons at each size up to 256, the image centred on a
/// transparent square and scaled down with Lanczos filtering.
fn write_ico(file: File, img: &DynamicImage) -> Result<()> {
    use image::codecs::ico::{IcoEncoder, IcoFrame};
    use image::imageops::{self, FilterType};
    let largest = img.width().max(img.height()).min(256);
    let mut sizes: Vec<u32> = ICON_SIZES.into_iter().filter(|&s| s < largest).collect();
    sizes.insert(0, largest);
    let rgba = img.to_rgba8();
    let mut frames = Vec::new();
    for size in sizes {
        let scale = size as f32 / img.width().max(img.height()) as f32;
        let (w, h) = (
            ((img.width() as f32 * scale).round() as u32).clamp(1, size),
            ((img.height() as f32 * scale).round() as u32).clamp(1, size),
        );
        let fitted = if (w, h) == (img.width(), img.height()) {
            rgba.clone()
        } else {
            imageops::resize(&rgba, w, h, FilterType::Lanczos3)
        };
        let mut square = image::RgbaImage::new(size, size);
        imageops::overlay(
            &mut square,
            &fitted,
            ((size - w) / 2) as i64,
            ((size - h) / 2) as i64,
        );
        let frame = IcoFrame::as_png(square.as_raw(), size, size, ExtendedColorType::Rgba8)
            .map_err(|e| encode_err("ico", e.to_string()))?;
        frames.push(frame);
    }
    let mut out = BufWriter::new(file);
    IcoEncoder::new(&mut out)
        .encode_images(&frames)
        .map_err(|e| encode_err("ico", e.to_string()))?;
    out.flush().map_err(|e| encode_err("ico", e.to_string()))
}

/// Binary PPM: 8-bit RGB, transparency flattened onto white.
fn write_ppm(file: File, img: &DynamicImage) -> Result<()> {
    use image::codecs::pnm::{PnmEncoder, PnmSubtype, SampleEncoding};
    let pixels = flatten_rgb(img);
    let mut out = BufWriter::new(file);
    PnmEncoder::new(&mut out)
        .with_subtype(PnmSubtype::Pixmap(SampleEncoding::Binary))
        .write_image(&pixels, img.width(), img.height(), ExtendedColorType::Rgb8)
        .map_err(|e| encode_err("ppm", e.to_string()))?;
    out.flush().map_err(|e| encode_err("ppm", e.to_string()))
}

/// WEBP (lossless), BMP, GIF, TIFF, TGA and QOI. Each gets 8-bit pixels in
/// a layout its encoder takes.
fn write_other(file: File, img: &DynamicImage, ext: &str) -> Result<()> {
    use image::codecs::{
        bmp::BmpEncoder, gif::GifEncoder, qoi::QoiEncoder, tga::TgaEncoder, tiff::TiffEncoder,
        webp::WebPEncoder,
    };
    let img = eight_bit(img, ext);
    let mut out = BufWriter::new(file);
    let result = match ext {
        "webp" => img.write_with_encoder(WebPEncoder::new_lossless(&mut out)),
        "bmp" => img.write_with_encoder(BmpEncoder::new(&mut out)),
        "gif" => img.write_with_encoder(GifEncoder::new(&mut out)),
        "tiff" => img.write_with_encoder(TiffEncoder::new(&mut out)),
        "tga" => img.write_with_encoder(TgaEncoder::new(&mut out)),
        "qoi" => img.write_with_encoder(QoiEncoder::new(&mut out)),
        other => {
            return Err(Error::Registry(format!(
                "Kiln can't write {} images.",
                other.to_ascii_uppercase()
            )))
        }
    };
    result.map_err(|e| encode_err(ext, e.to_string()))?;
    out.flush().map_err(|e| encode_err(ext, e.to_string()))
}

/// Reduce deeper images to 8 bits per channel, except TIFF, which keeps 16.
/// GIF always gets RGBA; TIFF has no gray-with-alpha layout, so that
/// becomes RGBA too. QOI has only RGB and RGBA.
fn eight_bit(img: &DynamicImage, ext: &str) -> DynamicImage {
    use image::ColorType as C;
    match (ext, img.color()) {
        ("gif", C::Rgba8) => img.clone(),
        ("gif", _) => DynamicImage::ImageRgba8(img.to_rgba8()),
        ("qoi", C::Rgb8 | C::Rgba8) => img.clone(),
        ("qoi", c) if c.has_alpha() => DynamicImage::ImageRgba8(img.to_rgba8()),
        ("qoi", _) => DynamicImage::ImageRgb8(img.to_rgb8()),
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

/// A camera RAW file, developed by Windows with the camera's own colour
/// settings and turned upright, at 16 bits per channel.
fn decode_raw(input: &Path) -> Result<DynamicImage> {
    match fs::metadata(input) {
        Ok(m) if m.is_file() => {}
        Ok(_) => return Err(Error::InputNotAFile(input.to_path_buf())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::InputNotFound(input.to_path_buf()))
        }
        Err(e) => return Err(Error::io(input, e)),
    }
    wic::decode(input)
}

#[cfg(windows)]
pub(crate) mod wic {
    use std::path::Path;

    use image::{DynamicImage, ImageBuffer, Rgb};
    use windows::core::HSTRING;
    use windows::Win32::Foundation::GENERIC_READ;
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, GUID_WICPixelFormat48bppRGB, IWICBitmapFrameDecode,
        IWICImagingFactory, WICBitmapDitherTypeNone, WICBitmapPaletteTypeCustom,
        WICDecodeMetadataCacheOnDemand,
    };
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
    };

    use crate::error::{Error, Result};

    /// WINCODEC_ERR_COMPONENTNOTFOUND: no decoder for the file.
    const NO_DECODER: u32 = 0x8898_2F50;

    pub fn decode(input: &Path) -> Result<DynamicImage> {
        let fail = |e: windows::core::Error| Error::Decode {
            path: input.to_path_buf(),
            reason: if e.code().0 as u32 == NO_DECODER {
                "Windows can't read this camera's RAW files. Install the Raw Image Extension from the Microsoft Store.".to_owned()
            } else {
                e.message().to_string()
            },
        };
        // SAFETY: plain COM calls on interfaces this function owns; the
        // pixel buffer is sized from the converter's own dimensions.
        unsafe {
            // Already initialised on this thread is fine too.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let factory: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                    .map_err(fail)?;
            let decoder = factory
                .CreateDecoderFromFilename(
                    &HSTRING::from(input.as_os_str()),
                    None,
                    GENERIC_READ,
                    WICDecodeMetadataCacheOnDemand,
                )
                .map_err(fail)?;
            let frame = decoder.GetFrame(0).map_err(fail)?;
            let orientation = orientation(&frame);
            let converter = factory.CreateFormatConverter().map_err(fail)?;
            converter
                .Initialize(
                    &frame,
                    &GUID_WICPixelFormat48bppRGB,
                    WICBitmapDitherTypeNone,
                    None,
                    0.0,
                    WICBitmapPaletteTypeCustom,
                )
                .map_err(fail)?;
            let (mut w, mut h) = (0u32, 0u32);
            converter.GetSize(&mut w, &mut h).map_err(fail)?;
            let stride = w * 6;
            let mut bytes = vec![0u8; stride as usize * h as usize];
            converter
                .CopyPixels(std::ptr::null(), stride, &mut bytes)
                .map_err(fail)?;
            let samples: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect();
            let img: ImageBuffer<Rgb<u16>, Vec<u16>> =
                ImageBuffer::from_raw(w, h, samples).expect("the buffer matches the size");
            Ok(upright(DynamicImage::ImageRgb16(img), orientation))
        }
    }

    /// The picture's size as it is shown, turned upright, read from the
    /// file's header without developing it.
    pub fn size(input: &Path) -> Result<(u32, u32)> {
        let fail = |e: windows::core::Error| Error::Decode {
            path: input.to_path_buf(),
            reason: e.message().to_string(),
        };
        // SAFETY: plain COM calls on interfaces this function owns; no
        // pixels are copied.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let factory: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                    .map_err(fail)?;
            let decoder = factory
                .CreateDecoderFromFilename(
                    &HSTRING::from(input.as_os_str()),
                    None,
                    GENERIC_READ,
                    WICDecodeMetadataCacheOnDemand,
                )
                .map_err(fail)?;
            let frame = decoder.GetFrame(0).map_err(fail)?;
            let (mut w, mut h) = (0u32, 0u32);
            frame.GetSize(&mut w, &mut h).map_err(fail)?;
            // Orientations 5 to 8 turn the picture a quarter.
            Ok(if (5..=8).contains(&orientation(&frame)) {
                (h, w)
            } else {
                (w, h)
            })
        }
    }

    /// The frame's EXIF orientation, 1 when it has none.
    fn orientation(frame: &IWICBitmapFrameDecode) -> u16 {
        // SAFETY: reads one metadata value from a frame the caller owns.
        unsafe {
            frame
                .GetMetadataQueryReader()
                .ok()
                .and_then(|reader| {
                    let mut value = PROPVARIANT::default();
                    reader
                        .GetMetadataByName(&HSTRING::from("System.Photo.Orientation"), &mut value)
                        .ok()?;
                    u16::try_from(&value).ok()
                })
                .unwrap_or(1)
        }
    }

    /// Apply an EXIF orientation (1 to 8).
    fn upright(img: DynamicImage, orientation: u16) -> DynamicImage {
        match orientation {
            2 => img.fliph(),
            3 => img.rotate180(),
            4 => img.flipv(),
            5 => img.rotate90().fliph(),
            6 => img.rotate90(),
            7 => img.rotate270().fliph(),
            8 => img.rotate270(),
            _ => img,
        }
    }
}

#[cfg(not(windows))]
pub(crate) mod wic {
    use std::path::Path;

    use image::DynamicImage;

    use crate::error::{Error, Result};

    pub fn decode(input: &Path) -> Result<DynamicImage> {
        Err(Error::Decode {
            path: input.to_path_buf(),
            reason: "camera RAW files can only be read on Windows.".to_owned(),
        })
    }

    pub fn size(input: &Path) -> Result<(u32, u32)> {
        decode(input).map(|_| (0, 0))
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
        "ico" => head.starts_with(&[0, 0, 1, 0]),
        "qoi" => head.starts_with(b"qoif"),
        "ppm" => head.len() >= 2 && head[0] == b'P' && (b'1'..=b'7').contains(&head[1]),
        // TGA has no signature: check the header's colour map and image
        // type fields, and leave the rest to the decoder.
        "tga" => head.len() >= 18 && head[1] <= 1 && matches!(head[2], 1 | 2 | 3 | 9 | 10 | 11),
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

    let mut head = Vec::with_capacity(18);
    let file = File::open(input).map_err(|e| Error::io(input, e))?;
    file.take(18)
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

/// 8-bit RGB pixels, transparency flattened onto white.
pub(crate) fn flatten_rgb(img: &DynamicImage) -> Vec<u8> {
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
        assert!(signature_matches(&[0, 0, 1, 0, 1, 0], "ico"));
        assert!(signature_matches(b"qoif....", "qoi"));
        assert!(signature_matches(b"P6\n64 48", "ppm") && !signature_matches(b"P9", "ppm"));
    }

    #[test]
    fn icons_are_square_and_never_scaled_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.ico");
        encode(&DynamicImage::new_rgba8(64, 48), &path, "ico", 90).unwrap();
        let bytes = fs::read(&path).unwrap();
        // The directory lists 64, 48, 32, 24 and 16, each square.
        assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), 5);
        let sizes: Vec<(u8, u8)> = (0..5)
            .map(|i| (bytes[6 + 16 * i], bytes[7 + 16 * i]))
            .collect();
        assert_eq!(sizes, [(64, 64), (48, 48), (32, 32), (24, 24), (16, 16)]);
        let big = dir.path().join("b.ico");
        encode(&DynamicImage::new_rgb8(1000, 500), &big, "ico", 90).unwrap();
        let img = image::open(&big).unwrap();
        // 256 is stored as 0 in the directory and decodes as 256.
        assert_eq!((img.width(), img.height()), (256, 256));
    }

    #[cfg(windows)]
    #[test]
    fn wic_reads_the_size_from_the_header() {
        // WIC reads JPG too, so the gradient fixture stands in for RAW.
        let jpg = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/sample.jpg");
        assert_eq!(wic::size(&jpg).unwrap(), (64, 48));
        assert!(wic::size(Path::new("no_such_file.dng")).is_err());
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
