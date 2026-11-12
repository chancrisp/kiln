//! Raster conversion through the `image` crate. SVG is drawn by resvg,
//! and camera RAW, HEIC and AVIF are decoded by Windows (WIC, with the Raw
//! Image Extension, the HEIF Image Extensions or the AV1 Video Extension);
//! all four are read only. FFmpeg reads HEIC and AVIF where Windows can't.

use std::borrow::Cow;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use image::codecs::jpeg::JpegEncoder;
use image::metadata::Orientation;
use image::{
    DynamicImage, ExtendedColorType, ImageDecoder, ImageEncoder, ImageFormat, ImageReader, Limits,
};

use crate::converter::{CancelToken, Context, Converter, Options, Progress, Request};
use crate::error::{Error, Result};
use crate::facts;
use crate::svg_guard;
use crate::webp;

/// Transparent pixels are composited onto this, since JPG has no alpha.
const MATTE: [u8; 3] = [255, 255, 255];

/// The most an SVGZ may unpack to, in bytes.
const MAX_SVGZ_BYTES: u64 = 64 << 20;

/// The extensions this backend reads and writes.
const FORMATS: [&str; 10] = [
    "png", "jpg", "webp", "bmp", "gif", "tiff", "ico", "tga", "qoi", "ppm",
];

/// Camera RAW types, read through WIC.
pub(crate) const RAW: [&str; 10] = [
    "dng", "cr2", "cr3", "nef", "arw", "orf", "rw2", "raf", "pef", "srw",
];

/// Types Windows decodes through Microsoft's Store extensions, with FFmpeg
/// as the fallback.
pub(crate) const HEIF: [&str; 2] = ["heic", "avif"];

/// The largest colour profile carried into a result, in bytes. Real profiles
/// are a few kilobytes to a few hundred; a JPEG can hold none past about
/// 16.7 MB and refuses it only when it writes, which would fail the whole
/// conversion.
const MAX_ICC_BYTES: usize = 4 << 20;

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
    /// grayscale where the target allows it. An animated GIF converted to
    /// WEBP stays animated, with every frame, its delay and its loop count;
    /// converted to any other type, only its first frame is kept. ICO gets
    /// square icons from 256 pixels down.
    /// Pictures are turned upright by their EXIF orientation, and JPG, PNG,
    /// TIFF and WEBP keep the colour profile a picture came with.
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
        if from == "gif" && to == "webp" && facts::animated(req.input, from) {
            return animated_gif_to_webp(req.input, req.output, cap, quality, ctx.cancel);
        }
        let (img, icc) = if from == "svg" {
            (draw_svg(req.input, cap, ctx.cancel)?, None)
        } else {
            let fallback = Fallback {
                options: req.options,
                work: req.output.parent().unwrap_or(Path::new(".")),
            };
            let decoded = decode_with(req.input, from, ctx.cancel, Some(fallback))?;
            (fit(decoded.image, cap), decoded.icc)
        };
        if ctx.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        encode(&img, req.output, to, quality, icc.as_deref())
    }
}

/// The animated GIF at `input` as an animated lossy WEBP at `output`, with
/// every frame, its delay and its loop count. Each frame is scaled to `cap`
/// as a still picture is. Frames are read and handed on one at a time.
fn animated_gif_to_webp(
    input: &Path,
    output: &Path,
    cap: Option<u32>,
    quality: u8,
    cancel: &CancelToken,
) -> Result<()> {
    use image::codecs::gif::GifDecoder;
    use image::AnimationDecoder;
    check_signature(input, "gif")?;
    let file = File::open(input).map_err(|e| Error::io(input, e))?;
    let bad = |e: image::ImageError| Error::Decode {
        path: input.to_path_buf(),
        reason: e.to_string(),
    };
    let mut decoder = GifDecoder::new(BufReader::new(file)).map_err(bad)?;
    // As `decode_full` does: the canvas is counted against the limit before
    // anything is read, and the decoder holds itself to what is left.
    let mut limits = Limits::default();
    limits.reserve(decoder.total_bytes()).map_err(bad)?;
    decoder.set_limits(limits).map_err(bad)?;
    let (w, h) = decoder.dimensions();
    let size = crate::quality::scaled(w, h, cap, false);
    let loops = webp_loops(gif_repeat(input)?);
    let frames = decoder.into_frames().map(|frame| {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let frame = frame.map_err(bad)?;
        let delay = gif_frame_ms(frame.delay());
        let picture = fit(DynamicImage::ImageRgba8(frame.into_buffer()), cap);
        Ok((picture.into_rgba8(), delay))
    });
    let bytes = webp::encode_animation(frames, size, loops, quality)?;
    let mut out = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .map_err(|e| Error::io(output, e))?;
    out.write_all(&bytes)
        .map_err(|e| encode_err("webp", e.to_string()))
}

/// How long a GIF frame is shown, in ms. One under 20 ms is shown for 100
/// ms instead, as browsers do.
fn gif_frame_ms(delay: image::Delay) -> u32 {
    let ms = u32::try_from(Duration::from(delay).as_millis()).unwrap_or(u32::MAX);
    if ms < 20 {
        100
    } else {
        ms
    }
}

/// The loop count in the GIF at `input`. The image crate reports a GIF with
/// no loop extension as looping for ever, which a browser does not do, so
/// this reads it from the GIF's own header. A count of 0 is what the decoder
/// gives for no extension at all; an extension asking for 0 is `Infinite`.
fn gif_repeat(input: &Path) -> Result<gif::Repeat> {
    let bad = |reason: String| Error::Decode {
        path: input.to_path_buf(),
        reason,
    };
    let file = File::open(input).map_err(|e| Error::io(input, e))?;
    let decoder = gif::DecodeOptions::new()
        .read_info(BufReader::new(file))
        .map_err(|e| bad(e.to_string()))?;
    Ok(decoder.repeat())
}

/// The WEBP loop count for a GIF's. A GIF counts the repeats after the
/// first play and a WEBP counts the plays, so a count is one higher; with no
/// loop extension a GIF plays once, which is a count of 1. Zero is for ever
/// in a WEBP.
fn webp_loops(repeat: gif::Repeat) -> u16 {
    match repeat {
        gif::Repeat::Infinite => 0,
        gif::Repeat::Finite(0) => 1,
        gif::Repeat::Finite(repeats) => repeats.saturating_add(1),
    }
}

/// Whether this backend can read `ext`.
pub(crate) fn reads(ext: &str) -> bool {
    FORMATS.contains(&ext) || RAW.contains(&ext) || HEIF.contains(&ext) || ext == "svg"
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
/// images it may grow, up to 50 million pixels. Text uses the fonts
/// installed in Windows.
fn draw_svg(input: &Path, size: Option<u32>, cancel: &CancelToken) -> Result<DynamicImage> {
    render_svg(input, &svg_tree(input)?, size, cancel)
}

/// An SVG's own size, rounded, and a drawing of it with its long side at
/// most `cap`. Unlike a conversion, the drawing never grows.
pub(crate) fn svg_preview(
    input: &Path,
    cap: u32,
    cancel: &CancelToken,
) -> Result<((u32, u32), DynamicImage)> {
    let tree = svg_tree(input)?;
    let natural = tree.size();
    let (w, h) = (natural.width(), natural.height());
    let own = ((w.round() as u32).max(1), (h.round() as u32).max(1));
    let size = (w.max(h) > cap as f32).then_some(cap);
    Ok((own, render_svg(input, &tree, size, cancel)?))
}

/// Parse an SVG, with the fonts installed in Windows for its text. One
/// nested too deeply is refused before it is parsed, and so is one that
/// carries such an SVG as an image.
fn svg_tree(input: &Path) -> Result<resvg::usvg::Tree> {
    let data = fs::read(input).map_err(|e| Error::io(input, e))?;
    let bad = |reason: String| Error::Decode {
        path: input.to_path_buf(),
        reason,
    };
    let head = String::from_utf8_lossy(&data[..data.len().min(4096)]).to_ascii_lowercase();
    // SVGZ starts with the gzip magic number.
    if !head.contains("<svg") && !data.starts_with(&[0x1F, 0x8B]) {
        return Err(Error::WrongContent {
            path: input.to_path_buf(),
            code: "SVG".to_owned(),
        });
    }
    let data = unzip(&data).map_err(bad)?;
    if is_too_deep(&data) {
        return Err(svg_guard::too_deep());
    }
    // External files and URLs are never fetched; only data: images load. An
    // SVG that is such an image is parsed by the same reader, so it gets the
    // same look first.
    let hidden_depth = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&hidden_depth);
    let default_images = resvg::usvg::ImageHrefResolver::default_data_resolver();
    let mut opt = resvg::usvg::Options {
        image_href_resolver: resvg::usvg::ImageHrefResolver {
            resolve_data: Box::new(move |mime, image, opt| {
                if matches!(mime, "image/svg+xml" | "text/plain")
                    && unzip(&image).is_ok_and(|bytes| is_too_deep(&bytes))
                {
                    seen.store(true, Ordering::Relaxed);
                    return None;
                }
                default_images(mime, image, opt)
            }),
            resolve_string: Box::new(|_, _| None),
        },
        ..Default::default()
    };
    opt.fontdb_mut().load_system_fonts();
    let tree = resvg::usvg::Tree::from_data(&data, &opt);
    if hidden_depth.load(Ordering::Relaxed) {
        return Err(svg_guard::too_deep());
    }
    tree.map_err(|e| bad(e.to_string()))
}

