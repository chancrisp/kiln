//! What `inspect` reads: for images, size, transparency, animation, photo
//! or graphic and likely result sizes; for PDFs, pages and text; for audio
//! and video, what Kiln's own engine and FFprobe report, black bars and
//! likely result sizes. MP4, MOV, M4V and MKV are read by Kiln's own engine
//! alone.

mod pictures;
#[path = "../../../tests/support/mod.rs"]
mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use image::{Rgb, RgbImage};
use kiln_core::{
    inspect, suggest, Bars, CancelToken, Facts, Kiln, Kind, Look, Options, PdfFacts, Suggested,
};
use pictures::{exif_orientation, jpeg_with, left_red};
use support::{fixture, secret_pdf};

fn f(name: &str) -> Facts {
    inspect(
        &Kiln::new(),
        &fixture(name),
        &Options::default(),
        &CancelToken::new(),
    )
    .unwrap()
}

fn no_ffmpeg() -> Options {
    let mut o = Options::default();
    o.engines.ffmpeg = Some(PathBuf::from(r"C:\nowhere\ffmpeg.exe"));
    o
}

fn inspect_at(path: &Path) -> Facts {
    inspect(&Kiln::new(), path, &Options::default(), &CancelToken::new()).unwrap()
}

/// A GIF of `frames` 8 x 8 frames, each a different flat colour.
fn inspect_temp_gif(frames: u8) -> Facts {
    use image::codecs::gif::GifEncoder;
    use image::{Frame, Rgba, RgbaImage};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("anim.gif");
    let file = fs::File::create(&path).unwrap();
    let mut encoder = GifEncoder::new(file);
    encoder
        .encode_frames((0..frames).map(|i| {
            Frame::new(RgbaImage::from_pixel(
                8,
                8,
                Rgba([i * 100, 50, 200 - i * 50, 255]),
            ))
        }))
        .unwrap();
    drop(encoder);
    inspect_at(&path)
}

/// A `w` x `h` smooth gradient with noise of up to `amp` either way in
/// each channel, from a fixed-seed LCG.
fn smooth_plus_noise(w: u32, h: u32, amp: u32) -> RgbImage {
    let mut seed: u32 = 0x5EED_0001;
    let mut noise = move || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        ((seed >> 16) % (2 * amp + 1)) as i32 - amp as i32
    };
    RgbImage::from_fn(w, h, |x, y| {
        let base = [
            x * 200 / w + 20,
            y * 200 / h + 20,
            (x + y) * 150 / (w + h) + 60,
        ];
        Rgb(base.map(|b| (b as i32 + noise()).clamp(0, 255) as u8))
    })
}

/// The estimate `inspect` gives for `to`, and the size of a real
/// conversion to it, for a 1024 x 768 smooth picture with noise of up to
/// `amp` either way.
fn estimate_and_real(to: &str, amp: u32) -> (u64, u64) {
    let dir = tempfile::tempdir().unwrap();
    // BMP is lossless and converts to both JPG and PNG.
    let path = dir.path().join("picture.bmp");
    smooth_plus_noise(1024, 768, amp).save(&path).unwrap();
    let est = inspect_at(&path).estimates[to];
    let out = dir.path().join("out");
    fs::create_dir(&out).unwrap();
    let opts = Options {
        out_dir: Some(out),
        ..Options::default()
    };
    let real = Kiln::new().convert(&path, to, &opts).unwrap();
    (est, fs::metadata(real).unwrap().len())
}

#[test]
fn a_noisy_picture_is_a_photo() {
    let p = f("photo.png").picture.unwrap();
    assert_eq!(
        (p.width, p.height, p.look, p.alpha, p.animated),
        (64, 48, Look::Photo, false, false)
    );
}

#[test]
fn transparency_is_seen() {
    let p = f("rgba_half_transparent.png").picture.unwrap();
    assert!(p.alpha);
    assert_eq!(p.look, Look::Graphic);
}

#[test]
fn apng_frames_are_counted() {
    assert!(f("sample.apng").picture.unwrap().animated);
}

#[test]
fn a_still_gif_is_not_animated() {
    assert!(!f("sample.gif").picture.unwrap().animated);
}

#[test]
fn a_two_frame_gif_is_animated() {
    // Written to a temp folder with image::codecs::gif::GifEncoder, two 8 × 8 frames.
    assert!(inspect_temp_gif(2).picture.unwrap().animated);
}

