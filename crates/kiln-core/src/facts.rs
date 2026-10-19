//! What Kiln can tell about a file by looking inside it: for images, the
//! size, transparency, animation and whether it looks like a photo, plus
//! the likely size of a converted result. `inspect` reads and nothing
//! else; `suggest` turns its facts into suggestions.

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File};
use std::io::BufReader;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{AnimationDecoder, DynamicImage, ExtendedColorType, ImageEncoder, RgbaImage};

use crate::converter::{CancelToken, Options};
use crate::job::Kiln;
use crate::quality::MediaInfo;
use crate::raster;
use crate::registry::Kind;

/// Pictures with more pixels than this keep only their header facts.
const MAX_PIXELS: u64 = 50_000_000;
/// The long side of the preview that look and estimates are taken from.
const PREVIEW: u32 = 256;
// The next three are first guesses, checked against the fixtures. Tune
// them with `real_photos_and_screenshots_are_told_apart` (tests/facts.rs)
// when real photos and screenshots are in KILN_EXTRA_SAMPLES.
/// A preview with at most this many distinct colours is a graphic.
const GRAPHIC_COLOURS: usize = 64;
/// A preview where at least this share of pixels equals its right-hand
/// neighbour is a graphic.
const GRAPHIC_FLAT: f32 = 0.5;
/// How an encoded preview's size grows with the pixel count of the result.
const ESTIMATE_EXP: f64 = 1.0;

/// What `inspect` found in one file.
#[derive(Debug, Clone, PartialEq)]
pub struct Facts {
    /// Canonical extension, lowercase.
    pub ext: String,
    pub kind: Kind,
    pub bytes: u64,
    /// Images, and the frame size of video.
    pub picture: Option<Picture>,
    /// Audio and video: what ffprobe reports.
    pub media: Option<MediaInfo>,
    pub pdf: Option<PdfFacts>,
    /// Video only: whether black bars were found.
    pub bars: Option<bool>,
    /// Likely size of the result, by target extension, for targets a rule may pick.
    pub estimates: BTreeMap<String, u64>,
    /// Likely size after Compress.
    pub compressed: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    /// Any pixel is not fully opaque.
    pub alpha: bool,
    /// A GIF or APNG with more than one frame.
    pub animated: bool,
    pub look: Look,
}

/// Whether a picture reads as a photo or as flat artwork.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    Photo,
    Graphic,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PdfFacts {
    pub pages: u32,
    /// `None` when the PDF is encrypted or can't be read.
    pub has_text: Option<bool>,
}

/// Look inside `path`. `None` only when its extension isn't a type Kiln
/// knows or the file can't be stat'ed; a file that can't be read gives
/// type and size only. Never fails or panics.
pub fn inspect(kiln: &Kiln, path: &Path, opts: &Options, cancel: &CancelToken) -> Option<Facts> {
    let format = kiln.registry().format_of(path)?;
    let bytes = fs::metadata(path).ok()?.len();
    let facts = Facts {
        ext: format.ext.clone(),
        kind: format.kind,
        bytes,
        picture: None,
        media: None,
        pdf: None,
        bars: None,
        estimates: BTreeMap::new(),
        compressed: None,
    };
    if cancel.is_cancelled() {
        return Some(facts);
    }
    // Decoders run on untrusted files; a panic in one costs only facts.
    let read = panic::catch_unwind(AssertUnwindSafe(|| {
        let mut found = facts.clone();
        if let Kind::Image = format.kind {
            image_facts(kiln, path, opts, cancel, &mut found);
        }
        found
    }));
    Some(read.unwrap_or(facts))
}

/// Picture facts by image family, and estimates when there is a preview.
fn image_facts(kiln: &Kiln, path: &Path, opts: &Options, cancel: &CancelToken, facts: &mut Facts) {
    let ext = facts.ext.clone();
    let ext = ext.as_str();
    let photo = |(width, height): (u32, u32)| Picture {
        width,
        height,
        alpha: false,
        animated: false,
        look: Look::Photo,
    };
    if ext == "svg" {
        let Ok(((w, h), preview)) = raster::svg_preview(path, PREVIEW) else {
            return;
        };
        facts.picture = Some(Picture {
            width: w,
            height: h,
            alpha: any_transparent(&preview),
            animated: false,
            look: Look::Graphic,
        });
        facts.estimates = estimates(&preview, (w, h), opts);
    } else if raster::RAW.contains(&ext) {
        facts.picture = raster::wic::size(path).ok().map(photo);
    } else if matches!(ext, "heic" | "avif" | "jxl") {
        facts.picture = kiln
            .probe(path, opts)
            .and_then(|m| Some((m.width?, m.height?)))
            .map(photo);
    } else if raster::reads(ext) || ext == "apng" {
        raster_facts(path, ext, opts, cancel, facts);
    }
}

/// A raster type the image crate reads: decoded unless it is huge, and
/// judged on a preview at most `PREVIEW` pixels on the long side.
fn raster_facts(path: &Path, ext: &str, opts: &Options, cancel: &CancelToken, facts: &mut Facts) {
    let Ok((w, h)) = image::image_dimensions(path) else {
        return;
    };
    if !decodes(w, h) {
        facts.picture = Some(Picture {
            width: w,
            height: h,
            alpha: false,
            animated: false,
            look: Look::Photo,
        });
        return;
    }
    // An APNG is a PNG with extra chunks.
    let Ok(img) = raster::decode(path, if ext == "apng" { "png" } else { ext }) else {
        return;
    };
    if cancel.is_cancelled() {
        return;
    }
    let (w, h) = (img.width(), img.height());
    let alpha = any_transparent(&img);
    let preview = if w.max(h) > PREVIEW {
        img.thumbnail(PREVIEW, PREVIEW)
    } else {
        img
    };
    facts.picture = Some(Picture {
        width: w,
        height: h,
        alpha,
        animated: animated(path, ext),
        look: look_of(&preview.to_rgba8()),
    });
    facts.estimates = estimates(&preview, (w, h), opts);
}