/// `data`, unpacked when it is an SVGZ. Fails with the reason when it is
/// damaged or unpacks to more than 64 MB.
fn unzip(data: &[u8]) -> std::result::Result<Cow<'_, [u8]>, String> {
    if !data.starts_with(&[0x1F, 0x8B]) {
        return Ok(Cow::Borrowed(data));
    }
    let mut unpacked = Vec::new();
    flate2::read::GzDecoder::new(data)
        .take(MAX_SVGZ_BYTES + 1)
        .read_to_end(&mut unpacked)
        .map_err(|e| e.to_string())?;
    if unpacked.len() as u64 > MAX_SVGZ_BYTES {
        return Err("it unpacks to more than 64 MB.".to_owned());
    }
    Ok(Cow::Owned(unpacked))
}

/// Whether `svg`, unpacked, is nested too deeply to read. Text that isn't
/// UTF-8 is left for the parser to word.
fn is_too_deep(svg: &[u8]) -> bool {
    std::str::from_utf8(svg).is_ok_and(|text| svg_guard::nesting_depth(text).is_err())
}

/// Draw a parsed SVG at its own size, or with its long side at `size`. A
/// drawing that takes a minute is given up.
fn render_svg(
    input: &Path,
    tree: &resvg::usvg::Tree,
    size: Option<u32>,
    cancel: &CancelToken,
) -> Result<DynamicImage> {
    render_svg_within(input, tree, size, cancel, svg_guard::SVG_TIME_BUDGET)
}

/// [`render_svg`] giving up after `budget`. The drawing is also refused
/// when its effects would cost more than the limit allows at the size it is
/// drawn.
pub(crate) fn render_svg_within(
    input: &Path,
    tree: &resvg::usvg::Tree,
    size: Option<u32>,
    cancel: &CancelToken,
    budget: Duration,
) -> Result<DynamicImage> {
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
    let (w, h) = svg_guard::capped_size(nw * scale, nh * scale);
    let (sx, sy) = (w as f32 / nw, h as f32 / nh);
    if svg_guard::filter_cost(tree, sx.max(sy)) > svg_guard::MAX_FILTER_COST {
        return Err(svg_guard::too_heavy());
    }
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)
        .ok_or_else(|| bad("the drawing has no size.".to_owned()))?;
    let transform = resvg::tiny_skia::Transform::from_scale(sx, sy);
    draw_in_turn(tree, transform, &mut pixmap.as_mut(), cancel, budget)?;
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

/// Draw the elements at the top of `tree` onto `pixmap` one after another,
/// the same picture as drawing them all at once. Between elements it
/// stops if `cancel` is set, or if `budget` has gone since the first.
fn draw_in_turn(
    tree: &resvg::usvg::Tree,
    transform: resvg::tiny_skia::Transform,
    pixmap: &mut resvg::tiny_skia::PixmapMut,
    cancel: &CancelToken,
    budget: Duration,
) -> Result<()> {
    let started = Instant::now();
    let check = || {
        if cancel.is_cancelled() {
            Err(Error::Cancelled)
        } else if started.elapsed() >= budget {
            Err(svg_guard::too_slow())
        } else {
            Ok(())
        }
    };
    let elements = tree.root().children();
    if !elements.iter().all(draws_alone) {
        // One element can't be drawn by itself, so none of them are.
        check()?;
        resvg::render(tree, transform, pixmap);
        return Ok(());
    }
    for element in elements {
        check()?;
        // resvg draws a lone element at the corner of its pixmap; moving the
        // drawing by the element's own corner puts it back in place. The
        // scale has no shift of its own, so the two moves cancel exactly.
        let Some(corner) = element.abs_layer_bounding_box() else {
            continue;
        };
        resvg::render_node(
            element,
            transform.pre_translate(corner.x(), corner.y()),
            pixmap,
        );
    }
    Ok(())
}

/// Whether `resvg::render_node` draws `element` by itself. It leaves out an
/// element with no area, which is right for a fill but not for a straight
/// line with a stroke.
fn draws_alone(element: &resvg::usvg::Node) -> bool {
    use resvg::usvg::Node;
    element.abs_layer_bounding_box().is_some()
        || match element {
            Node::Path(path) => path.stroke().is_none(),
            Node::Image(_) => true,
            Node::Group(_) | Node::Text(_) => false,
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
        "ico" => ImageFormat::Ico,
        "tga" => ImageFormat::Tga,
        "qoi" => ImageFormat::Qoi,
        "ppm" => ImageFormat::Pnm,
        _ => return None,
    })
}

/// A decoded picture, with what it carried that a result should keep.
pub(crate) struct Decoded {
    /// Already turned upright by `orientation`.
    pub image: DynamicImage,
    /// The ICC colour profile stored in the file, as it was.
    pub icc: Option<Vec<u8>>,
    /// The EXIF orientation the file asked for, now applied to `image`.
    /// `NoTransforms` for a picture that was upright, or a type that has
    /// none.
    pub orientation: Orientation,
}

/// Decode an image of type `ext`, upright, checking its signature first.
/// An SVG is drawn with no job to cancel it: its callers read other types.
pub(crate) fn decode(input: &Path, ext: &str) -> Result<DynamicImage> {
    Ok(decode_full(input, ext, &CancelToken::new())?.image)
}

/// Where FFmpeg may decode a HEIC or AVIF that Windows can't: the job's
/// options, which name the FFmpeg to use, and the job's private folder,
/// which holds the PNG it writes.
#[derive(Clone, Copy)]
pub(crate) struct Fallback<'a> {
    pub options: &'a Options,
    pub work: &'a Path,
}

/// [`decode`], with the colour profile and the orientation that was applied.
/// An SVG is drawn upright, and RAW is turned upright by Windows, so neither
/// has either. An SVG's drawing stops if `cancel` is set, as in a
/// conversion. A HEIC or AVIF is read by Windows alone; see
/// [`decode_with`].
pub(crate) fn decode_full(input: &Path, ext: &str, cancel: &CancelToken) -> Result<Decoded> {
    decode_with(input, ext, cancel, None)
}

/// [`decode_full`], with FFmpeg reading a HEIC or AVIF that Windows can't
/// read, or one with transparency, or a JPG Kiln's own reader can't read,
/// when `fallback` is given. Without either, a HEIC's or AVIF's error
/// names the Store extension to get. Every job that may
/// read a HEIC or AVIF gives a fallback: without one, a transparent AVIF is
/// read opaque by Windows.
pub(crate) fn decode_with(
    input: &Path,
    ext: &str,
    cancel: &CancelToken,
    fallback: Option<Fallback<'_>>,
) -> Result<Decoded> {
    let plain = |image| Decoded {
        image,
        icc: None,
        orientation: Orientation::NoTransforms,
    };
    if ext == "svg" {
        return draw_svg(input, None, cancel).map(plain);
    }
    if RAW.contains(&ext) {
        return decode_raw(input).map(plain);
    }
    if HEIF.contains(&ext) {
        return decode_heif(input, ext, cancel, fallback);
    }
    let format = image_format(ext).ok_or_else(|| {
        Error::Registry(format!(
            "Kiln can't read {} images.",
            ext.to_ascii_uppercase()
        ))
    })?;
    check_signature(input, ext)?;
    match (decode_own(input, format), fallback) {
        (Ok(decoded), _) => Ok(decoded),
        // A JPG in a form Kiln's own reader doesn't take (a lossless or
        // 12-bit one, say) is read by FFmpeg when there is one (D21).
        // Without it, or if FFmpeg can't read it either, the reader's error
        // stands. A damaged JPG in a form the reader takes is not handed
        // on: FFmpeg fills what it can't read, and the picture would be
        // saved as if it were whole. One over the memory cap is not handed
        // on either.
        (Err(Unread { error, other_may }), Some(fallback)) if other_may && ext == "jpg" => {
            let Some(form) = jpeg_form(input).filter(JpegForm::beyond_own_reader) else {
                return Err(error);
            };
            match decode_by_ffmpeg(input, ext, cancel, fallback) {
                // FFmpeg leaves the pixels as stored (`decode_to_png`).
                // One that carries the Exif into its PNG has had them
                // turned by the PNG's read; otherwise they are turned here
                // as the JPG's Exif asks, as Kiln's reader turns one.
                Ok(mut decoded) => {
                    if decoded.orientation == Orientation::NoTransforms {
                        decoded.image.apply_orientation(form.orientation);
                        decoded.orientation = form.orientation;
                    }
                    Ok(decoded)
                }
                Err(Error::Cancelled) => Err(Error::Cancelled),
                Err(_) => Err(error),
            }
        }
        (Err(unread), _) => Err(unread.error),
    }
}