#[test]
fn svg_is_a_graphic() {
    let p = f("sample.svg").picture.unwrap();
    assert_eq!(p.look, Look::Graphic);
    assert_eq!((p.width, p.height), (64, 48));
}

#[test]
fn heic_is_a_photo_by_its_type() {
    let p = f("sample.heic").picture.unwrap();
    assert_eq!(p.look, Look::Photo);
    assert!(p.width > 0);
}

#[test]
fn jxl_and_avif_transparency_comes_from_ffprobe() {
    for name in ["sample.jxl", "sample.avif"] {
        assert!(!f(name).picture.unwrap().alpha, "{name}");
    }
    // A transparent JXL, made from a half-transparent PNG.
    let dir = tempfile::tempdir().unwrap();
    let jxl = dir.path().join("alpha.jxl");
    let made = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(fixture("rgba_half_transparent.png"))
        .args(["-c:v", "libjxl"])
        .arg(&jxl)
        .status()
        .unwrap();
    assert!(made.success());
    assert!(inspect_at(&jxl).picture.unwrap().alpha);
}

#[test]
fn unreadable_images_keep_type_and_size() {
    for name in ["not_really.png", "truncated.png"] {
        let facts = f(name);
        assert_eq!(facts.kind, Kind::Image);
        assert_eq!(facts.bytes, std::fs::metadata(fixture(name)).unwrap().len());
        assert!(facts.picture.is_none());
        assert!(facts.estimates.is_empty());
    }
}

#[test]
fn unknown_or_missing_files_give_nothing() {
    let k = Kiln::new();
    let (o, c) = (Options::default(), CancelToken::new());
    assert!(inspect(&k, Path::new("no_such_file.png"), &o, &c).is_none());
    let dir = tempfile::tempdir().unwrap();
    let odd = dir.path().join("notes.xyz");
    fs::write(&odd, b"hello").unwrap();
    assert!(inspect(&k, &odd, &o, &c).is_none());
}

#[test]
fn image_estimates_land_within_2x() {
    // A 1024 × 768 smooth-plus-noise picture drawn into a temp folder, then inspected.
    // Each estimate is compared with a real Kiln::convert to that target.
    // A detailed picture (noise of up to 40 either way) and a smooth one
    // (up to 4, about the grain of a camera sensor).
    for amp in [40, 4] {
        for to in ["jpg", "png"] {
            let (est, real) = estimate_and_real(to, amp);
            assert!(
                est * 2 >= real && est <= real * 2,
                "±{amp} {to}: {est} vs {real}"
            );
        }
    }
}

/// Only one PDF is read at a time, so tests that read one take turns.
static PDF: Mutex<()> = Mutex::new(());

fn one_pdf_at_a_time() -> MutexGuard<'static, ()> {
    PDF.lock().unwrap_or_else(|e| e.into_inner())
}

#[test]
fn a_pdf_with_text() {
    let _turn = one_pdf_at_a_time();
    assert_eq!(f("sample.pdf").pdf.unwrap().has_text, Some(true));
}

#[test]
fn a_huge_pdf_is_not_read() {
    let _turn = one_pdf_at_a_time();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("huge.pdf");
    let file = fs::File::create(&path).unwrap();
    file.set_len(200_000_001).unwrap();
    drop(file);
    let t = Instant::now();
    let p = inspect_at(&path);
    assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
    assert_eq!(
        (p.kind, p.bytes, p.pdf),
        (Kind::Document, 200_000_001, None)
    );
    fs::remove_file(&path).unwrap();
}

#[test]
fn a_scanned_pdf() {
    let _turn = one_pdf_at_a_time();
    assert_eq!(
        f("scan.pdf").pdf.unwrap(),
        PdfFacts {
            pages: 1,
            has_text: Some(false)
        }
    );
}

/// A PDF that asks for a password, or has only an owner password and opens
/// without one, is counted but its text isn't looked at.
#[test]
fn a_protected_pdf_gets_no_text_fact() {
    let _turn = one_pdf_at_a_time();
    let dir = tempfile::tempdir().unwrap();
    let plain = dir.path().join("plain.pdf");
    secret_pdf(&plain, 2, None);
    assert_eq!(
        inspect_at(&plain).pdf,
        Some(PdfFacts {
            pages: 2,
            has_text: Some(true)
        })
    );

    let owner = dir.path().join("owner.pdf");
    secret_pdf(&owner, 2, Some(""));
    assert_eq!(
        inspect_at(&owner).pdf,
        Some(PdfFacts {
            pages: 2,
            has_text: None
        })
    );

    let user = dir.path().join("user.pdf");
    secret_pdf(&user, 2, Some("secret"));
    assert_eq!(inspect_at(&user).pdf.map(|p| p.has_text), Some(None));
}

