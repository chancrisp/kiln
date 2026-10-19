//! What Kiln can tell about a file by looking inside it: for images, the
//! size, transparency, animation and whether it looks like a photo; for
//! PDFs, the pages and whether they hold text; for audio and video, what
//! FFprobe reports and whether there are black bars; and the likely size
//! of a converted result. `inspect` reads and nothing else; `suggest`
//! turns its facts into suggestions.

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File};
use std::io::BufReader;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{AnimationDecoder, DynamicImage, ExtendedColorType, ImageEncoder, RgbaImage};

use crate::converter::{CancelToken, Context, Options};
use crate::job::Kiln;
use crate::quality::{self, MediaInfo, Preset, Quality};
use crate::raster;
use crate::registry::Kind;

/// Pictures with more pixels than this keep only their header facts.
const MAX_PIXELS: u64 = 50_000_000;
/// The long side of the preview that look is judged on, and the largest
/// side of the full-size crop that estimates are encoded from.
const PREVIEW: u32 = 256;
// The next two are first guesses, checked against the fixtures. Tune
// them with `real_photos_and_screenshots_are_told_apart` (tests/facts.rs)
// when real photos and screenshots are in KILN_EXTRA_SAMPLES.
/// A preview with at most this many distinct colours is a graphic.
const GRAPHIC_COLOURS: usize = 64;
/// A preview where at least this share of pixels equals its right-hand
/// neighbour is a graphic.
const GRAPHIC_FLAT: f32 = 0.5;

/// How long one FFmpeg or FFprobe run, or one PDF read, may take.
const LIMIT: Duration = Duration::from_secs(2);
/// How often a wait looks at the caller's cancel token and the clock.
const POLL: Duration = Duration::from_millis(25);
/// Seconds of video the black-bar detector reads.
const BAR_SCAN_SECONDS: f64 = 3.0;
/// PDFs larger than this skip the text check.
const MAX_PDF_BYTES: u64 = 200_000_000;
/// The pages of a PDF looked at for text.
const TEXT_PAGES: usize = 3;

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
        match format.kind {
            Kind::Image => image_facts(path, opts, cancel, &mut found),
            Kind::Audio | Kind::Video => media_facts(kiln, path, opts, cancel, &mut found),
            Kind::Document if format.ext == "pdf" => pdf_facts(path, cancel, &mut found),
            _ => {}
        }
        found
    }));
    Some(read.unwrap_or(facts))
}

/// Picture facts by image family, and estimates when there is a preview.
fn image_facts(path: &Path, opts: &Options, cancel: &CancelToken, facts: &mut Facts) {
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
        // A vector has no full-size picture to crop, so the preview stands in.
        let cap = opts.quality.size.resolve(None);
        let result = crate::quality::scaled(w, h, cap, false);
        facts.estimates = estimates(&preview, result, opts);
    } else if raster::RAW.contains(&ext) {
        facts.picture = raster::wic::size(path).ok().map(photo);
    } else if matches!(ext, "heic" | "avif" | "jxl") {
        facts.picture = probe(path, opts, cancel)
            .and_then(|m| Some((m.width?, m.height?)))
            .map(photo);
    } else if raster::reads(ext) || ext == "apng" {
        raster_facts(path, ext, opts, cancel, facts);
    }
}

/// What FFprobe reports for audio or video, the frame and black bars of
/// video, and likely sizes as MP4 or MP3 and after Compress.
fn media_facts(kiln: &Kiln, path: &Path, opts: &Options, cancel: &CancelToken, facts: &mut Facts) {
    let Some(info) = probe(path, opts, cancel) else {
        return;
    };
    let video = facts.kind == Kind::Video;
    if video {
        if let (Some(width), Some(height)) = (info.width, info.height) {
            facts.picture = Some(Picture {
                width,
                height,
                alpha: false,
                animated: false,
                look: Look::Photo,
            });
            facts.bars = bars(path, (width, height), opts, cancel);
        }
    }
    let registry = kiln.registry();
    let target = if video { "mp4" } else { "mp3" };
    if let Some(size) = registry
        .format(target)
        .and_then(|to| quality::estimate(to, &opts.quality, &info))
    {
        facts.estimates.insert(target.to_owned(), size);
    }
    let small = Quality {
        preset: Preset::Small,
        ..opts.quality
    };
    facts.compressed = registry
        .format(&facts.ext)
        .and_then(|own| quality::estimate(own, &small, &info));
    facts.media = Some(info);
}

