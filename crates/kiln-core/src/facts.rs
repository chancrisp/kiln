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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{AnimationDecoder, DynamicImage, ExtendedColorType, ImageEncoder, RgbaImage};

use crate::converter::{CancelToken, Context, Options};
use crate::ffmpeg::StillStream;
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

/// How long one FFmpeg or FFprobe run, or one step of a PDF read, may
/// take.
const LIMIT: Duration = Duration::from_secs(2);
/// How often a wait looks at the caller's cancel token and the clock.
const POLL: Duration = Duration::from_millis(25);
/// Seconds of video the black-bar detector reads.
const BAR_SCAN_SECONDS: f64 = 3.0;
/// PDFs larger than this aren't read: they get type and size only.
const MAX_PDF_BYTES: u64 = 200_000_000;
/// The pages of a PDF looked at for text.
const TEXT_PAGES: u32 = 3;
/// How long a PDF helper may keep the one helper slot. One still working
/// after this is left to finish on its own, its result thrown away, and
/// the next PDF may start a helper of its own.
const HELPER_TIMEOUT: Duration = Duration::from_secs(20);

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
    /// Video only: which black bars were found. `None` when the check
    /// failed, was cut off or wasn't run.
    pub bars: Option<Bars>,
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

/// Which black bars a video has, by axis. A bar counts only when it takes
/// at least 4% of the frame on its axis, about 2% a side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bars {
    /// No bars worth cropping.
    None,
    /// Bars on the left and right only.
    Sides,
    /// Bars on the top and bottom only.
    AboveBelow,
    /// Bars on both axes.
    Around,
}

impl Bars {
    /// The bars a `crop` (width, height) of a `frame` (width, height)
    /// leaves out. An axis counts when the crop is at least 4% shorter
    /// than the frame on it, compared exactly in integers: `(W - w) * 100
    /// >= W * 4`. A crop with a zero side, or a zero frame, is `None`.
    pub fn of(frame: (u32, u32), crop: (u32, u32)) -> Bars {
        let ((fw, fh), (w, h)) = (frame, crop);
        if fw == 0 || fh == 0 || w == 0 || h == 0 {
            return Bars::None;
        }
        let thick = |whole: u32, kept: u32| {
            u64::from(whole.saturating_sub(kept)) * 100 >= u64::from(whole) * 4
        };
        match (thick(fw, w), thick(fh, h)) {
            (true, true) => Bars::Around,
            (true, false) => Bars::Sides,
            (false, true) => Bars::AboveBelow,
            (false, false) => Bars::None,
        }
    }
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
        let streams = still_streams(path, opts, cancel);
        let first = streams.first();
        facts.picture = first.and_then(|s| Some((s.width?, s.height?))).map(photo);
        if let Some(p) = facts.picture.as_mut().filter(|_| ext != "heic") {
            // FFmpeg gives an AVIF's alpha as a second, gray video stream.
            // Other extra streams, such as the tiles of a grid, are colour.
            p.alpha = first.is_some_and(|s| alpha_format(&s.pix_fmt))
                || (ext == "avif"
                    && streams
                        .iter()
                        .skip(1)
                        .any(|s| s.pix_fmt.starts_with("gray")));
        }
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
    let _low = crate::process::low_priority();
    let info = crate::ffmpeg::probe_media(opts, path, &mut ctx).ok()?;
    (info.duration.is_some() || info.width.is_some()).then_some(info)
}

/// Size and pixel format of every video stream of a still image, from one
/// FFprobe run within `LIMIT`. Empty when FFprobe is missing, can't read
/// the file or is cut off.
fn still_streams(path: &Path, opts: &Options, cancel: &CancelToken) -> Vec<StillStream> {
    if cancel.is_cancelled() {
        return vec![];
    }
    let deadline = Deadline::new(cancel);
    let mut ctx = Context {
        progress: &mut |_| {},
        cancel: &deadline.token,
    };
    let _low = crate::process::low_priority();
    crate::ffmpeg::probe_still(opts, path, &mut ctx).unwrap_or_default()
}

/// Whether an FFmpeg pixel format carries alpha.
fn alpha_format(pix_fmt: &str) -> bool {
    ["yuva", "rgba", "gbrap", "ya"]
        .iter()
        .any(|p| pix_fmt.starts_with(p))
        || matches!(pix_fmt, "bgra" | "argb" | "abgr")
}