#[test]
fn black_bars_are_found() {
    assert_eq!(f("letterbox.mp4").bars, Some(Bars::AboveBelow));
}

#[test]
fn no_bars_are_found() {
    assert_eq!(f("sample.mp4").bars, Some(Bars::None));
}

#[test]
fn video_has_a_frame_and_estimates() {
    let v = f("sample.mov");
    assert!(v.media.unwrap().duration.is_some());
    assert!(v.picture.is_some());
    assert!(v.estimates.contains_key("mp4"));
    assert!(v.compressed.is_some());
}

#[test]
fn audio_has_no_picture() {
    let a = f("sample.wav");
    assert!(a.media.is_some() && a.picture.is_none() && a.bars.is_none());
    assert!(a.estimates.contains_key("mp3"));
}

#[test]
fn facts_for_mp4_come_without_ffprobe() {
    for name in ["sample.mp4", "sample.mov", "sample.m4v", "sample.mkv"] {
        let v = inspect(
            &Kiln::new(),
            &fixture(name),
            &no_ffmpeg(),
            &CancelToken::new(),
        )
        .unwrap();
        assert!(v.bytes > 0, "{name}");
        // The picture, its bars and the sound are all Kiln's own reading.
        let p = v.picture.as_ref().expect(name);
        assert_eq!((p.width, p.height), (64, 48), "{name}");
        assert_eq!(v.bars, Some(Bars::None), "{name}");
        let m = v.media.as_ref().unwrap();
        assert!(m.has_audio, "{name}");
        assert_eq!(
            m.video_codec,
            Some(kiln_media::video::VideoCodec::H264),
            "{name}"
        );
        assert_eq!((m.width, m.height), (Some(64), Some(48)), "{name}");
        assert!(m.fps.is_some_and(|f| f > 1.0), "{name}: {m:?}");
        assert_eq!(m.rotation, 0, "{name}");
        assert_eq!(m.hdr, Some(kiln_media::video::Hdr::None), "{name}");
        assert_eq!(m.bit_depth, Some(8), "{name}");
        let secs = m.duration.unwrap();
        assert!((0.9..1.2).contains(&secs), "{name}: {secs}");
        // A copy's size is the picture's bitrate, worked out from the file
        // where the container states none, as Matroska doesn't (R54).
        assert!(v.estimates.contains_key("mp4"), "{name}");
        assert!(v.compressed.is_some(), "{name}");
    }
    let mp4 = inspect(
        &Kiln::new(),
        &fixture("sample.mp4"),
        &no_ffmpeg(),
        &CancelToken::new(),
    )
    .unwrap();
    assert_eq!(mp4.media.unwrap().codec.as_deref(), Some("AAC"));
    // Black bars too.
    let letterbox = inspect(
        &Kiln::new(),
        &fixture("letterbox.mp4"),
        &no_ffmpeg(),
        &CancelToken::new(),
    )
    .unwrap();
    assert_eq!(letterbox.bars, Some(Bars::AboveBelow));
}

#[test]
fn a_rotated_or_hdr_video_says_so() {
    let data = |name: &str| support::media_data(&format!("video/{name}"));
    let rot = inspect(
        &Kiln::new(),
        &data("rot90.mp4"),
        &no_ffmpeg(),
        &CancelToken::new(),
    )
    .unwrap();
    assert_eq!(rot.media.unwrap().rotation, 90);
    let hdr = inspect(
        &Kiln::new(),
        &data("hevc10_hdr.mp4"),
        &no_ffmpeg(),
        &CancelToken::new(),
    )
    .unwrap();
    let m = hdr.media.unwrap();
    assert_eq!(m.video_codec, Some(kiln_media::video::VideoCodec::Hevc));
    assert_eq!(m.hdr, Some(kiln_media::video::Hdr::Pq));
    assert_eq!(m.bit_depth, Some(10));
    assert!(!m.has_audio);
}