/// What a JPG's header says of its picture, read up to its frame header
/// without decoding anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct JpegForm {
    /// The start-of-frame marker: 0xC0 baseline, 0xC1 extended, 0xC2
    /// progressive, 0xC3 lossless, 0xC9 to 0xCB arithmetic coding, and the
    /// hierarchical ones.
    frame: u8,
    /// Bits in each sample.
    precision: u8,
    /// What the first Exif block before the frame header asks for.
    orientation: Orientation,
}

impl JpegForm {
    /// A form Kiln's reader doesn't take: anything but 8-bit baseline,
    /// extended or progressive with Huffman coding.
    fn beyond_own_reader(&self) -> bool {
        !matches!(self.frame, 0xC0..=0xC2) || self.precision != 8
    }
}

/// How far into a JPG its frame header is looked for: past the metadata
/// a camera writes before it.
const JPEG_HEAD_BYTES: u64 = 1 << 20;

/// The form of the JPG at `input`; `None` when its header can't be read
/// to the frame header.
fn jpeg_form(input: &Path) -> Option<JpegForm> {
    let mut head = Vec::new();
    File::open(input)
        .ok()?
        .take(JPEG_HEAD_BYTES)
        .read_to_end(&mut head)
        .ok()?;
    jpeg_form_in(&head)
}

fn jpeg_form_in(src: &[u8]) -> Option<JpegForm> {
    if !src.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut orientation = None;
    let mut pos = 2;
    loop {
        if *src.get(pos)? != 0xFF {
            return None;
        }
        // Fill bytes before a marker.
        while src.get(pos + 1) == Some(&0xFF) {
            pos += 1;
        }
        let marker = *src.get(pos + 1)?;
        match marker {
            0x01 | 0xD0..=0xD7 => {
                pos += 2;
                continue;
            }
            // The picture's data, or its end, before any frame header.
            0xD9 | 0xDA => return None,
            _ => {}
        }
        let len = usize::from(u16::from_be_bytes([*src.get(pos + 2)?, *src.get(pos + 3)?]));
        let payload = src.get(pos + 4..(pos + 2).checked_add(len)?)?;
        match marker {
            // Huffman and arithmetic tables, and JPG extensions, share the
            // range with the frame headers.
            0xC4 | 0xC8 | 0xCC => {}
            0xC0..=0xCF => {
                return Some(JpegForm {
                    frame: marker,
                    precision: *payload.first()?,
                    orientation: orientation.unwrap_or(Orientation::NoTransforms),
                });
            }
            0xE1 if orientation.is_none() => {
                if let Some(tiff) = payload.strip_prefix(b"Exif\0\0") {
                    orientation = Orientation::from_exif_chunk(tiff);
                }
            }
            _ => {}
        }
        pos += 2 + len;
    }
}

/// Why Kiln's own reader didn't read a picture, and whether another
/// reader may: it is in a form the reader doesn't take, or damaged, not
/// over the memory cap or out of reach.
struct Unread {
    error: Error,
    other_may: bool,
}

/// The picture at `input`, of type `format`, read by Kiln's own reader.
fn decode_own(input: &Path, format: ImageFormat) -> std::result::Result<Decoded, Unread> {
    let file = File::open(input).map_err(|e| Unread {
        error: Error::io(input, e),
        other_may: false,
    })?;
    let bad = |e: image::ImageError| Unread {
        other_may: matches!(
            e,
            image::ImageError::Decoding(_) | image::ImageError::Unsupported(_)
        ),
        error: Error::Decode {
            path: input.to_path_buf(),
            reason: e.to_string(),
        },
    };
    let mut decoder = open_decoder(file, format).map_err(bad)?;
    // The image crate's own `decode` reserves the picture's memory against
    // its limit before it reads anything; a decoder taken from the reader
    // is left to do that itself.
    let mut limits = Limits::default();
    limits.reserve(decoder.total_bytes()).map_err(bad)?;
    // Both are read after that check and before the decoder is used up.
    // Damaged metadata is a picture without it, not one that can't be
    // opened, and a profile past the cap is not kept.
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let icc = decoder
        .icc_profile()
        .ok()
        .flatten()
        .filter(|profile| profile.len() <= MAX_ICC_BYTES);
    decoder.set_limits(limits).map_err(bad)?;
    let mut image = DynamicImage::from_decoder(decoder).map_err(bad)?;
    image.apply_orientation(orientation);
    Ok(Decoded {
        image,
        icc,
        orientation,
    })
}

/// A decoder for `file`, a picture of type `format`, that has not read its
/// pixels. The TIFF decoder is made here and not by the reader: once limits
/// are set on it, it can no longer read a field larger than the picture's
/// pixels, which a colour profile on a small picture is.
fn open_decoder(file: File, format: ImageFormat) -> image::ImageResult<Box<dyn ImageDecoder>> {
    let reader = BufReader::new(file);
    Ok(if format == ImageFormat::Tiff {
        Box::new(image::codecs::tiff::TiffDecoder::new(reader)?)
    } else {
        Box::new(ImageReader::with_format(reader, format).into_decoder()?)
    })
}

/// The size of the picture at `input`, a file of type `ext`, as it is shown:
/// turned upright, read from the file's header without decoding it. `None`
/// when the header can't be read.
pub(crate) fn upright_size(input: &Path, ext: &str) -> Option<(u32, u32)> {
    let file = File::open(input).ok()?;
    let mut decoder = open_decoder(file, image_format(ext)?).ok()?;
    let (w, h) = decoder.dimensions();
    // Orientations 5 to 8 turn the picture a quarter.
    let quarter = decoder
        .orientation()
        .is_ok_and(|o| (5..=8).contains(&o.to_exif()));
    Some(if quarter { (h, w) } else { (w, h) })
}

/// Whether the colour profile `profile` suits `img`: its colour space, the
/// four bytes at offset 16 of an ICC profile, is RGB for a colour picture
/// and gray for a gray one. The JPEG decoder turns a CMYK picture into RGB
/// but still returns its CMYK profile, which would turn the colours wrong.
/// A profile over [`MAX_ICC_BYTES`] doesn't fit any picture.
pub(crate) fn icc_fits(profile: &[u8], img: &DynamicImage) -> bool {
    icc_suits(profile, is_gray(img))
}

/// Whether the colour profile `profile` suits a picture that is gray or
/// colour, as [`icc_fits`] judges.
fn icc_suits(profile: &[u8], gray: bool) -> bool {
    let space: &[u8] = if gray { b"GRAY" } else { b"RGB " };
    profile.len() <= MAX_ICC_BYTES && profile.get(16..20) == Some(space)
}

/// Give `encoder` the colour profile `icc`, if there is one and it suits
/// `img`. An encoder that can't hold one writes the picture without.
fn embed_profile(encoder: &mut impl ImageEncoder, icc: Option<&[u8]>, img: &DynamicImage) {
    if let Some(profile) = icc.filter(|p| icc_fits(p, img)) {
        let _ = encoder.set_icc_profile(profile.to_vec());
    }
}

/// Write `img` as `ext` to a new file at `output`. `quality` applies to JPG
/// and WEBP. JPG, PNG, TIFF and WEBP carry the colour profile `icc` when it
/// suits the picture; other types are written without one.
pub(crate) fn encode(
    img: &DynamicImage,
    output: &Path,
    ext: &str,
    quality: u8,
    icc: Option<&[u8]>,
) -> Result<()> {
    let out = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .map_err(|e| Error::io(output, e))?;
    match ext {
        "jpg" => write_jpeg(out, img, quality, icc),
        "webp" => write_webp(out, img, quality, icc),
        "png" => write_png(out, img, icc),
        "ico" => write_ico(out, img),
        "ppm" => write_ppm(out, img),
        other => write_other(out, img, other, icc),
    }
}

/// `img` as a JPG, PNG or WEBP in memory, as [`encode`] writes it to a
/// file, for trial encodes that are measured before anything is saved.
pub(crate) fn encode_bytes(
    img: &DynamicImage,
    ext: &str,
    quality: u8,
    icc: Option<&[u8]>,
) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    match ext {
        "jpg" => write_jpeg(&mut bytes, img, quality, icc)?,
        "png" => write_png(&mut bytes, img, icc)?,
        "webp" => return webp::encode_lossy(img, quality, icc.filter(|p| icc_suits(p, false))),
        other => {
            return Err(Error::Registry(format!(
                "Kiln can't write {} images here.",
                other.to_ascii_uppercase()
            )))
        }
    }
    Ok(bytes)
}