/// FFprobe's report, as `Kiln::probe` gives it, within `LIMIT`. `None`
/// when FFprobe is missing, can't read the file or is cut off.
fn probe(path: &Path, opts: &Options, cancel: &CancelToken) -> Option<MediaInfo> {
    if cancel.is_cancelled() {
        return None;
    }
    let deadline = Deadline::new(cancel);
    let mut ctx = Context {
        progress: &mut |_| {},
        cancel: &deadline.token,
    };
    let info = crate::ffmpeg::probe_media(opts, path, &mut ctx).ok()?;
    (info.duration.is_some() || info.width.is_some()).then_some(info)
}

/// Whether FFmpeg's black-bar detector finds bars in the first
/// `BAR_SCAN_SECONDS` of a `frame`-sized video. `None` when the run fails
/// or is cut off.
fn bars(path: &Path, frame: (u32, u32), opts: &Options, cancel: &CancelToken) -> Option<bool> {
    if cancel.is_cancelled() {
        return None;
    }
    let deadline = Deadline::new(cancel);
    let mut ctx = Context {
        progress: &mut |_| {},
        cancel: &deadline.token,
    };
    let log = crate::ffmpeg::analyse(
        opts,
        &mut ctx,
        path,
        "-vf",
        "cropdetect=limit=24:round=2:reset=0",
        Some(BAR_SCAN_SECONDS),
    )
    .ok()?;
    Some(
        crate::tools::last_crop(&log).is_some_and(|(w, h, _, _)| (w, h) != frame && w > 0 && h > 0),
    )
}

/// A PDF's page count and whether its first pages hold text. Each read
/// is cut off after `LIMIT`: a PDF that can't be loaded in time gets no
/// PDF facts, and one whose text can't be checked gets `has_text: None`.
fn pdf_facts(path: &Path, cancel: &CancelToken, facts: &mut Facts) {
    let owned = path.to_path_buf();
    let loaded = within(cancel, move || {
        let bytes = fs::read(&owned).ok()?;
        let doc = lopdf::Document::load_mem(&bytes).ok()?;
        let pages = u32::try_from(doc.get_pages().len()).unwrap_or(u32::MAX);
        Some((Arc::new(bytes), pages, doc.is_encrypted()))
    });
    let Some((bytes, pages, encrypted)) = loaded.flatten() else {
        return;
    };
    let has_text = if encrypted || facts.bytes > MAX_PDF_BYTES {
        None
    } else {
        within(cancel, move || {
            crate::pdf::text_in_first(&bytes, TEXT_PAGES).ok()
        })
        .flatten()
    };
    facts.pdf = Some(PdfFacts { pages, has_text });
}

/// Run `work` on its own thread and wait for it until `LIMIT` has passed
/// or `cancel` is set. `None` when it ran out of time, was cancelled or
/// panicked. A thread that is cut off is left to finish on its own.
fn within<T: Send + 'static>(
    cancel: &CancelToken,
    work: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    if cancel.is_cancelled() {
        return None;
    }
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("kiln-inspect".to_owned())
        .spawn(move || {
            let _ = tx.send(work());
        })
        .ok()?;
    let start = Instant::now();
    loop {
        match rx.recv_timeout(POLL) {
            Ok(found) => return Some(found),
            Err(RecvTimeoutError::Disconnected) => return None,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if cancel.is_cancelled() || start.elapsed() >= LIMIT {
            return None;
        }
    }
}