#[test]
fn a_missing_ffmpeg_gives_type_and_size() {
    // A video Kiln can't open itself has nothing without it.
    let avi = inspect(
        &Kiln::new(),
        &fixture("sample.avi"),
        &no_ffmpeg(),
        &CancelToken::new(),
    )
    .unwrap();
    assert!(avi.media.is_none() && avi.picture.is_none());
}

#[test]
fn a_cancelled_inspect_returns_at_once() {
    let c = CancelToken::new();
    c.cancel();
    let t = Instant::now();
    let v = inspect(
        &Kiln::new(),
        &fixture("sample.mov"),
        &Options::default(),
        &c,
    )
    .unwrap();
    assert!(t.elapsed() < Duration::from_secs(1));
    assert!(v.bars.is_none());
}

#[test]
fn broken_audio_keeps_type_and_size() {
    let a = f("corrupt.wav");
    assert!(a.media.is_none() && a.bytes > 0);
}

/// Converts `path` to JPG with Kiln into `out` and returns the bytes
/// written. A JPG goes through lossless BMP first, since Kiln doesn't
/// convert a type to itself.
fn real_jpg_size(path: &Path, out: &Path) -> u64 {
    let kiln = Kiln::new();
    let opts = Options {
        out_dir: Some(out.to_path_buf()),
        ..Options::default()
    };
    let is_jpg = kiln
        .registry()
        .format_of(path)
        .is_some_and(|f| f.ext == "jpg");
    let source = if is_jpg {
        kiln.convert(path, "bmp", &opts).unwrap()
    } else {
        path.to_path_buf()
    };
    let jpg = kiln.convert(&source, "jpg", &opts).unwrap();
    fs::metadata(jpg).unwrap().len()
}

#[test]
#[ignore = "needs KILN_EXTRA_SAMPLES"]
fn real_photos_and_screenshots_are_told_apart() {
    // $KILN_EXTRA_SAMPLES/photos/* must be Photo, $KILN_EXTRA_SAMPLES/graphics/* Graphic,
    // and every jpg estimate in photos/ within 2× of a real encode.
    let Some(root) = std::env::var_os("KILN_EXTRA_SAMPLES") else {
        return;
    };
    let root = Path::new(&root);
    let kiln = Kiln::new();
    let mut checked = 0;
    let mut wrong = Vec::new();
    for (folder, want) in [("photos", Look::Photo), ("graphics", Look::Graphic)] {
        let Ok(entries) = fs::read_dir(root.join(folder)) else {
            continue;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            let is_image = kiln
                .registry()
                .format_of(&path)
                .is_some_and(|f| f.kind == Kind::Image);
            if !path.is_file() || !is_image {
                continue;
            }
            let facts = inspect_at(&path);
            let picture = facts
                .picture
                .as_ref()
                .unwrap_or_else(|| panic!("{} has no picture facts", path.display()));
            checked += 1;
            if picture.look != want {
                wrong.push(format!("{}: {:?}", path.display(), picture.look));
            }
            if want == Look::Photo {
                if let Some(&est) = facts.estimates.get("jpg") {
                    let out = tempfile::tempdir().unwrap();
                    let real = real_jpg_size(&path, out.path());
                    if !(est * 2 >= real && est <= real * 2) {
                        wrong.push(format!("{}: jpg {est} vs {real}", path.display()));
                    }
                }
            }
        }
    }
    assert!(
        checked > 0,
        "no images in {}/photos or graphics",
        root.display()
    );
    assert!(wrong.is_empty(), "{checked} checked:\n{}", wrong.join("\n"));
}

/// A 64 x 48 AVIF made by FFmpeg in `dir`: half-transparent red, with its
/// alpha as a second stream as FFmpeg writes it, or opaque red. `None`
/// when FFmpeg is missing or can't encode AVIF here.
fn made_avif(dir: &Path, transparent: bool) -> Option<PathBuf> {
    let path = dir.join(if transparent {
        "alpha.avif"
    } else {
        "opaque.avif"
    });
    let mut cmd = std::process::Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi"]);
    if transparent {
        cmd.args(["-i", "color=c=red@0.5:s=64x48,format=yuva420p"])
            .args(["-filter_complex", "[0:v]split[c][a];[a]alphaextract[al]"])
            .args(["-map", "[c]", "-map", "[al]"]);
    } else {
        cmd.args(["-i", "color=c=red:s=64x48"]);
    }
    cmd.args([
        "-frames:v",
        "1",
        "-c:v",
        "libaom-av1",
        "-still-picture",
        "1",
    ])
    .arg(&path);
    let made = cmd.output().is_ok_and(|o| o.status.success()) && path.exists();
    if !made {
        eprintln!("skipped: FFmpeg is missing or can't encode AVIF with libaom-av1");
    }
    made.then_some(path)
}