fn write_png(file: impl Write, img: &DynamicImage, icc: Option<&[u8]>) -> Result<()> {
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    let mut out = BufWriter::new(file);
    let mut encoder =
        PngEncoder::new_with_quality(&mut out, CompressionType::Best, FilterType::Adaptive);
    embed_profile(&mut encoder, icc, img);
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

/// WEBP, lossy at `quality`, through libwebp. It holds colour only, so a
/// gray picture is written as RGB and the colour profile is judged as for an
/// RGB one.
fn write_webp(mut file: File, img: &DynamicImage, quality: u8, icc: Option<&[u8]>) -> Result<()> {
    let profile = icc.filter(|p| icc_suits(p, false));
    let bytes = webp::encode_lossy(img, quality, profile)?;
    file.write_all(&bytes)
        .map_err(|e| encode_err("webp", e.to_string()))
}

/// BMP, GIF, TIFF, TGA and QOI. Each gets 8-bit pixels in a layout its
/// encoder takes. TIFF also gets the colour profile.
fn write_other(file: File, img: &DynamicImage, ext: &str, icc: Option<&[u8]>) -> Result<()> {
    use image::codecs::{
        bmp::BmpEncoder, gif::GifEncoder, qoi::QoiEncoder, tga::TgaEncoder, tiff::TiffEncoder,
    };
    let img = eight_bit(img, ext);
    let mut out = BufWriter::new(file);
    let result = match ext {
        "bmp" => img.write_with_encoder(BmpEncoder::new(&mut out)),
        "gif" => img.write_with_encoder(GifEncoder::new(&mut out)),
        "tiff" => {
            // The profile is judged against the layout TIFF gets.
            let mut encoder = TiffEncoder::new(&mut out);
            embed_profile(&mut encoder, icc, &img);
            img.write_with_encoder(encoder)
        }
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
    must_be_a_file(input)?;
    wic::decode(input)
}

/// A HEIC or AVIF, read by Windows, or by FFmpeg when Windows can't read
/// it and `fallback` says where FFmpeg may work. Windows' decoders turn the
/// picture upright themselves but drop alpha and pass on no colour
/// profile, so the file's own profile is read from its boxes, and a picture
/// with an alpha plane goes to FFmpeg first. A transparent AVIF is never
/// read without its alpha: with no FFmpeg, it is refused.
fn decode_heif(
    input: &Path,
    ext: &str,
    cancel: &CancelToken,
    fallback: Option<Fallback<'_>>,
) -> Result<Decoded> {
    // Windows hands a file to whichever decoder claims it, so the file
    // must be a HEIF file first.
    check_signature(input, ext)?;
    let extras = crate::heif::extras(input);
    let icc = extras.icc.filter(|p| p.len() <= MAX_ICC_BYTES);
    let with_profile = |mut decoded: Decoded| {
        if decoded.icc.is_none() {
            decoded.icc = icc.clone();
        }
        decoded
    };
    // FFmpeg's answer when it was asked first, for a transparent picture.
    let mut tried = None;
    if extras.alpha {
        if let Some(fallback) = fallback {
            match decode_by_ffmpeg(input, ext, cancel, fallback) {
                Ok(decoded) => return Ok(with_profile(decoded)),
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(Error::EngineMissing { .. }) if ext == "avif" => {
                    return Err(Error::Decode {
                        path: input.to_path_buf(),
                        reason: "Needs FFmpeg to keep this AVIF's transparency.".to_owned(),
                    })
                }
                Err(e) => tried = Some(e),
            }
        }
    }
    let unread = if windows_hidden() {
        wic::Unread::hidden(input)
    } else {
        match wic::decode_any(input) {
            Ok(decoded) => return Ok(with_profile(decoded)),
            Err(unread) => unread,
        }
    };
    // Any failure in Windows goes to FFmpeg when there is one.
    let ffmpeg = match (tried, fallback) {
        (Some(e), _) => e,
        (None, Some(fallback)) => match decode_by_ffmpeg(input, ext, cancel, fallback) {
            Ok(decoded) => return Ok(with_profile(decoded)),
            Err(e) => e,
        },
        (None, None) => Error::EngineMissing { engine: "FFmpeg" },
    };
    Err(match (ffmpeg, unread.lacks) {
        (Error::EngineMissing { .. }, Lacks::Nothing) => unread.error,
        (Error::EngineMissing { .. }, missing) => needs_decoder(ext, missing),
        (other, _) => other,
    })
}

/// What a Windows failure to read a HEIC or AVIF says is missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lacks {
    /// No decoder for the file: the HEIF Image Extensions, or the AV1
    /// Video Extension.
    Decoder,
    /// A decoder, but not the codec inside it: the HEVC Video Extensions.
    Codec,
    /// Neither: the decoder read the file and failed.
    Nothing,
}

/// What the Windows error `code` says is missing:
/// WINCODEC_ERR_COMPONENTNOTFOUND and WINCODEC_ERR_UNKNOWNIMAGEFORMAT mean
/// no decoder, and MF_E_TOPO_CODEC_NOT_FOUND a decoder without its codec.
/// Other Media Foundation codes, such as MF_E_INVALID_FILE_FORMAT for a
/// damaged file, say nothing is missing.
fn lacks(code: u32) -> Lacks {
    match code {
        0x8898_2F50 | 0x8898_2F07 => Lacks::Decoder,
        0xC00D_5212 => Lacks::Codec,
        _ => Lacks::Nothing,
    }
}

/// The picture at `input` through FFmpeg: written as a PNG in the job's
/// folder, read back, and the PNG removed.
fn decode_by_ffmpeg(
    input: &Path,
    ext: &str,
    cancel: &CancelToken,
    fallback: Fallback<'_>,
) -> Result<Decoded> {
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let png = free_path(fallback.work, "decoded", "png");
    let decoded = crate::ffmpeg::decode_to_png(fallback.options, cancel, input, ext, &png)
        .and_then(|()| decode_full(&png, "png", cancel));
    // Kiln's own file in the job's folder, which goes with the job anyway.
    let _ = fs::remove_file(&png);
    decoded
}

/// A path in `dir` named `stem`, or `stem-2`, `stem-3` and so on, that
/// doesn't exist yet.
fn free_path(dir: &Path, stem: &str, ext: &str) -> std::path::PathBuf {
    let mut path = dir.join(format!("{stem}.{ext}"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem}-{n}.{ext}"));
        n += 1;
    }
    path
}

/// The Microsoft Store products a HEIC or AVIF needs.
const HEIF_EXTENSIONS: &str = "9PMMSR1CGPWG";
const HEVC_EXTENSIONS: &str = "9N4WGH0Z6VHQ";
const AV1_EXTENSION: &str = "9MVZQVXJBQ9V";

/// The error for a HEIC or AVIF Kiln can't read, Windows lacking `missing`:
/// it names the Store extensions, and the row's Get it link opens the page
/// of the one that is missing. A HEIC needs two: the HEIF Image Extensions
/// to open the file and the HEVC Video Extensions for the picture inside.
fn needs_decoder(ext: &str, missing: Lacks) -> Error {
    if ext == "avif" {
        Error::NeedsExtension {
            message: "Needs Microsoft's AV1 Video Extension, or FFmpeg.".to_owned(),
            product: AV1_EXTENSION,
        }
    } else {
        Error::NeedsExtension {
            message: "Needs Microsoft's HEIF Image and HEVC Video Extensions, or FFmpeg."
                .to_owned(),
            product: if missing == Lacks::Codec {
                HEVC_EXTENSIONS
            } else {
                HEIF_EXTENSIONS
            },
        }
    }
}

/// True when, in a debug build, `KILN_HIDE_WIC` asks to act as if Windows
/// had no decoder for HEIC or AVIF.
pub(crate) fn windows_hidden() -> bool {
    #[cfg(debug_assertions)]
    if std::env::var_os("KILN_HIDE_WIC").is_some() {
        return true;
    }
    false
}

/// The size of the HEIC or AVIF at `input` as it is shown, read by Windows
/// from the file's header. `None` when Windows can't read it.
pub(crate) fn heif_size(input: &Path) -> Option<(u32, u32)> {
    if windows_hidden() || check_signature(input, "heic").is_err() {
        return None;
    }
    wic::size(input).ok()
}

/// `Ok` for an existing file; the input errors otherwise.
fn must_be_a_file(input: &Path) -> Result<()> {
    match fs::metadata(input) {
        Ok(m) if m.is_file() => Ok(()),
        Ok(_) => Err(Error::InputNotAFile(input.to_path_buf())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(Error::InputNotFound(input.to_path_buf()))
        }
        Err(e) => Err(Error::io(input, e)),
    }
}

#[cfg(windows)]
pub(crate) mod wic {
    use std::path::Path;

    use image::metadata::Orientation;
    use image::{DynamicImage, ImageBuffer, Limits, Rgb, Rgba};
    use windows::core::{Interface, GUID, HSTRING};
    use windows::Win32::Foundation::GENERIC_READ;
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, GUID_WICPixelFormat32bppRGBA, GUID_WICPixelFormat48bppRGB,
        GUID_WICPixelFormat64bppRGBA, IWICBitmapFrameDecode, IWICColorContext, IWICImagingFactory,
        IWICPixelFormatInfo2, WICBitmapDitherTypeNone, WICBitmapPaletteTypeCustom,
        WICColorContextProfile, WICDecodeMetadataCacheOnDemand,
    };
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_MULTITHREADED,
    };

    use super::{lacks, Decoded, Lacks, MAX_ICC_BYTES};
    use crate::error::{Error, Result};

    /// WINCODEC_ERR_COMPONENTNOTFOUND: no decoder for the file.
    const NO_DECODER: u32 = 0x8898_2F50;

    /// COM on this thread for one read: started (or counted once more where
    /// it already runs) and, when that worked, ended again when dropped, as
    /// every CoInitializeEx needs its CoUninitialize. Made before any COM
    /// object, so it is dropped after them.
    struct Com;

    impl Com {
        fn start() -> Option<Com> {
            // SAFETY: starts COM on the calling thread; a thread already in
            // another apartment makes it fail, and nothing is then undone.
            unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
                .is_ok()
                .then_some(Com)
        }
    }

    impl Drop for Com {
        fn drop(&mut self) {
            // SAFETY: balances the CoInitializeEx that made this value.
            unsafe { CoUninitialize() };
        }
    }

    /// Why Windows didn't read a picture: what it lacks, and its error,
    /// which names the Windows code for Show details.
    pub struct Unread {
        pub lacks: Lacks,
        pub error: Error,
    }

    impl Unread {
        /// As when Windows has no decoder for `input`.
        pub fn hidden(input: &Path) -> Self {
            Unread {
                lacks: Lacks::Decoder,
                error: Error::Decode {
                    path: input.to_path_buf(),
                    reason: "Windows has no decoder for it.".to_owned(),
                },
            }
        }
    }

    /// The picture at `input` through whichever decoder Windows has for it,
    /// upright, with its colour profile: 8 bits per channel, or 16 for a
    /// deeper picture (HDR is decoded, not tone-mapped).
    pub fn decode_any(input: &Path) -> std::result::Result<Decoded, Unread> {
        match read_any(input) {
            Ok(decoded) => Ok(decoded),
            Err(Failure::Com(e)) => {
                let code = e.code().0 as u32;
                Err(Unread {
                    lacks: lacks(code),
                    error: Error::Decode {
                        path: input.to_path_buf(),
                        reason: format!("{} (Windows error 0x{code:08X})", e.message()),
                    },
                })
            }
            Err(Failure::Kiln(error)) => Err(Unread {
                lacks: Lacks::Nothing,
                error,
            }),
        }
    }

    enum Failure {
        Com(windows::core::Error),
        Kiln(Error),
    }

    impl From<windows::core::Error> for Failure {
        fn from(e: windows::core::Error) -> Self {
            Failure::Com(e)
        }
    }

    fn read_any(input: &Path) -> std::result::Result<Decoded, Failure> {
        // SAFETY: plain COM calls on interfaces this function owns. The
        // pixel buffer is sized from the converter's own dimensions and is
        // reserved against the image crate's memory limit first; a 16-bit
        // buffer is lent to Windows as bytes, which any u16 can be. The
        // file is opened for reading only.
        unsafe {
            let _com = Com::start();
            let factory: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
            let decoder = factory.CreateDecoderFromFilename(
                &HSTRING::from(input.as_os_str()),
                None,
                GENERIC_READ,
                WICDecodeMetadataCacheOnDemand,
            )?;
            let frame = decoder.GetFrame(0)?;
            let (deep, alpha) = depth_and_alpha(&factory, &frame.GetPixelFormat()?);
            let (format, bytes_per_pixel) = if deep {
                (GUID_WICPixelFormat64bppRGBA, 8u32)
            } else {
                (GUID_WICPixelFormat32bppRGBA, 4u32)
            };
            let converter = factory.CreateFormatConverter()?;
            converter.Initialize(
                &frame,
                &format,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeCustom,
            )?;
            let (mut w, mut h) = (0u32, 0u32);
            converter.GetSize(&mut w, &mut h)?;
            let (stride, total, needed) =
                pixel_bytes(w, h, bytes_per_pixel, alpha).ok_or_else(|| too_big(input))?;
            let mut limits = Limits::default();
            limits.reserve(needed).map_err(|e| {
                Failure::Kiln(Error::Decode {
                    path: input.to_path_buf(),
                    reason: e.to_string(),
                })
            })?;
            let image = if deep {
                // One buffer of 16-bit samples, which Windows fills in the
                // machine's own byte order.
                let mut samples = vec![0u16; total / 2];
                let view = std::slice::from_raw_parts_mut(samples.as_mut_ptr().cast::<u8>(), total);
                converter.CopyPixels(std::ptr::null(), stride, view)?;
                let img: ImageBuffer<Rgba<u16>, Vec<u16>> =
                    ImageBuffer::from_raw(w, h, samples).expect("the buffer matches the size");
                let img = DynamicImage::ImageRgba16(img);
                if alpha {
                    img
                } else {
                    DynamicImage::ImageRgb16(img.to_rgb16())
                }
            } else {
                let mut bytes = vec![0u8; total];
                converter.CopyPixels(std::ptr::null(), stride, &mut bytes)?;
                let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
                    ImageBuffer::from_raw(w, h, bytes).expect("the buffer matches the size");
                let img = DynamicImage::ImageRgba8(img);
                if alpha {
                    img
                } else {
                    DynamicImage::ImageRgb8(img.to_rgb8())
                }
            };
            let turn = orientation(&frame);
            Ok(Decoded {
                image: upright(image, turn),
                icc: profile(&factory, &frame),
                orientation: Orientation::from_exif(turn as u8)
                    .unwrap_or(Orientation::NoTransforms),
            })
        }
    }

    /// For a `w` x `h` picture of `bytes_per_pixel`: the row size, the
    /// pixels' size, and the memory a read needs at most: the pixels, and
    /// the copy without alpha made from them for an opaque picture. `None`
    /// when a row's size doesn't fit.
    pub(super) fn pixel_bytes(
        w: u32,
        h: u32,
        bytes_per_pixel: u32,
        alpha: bool,
    ) -> Option<(u32, usize, u64)> {
        let stride = w.checked_mul(bytes_per_pixel)?;
        let total = u64::from(stride) * u64::from(h);
        let opaque = if alpha { 0 } else { total / 4 * 3 };
        Some((stride, usize::try_from(total).ok()?, total + opaque))
    }

    fn too_big(input: &Path) -> Failure {
        Failure::Kiln(Error::Decode {
            path: input.to_path_buf(),
            reason: "the picture is too large.".to_owned(),
        })
    }

    /// Whether the pixel format `format` has more than 8 bits in its first
    /// channel, and whether it can be transparent. A format Windows can't
    /// describe is taken as 8 bits with alpha.
    fn depth_and_alpha(factory: &IWICImagingFactory, format: &GUID) -> (bool, bool) {
        // SAFETY: reads a description of a pixel format; the mask buffer is
        // as large as any pixel can be.
        unsafe {
            let Ok(info) = factory
                .CreateComponentInfo(format)
                .and_then(|c| c.cast::<IWICPixelFormatInfo2>())
            else {
                return (false, true);
            };
            let alpha = info.SupportsTransparency().is_ok_and(|b| b.as_bool());
            let mut mask = [0u8; 64];
            let mut used = 0u32;
            let deep = info.GetChannelMask(0, &mut mask, &mut used).is_ok()
                && mask[..(used as usize).min(mask.len())]
                    .iter()
                    .map(|b| b.count_ones())
                    .sum::<u32>()
                    > 8;
            (deep, alpha)
        }
    }

    /// The frame's first ICC colour profile, when it has one of a size Kiln
    /// keeps.
    fn profile(factory: &IWICImagingFactory, frame: &IWICBitmapFrameDecode) -> Option<Vec<u8>> {
        // SAFETY: asks the frame how many colour contexts it has, then fills
        // that many contexts this function made; profile bytes go into a
        // buffer of the size the context reports.
        unsafe {
            let mut count = 0u32;
            frame.GetColorContexts(&mut [], &mut count).ok()?;
            if count == 0 {
                return None;
            }
            let mut contexts: Vec<Option<IWICColorContext>> = (0..count)
                .map(|_| factory.CreateColorContext().ok())
                .collect();
            if contexts.iter().any(Option::is_none) {
                return None;
            }
            frame.GetColorContexts(&mut contexts, &mut count).ok()?;
            contexts.into_iter().flatten().find_map(|context| {
                if context.GetType().ok()? != WICColorContextProfile {
                    return None;
                }
                let mut size = 0u32;
                context.GetProfileBytes(&mut [], &mut size).ok()?;
                if size == 0 || size as usize > MAX_ICC_BYTES {
                    return None;
                }
                let mut bytes = vec![0u8; size as usize];
                context.GetProfileBytes(&mut bytes, &mut size).ok()?;
                bytes.truncate(size as usize);
                Some(bytes)
            })
        }
    }

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
            let _com = Com::start();
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
            let _com = Com::start();
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

    /// Why Windows didn't read a picture: here, it never does.
    pub struct Unread {
        pub lacks: super::Lacks,
        pub error: Error,
    }

    impl Unread {
        pub fn hidden(input: &Path) -> Self {
            Unread {
                lacks: super::Lacks::Decoder,
                error: Error::Decode {
                    path: input.to_path_buf(),
                    reason: "Windows has no decoder for it.".to_owned(),
                },
            }
        }
    }

    /// Windows' decoders aren't here, so nothing is read.
    pub fn decode_any(input: &Path) -> std::result::Result<super::Decoded, Unread> {
        Err(Unread::hidden(input))
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
        "heic" | "avif" => heif_brand(head),
        _ => false,
    }
}