/// Whether a `w` x `h` picture is small enough to decode.
fn decodes(w: u32, h: u32) -> bool {
    u64::from(w) * u64::from(h) <= MAX_PIXELS
}

fn any_transparent(img: &DynamicImage) -> bool {
    use image::GenericImageView;
    img.color().has_alpha() && img.pixels().any(|(_, _, p)| p.0[3] < 255)
}

/// `Graphic` when the picture has few distinct colours, or when many
/// pixels equal their right-hand neighbour; otherwise `Photo`.
fn look_of(img: &RgbaImage) -> Look {
    let mut colours = HashSet::new();
    for p in img.pixels() {
        colours.insert([p.0[0], p.0[1], p.0[2]]);
        if colours.len() > GRAPHIC_COLOURS {
            break;
        }
    }
    if colours.len() <= GRAPHIC_COLOURS {
        return Look::Graphic;
    }
    let (w, h) = img.dimensions();
    let pairs = u64::from(w.saturating_sub(1)) * u64::from(h);
    if pairs == 0 {
        return Look::Photo;
    }
    let flat = img
        .rows()
        .map(|row| {
            let row: Vec<_> = row.collect();
            row.windows(2).filter(|p| p[0] == p[1]).count() as u64
        })
        .sum::<u64>();
    if flat as f32 / pairs as f32 >= GRAPHIC_FLAT {
        Look::Graphic
    } else {
        Look::Photo
    }
}

/// Whether a GIF or PNG has a second frame. Frames are decoded one at a
/// time and counting stops at two.
fn animated(path: &Path, ext: &str) -> bool {
    use image::codecs::{gif::GifDecoder, png::PngDecoder};
    let Ok(file) = File::open(path) else {
        return false;
    };
    let reader = BufReader::new(file);
    let two = |frames: image::Frames<'_>| frames.take(2).filter(Result::is_ok).count() == 2;
    match ext {
        "gif" => GifDecoder::new(reader).is_ok_and(|d| two(d.into_frames())),
        "png" | "apng" => PngDecoder::new(reader)
            .ok()
            .filter(|d| d.is_apng().unwrap_or(false))
            .and_then(|d| d.apng().ok())
            .is_some_and(|d| two(d.into_frames())),
        _ => false,
    }
}

/// Likely JPG and PNG sizes of a `full`-sized picture, from its preview:
/// the preview is encoded as each, flattened onto white, and the byte
/// count scaled by the ratio of the result's pixels to the preview's.
fn estimates(preview: &DynamicImage, full: (u32, u32), opts: &Options) -> BTreeMap<String, u64> {
    let (pw, ph) = (preview.width(), preview.height());
    let rgb = raster::flatten_rgb(preview);
    // The size limit applies as the image backend applies it.
    let cap = opts.quality.size.resolve(None);
    let (rw, rh) = crate::quality::scaled(full.0, full.1, cap, false);
    let factor = ((f64::from(rw) * f64::from(rh)) / (f64::from(pw) * f64::from(ph)).max(1.0))
        .powf(ESTIMATE_EXP);
    let mut out = BTreeMap::new();
    let mut jpg = Vec::new();
    let quality = opts.jpeg_quality.clamp(1, 100);
    if JpegEncoder::new_with_quality(&mut jpg, quality)
        .write_image(&rgb, pw, ph, ExtendedColorType::Rgb8)
        .is_ok()
    {
        out.insert("jpg".to_owned(), (jpg.len() as f64 * factor).round() as u64);
    }
    let mut png = Vec::new();
    if PngEncoder::new_with_quality(&mut png, CompressionType::Best, FilterType::Adaptive)
        .write_image(&rgb, pw, ph, ExtendedColorType::Rgb8)
        .is_ok()
    {
        out.insert("png".to_owned(), (png.len() as f64 * factor).round() as u64);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    /// Every pixel its own, from a fixed-seed LCG.
    fn noise(w: u32, h: u32) -> RgbaImage {
        let mut seed: u32 = 7;
        RgbaImage::from_fn(w, h, |_, _| {
            let mut next = || {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 24) as u8
            };
            Rgba([next(), next(), next(), 255])
        })
    }

    /// Vertical bands of four colours, 8 pixels wide.
    fn stripes(w: u32, h: u32) -> RgbaImage {
        const BANDS: [[u8; 4]; 4] = [
            [230, 60, 40, 255],
            [250, 250, 245, 255],
            [30, 90, 160, 255],
            [20, 20, 20, 255],
        ];
        RgbaImage::from_fn(w, h, |x, _| Rgba(BANDS[(x / 8 % 4) as usize]))
    }

    #[test]
    fn noise_reads_as_a_photo() {
        assert_eq!(look_of(&noise(64, 48)), Look::Photo);
    }

    #[test]
    fn stripes_read_as_a_graphic() {
        assert_eq!(look_of(&stripes(64, 48)), Look::Graphic);
    }

    #[test]
    fn huge_pictures_are_not_decoded() {
        assert!(decodes(7000, 7000));
        assert!(!decodes(7072, 7072));
    }

    #[test]
    fn many_colours_in_flat_runs_read_as_a_graphic() {
        // 256 distinct colours, each in a run of 4: three of four pixels
        // equal their right-hand neighbour.
        let img = RgbaImage::from_fn(64, 64, |x, y| {
            Rgba([(x / 4 * 16) as u8, (y * 4) as u8, 100, 255])
        });
        assert_eq!(look_of(&img), Look::Graphic);
    }
}