#[test]
fn transparency_in_avif_is_seen() {
    let dir = tempfile::tempdir().unwrap();
    let Some(path) = made_avif(dir.path(), true) else {
        return;
    };
    let facts = inspect_at(&path);
    assert!(facts.picture.as_ref().unwrap().alpha);
    let kiln = Kiln::new();
    let offered = kiln.offered(&[path.as_path()], &Options::default());
    let s = suggest(&[facts], &offered);
    assert_ne!(
        s.convert.map(|c| c.action),
        Some(Suggested::Convert("jpg".to_owned()))
    );
}

#[test]
fn an_opaque_avif_has_no_alpha() {
    let dir = tempfile::tempdir().unwrap();
    let Some(path) = made_avif(dir.path(), false) else {
        return;
    };
    let pic = inspect_at(&path).picture.unwrap();
    assert!(!pic.alpha);
    assert_eq!((pic.width, pic.height), (64, 48));
}

#[test]
fn facts_report_the_upright_size() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sideways.jpg");
    // Stored 8 wide and 4 high, shown 4 wide and 8 high.
    jpeg_with(&path, &left_red(8, 4), 6, None, 95);
    let picture = inspect_at(&path).picture.unwrap();
    assert_eq!((picture.width, picture.height), (4, 8));
}

#[test]
fn a_picture_too_big_to_decode_still_reports_its_upright_size() {
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    use image::{ExtendedColorType, ImageEncoder};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("huge.png");
    // 56 million pixels: past what is decoded, so only the header is read.
    let (w, h) = (8000u32, 7000u32);
    let mut bytes = Vec::new();
    let mut encoder =
        PngEncoder::new_with_quality(&mut bytes, CompressionType::Fast, FilterType::NoFilter);
    encoder.set_exif_metadata(exif_orientation(6)).unwrap();
    encoder
        .write_image(&vec![0u8; (w * h) as usize], w, h, ExtendedColorType::L8)
        .unwrap();
    fs::write(&path, bytes).unwrap();
    let picture = inspect_at(&path).picture.unwrap();
    assert_eq!((picture.width, picture.height), (h, w));
}

/// Whether Windows reads `name` here, found by converting a copy to PNG with
/// no FFmpeg; the test says it is skipped when not.
fn windows_reads(test: &str, name: &str) -> bool {
    let (dir, input) = support::stage(&fixture(name));
    let mut opts = no_ffmpeg();
    opts.out_dir = Some(dir.path().to_path_buf());
    match Kiln::new().convert(&input, "png", &opts) {
        Ok(_) => true,
        Err(kiln_core::Error::NeedsExtension { message, .. }) => {
            eprintln!("{test}: skipped, Windows has no decoder here ({message})");
            false
        }
        Err(e) => panic!("{test}: {e:?}"),
    }
}

#[test]
fn heic_and_avif_sizes_are_read_by_windows() {
    for name in ["sample.heic", "sample.avif"] {
        if !windows_reads("sizes", name) {
            continue;
        }
        let facts = inspect(
            &Kiln::new(),
            &fixture(name),
            &no_ffmpeg(),
            &CancelToken::new(),
        )
        .unwrap();
        let p = facts
            .picture
            .unwrap_or_else(|| panic!("{name}: no picture"));
        assert_eq!((p.width, p.height), (64, 48), "{name}");
        assert_eq!(p.look, Look::Photo, "{name}");
        assert!(!p.alpha, "{name}");
    }
}

#[test]
fn a_transparent_avif_is_seen_without_ffmpeg() {
    let dir = tempfile::tempdir().unwrap();
    let Some(path) = made_avif(dir.path(), true) else {
        return;
    };
    if !windows_reads("avif alpha", "sample.avif") {
        return;
    }
    let p = inspect(&Kiln::new(), &path, &no_ffmpeg(), &CancelToken::new())
        .unwrap()
        .picture
        .unwrap();
    assert!(p.alpha);
    assert_eq!((p.width, p.height), (64, 48));
}