/// Whether `head` starts with an ISO `ftyp` box naming a HEIF brand, as
/// its major brand or a compatible one within what was read.
fn heif_brand(head: &[u8]) -> bool {
    const BRANDS: [&[u8]; 10] = [
        b"heic", b"heix", b"heim", b"heis", b"hevc", b"hevx", b"mif1", b"msf1", b"avif", b"avis",
    ];
    if head.len() < 12 || &head[4..8] != b"ftyp" {
        return false;
    }
    let size = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
    let compatible = head.get(16..size.min(head.len())).unwrap_or_default();
    std::iter::once(&head[8..12])
        .chain(compatible.chunks_exact(4))
        .any(|brand| BRANDS.contains(&brand))
}

/// Reject anything that isn't an existing file starting with the right
/// signature for `ext`, and a JPG cut short. The extension alone is not
/// trusted.
pub(crate) fn check_signature(input: &Path, ext: &str) -> Result<()> {
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

    // Enough for a HEIF file's brands.
    let mut head = Vec::with_capacity(64);
    let file = File::open(input).map_err(|e| Error::io(input, e))?;
    file.take(64)
        .read_to_end(&mut head)
        .map_err(|e| Error::io(input, e))?;
    if !signature_matches(&head, ext) {
        return Err(Error::WrongContent {
            path: input.to_path_buf(),
            code: ext.to_ascii_uppercase(),
        });
    }
    // Every read of a JPG passes here (D26): a conversion, Make smaller,
    // Resize, Remove location data and the estimates. One cut short is
    // refused before it is decoded: Kiln's reader would fill what is
    // missing with grey and the result would be saved as if whole.
    if ext == "jpg" && crate::check::jpeg_cut_short(input) {
        Err(Error::CutShort {
            path: input.to_path_buf(),
            code: "JPG".to_owned(),
        })
    } else {
        Ok(())
    }
}