/// A cancel token of its own for one FFmpeg or FFprobe run, set when the
/// caller's token is set or `LIMIT` has passed. A watcher thread checks
/// every `POLL`; dropping the `Deadline` stops it.
struct Deadline {
    token: CancelToken,
    stop: Option<Sender<()>>,
    watcher: Option<JoinHandle<()>>,
}

impl Deadline {
    fn new(cancel: &CancelToken) -> Self {
        let token = CancelToken::new();
        let (stop, stopped) = mpsc::channel::<()>();
        let (mine, theirs) = (token.clone(), cancel.clone());
        let start = Instant::now();
        let watcher = thread::Builder::new()
            .name("kiln-deadline".to_owned())
            .spawn(move || loop {
                if theirs.is_cancelled() || start.elapsed() >= LIMIT {
                    mine.cancel();
                    return;
                }
                if !matches!(stopped.recv_timeout(POLL), Err(RecvTimeoutError::Timeout)) {
                    return;
                }
            });
        // Without a watcher nothing would enforce the limit, so nothing runs.
        if watcher.is_err() {
            token.cancel();
        }
        Self {
            token,
            stop: Some(stop),
            watcher: watcher.ok(),
        }
    }
}

impl Drop for Deadline {
    fn drop(&mut self) {
        // Closing the channel wakes the watcher at once.
        self.stop.take();
        if let Some(watcher) = self.watcher.take() {
            let _ = watcher.join();
        }
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
    let look = if w.max(h) > PREVIEW {
        look_of(&img.thumbnail(PREVIEW, PREVIEW).to_rgba8())
    } else {
        look_of(&img.to_rgba8())
    };
    facts.picture = Some(Picture {
        width: w,
        height: h,
        alpha,
        animated: animated(path, ext),
        look,
    });
    // The picture as the image backend would write it, size limit and all.
    let result = raster::fit(img, opts.quality.size.resolve(None));
    let (rw, rh) = (result.width(), result.height());
    let (cw, ch) = (rw.min(PREVIEW), rh.min(PREVIEW));
    let crop = result.crop_imm((rw - cw) / 2, (rh - ch) / 2, cw, ch);
    facts.estimates = estimates(&crop, (rw, rh), opts);
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

/// Likely JPG and PNG sizes of a `result`-sized picture, from a sample of
/// it: the sample is encoded as each, flattened onto white, and the byte
/// count scaled by the ratio of the result's pixels to the sample's.
fn estimates(sample: &DynamicImage, result: (u32, u32), opts: &Options) -> BTreeMap<String, u64> {
    let (pw, ph) = (sample.width(), sample.height());
    let rgb = raster::flatten_rgb(sample);
    let factor =
        (f64::from(result.0) * f64::from(result.1)) / (f64::from(pw) * f64::from(ph)).max(1.0);
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

    #[test]
    fn slow_work_is_cut_off_at_the_limit() {
        let t = Instant::now();
        let got = within(&CancelToken::new(), || thread::sleep(LIMIT * 3));
        assert!(got.is_none());
        assert!(t.elapsed() >= LIMIT && t.elapsed() < LIMIT * 2);
    }

    #[test]
    fn a_deadline_follows_the_caller_and_the_clock() {
        let caller = CancelToken::new();
        let d = Deadline::new(&caller);
        assert!(!d.token.is_cancelled());
        caller.cancel();
        thread::sleep(POLL * 4);
        assert!(d.token.is_cancelled());

        let t = Instant::now();
        let d = Deadline::new(&CancelToken::new());
        while !d.token.is_cancelled() && t.elapsed() < LIMIT * 3 {
            thread::sleep(POLL);
        }
        assert!(d.token.is_cancelled());
        assert!(t.elapsed() >= LIMIT && t.elapsed() < LIMIT * 2);
    }
}