/// Which bars FFmpeg's black-bar detector finds in the first
/// `BAR_SCAN_SECONDS` of a `frame`-sized video. `None` when the run fails
/// or is cut off.
fn bars(path: &Path, frame: (u32, u32), opts: &Options, cancel: &CancelToken) -> Option<Bars> {
    if cancel.is_cancelled() {
        return None;
    }
    let deadline = Deadline::new(cancel);
    let mut ctx = Context {
        progress: &mut |_| {},
        cancel: &deadline.token,
    };
    let _low = crate::process::low_priority();
    // One decoding thread: this is background work.
    let log = crate::ffmpeg::analyse_with(
        opts,
        &mut ctx,
        path,
        &["-threads", "1"],
        "-vf",
        "cropdetect=limit=24:round=2:reset=0",
        Some(BAR_SCAN_SECONDS),
    )
    .ok()?;
    Some(crate::tools::last_crop(&log).map_or(Bars::None, |(w, h, _, _)| Bars::of(frame, (w, h))))
}

/// A PDF's page count and whether its first pages hold text. A PDF over
/// `MAX_PDF_BYTES` isn't read at all. One helper thread loads the PDF and
/// then reads its first `TEXT_PAGES` pages; each step is waited on for
/// `LIMIT`. A PDF that can't be loaded in time gets no PDF facts, and one
/// whose text can't be checked in time gets `has_text: None`.
fn pdf_facts(path: &Path, cancel: &CancelToken, facts: &mut Facts) {
    if facts.bytes > MAX_PDF_BYTES || cancel.is_cancelled() {
        return;
    }
    // A helper that was cut off may still be working; while it is, up to
    // `HELPER_TIMEOUT`, the next PDF gets no PDF facts, so cut-off work
    // never piles up.
    let Some(busy) = PDF_HELPER.claim(HELPER_TIMEOUT) else {
        return;
    };
    enum Found {
        Loaded { pages: u32, encrypted: bool },
        Text(Option<bool>),
    }
    let (tx, rx) = mpsc::channel();
    let owned = path.to_path_buf();
    // `busy` is let go before the last word is sent, so the next PDF never
    // finds the slot taken by a helper that has finished. A panic lets it
    // go too.
    let started = helper(move || {
        let loaded = fs::read(&owned)
            .ok()
            .and_then(|bytes| lopdf::Document::load_mem(&bytes).ok());
        let Some(doc) = loaded else {
            drop(busy);
            return;
        };
        let pages = u32::try_from(doc.get_pages().len()).unwrap_or(u32::MAX);
        let encrypted = doc.is_encrypted();
        if encrypted {
            drop(doc);
            drop(busy);
            let _ = tx.send(Found::Loaded { pages, encrypted });
            return;
        }
        // Nobody waiting any more: stop here.
        if tx.send(Found::Loaded { pages, encrypted }).is_err() {
            return;
        }
        let text = crate::pdf::text_in_first(&doc, TEXT_PAGES).ok();
        drop(doc);
        drop(busy);
        let _ = tx.send(Found::Text(text));
    });
    if !started {
        return;
    }
    let Some(Found::Loaded { pages, encrypted }) = wait(&rx, cancel) else {
        return;
    };
    let has_text = if encrypted {
        None
    } else {
        match wait(&rx, cancel) {
            Some(Found::Text(t)) => t,
            _ => None,
        }
    };
    facts.pdf = Some(PdfFacts { pages, has_text });
}

/// The slot PDF helper threads take turns in.
static PDF_HELPER: HelperSlot = HelperSlot::new();

/// Room for one helper at a time: which claim holds it, and since when.
struct HelperSlot {
    held: Mutex<Option<(u64, Instant)>>,
    next: AtomicU64,
}

impl HelperSlot {
    const fn new() -> Self {
        Self {
            held: Mutex::new(None),
            next: AtomicU64::new(0),
        }
    }