fn write_jpeg(file: impl Write, img: &DynamicImage, quality: u8, icc: Option<&[u8]>) -> Result<()> {
    let (w, h) = (img.width(), img.height());
    let mut out = BufWriter::new(file);
    let mut encoder = JpegEncoder::new_with_quality(&mut out, quality);
    embed_profile(&mut encoder, icc, img);

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

    /// A JPG's header to its frame header: an Exif block asking for
    /// `orientation` when given, then a frame header `frame` of
    /// `precision` bits.
    fn jpeg_head(orientation: Option<u16>, frame: u8, precision: u8) -> Vec<u8> {
        let mut out = vec![0xFF, 0xD8];
        if let Some(o) = orientation {
            let mut tiff = b"II*\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0".to_vec();
            tiff.extend(o.to_le_bytes());
            tiff.extend([0, 0, 0, 0, 0, 0]);
            let body = [b"Exif\0\0".as_slice(), &tiff].concat();
            out.extend([0xFF, 0xE1]);
            out.extend(u16::try_from(body.len() + 2).unwrap().to_be_bytes());
            out.extend(body);
        }
        // A Huffman table first, which shares the frame headers' range.
        out.extend([0xFF, 0xC4, 0x00, 0x03, 0x00]);
        out.extend([
            0xFF, frame, 0x00, 0x0B, precision, 0, 40, 0, 64, 1, 1, 0x11, 0,
        ]);
        out.extend([0xFF, 0xDA, 0x00, 0x02]);
        out
    }

    #[test]
    fn a_jpgs_form_is_read_from_its_header() {
        let form = jpeg_form_in(&jpeg_head(Some(6), 0xC3, 8)).unwrap();
        assert_eq!((form.frame, form.precision), (0xC3, 8));
        assert_eq!(form.orientation, Orientation::Rotate90);
        assert!(form.beyond_own_reader(), "lossless");
        let form = jpeg_form_in(&jpeg_head(None, 0xC0, 8)).unwrap();
        assert_eq!(form.orientation, Orientation::NoTransforms);
        assert!(!form.beyond_own_reader(), "baseline");
        assert!(!jpeg_form_in(&jpeg_head(None, 0xC2, 8))
            .unwrap()
            .beyond_own_reader());
        for (frame, precision) in [(0xC1, 12), (0xC9, 8), (0xC5, 8)] {
            let form = jpeg_form_in(&jpeg_head(None, frame, precision)).unwrap();
            assert!(form.beyond_own_reader(), "{frame:X} {precision}");
        }
        // No frame header before the picture's data, or cut short.
        assert_eq!(jpeg_form_in(&[0xFF, 0xD8, 0xFF, 0xDA, 0x00, 0x02]), None);
        let head = jpeg_head(Some(6), 0xC3, 8);
        assert_eq!(jpeg_form_in(&head[..30]), None);
        assert_eq!(jpeg_form_in(b"not a jpg"), None);
    }

    #[test]
    fn blend_endpoints() {
        assert_eq!(blend(10, 255, 255), 10);
        assert_eq!(blend(10, 0, 255), 255);
        assert_eq!(blend(0, 128, 255), 127);
    }

    #[test]
    fn a_heic_or_avif_must_start_with_a_heif_brand() {
        let ftyp = |brands: &[u8]| {
            let mut b = ((brands.len() + 12) as u32).to_be_bytes().to_vec();
            b.extend_from_slice(b"ftyp");
            b.extend_from_slice(&brands[..4]);
            b.extend_from_slice(&[0, 0, 0, 0]);
            b.extend_from_slice(&brands[4..]);
            b
        };
        for ext in ["heic", "avif"] {
            assert!(signature_matches(&ftyp(b"heicmif1heic"), ext));
            assert!(signature_matches(&ftyp(b"mif1heic"), ext));
            assert!(signature_matches(&ftyp(b"avifmif1miaf"), ext));
            // A compatible brand counts too.
            assert!(signature_matches(&ftyp(b"miafavif"), ext));
            // An MP4 or a PNG named .heic doesn't.
            assert!(!signature_matches(&ftyp(b"isomiso2mp41"), ext));
            assert!(!signature_matches(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR", ext));
            assert!(!signature_matches(b"\0\0\0\x0cftyp", ext));
        }
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
        encode(&DynamicImage::new_rgba8(64, 48), &path, "ico", 90, None).unwrap();
        let bytes = fs::read(&path).unwrap();
        // The directory lists 64, 48, 32, 24 and 16, each square.
        assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), 5);
        let sizes: Vec<(u8, u8)> = (0..5)
            .map(|i| (bytes[6 + 16 * i], bytes[7 + 16 * i]))
            .collect();
        assert_eq!(sizes, [(64, 64), (48, 48), (32, 32), (24, 24), (16, 16)]);
        let big = dir.path().join("b.ico");
        encode(&DynamicImage::new_rgb8(1000, 500), &big, "ico", 90, None).unwrap();
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

    /// An ICC header, 128 bytes, naming only the colour space of its data.
    fn profile(space: &[u8; 4]) -> Vec<u8> {
        let mut bytes = vec![0u8; 128];
        bytes[16..20].copy_from_slice(space);
        bytes
    }

    #[test]
    fn a_profile_must_match_the_colour_of_the_picture() {
        let rgb = DynamicImage::new_rgb8(2, 2);
        let rgba = DynamicImage::new_rgba8(2, 2);
        let gray = DynamicImage::new_luma8(2, 2);
        let gray_alpha = DynamicImage::new_luma_a8(2, 2);
        assert!(icc_fits(&profile(b"RGB "), &rgb));
        assert!(icc_fits(&profile(b"RGB "), &rgba));
        assert!(icc_fits(&profile(b"GRAY"), &gray));
        assert!(icc_fits(&profile(b"GRAY"), &gray_alpha));
        assert!(!icc_fits(&profile(b"GRAY"), &rgb));
        assert!(!icc_fits(&profile(b"RGB "), &gray));
        assert!(!icc_fits(&profile(b"CMYK"), &rgb));
        assert!(!icc_fits(&profile(b"Lab "), &rgb));
        // Too short to name a colour space.
        assert!(!icc_fits(b"RGB ", &rgb));
        assert!(!icc_fits(&[], &rgb));
        // Real profiles are far smaller than the cap; a bigger one is left
        // out, since a JPEG can't hold one past about 16.7 MB.
        let mut big = profile(b"RGB ");
        big.resize(MAX_ICC_BYTES, 0);
        assert!(icc_fits(&big, &rgb));
        big.push(0);
        assert!(!icc_fits(&big, &rgb));
    }

    #[test]
    fn a_gif_frame_under_20_ms_is_shown_for_100() {
        let ms = |n| gif_frame_ms(image::Delay::from_numer_denom_ms(n, 1));
        assert_eq!([ms(0), ms(10), ms(19)], [100, 100, 100]);
        assert_eq!([ms(20), ms(40), ms(655_350)], [20, 40, 655_350]);
    }

    #[test]
    fn a_gif_loop_count_becomes_a_webp_one() {
        use gif::Repeat;
        // An extension asking for ever.
        assert_eq!(webp_loops(Repeat::Infinite), 0);
        // No extension: a GIF plays once.
        assert_eq!(webp_loops(Repeat::Finite(0)), 1);
        assert_eq!(webp_loops(Repeat::Finite(1)), 2);
        assert_eq!(webp_loops(Repeat::Finite(65_534)), 65_535);
        // A GIF can repeat 65,535 times; a WebP can't count past that.
        assert_eq!(webp_loops(Repeat::Finite(65_535)), 65_535);
    }

    #[test]
    fn webp_is_colour_so_its_profile_is_judged_as_for_rgb() {
        let dir = tempfile::tempdir().unwrap();
        let gray = DynamicImage::new_luma8(8, 8);
        let written = |name: &str, space: &[u8; 4]| {
            let path = dir.path().join(name);
            encode(&gray, &path, "webp", 90, Some(&profile(space))).unwrap();
            let bytes = fs::read(path).unwrap();
            bytes.windows(4).any(|w| w == b"ICCP")
        };
        // A gray profile doesn't suit the RGB a gray picture is written as.
        assert!(!written("gray.webp", b"GRAY"));
        assert!(written("rgb.webp", b"RGB "));
    }

    /// A minimal EXIF block holding orientation 6, big-endian.
    const SIX: &[u8] = b"MM\0*\0\0\0\x08\0\x01\x01\x12\0\x03\0\0\0\x01\0\x06\0\0\0\0\0\0";

    fn sideways_jpeg(dir: &tempfile::TempDir) -> std::path::PathBuf {
        let path = dir.path().join("sideways.jpg");
        let mut bytes = Vec::new();
        let mut encoder = JpegEncoder::new_with_quality(&mut bytes, 90);
        encoder.set_exif_metadata(SIX.to_vec()).unwrap();
        encoder
            .write_image(&[200u8; 8 * 4 * 3], 8, 4, ExtendedColorType::Rgb8)
            .unwrap();
        fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn a_decoded_picture_is_upright_and_says_what_was_done() {
        let dir = tempfile::tempdir().unwrap();
        let path = sideways_jpeg(&dir);
        let decoded = decode_full(&path, "jpg", &CancelToken::new()).unwrap();
        assert_eq!((decoded.image.width(), decoded.image.height()), (4, 8));
        assert_eq!(decoded.orientation, image::metadata::Orientation::Rotate90);
        assert_eq!(decoded.icc, None);
        // `decode` gives the same upright picture.
        let again = decode(&path, "jpg").unwrap();
        assert_eq!((again.width(), again.height()), (4, 8));
    }

    #[test]
    fn the_upright_size_is_read_from_the_header() {
        let dir = tempfile::tempdir().unwrap();
        let path = sideways_jpeg(&dir);
        assert_eq!(upright_size(&path, "jpg"), Some((4, 8)));
        // An upright picture keeps its size, and what isn't a picture of
        // that type, or isn't there, has none.
        let upright = dir.path().join("upright.png");
        DynamicImage::new_rgb8(8, 4).save(&upright).unwrap();
        assert_eq!(upright_size(&upright, "png"), Some((8, 4)));
        assert_eq!(upright_size(&upright, "jpg"), None);
        assert_eq!(upright_size(&dir.path().join("none.png"), "png"), None);
        assert_eq!(upright_size(&upright, "dng"), None);
    }

    /// A PNG whose header claims `w` x `h` pixels of gray and that has no
    /// picture data worth the name.
    fn png_claiming(w: u32, h: u32) -> Vec<u8> {
        fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
            let mut crc = flate2::Crc::new();
            crc.update(kind);
            crc.update(data);
            let mut out = (data.len() as u32).to_be_bytes().to_vec();
            out.extend(kind);
            out.extend(data);
            out.extend(crc.sum().to_be_bytes());
            out
        }
        let mut header = w.to_be_bytes().to_vec();
        header.extend(h.to_be_bytes());
        header.extend([8, 0, 0, 0, 0]); // 8-bit gray, no interlace
        let mut png = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n'];
        png.extend(chunk(b"IHDR", &header));
        png.extend(chunk(b"IDAT", &[0]));
        png.extend(chunk(b"IEND", &[]));
        png
    }

    #[test]
    fn a_picture_needing_more_than_the_memory_limit_is_refused_before_decoding() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vast.png");
        // 900 MB of gray, past the 512 MiB the image crate allows.
        fs::write(&path, png_claiming(30_000, 30_000)).unwrap();
        let err = decode_full(&path, "png", &CancelToken::new())
            .err()
            .unwrap();
        assert!(matches!(&err, Error::Decode { reason, .. } if reason == "Memory limit exceeded"));
        assert_eq!(upright_size(&path, "png"), Some((30_000, 30_000)));
    }

    #[test]
    fn deep_images_become_eight_bit() {
        let deep = DynamicImage::new_rgb16(2, 2);
        assert_eq!(eight_bit(&deep, "bmp").color(), image::ColorType::Rgb8);
        assert_eq!(eight_bit(&deep, "tiff").color(), image::ColorType::Rgb16);
        let gray = DynamicImage::new_luma8(2, 2);
        assert_eq!(eight_bit(&gray, "bmp").color(), image::ColorType::L8);
        assert_eq!(eight_bit(&gray, "gif").color(), image::ColorType::Rgba8);
        let ga = DynamicImage::new_luma_a8(2, 2);
        assert_eq!(eight_bit(&ga, "tiff").color(), image::ColorType::Rgba8);
    }

    /// Three squares side by side, 10 pixels each.
    const THREE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="30" height="10"><rect width="10" height="10" fill="red"/><rect x="10" width="10" height="10" fill="green"/><rect x="20" width="10" height="10" fill="blue"/></svg>"##;

    fn svg_file(dir: &tempfile::TempDir, svg: &str) -> std::path::PathBuf {
        let path = dir.path().join("test.svg");
        fs::write(&path, svg).unwrap();
        path
    }

    #[test]
    fn a_drawing_past_its_time_budget_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = svg_file(&dir, THREE);
        let tree = svg_tree(&path).unwrap();
        let cancel = CancelToken::new();

        let err = render_svg_within(&path, &tree, None, &cancel, Duration::ZERO).unwrap_err();
        assert!(matches!(err, Error::Unsafe(_)), "{err:?}");
        assert_eq!(err.to_string(), "This SVG took too long to draw.");

        // With time to spare the same drawing is made.
        let img = render_svg_within(&path, &tree, None, &cancel, Duration::from_secs(60)).unwrap();
        assert_eq!((img.width(), img.height()), (30, 10));
    }

    #[test]
    fn a_cancelled_drawing_stops_between_elements() {
        let dir = tempfile::tempdir().unwrap();
        let path = svg_file(&dir, THREE);
        let tree = svg_tree(&path).unwrap();
        let cancel = CancelToken::new();
        cancel.cancel();
        let err =
            render_svg_within(&path, &tree, None, &cancel, svg_guard::SVG_TIME_BUDGET).unwrap_err();
        assert!(matches!(err, Error::Cancelled), "{err:?}");
    }

    #[test]
    fn a_tool_s_svg_drawing_stops_when_its_job_is_cancelled() {
        // Crop and Merge draw an SVG through `decode_full`, with their
        // job's token.
        let dir = tempfile::tempdir().unwrap();
        let path = svg_file(&dir, THREE);
        let cancel = CancelToken::new();
        cancel.cancel();
        let err = decode_full(&path, "svg", &cancel).err().unwrap();
        assert!(matches!(err, Error::Cancelled), "{err:?}");
        // Not cancelled, the same drawing is made.
        let drawn = decode_full(&path, "svg", &CancelToken::new()).unwrap();
        assert_eq!((drawn.image.width(), drawn.image.height()), (30, 10));
    }

    /// An SVG with each kind of thing that sits at the top: a gradient, a
    /// group with opacity, a clip, a mask, a blur, a pattern, a transform and
    /// text.
    const BUSY: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="90" viewBox="0 0 60 45">
  <defs>
    <linearGradient id="g" x1="0" x2="1"><stop offset="0" stop-color="#003"/><stop offset="1" stop-color="#f08"/></linearGradient>
    <clipPath id="c"><circle cx="30" cy="22" r="14"/></clipPath>
    <mask id="m"><rect width="60" height="45" fill="white"/><circle cx="30" cy="22" r="6" fill="black"/></mask>
    <filter id="f"><feGaussianBlur stdDeviation="1.5"/></filter>
    <pattern id="p" width="6" height="6" patternUnits="userSpaceOnUse"><circle cx="3" cy="3" r="2" fill="#0a0"/></pattern>
  </defs>
  <rect width="60" height="45" fill="url(#g)"/>
  <g opacity="0.6"><rect x="5.5" y="4.25" width="30" height="20" fill="#fc0"/></g>
  <g clip-path="url(#c)"><rect width="60" height="45" fill="#0cf"/></g>
  <rect x="2" y="30" width="26" height="12" fill="url(#p)" mask="url(#m)"/>
  <g filter="url(#f)"><rect x="34" y="28" width="18" height="10" fill="#f00"/></g>
  <g transform="rotate(12 30 22) translate(3.3 1.7)"><path d="M10 10 L50 12 L30 40 Z" fill="none" stroke="#000" stroke-width="1.3"/></g>
  <text x="4" y="12" font-size="8" fill="#fff">Kiln</text>
</svg>"##;

    /// A straight line with a stroke has no area for `render_node` to go by.
    const LINE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="90"><rect width="120" height="90" fill="#eee"/><line x1="10" y1="40.5" x2="110" y2="40.5" stroke="#c00" stroke-width="3"/><path d="M20 70 H100" stroke="#00c" stroke-width="2"/></svg>"##;

    /// The pixels of `svg` drawn all at once by resvg, and drawn in turn, at
    /// `scale`, and whether every element at the top can be drawn alone.
    fn both_ways(svg: &str, scale: f32) -> (Vec<u8>, Vec<u8>, bool) {
        // With the fonts installed in Windows, as svg_tree has them, so the
        // text is drawn too.
        let mut opt = resvg::usvg::Options::default();
        opt.fontdb_mut().load_system_fonts();
        let tree = resvg::usvg::Tree::from_str(svg, &opt).unwrap();
        let size = tree.size();
        let (w, h) = (
            (size.width() * scale).ceil() as u32,
            (size.height() * scale).ceil() as u32,
        );
        let transform = resvg::tiny_skia::Transform::from_scale(scale, scale);
        let mut whole = resvg::tiny_skia::Pixmap::new(w, h).unwrap();
        resvg::render(&tree, transform, &mut whole.as_mut());
        let mut turns = resvg::tiny_skia::Pixmap::new(w, h).unwrap();
        draw_in_turn(
            &tree,
            transform,
            &mut turns.as_mut(),
            &CancelToken::new(),
            Duration::from_secs(60),
        )
        .unwrap();
        let alone = tree.root().children().iter().all(draws_alone);
        (whole.take(), turns.take(), alone)
    }

    #[test]
    fn drawing_element_by_element_gives_the_same_pixels() {
        let sample = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/sample.svg"),
        )
        .unwrap();
        for (name, svg) in [
            ("sample", sample.as_str()),
            ("busy", BUSY),
            ("three", THREE),
        ] {
            for scale in [1.0, 2.37, 0.4] {
                let (whole, turns, alone) = both_ways(svg, scale);
                assert!(alone, "{name} is drawn element by element");
                assert!(whole == turns, "{name} at {scale}: the pixels differ");
                assert!(whole.iter().any(|&b| b != 0), "{name} is blank");
            }
        }
    }

    #[test]
    fn a_stroked_line_keeps_the_whole_drawing_in_one_piece() {
        // The line has no area of its own, so render_node would leave it out
        // and the drawing is made whole.
        let tree = resvg::usvg::Tree::from_str(LINE, &resvg::usvg::Options::default()).unwrap();
        assert!(tree.root().children()[1].abs_layer_bounding_box().is_none());
        for scale in [1.0, 1.7] {
            let (whole, turns, alone) = both_ways(LINE, scale);
            assert!(!alone);
            assert!(whole == turns, "at {scale}: the pixels differ");
        }
        // Inside a group the line has the group's area, and the group is
        // drawn by itself.
        let grouped = LINE
            .replace("<line", "<g><line")
            .replace("/><path", "/></g><path");
        let tree = resvg::usvg::Tree::from_str(&grouped, &resvg::usvg::Options::default()).unwrap();
        let alone: Vec<bool> = tree.root().children().iter().map(draws_alone).collect();
        assert_eq!(alone, [true, true, false]);
    }

    #[test]
    fn a_zero_area_fill_is_left_out_not_drawn() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><path d="M0 5 H20" fill="red"/><rect width="10" height="10" fill="blue"/></svg>"##;
        let (whole, turns, alone) = both_ways(svg, 1.0);
        assert!(alone);
        assert!(whole == turns);
    }

    #[test]
    fn an_svg_over_the_pixel_limit_is_drawn_smaller() {
        let dir = tempfile::tempdir().unwrap();
        let path = svg_file(
            &dir,
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="20000" height="10000"><rect width="100%" height="100%" fill="red"/></svg>"##,
        );
        let tree = svg_tree(&path).unwrap();
        let cancel = CancelToken::new();
        // Asked for 20,000 pixels on the long side: the 50 megapixel limit
        // holds, and the shape does too.
        let img = render_svg(&path, &tree, Some(20_000), &cancel).unwrap();
        assert!(img.width() as u64 * img.height() as u64 <= 50_000_000);
        assert_eq!(img.width() / img.height(), 2);
    }

    #[test]
    fn an_svgz_that_unpacks_too_large_is_refused() {
        use std::io::Write as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.svgz");
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1\" height=\"1\"><!--")
            .unwrap();
        let chunk = vec![b'x'; 1 << 20];
        for _ in 0..65 {
            gz.write_all(&chunk).unwrap();
        }
        gz.write_all(b"--></svg>").unwrap();
        fs::write(&path, gz.finish().unwrap()).unwrap();
        let err = svg_tree(&path).unwrap_err();
        assert!(matches!(err, Error::Decode { .. }), "{err:?}");
        assert!(err.to_string().contains("more than 64 MB"), "{err}");
    }

    #[test]
    fn an_ordinary_svgz_is_read() {
        use std::io::Write as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("small.svgz");
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(THREE.as_bytes()).unwrap();
        fs::write(&path, gz.finish().unwrap()).unwrap();
        let tree = svg_tree(&path).unwrap();
        assert_eq!((tree.size().width(), tree.size().height()), (30.0, 10.0));
    }

    #[cfg(windows)]
    #[test]
    fn a_windows_read_reserves_the_copy_without_alpha_too() {
        // 16 bits a channel, opaque: the pixels and the RGB copy.
        assert_eq!(
            wic::pixel_bytes(10, 4, 8, false),
            Some((80, 320, 320 + 240))
        );
        // With alpha the pixels are kept as they are.
        assert_eq!(wic::pixel_bytes(10, 4, 4, true), Some((40, 160, 160)));
        assert_eq!(wic::pixel_bytes(u32::MAX, 1, 8, false), None);
    }

    #[cfg(windows)]
    #[test]
    fn a_windows_read_leaves_com_as_it_found_it() {
        use windows::Win32::System::Com::{
            CoGetApartmentType, APTTYPE, APTTYPEQUALIFIER, APTTYPEQUALIFIER_IMPLICIT_MTA,
        };
        let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/sample.heic");
        // A thread of its own, which starts with no COM.
        std::thread::spawn(move || {
            let _ = wic::decode_any(&sample);
            let _ = wic::size(&sample);
            let (mut kind, mut qualifier) = (APTTYPE::default(), APTTYPEQUALIFIER::default());
            // SAFETY: asks about this thread's COM, writing two locals.
            let started = unsafe { CoGetApartmentType(&mut kind, &mut qualifier) };
            // Not started, or only in the process's shared apartment, which
            // a thread is in while another thread keeps COM running.
            assert!(
                started.is_err() || qualifier == APTTYPEQUALIFIER_IMPLICIT_MTA,
                "COM is still started: {kind:?} {qualifier:?}"
            );
        })
        .join()
        .unwrap();
    }

    #[test]
    fn what_windows_lacks_is_told_from_its_code() {
        assert_eq!(lacks(0x8898_2F50), Lacks::Decoder);
        assert_eq!(lacks(0x8898_2F07), Lacks::Decoder);
        // The decoder is there, its codec isn't.
        assert_eq!(lacks(0xC00D_5212), Lacks::Codec);
        // A damaged file: MF_E_INVALID_FILE_FORMAT.
        assert_eq!(lacks(0xC00D_36C4), Lacks::Nothing);
        assert_eq!(lacks(0x8007_0057), Lacks::Nothing);
        assert_eq!(lacks(0x8898_2F81), Lacks::Nothing);
    }

    #[test]
    fn a_heic_names_both_extensions_and_links_the_missing_one() {
        let both = "Needs Microsoft's HEIF Image and HEVC Video Extensions, or FFmpeg.";
        for (missing, want) in [
            (Lacks::Decoder, "9PMMSR1CGPWG"),
            (Lacks::Codec, "9N4WGH0Z6VHQ"),
        ] {
            match needs_decoder("heic", missing) {
                Error::NeedsExtension { message, product } => {
                    assert_eq!(message, both);
                    assert_eq!(product, want, "{missing:?}");
                }
                e => panic!("{e:?}"),
            }
        }
        for missing in [Lacks::Decoder, Lacks::Codec] {
            match needs_decoder("avif", missing) {
                Error::NeedsExtension { message, product } => {
                    assert_eq!(message, "Needs Microsoft's AV1 Video Extension, or FFmpeg.");
                    assert_eq!(product, "9MVZQVXJBQ9V");
                }
                e => panic!("{e:?}"),
            }
        }
    }
}