    fn held(&self) -> std::sync::MutexGuard<'_, Option<(u64, Instant)>> {
        self.held.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The slot, when it is free or its holder has had it for `timeout`.
    fn claim(&'static self, timeout: Duration) -> Option<PdfHelper> {
        let mut held = self.held();
        if held.is_some_and(|(_, since)| since.elapsed() < timeout) {
            return None;
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        *held = Some((id, Instant::now()));
        Some(PdfHelper { slot: self, id })
    }
}

/// A claim on a `HelperSlot`. Dropping it frees the slot, unless the
/// claim ran out of time and another has taken the slot since.
struct PdfHelper {
    slot: &'static HelperSlot,
    id: u64,
}

impl Drop for PdfHelper {
    fn drop(&mut self) {
        let mut held = self.slot.held();
        if held.is_some_and(|(id, _)| id == self.id) {
            *held = None;
        }
    }
}

/// Run `work` on a helper thread of its own, below normal priority like
/// the inspect thread. `false` when the thread couldn't be started.
fn helper(work: impl FnOnce() + Send + 'static) -> bool {
    thread::Builder::new()
        .name("kiln-inspect".to_owned())
        .spawn(move || {
            crate::process::lower_thread_priority();
            work();
        })
        .is_ok()
}

/// The next thing a helper sends on `rx`, waiting until `LIMIT` has passed
/// or `cancel` is set. `None` when it ran out of time, was cancelled, or
/// the helper stopped without sending.
fn wait<T>(rx: &Receiver<T>, cancel: &CancelToken) -> Option<T> {
    let start = Instant::now();
    loop {
        if cancel.is_cancelled() {
            return None;
        }
        match rx.recv_timeout(POLL) {
            Ok(found) => return Some(found),
            Err(RecvTimeoutError::Disconnected) => return None,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if start.elapsed() >= LIMIT {
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
    fn alpha_is_read_from_the_pixel_format() {
        for f in [
            "yuva420p",
            "yuva444p10le",
            "rgba",
            "rgba64le",
            "gbrap",
            "gbrap16le",
            "ya8",
            "ya16be",
            "bgra",
            "argb",
            "abgr",
        ] {
            assert!(alpha_format(f), "{f}");
        }
        for f in [
            "yuv420p", "yuv444p", "rgb24", "gbrp", "gray", "bgr24", "nv12",
        ] {
            assert!(!alpha_format(f), "{f}");
        }
    }

    #[test]
    fn bars_are_read_by_axis() {
        let hd = (1920, 1080);
        assert_eq!(Bars::of(hd, (1440, 1080)), Bars::Sides);
        assert_eq!(Bars::of(hd, (1920, 800)), Bars::AboveBelow);
        assert_eq!(Bars::of(hd, (1440, 800)), Bars::Around);
        assert_eq!(Bars::of(hd, hd), Bars::None);
        // About 1% a side is too thin to crop.
        assert_eq!(Bars::of(hd, (1880, 1060)), Bars::None);
        // 4% of the axis, 2% a side, is the least that counts.
        assert_eq!(Bars::of((1000, 1000), (960, 961)), Bars::Sides);
        assert_eq!(Bars::of((1000, 1000), (961, 960)), Bars::AboveBelow);
        // A crop with no picture left, or larger than the frame, is no bars.
        assert_eq!(Bars::of(hd, (0, 1080)), Bars::None);
        assert_eq!(Bars::of(hd, (1920, 0)), Bars::None);
        assert_eq!(Bars::of(hd, (1936, 1088)), Bars::None);
        assert_eq!(Bars::of((0, 0), (0, 0)), Bars::None);
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
        let (tx, rx) = mpsc::channel();
        assert!(helper(move || {
            thread::sleep(LIMIT * 3);
            let _ = tx.send(());
        }));
        let t = Instant::now();
        assert!(wait(&rx, &CancelToken::new()).is_none());
        assert!(t.elapsed() >= LIMIT && t.elapsed() < LIMIT * 2);
    }

    #[test]
    fn one_pdf_helper_at_a_time() {
        let first = PDF_HELPER.claim(HELPER_TIMEOUT);
        // Another test may hold it; either way, a second claim fails.
        assert!(PDF_HELPER.claim(HELPER_TIMEOUT).is_none());
        if first.is_some() {
            drop(first);
            assert!(PDF_HELPER.claim(HELPER_TIMEOUT).is_some());
        }
    }

    #[test]
    fn a_stuck_pdf_helper_lets_go_after_the_timeout() {
        static SLOT: HelperSlot = HelperSlot::new();
        let timeout = Duration::from_millis(200);
        // A slow helper: it holds the slot until told to finish.
        let stuck = SLOT.claim(timeout).unwrap();
        let (finish, told) = mpsc::channel::<()>();
        let (finished, done) = mpsc::channel();
        assert!(helper(move || {
            let _ = told.recv();
            drop(stuck);
            let _ = finished.send(());
        }));
        assert!(SLOT.claim(timeout).is_none());
        thread::sleep(timeout + POLL);
        let next = SLOT
            .claim(timeout)
            .expect("free once the timeout has passed");
        // The stuck helper finishing late doesn't free the slot it lost.
        finish.send(()).unwrap();
        done.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(SLOT.claim(timeout).is_none());
        drop(next);
        assert!(SLOT.claim(timeout).is_some());
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
