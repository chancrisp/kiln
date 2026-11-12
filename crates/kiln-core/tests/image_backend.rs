//! The image backend, through the full `Kiln::convert` path.

mod pictures;
#[path = "../../../tests/support/mod.rs"]
mod support;

use std::fs;
use std::path::{Path, PathBuf};

use image::{ColorType, GenericImageView, ImageFormat};
use kiln_core::{Error, Kiln, Options};
use pictures::{
    corner_marked, display_p3, icc_of, jpeg_with, left_red, near, orientation_of, png_with, BLUE,
    GREEN, RED,
};
use support::{files_in, fixture};
use tempfile::TempDir;

fn staged(name: &str) -> (TempDir, PathBuf) {
    support::stage(&fixture(name))
}

#[derive(Default)]
struct JpegOptions {
    quality: Option<u8>,
    out_dir: Option<PathBuf>,
}

fn png_to_jpg(input: &Path, opts: &JpegOptions) -> kiln_core::Result<PathBuf> {
    let options = Options {
        out_dir: opts.out_dir.clone(),
        jpeg_quality: opts.quality.unwrap_or(90),
        ..Options::default()
    };
    Kiln::new().convert(input, "jpg", &options)
}

fn assert_jpeg(path: &Path) -> image::DynamicImage {
    let bytes = fs::read(path).unwrap();
    assert_eq!(&bytes[..3], &[0xFF, 0xD8, 0xFF], "missing JPEG SOI marker");
    assert_eq!(image::guess_format(&bytes).unwrap(), ImageFormat::Jpeg);
    image::load_from_memory_with_format(&bytes, ImageFormat::Jpeg).unwrap()
}

fn close(a: u8, b: u8, tol: u8) -> bool {
    a.abs_diff(b) <= tol
}

#[test]
fn rgb_png_becomes_a_jpg_next_to_the_input() {
    let (dir, input) = staged("rgb_gradient.png");
    let out = png_to_jpg(&input, &JpegOptions::default()).unwrap();

    assert_eq!(out, dir.path().join("rgb_gradient.jpg"));
    let img = assert_jpeg(&out);
    assert_eq!(img.dimensions(), (64, 48));
    assert_eq!(img.color(), ColorType::Rgb8);

    // Spot-check a pixel against the source gradient (x*4, y*5, 128).
    let p = img.to_rgb8().get_pixel(40, 20).0;
    assert!(
        close(p[0], 160, 12) && close(p[1], 100, 12) && close(p[2], 128, 12),
        "{p:?}"
    );
}

#[test]
fn transparency_is_flattened_onto_white() {
    let (_dir, input) = staged("rgba_half_transparent.png");
    let out = png_to_jpg(&input, &JpegOptions::default()).unwrap();
    let img = assert_jpeg(&out).to_rgb8();
    assert_eq!(img.dimensions(), (32, 32));

    let left = img.get_pixel(4, 16).0;
    let right = img.get_pixel(28, 16).0;
    assert!(left.iter().all(|&c| c >= 245), "transparent area {left:?}");
    assert!(
        close(right[0], 20, 12) && close(right[1], 40, 12) && close(right[2], 120, 12),
        "opaque area {right:?}"
    );
}

#[test]
fn grayscale_stays_grayscale() {
    let (_dir, input) = staged("gray8.png");
    let out = png_to_jpg(&input, &JpegOptions::default()).unwrap();
    let img = assert_jpeg(&out);
    assert_eq!(img.color(), ColorType::L8);
    assert_eq!(img.dimensions(), (40, 20));
}

#[test]
fn sixteen_bit_input_is_reduced_to_eight_bit() {
    let (_dir, input) = staged("rgb16.png");
    let out = png_to_jpg(&input, &JpegOptions::default()).unwrap();
    let img = assert_jpeg(&out).to_rgb8();
    // 10000/257, 40000/257, 20000/257 is roughly (39, 156, 78).
    let p = img.get_pixel(12, 12).0;
    assert!(
        close(p[0], 39, 8) && close(p[1], 156, 8) && close(p[2], 78, 8),
        "{p:?}"
    );
}

#[test]
fn existing_outputs_are_never_overwritten() {
    let (dir, input) = staged("rgb_gradient.png");
    fs::write(dir.path().join("rgb_gradient.jpg"), b"precious").unwrap();

    let first = png_to_jpg(&input, &JpegOptions::default()).unwrap();
    let second = png_to_jpg(&input, &JpegOptions::default()).unwrap();

    assert_eq!(first.file_name().unwrap(), "rgb_gradient (1).jpg");
    assert_eq!(second.file_name().unwrap(), "rgb_gradient (2).jpg");
    assert_eq!(
        fs::read(dir.path().join("rgb_gradient.jpg")).unwrap(),
        b"precious"
    );
    assert_jpeg(&first);
    assert_jpeg(&second);
}

#[test]
fn output_folder_can_be_chosen() {
    let (_src, input) = staged("rgb_gradient.png");
    let dest = tempfile::tempdir().unwrap();
    let opts = JpegOptions {
        out_dir: Some(dest.path().to_path_buf()),
        ..JpegOptions::default()
    };
    let out = png_to_jpg(&input, &opts).unwrap();
    assert_eq!(out, dest.path().join("rgb_gradient.jpg"));
    assert_jpeg(&out);
}

#[test]
fn missing_output_folder_is_an_error() {
    let (dir, input) = staged("rgb_gradient.png");
    let opts = JpegOptions {
        out_dir: Some(dir.path().join("nope")),
        ..JpegOptions::default()
    };
    assert!(matches!(
        png_to_jpg(&input, &opts),
        Err(Error::OutputDirNotFound(_))
    ));
}

#[test]
fn reserved_windows_names_are_sanitized() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("CON.png");
    fs::copy(fixture("rgb_gradient.png"), &input).unwrap();
    let out = png_to_jpg(&input, &JpegOptions::default()).unwrap();
    assert_eq!(out.file_name().unwrap(), "CON_.jpg");
}

#[test]
fn quality_changes_the_output_size() {
    let (_dir, input) = staged("rgb_gradient.png");
    let low = png_to_jpg(
        &input,
        &JpegOptions {
            quality: Some(10),
            ..JpegOptions::default()
        },
    )
    .unwrap();
    let high = png_to_jpg(
        &input,
        &JpegOptions {
            quality: Some(100),
            ..JpegOptions::default()
        },
    )
    .unwrap();
    assert!(fs::metadata(&low).unwrap().len() < fs::metadata(&high).unwrap().len());
}

#[test]
fn out_of_range_quality_is_rejected() {
    let (dir, input) = staged("rgb_gradient.png");
    for q in [0, 101] {
        let opts = JpegOptions {
            quality: Some(q),
            ..JpegOptions::default()
        };
        assert!(matches!(
            png_to_jpg(&input, &opts),
            Err(Error::InvalidQuality(_))
        ));
    }
    assert_eq!(files_in(dir.path()), ["rgb_gradient.png"]);
}

#[test]
fn missing_input_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let err = png_to_jpg(&dir.path().join("ghost.png"), &JpegOptions::default()).unwrap_err();
    assert!(matches!(err, Error::InputNotFound(_)));
    assert!(err.to_string().ends_with("doesn't exist."));
}

#[test]
fn a_folder_is_not_an_input() {
    let dir = tempfile::tempdir().unwrap();
    let err = png_to_jpg(dir.path(), &JpegOptions::default()).unwrap_err();
    assert!(matches!(err, Error::InputNotAFile(_)));
}

#[test]
fn non_png_content_is_rejected_whatever_the_extension() {
    let (dir, input) = staged("not_really.png");
    let err = png_to_jpg(&input, &JpegOptions::default()).unwrap_err();
    assert!(matches!(err, Error::WrongContent { .. }));
    assert_eq!(files_in(dir.path()), ["not_really.png"]);
}

#[test]
fn corrupt_png_fails_and_leaves_no_output() {
    let (dir, input) = staged("truncated.png");
    let err = png_to_jpg(&input, &JpegOptions::default()).unwrap_err();
    assert!(matches!(err, Error::Decode { .. }), "{err:?}");
    assert_eq!(files_in(dir.path()), ["truncated.png"]);
}

#[test]
fn error_messages_follow_the_voice_rules() {
    let dir = tempfile::tempdir().unwrap();
    let errors = [
        Error::InputNotFound(dir.path().join("a.png")),
        Error::InputNotAFile(dir.path().to_path_buf()),
        Error::WrongContent {
            path: dir.path().join("a.png"),
            code: "PNG".into(),
        },
        Error::OutputDirNotFound(dir.path().to_path_buf()),
        Error::InvalidQuality(0),
    ];
    for e in errors {
        let msg = e.to_string();
        assert!(!msg.contains('!'), "exclamation mark in {msg:?}");
        assert!(msg.ends_with('.'), "{msg:?}");
    }
}

fn convert(input: &Path, to: &str) -> kiln_core::Result<PathBuf> {
    Kiln::new().convert(input, to, &Options::default())
}

#[test]
fn every_raster_type_converts_to_every_other_keeping_pixels() {
    let types = [
        ("png", ImageFormat::Png),
        ("jpg", ImageFormat::Jpeg),
        ("webp", ImageFormat::WebP),
        ("bmp", ImageFormat::Bmp),
        ("gif", ImageFormat::Gif),
        ("tiff", ImageFormat::Tiff),
    ];
    for (from, _) in types {
        let (_dir, input) = staged(
            &support::sample_for(from)
                .file_name()
                .unwrap()
                .to_string_lossy(),
        );
        for (to, format) in types {
            if to == from {
                continue;
            }
            let out = convert(&input, to).unwrap();
            let bytes = fs::read(&out).unwrap();
            assert_eq!(
                image::guess_format(&bytes).unwrap(),
                format,
                "{from} -> {to}"
            );
            let img = image::load_from_memory_with_format(&bytes, format).unwrap();
            assert_eq!(img.dimensions(), (64, 48), "{from} -> {to}");
            // The gradient is (x*4, y*5, 128); lossy steps stay close.
            let p = img.to_rgb8().get_pixel(40, 20).0;
            assert!(
                close(p[0], 160, 24) && close(p[1], 100, 24) && close(p[2], 128, 24),
                "{from} -> {to}: {p:?}"
            );
        }
    }
}

#[test]
fn lossless_types_keep_exact_pixels() {
    let (_dir, input) = staged("rgb_gradient.png");
    let src = image::open(&input).unwrap().to_rgb8();
    for to in ["bmp", "tiff"] {
        let out = convert(&input, to).unwrap();
        assert_eq!(image::open(&out).unwrap().to_rgb8(), src, "{to}");
    }
    // WebP is lossy, like JPG: close to the source, not equal to it.
    let out = convert(&input, "webp").unwrap();
    let p = image::open(&out).unwrap().to_rgb8().get_pixel(40, 20).0;
    assert!(
        close(p[0], 160, 12) && close(p[1], 100, 12) && close(p[2], 128, 12),
        "webp: {p:?}"
    );
}

#[test]
fn transparency_survives_where_the_target_has_alpha() {
    let (_dir, input) = staged("rgba_half_transparent.png");
    for to in ["webp", "gif", "tiff", "bmp"] {
        let out = convert(&input, to).unwrap();
        let img = image::open(&out).unwrap().to_rgba8();
        assert_eq!(img.get_pixel(2, 2).0[3], 0, "{to} lost the clear half");
        assert_eq!(img.get_pixel(30, 2).0[3], 255, "{to}");
    }
}

#[test]
fn sixteen_bit_png_becomes_eight_bit_where_needed() {
    let (_dir, input) = staged("rgb16.png");
    for to in ["webp", "bmp", "gif"] {
        let out = convert(&input, to).unwrap();
        let p = image::open(&out).unwrap().to_rgb8().get_pixel(5, 5).0;
        // WebP is lossy, so it gets the tolerance JPG has above.
        let tolerance = if to == "webp" { 8 } else { 4 };
        assert!(close(p[1], 155, tolerance), "{to}: {p:?}");
    }
    let tiff = convert(&input, "tiff").unwrap();
    assert_eq!(image::open(&tiff).unwrap().color(), ColorType::Rgb16);
}

#[test]
fn the_tif_spelling_is_read_and_tiff_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("scan.tif");
    fs::copy(fixture("sample.tiff"), &input).unwrap();
    let out = convert(&input, "png").unwrap();
    assert_eq!(out.file_name().unwrap(), "scan.png");
    let back = convert(&out, "tif").unwrap();
    assert_eq!(back.file_name().unwrap(), "scan.tiff");
}

#[test]
fn content_must_match_the_extension_for_every_type() {
    let dir = tempfile::tempdir().unwrap();
    for ext in ["jpg", "webp", "bmp", "gif", "tiff"] {
        let input = dir.path().join(format!("fake.{ext}"));
        fs::copy(fixture("rgb_gradient.png"), &input).unwrap();
        let err = convert(&input, "png").unwrap_err();
        assert!(matches!(err, Error::WrongContent { .. }), "{ext}: {err:?}");
    }
    assert_eq!(files_in(dir.path()).len(), 5, "no outputs were written");
}

/// Where the red half of an 8 x 4 picture, red on the left as stored, ends
/// up once EXIF orientation 1 to 8 has turned it upright.
#[derive(Clone, Copy)]
enum Red {
    Left,
    Right,
    Top,
    Bottom,
}

/// What one EXIF orientation makes of an 8 x 4 picture stored red on the
/// left with its top-left pixel green: its upright size, where its red half
/// is, and where the green pixel is. The half alone can't tell 1 from 4, 2
/// from 3, 5 from 6 or 7 from 8; the green pixel does.
struct Upright {
    orientation: u16,
    size: (u32, u32),
    red: Red,
    green: (u32, u32),
}

const fn upright(orientation: u16, size: (u32, u32), red: Red, green: (u32, u32)) -> Upright {
    Upright {
        orientation,
        size,
        red,
        green,
    }
}

const UPRIGHT: [Upright; 8] = [
    upright(1, (8, 4), Red::Left, (0, 0)),
    upright(2, (8, 4), Red::Right, (7, 0)),
    upright(3, (8, 4), Red::Right, (7, 3)),
    upright(4, (8, 4), Red::Left, (0, 3)),
    upright(5, (4, 8), Red::Top, (0, 0)),
    upright(6, (4, 8), Red::Top, (3, 0)),
    upright(7, (4, 8), Red::Bottom, (3, 7)),
    upright(8, (4, 8), Red::Bottom, (0, 7)),
];

/// Every corner of `img` is red or blue as `red` says, except `green`, which
/// is green. The JPEG tolerance is 40.
fn assert_corners(img: &image::RgbImage, red: Red, green: (u32, u32), what: &str) {
    let (w, h) = img.dimensions();
    for (x, y) in [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)] {
        let is_red = match red {
            Red::Left => x < w / 2,
            Red::Right => x >= w / 2,
            Red::Top => y < h / 2,
            Red::Bottom => y >= h / 2,
        };
        let want = if (x, y) == green {
            GREEN
        } else if is_red {
            RED
        } else {
            BLUE
        };
        let got = img.get_pixel(x, y).0;
        assert!(
            near(got, want, 40),
            "{what}: ({x}, {y}) is {got:?}, not {want:?}"
        );
    }
}

#[test]
fn every_exif_orientation_comes_out_upright() {
    let dir = tempfile::tempdir().unwrap();
    for Upright {
        orientation,
        size,
        red,
        green,
    } in UPRIGHT
    {
        let input = dir.path().join(format!("o{orientation}.jpg"));
        jpeg_with(&input, &corner_marked(8, 4), orientation, None, 95);
        let out = convert(&input, "png").unwrap();
        let img = image::open(&out).unwrap().to_rgb8();
        assert_eq!(img.dimensions(), size, "orientation {orientation}");
        assert_corners(&img, red, green, &format!("orientation {orientation}"));
        // The pixels are turned, so no orientation is left to turn again.
        assert_eq!(
            orientation_of(&out),
            image::metadata::Orientation::NoTransforms
        );
    }
}

#[test]
fn a_png_with_exif_orientation_is_turned() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("sideways.png");
    png_with(&input, &corner_marked(8, 4), 6, None);
    let out = convert(&input, "jpg").unwrap();
    let img = assert_jpeg(&out).to_rgb8();
    assert_eq!(img.dimensions(), (4, 8));
    assert_corners(&img, Red::Top, (3, 0), "orientation 6");
}

#[test]
fn an_icc_profile_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("wide.png");
    let profile = display_p3();
    png_with(&input, &left_red(8, 4), 1, Some(&profile));
    assert_eq!(icc_of(&input).as_deref(), Some(profile.as_slice()));

    let jpg = convert(&input, "jpg").unwrap();
    let tiff = convert(&input, "tiff").unwrap();
    // A PNG can't be converted to a PNG, so the TIFF goes on to one. The
    // profile is larger than the 8 x 4 picture's pixels, which the TIFF
    // decoder reads only before its limits are set.
    let png = convert(&tiff, "png").unwrap();
    for (to, out) in [("jpg", &jpg), ("tiff", &tiff), ("png", &png)] {
        assert_eq!(
            icc_of(out).as_deref(),
            Some(profile.as_slice()),
            "{to} changed the profile"
        );
    }
    // BMP has nowhere to put it.
    let bmp = convert(&input, "bmp").unwrap();
    assert_eq!(image::open(&bmp).unwrap().dimensions(), (8, 4));
}

#[test]
fn a_profile_too_big_for_a_jpeg_is_left_out_not_a_failure() {
    let dir = tempfile::tempdir().unwrap();
    // 17 MiB claiming to be RGB: past the 65,519 x 255 bytes a JPEG can hold,
    // which its encoder refuses only when it writes. All zeros, so the PNG
    // that carries it stays small.
    let mut huge = vec![0u8; 17 << 20];
    huge[16..20].copy_from_slice(b"RGB ");
    let input = dir.path().join("heavy.png");
    png_with(&input, &left_red(8, 4), 1, Some(&huge));
    assert_eq!(icc_of(&input).map(|p| p.len()), Some(huge.len()));
    for to in ["jpg", "tiff", "webp"] {
        let out = convert(&input, to).unwrap();
        assert_eq!(image::open(&out).unwrap().dimensions(), (8, 4), "{to}");
        assert_eq!(icc_of(&out), None, "{to} kept a profile that big");
    }
}

#[test]
fn a_cmyk_profile_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    // A profile for CMYK ink on a picture of red, green and blue.
    let mut cmyk = vec![0u8; 128];
    cmyk[16..20].copy_from_slice(b"CMYK");
    let as_png = dir.path().join("inks.png");
    png_with(&as_png, &left_red(8, 4), 1, Some(&cmyk));
    let as_jpg = dir.path().join("inks.jpg");
    jpeg_with(&as_jpg, &left_red(8, 4), 1, Some(&cmyk), 95);
    // Each input carries the profile, and each type that can hold one is
    // written from one of them.
    for (input, to) in [
        (&as_png, "jpg"),
        (&as_png, "tiff"),
        (&as_png, "webp"),
        (&as_jpg, "png"),
        (&as_jpg, "tiff"),
        (&as_jpg, "webp"),
    ] {
        assert_eq!(icc_of(input).as_deref(), Some(cmyk.as_slice()));
        let out = convert(input, to).unwrap();
        assert_eq!(icc_of(&out), None, "{to} kept a CMYK profile");
    }
}

/// The chunks of a WebP file, each as its four-letter name and its bytes.
fn webp_chunks(bytes: &[u8]) -> Vec<([u8; 4], &[u8])> {
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WEBP");
    let mut chunks = Vec::new();
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let name: [u8; 4] = bytes[at..at + 4].try_into().unwrap();
        let len = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        chunks.push((name, &bytes[at + 8..at + 8 + len]));
        // Chunks are padded to an even length.
        at += 8 + len + (len & 1);
    }
    chunks
}

/// Whether the WebP `bytes` has a chunk called `name`.
fn has_chunk(bytes: &[u8], name: &[u8; 4]) -> bool {
    webp_chunks(bytes).iter().any(|(n, _)| n == name)
}

#[test]
fn webp_is_lossy_at_the_preset_quality() {
    let (_dir, input) = staged("photo.png");
    let webp_at = |jpeg_quality| {
        let options = Options {
            jpeg_quality,
            ..Options::default()
        };
        let out = Kiln::new().convert(&input, "webp", &options).unwrap();
        fs::read(out).unwrap()
    };
    let small = webp_at(75);
    let best = webp_at(97);
    for bytes in [&small, &best] {
        assert_eq!(image::guess_format(bytes).unwrap(), ImageFormat::WebP);
        // `VP8 ` holds a lossy picture, `VP8L` a lossless one.
        assert!(has_chunk(bytes, b"VP8 "), "not lossy");
        assert!(!has_chunk(bytes, b"VP8L"), "lossless");
        let img = image::load_from_memory_with_format(bytes, ImageFormat::WebP).unwrap();
        assert_eq!(img.dimensions(), (64, 48));
    }
    assert!(
        small.len() < best.len(),
        "quality 75 is {} bytes, 97 is {}",
        small.len(),
        best.len()
    );
}

#[test]
fn webp_keeps_transparency() {
    let (_dir, input) = staged("rgba_half_transparent.png");
    let src = image::open(&input).unwrap().to_rgba8();
    let out = convert(&input, "webp").unwrap();
    let bytes = fs::read(&out).unwrap();
    // Lossy colour with an alpha plane of its own.
    assert!(has_chunk(&bytes, b"VP8 ") && has_chunk(&bytes, b"ALPH"));
    let img = image::load_from_memory_with_format(&bytes, ImageFormat::WebP)
        .unwrap()
        .to_rgba8();
    assert_eq!(img.dimensions(), src.dimensions());
    for (x, y, want) in src.enumerate_pixels() {
        let got = img.get_pixel(x, y).0[3];
        assert!(
            close(got, want.0[3], 2),
            "({x}, {y}) has alpha {got}, not {}",
            want.0[3]
        );
    }
}

#[test]
fn webp_keeps_an_icc_profile() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("wide.png");
    let profile = display_p3();
    png_with(&input, &left_red(8, 4), 1, Some(&profile));

    let out = convert(&input, "webp").unwrap();
    let bytes = fs::read(&out).unwrap();
    let chunks = webp_chunks(&bytes);
    let (_, held) = chunks
        .iter()
        .find(|(name, _)| name == b"ICCP")
        .expect("no ICCP chunk");
    assert_eq!(*held, profile.as_slice());
    assert!(has_chunk(&bytes, b"VP8 "), "not lossy");
    // The picture still reads, with the profile where a decoder looks.
    assert_eq!(image::open(&out).unwrap().dimensions(), (8, 4));
    assert_eq!(icc_of(&out).as_deref(), Some(profile.as_slice()));
}

/// A GIF at `path` of square frames `side` pixels across, one per entry of
/// `delays` (in ms), each a flat colour: red, green, blue, then round again.
/// With no `repeat` the GIF has no loop extension, so it plays once.
fn animated_gif(
    path: &Path,
    side: u32,
    delays: &[u32],
    repeat: Option<image::codecs::gif::Repeat>,
) {
    use image::{Delay, Frame, Rgba, RgbaImage};
    let colours = [RED, GREEN, BLUE];
    let mut encoder = image::codecs::gif::GifEncoder::new(fs::File::create(path).unwrap());
    if let Some(repeat) = repeat {
        encoder.set_repeat(repeat).unwrap();
    }
    for (i, ms) in delays.iter().enumerate() {
        let [r, g, b] = colours[i % colours.len()];
        let pixels = RgbaImage::from_pixel(side, side, Rgba([r, g, b, 255]));
        let delay = Delay::from_numer_denom_ms(*ms, 1);
        encoder
            .encode_frame(Frame::from_parts(pixels, 0, 0, delay))
            .unwrap();
    }
}

/// What the WebP at `path` shows: whether it is animated, its loop count,
/// and each frame's size, delay in ms and middle pixel.
fn webp_animation(path: &Path) -> (bool, image::metadata::LoopCount, Vec<WebpFrame>) {
    use image::codecs::webp::WebPDecoder;
    use image::AnimationDecoder;
    let decoder = WebPDecoder::new(std::io::BufReader::new(fs::File::open(path).unwrap())).unwrap();
    let (animated, loops) = (decoder.has_animation(), decoder.loop_count());
    let frames = decoder
        .into_frames()
        .map(|frame| {
            let frame = frame.unwrap();
            let delay = std::time::Duration::from(frame.delay()).as_millis();
            let (w, h) = frame.buffer().dimensions();
            let middle = frame.buffer().get_pixel(w / 2, h / 2).0;
            WebpFrame {
                size: (w, h),
                delay_ms: delay as u32,
                middle: [middle[0], middle[1], middle[2]],
            }
        })
        .collect();
    (animated, loops, frames)
}

struct WebpFrame {
    size: (u32, u32),
    delay_ms: u32,
    middle: [u8; 3],
}

#[test]
fn an_animated_gif_stays_animated() {
    use image::codecs::gif::Repeat;
    use image::metadata::LoopCount;
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("spin.gif");
    // 10 ms is shorter than a browser shows a frame, so it becomes 100 ms.
    animated_gif(&input, 16, &[100, 200, 10], Some(Repeat::Infinite));

    let out = convert(&input, "webp").unwrap();
    let (animated, loops, frames) = webp_animation(&out);
    assert!(animated, "the WebP is a still");
    assert!(
        matches!(loops, LoopCount::Infinite),
        "does not loop for ever"
    );
    let delays: Vec<u32> = frames.iter().map(|f| f.delay_ms).collect();
    assert_eq!(delays, [100, 200, 100]);
    // Each frame is its own picture, and the WebP is lossy.
    for (frame, want) in frames.iter().zip([RED, GREEN, BLUE]) {
        assert_eq!(frame.size, (16, 16));
        assert!(
            near(frame.middle, want, 40),
            "{:?} is not {want:?}",
            frame.middle
        );
    }
    let bytes = fs::read(&out).unwrap();
    assert!(has_chunk(&bytes, b"ANIM") && has_chunk(&bytes, b"ANMF"));
    assert!(!has_chunk(&bytes, b"VP8L"), "a frame is lossless");
}

#[test]
fn a_gif_that_repeats_a_set_number_of_times_plays_that_many_more() {
    use image::codecs::gif::Repeat;
    use image::metadata::LoopCount;
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("twice_more.gif");
    animated_gif(&input, 16, &[100, 100], Some(Repeat::Finite(2)));

    let (animated, loops, frames) = webp_animation(&convert(&input, "webp").unwrap());
    assert!(animated && frames.len() == 2);
    // A GIF counts the repeats after the first play; a WebP counts the plays.
    assert!(
        matches!(loops, LoopCount::Finite(n) if n.get() == 3),
        "wrong loop count"
    );
}

#[test]
fn a_gif_with_no_loop_extension_plays_once() {
    use image::metadata::LoopCount;
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("once.gif");
    animated_gif(&input, 16, &[100, 100], None);

    let (animated, loops, frames) = webp_animation(&convert(&input, "webp").unwrap());
    assert!(animated && frames.len() == 2);
    // Browsers play such a GIF once; a WebP that says "for ever" would not.
    assert!(
        matches!(loops, LoopCount::Finite(n) if n.get() == 1),
        "wrong loop count"
    );
}

#[test]
fn a_gif_that_loops_for_ever_still_does() {
    use image::codecs::gif::Repeat;
    use image::metadata::LoopCount;
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("always.gif");
    animated_gif(&input, 16, &[100, 100], Some(Repeat::Infinite));

    let (_, loops, _) = webp_animation(&convert(&input, "webp").unwrap());
    assert!(matches!(loops, LoopCount::Infinite));
}

#[test]
fn a_still_gif_stays_a_still_webp() {
    use image::codecs::gif::Repeat;
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("still.gif");
    animated_gif(&input, 16, &[100], Some(Repeat::Infinite));

    let out = convert(&input, "webp").unwrap();
    let (animated, _, _) = webp_animation(&out);
    assert!(!animated, "the WebP is animated");
    assert!(!has_chunk(&fs::read(&out).unwrap(), b"ANMF"));
    assert_eq!(image::open(&out).unwrap().dimensions(), (16, 16));
}

#[test]
fn the_size_cap_applies_to_every_frame() {
    use image::codecs::gif::Repeat;
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("big.gif");
    animated_gif(&input, 16, &[100, 100, 100], Some(Repeat::Infinite));

    let mut options = Options::default();
    options.quality.size = kiln_core::Limit::Max(8);
    let out = Kiln::new().convert(&input, "webp", &options).unwrap();
    let (animated, _, frames) = webp_animation(&out);
    assert!(animated);
    assert_eq!(frames.len(), 3);
    for frame in &frames {
        assert_eq!(frame.size, (8, 8));
    }
}

/// The picture XObjects of the PDF at `path`: their dictionary and raw bytes.
fn pdf_images(path: &Path) -> Vec<lopdf::Stream> {
    let doc = lopdf::Document::load(path).unwrap();
    doc.objects
        .values()
        .filter_map(|o| match o {
            lopdf::Object::Stream(s)
                if s.dict.get(b"Subtype").and_then(lopdf::Object::as_name).ok()
                    == Some(b"Image".as_slice()) =>
            {
                Some(s.clone())
            }
            _ => None,
        })
        .collect()
}

#[test]
fn an_upright_jpeg_page_embeds_its_own_bytes_and_a_turned_one_does_not() {
    let dir = tempfile::tempdir().unwrap();

    let upright = dir.path().join("upright.jpg");
    jpeg_with(&upright, &left_red(8, 4), 1, None, 95);
    let pdf = convert(&upright, "pdf").unwrap();
    let images = pdf_images(&pdf);
    assert_eq!(images.len(), 1);
    assert_eq!(
        images[0]
            .dict
            .get(b"Filter")
            .and_then(lopdf::Object::as_name)
            .unwrap(),
        b"DCTDecode"
    );
    assert_eq!(images[0].content, fs::read(&upright).unwrap());

    let turned = dir.path().join("turned.jpg");
    jpeg_with(&turned, &left_red(8, 4), 6, None, 95);
    let pdf = convert(&turned, "pdf").unwrap();
    let images = pdf_images(&pdf);
    assert_eq!(images.len(), 1);
    let image = &images[0];
    assert_ne!(
        image
            .dict
            .get(b"Filter")
            .and_then(lopdf::Object::as_name)
            .unwrap(),
        b"DCTDecode",
        "the sideways bytes were embedded"
    );
    let size = |key: &[u8]| image.dict.get(key).and_then(lopdf::Object::as_i64).unwrap();
    assert_eq!((size(b"Width"), size(b"Height")), (4, 8));
    // Red is at the top of the page.
    let pixels = image.decompressed_content().unwrap();
    assert!(near([pixels[0], pixels[1], pixels[2]], RED, 40));
    let last = pixels.len() - 3;
    assert!(near(
        [pixels[last], pixels[last + 1], pixels[last + 2]],
        BLUE,
        40
    ));
}

// --- HEIC and AVIF, read through Windows (WIC) ------------------------------

/// Options with no FFmpeg anywhere, so only Windows can read the picture.
fn without_ffmpeg(dir: &Path) -> Options {
    Options {
        engines: kiln_core::Engines {
            ffmpeg: Some(dir.join("no-such-dir").join("ffmpeg.exe")),
            ..kiln_core::Engines::default()
        },
        ..Options::default()
    }
}

/// `input` converted to `to` with Windows alone, or `None` with the reason
/// printed when this PC has no decoder for it.
fn through_windows(test: &str, input: &Path, to: &str) -> Option<PathBuf> {
    let dir = input.parent().unwrap();
    match Kiln::new().convert(input, to, &without_ffmpeg(dir)) {
        Ok(out) => Some(out),
        Err(Error::NeedsExtension { message, .. }) => {
            eprintln!("{test}: skipped, Windows has no decoder here ({message})");
            None
        }
        Err(e) => panic!("{test}: {e:?}"),
    }
}

#[test]
fn heic_becomes_jpg_and_png_through_windows() {
    let (_dir, input) = staged("sample.heic");
    for (to, format) in [("jpg", ImageFormat::Jpeg), ("png", ImageFormat::Png)] {
        let Some(out) = through_windows("heic", &input, to) else {
            return;
        };
        let bytes = fs::read(&out).unwrap();
        assert_eq!(image::guess_format(&bytes).unwrap(), format, "{to}");
        let img = image::load_from_memory_with_format(&bytes, format).unwrap();
        assert_eq!(img.dimensions(), (64, 48), "{to}");
        // The gradient is (x*4, y*5, 128), through HEVC.
        let p = img.to_rgb8().get_pixel(40, 20).0;
        assert!(
            close(p[0], 160, 24) && close(p[1], 100, 24) && close(p[2], 128, 24),
            "{to}: {p:?}"
        );
    }
}

#[test]
fn avif_becomes_png_through_windows() {
    let (_dir, input) = staged("sample.avif");
    let Some(out) = through_windows("avif", &input, "png") else {
        return;
    };
    let img = image::open(&out).unwrap();
    assert_eq!(img.dimensions(), (64, 48));
    let p = img.to_rgb8().get_pixel(40, 20).0;
    assert!(
        close(p[0], 160, 24) && close(p[1], 100, 24) && close(p[2], 128, 24),
        "{p:?}"
    );
}

#[test]
fn a_heic_s_colour_profile_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let profile = display_p3();
    let input = dir.path().join("wide.heic");
    let sample = fs::read(fixture("sample.heic")).unwrap();
    fs::write(&input, pictures::heic_with(&sample, None, Some(&profile))).unwrap();
    for to in ["jpg", "png"] {
        let Some(out) = through_windows("heic profile", &input, to) else {
            return;
        };
        assert_eq!(
            icc_of(&out).as_deref(),
            Some(profile.as_slice()),
            "{to} lost the profile"
        );
    }
    // The profile is carried, not applied: the pixels are the plain
    // fixture's.
    let plain = dir.path().join("plain.heic");
    fs::write(&plain, &sample).unwrap();
    let (Some(wide), Some(plain)) = (
        through_windows("heic profile", &input, "tiff"),
        through_windows("heic profile", &plain, "tiff"),
    ) else {
        return;
    };
    assert_eq!(
        image::open(&wide).unwrap().to_rgb8(),
        image::open(&plain).unwrap().to_rgb8()
    );
}

#[test]
fn a_turned_heic_comes_out_upright() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("turned.heic");
    let sample = fs::read(fixture("sample.heic")).unwrap();
    // Stored 64 x 48, shown a quarter turn anticlockwise: 48 x 64, with the
    // stored top-right corner (red 252, green 0) at the top left.
    fs::write(&input, pictures::heic_with(&sample, Some(1), None)).unwrap();
    let Some(out) = through_windows("turned heic", &input, "png") else {
        return;
    };
    let img = image::open(&out).unwrap().to_rgb8();
    assert_eq!(img.dimensions(), (48, 64));
    let top_left = img.get_pixel(1, 1).0;
    assert!(top_left[0] > 220 && top_left[1] < 40, "{top_left:?}");
    // The stored top-left (black and blue) is at the bottom left.
    let bottom_left = img.get_pixel(1, 62).0;
    assert!(
        bottom_left[0] < 40 && bottom_left[1] < 40,
        "{bottom_left:?}"
    );
    assert_eq!(
        orientation_of(&out),
        image::metadata::Orientation::NoTransforms
    );
}

/// True when `img`, 48 x 64, is the sample gradient (x*4, y*5, 128)
/// turned a quarter clockwise: the stored bottom left (green) at the top
/// left, the stored top left (dark) at the top right.
fn turned_clockwise(img: &image::RgbImage) -> bool {
    let top_left = img.get_pixel(1, 1).0;
    let top_right = img.get_pixel(46, 1).0;
    img.dimensions() == (48, 64)
        && top_left[1] > 200
        && top_left[0] < 40
        && top_right[0] < 40
        && top_right[1] < 40
}

#[test]
fn a_phone_heic_with_irot_and_exif_is_turned_once() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("phone.heic");
    let sample = fs::read(fixture("sample.heic")).unwrap();
    // As phones write a portrait photo: the turn in `irot` (three quarters
    // anticlockwise) and the same turn in an Exif item (orientation 6).
    let file = pictures::heif(&pictures::Heif {
        quarters: Some(3),
        exif_orientation: Some(6),
        ..pictures::Heif::new(&sample)
    });
    fs::write(&input, file).unwrap();
    let Some(out) = through_windows("phone heic", &input, "png") else {
        return;
    };
    let img = image::open(&out).unwrap().to_rgb8();
    assert!(
        turned_clockwise(&img),
        "{:?} {:?} {:?}",
        img.dimensions(),
        img.get_pixel(1, 1),
        img.get_pixel(46, 1)
    );
    let facts = kiln_core::inspect(
        &Kiln::new(),
        &input,
        &without_ffmpeg(dir.path()),
        &kiln_core::CancelToken::new(),
    )
    .unwrap();
    let p = facts.picture.unwrap();
    assert_eq!((p.width, p.height), (48, 64));
}

#[test]
fn a_transparent_avif_keeps_its_alpha_in_a_png() {
    let dir = tempfile::tempdir().unwrap();
    let avif = dir.path().join("alpha.avif");
    // Made by FFmpeg. Windows' decoder drops alpha, so FFmpeg reads it too.
    let made = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi"])
        .args(["-i", "color=c=red@0.5:s=64x48,format=yuva420p"])
        .args(["-filter_complex", "[0:v]split[c][a];[a]alphaextract[al]"])
        .args(["-map", "[c]", "-map", "[al]"])
        .args([
            "-frames:v",
            "1",
            "-c:v",
            "libaom-av1",
            "-still-picture",
            "1",
        ])
        .arg(&avif)
        .output()
        .is_ok_and(|o| o.status.success());
    if !made {
        eprintln!("avif alpha: skipped, FFmpeg is missing or can't encode AVIF");
        return;
    }
    let out = convert(&avif, "png").unwrap();
    let img = image::open(&out).unwrap();
    assert!(img.color().has_alpha(), "{:?}", img.color());
    let p = img.to_rgba8().get_pixel(32, 24).0;
    assert!((100..=155).contains(&p[3]), "half transparent: {p:?}");
    assert!(p[0] > 200 && p[1] < 60 && p[2] < 60, "red: {p:?}");

    // Without FFmpeg, Windows would read it opaque, so it is refused, and
    // nothing is saved with its background filled in.
    let alone = dir.path().join("alone");
    fs::create_dir(&alone).unwrap();
    let copy = alone.join("alpha.avif");
    fs::copy(&avif, &copy).unwrap();
    let err = Kiln::new()
        .convert(&copy, "png", &without_ffmpeg(&alone))
        .unwrap_err();
    assert!(matches!(&err, Error::Decode { .. }), "{err:?}");
    assert_eq!(
        kiln_core::JobError::from(err).message,
        "Needs FFmpeg to keep this AVIF's transparency."
    );
    assert_eq!(files_in(&alone), ["alpha.avif"]);
}

#[test]
fn a_grid_heic_or_avif_is_read_whole_through_windows() {
    for name in ["sample.heic", "sample.avif"] {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join(format!("grid-{name}"));
        let sample = fs::read(fixture(name)).unwrap();
        let file = pictures::heif(&pictures::Heif {
            grid: true,
            ..pictures::Heif::new(&sample)
        });
        fs::write(&input, file).unwrap();
        let Some(out) = through_windows("grid", &input, "png") else {
            continue;
        };
        let img = image::open(&out).unwrap().to_rgb8();
        assert_eq!(img.dimensions(), (128, 48), "{name}");
        for x in [40, 104] {
            let p = img.get_pixel(x, 20).0;
            assert!(
                pictures::near(p, [160, 100, 128], 24),
                "{name} at {x}: {p:?}"
            );
        }
    }
}

#[test]
fn a_heic_windows_refuses_goes_to_ffmpeg() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("odd.heic");
    let sample = fs::read(fixture("sample.heic")).unwrap();
    // No `pitm` box: Windows' decoder refuses the file with an error that
    // isn't "no decoder".
    let mut file = pictures::heif(&pictures::Heif::new(&sample));
    let at = file.windows(4).position(|w| w == b"pitm").unwrap();
    file[at] = b'x';
    fs::write(&input, file).unwrap();
    if !windows_reads_heic(dir.path()) {
        return;
    }
    // Without FFmpeg: Windows' own error, with its code in the details.
    let err = Kiln::new()
        .convert(&input, "png", &without_ffmpeg(dir.path()))
        .unwrap_err();
    assert!(matches!(&err, Error::Decode { .. }), "{err:?}");
    assert!(err.to_string().contains("(Windows error 0x"), "{err}");
    // With FFmpeg, FFmpeg is asked.
    let Some(ffmpeg) =
        kiln_core::find_on_path(&std::env::var_os("PATH").unwrap_or_default(), "ffmpeg")
    else {
        eprintln!("a_heic_windows_refuses_goes_to_ffmpeg: FFmpeg half skipped, not on PATH");
        return;
    };
    let with = Options {
        engines: kiln_core::Engines {
            ffmpeg: Some(ffmpeg),
            ..kiln_core::Engines::default()
        },
        ..Options::default()
    };
    let out = Kiln::new().convert(&input, "png", &with).unwrap();
    assert_eq!(image::open(&out).unwrap().dimensions(), (64, 48));
    assert_eq!(files_in(dir.path()), ["odd.heic", "odd.png"]);
}

/// True when Windows reads the sample HEIC on this PC; says so when not.
fn windows_reads_heic(dir: &Path) -> bool {
    let probe = dir.join("probe");
    fs::create_dir(&probe).unwrap();
    let input = probe.join("sample.heic");
    fs::copy(fixture("sample.heic"), &input).unwrap();
    let read = through_windows("windows reads heic", &input, "png").is_some();
    for name in files_in(&probe) {
        fs::remove_file(probe.join(name)).unwrap();
    }
    fs::remove_dir(&probe).unwrap();
    read
}

#[test]
fn a_png_named_heic_is_refused_before_windows_reads_it() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("photo.heic");
    fs::copy(fixture("photo.png"), &input).unwrap();
    let err = Kiln::new()
        .convert(&input, "jpg", &without_ffmpeg(dir.path()))
        .unwrap_err();
    assert!(
        matches!(&err, Error::WrongContent { code, .. } if code == "HEIC"),
        "{err:?}"
    );
    assert_eq!(files_in(dir.path()), ["photo.heic"]);
}

/// An AVIF of the colour `colour`, 64 x 48, made by FFmpeg in `pix_fmt`;
/// `None` when FFmpeg is missing or can't write AVIF.
fn made_avif(dir: &Path, name: &str, colour: &str, pix_fmt: &str) -> Option<PathBuf> {
    let avif = dir.join(name);
    let made = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi"])
        .arg("-i")
        .arg(format!("color=c={colour}:s=64x48"))
        .args(["-frames:v", "1", "-pix_fmt", pix_fmt])
        .args(["-c:v", "libaom-av1", "-still-picture", "1"])
        .arg(&avif)
        .output()
        .is_ok_and(|o| o.status.success());
    made.then_some(avif)
}

#[test]
fn a_10_bit_avif_has_the_colours_of_an_8_bit_one() {
    let dir = tempfile::tempdir().unwrap();
    let (Some(eight), Some(ten)) = (
        made_avif(dir.path(), "eight.avif", "0x9a6e3c", "yuv420p"),
        made_avif(dir.path(), "ten.avif", "0x9a6e3c", "yuv420p10le"),
    ) else {
        eprintln!("10-bit avif: skipped, FFmpeg is missing or can't encode AVIF");
        return;
    };
    let (Some(eight), Some(ten)) = (
        through_windows("8-bit avif", &eight, "png"),
        through_windows("10-bit avif", &ten, "png"),
    ) else {
        return;
    };
    // Windows may hand over a 10-bit picture at 8 or 16 bits a channel
    // (this PC's AV1 Video Extension gives 8); either way its colours are
    // the 8-bit picture's, with no change of gamma.
    let deep = image::open(&ten).unwrap();
    eprintln!("10-bit avif read as {:?}", deep.color());
    let a = image::open(&eight).unwrap().to_rgb8().get_pixel(32, 24).0;
    let b = deep.to_rgb8().get_pixel(32, 24).0;
    assert!(
        (0..3).all(|i| close(a[i], b[i], 24)),
        "8-bit {a:?}, 10-bit {b:?}"
    );
    assert!(
        close(a[0], 0x9a, 24) && close(a[1], 0x6e, 24) && close(a[2], 0x3c, 24),
        "{a:?}"
    );
}

#[test]
fn a_jpg_cut_short_is_refused_as_damaged_even_with_ffmpeg() {
    // D21 hands FFmpeg a JPG in a form Kiln's reader doesn't take, here a
    // lossless one. FFmpeg fills what it can't read of one cut short,
    // which would be saved as if it were whole (M4): it is told to stop
    // at the first error instead, and the reader's error stands.
    let test = "a_jpg_cut_short_is_refused_as_damaged_even_with_ffmpeg";
    if !support::have_ffmpeg(test) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let whole = dir.path().join("whole.jpg");
    let made = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-f", "lavfi", "-i"])
        .arg("testsrc2=size=640x480:rate=1")
        .args(["-frames:v", "1", "-c:v", "ljpeg", "-pix_fmt", "bgr24"])
        .arg(&whole)
        .status()
        .unwrap();
    assert!(made.success());
    let bytes = fs::read(&whole).unwrap();
    fs::remove_file(&whole).unwrap();
    let cut = dir.path().join("cut.jpg");
    fs::write(&cut, &bytes[..bytes.len() / 2]).unwrap();
    let err = Kiln::new()
        .convert(&cut, "png", &Options::default())
        .unwrap_err();
    // Since D26 it is refused before any reader sees it: it has no end
    // marker.
    assert!(matches!(err, Error::CutShort { .. }), "{err:?}");
    assert_eq!(files_in(dir.path()), ["cut.jpg"]);
}

/// `jpeg` with an Exif block asking for `orientation` put straight after
/// its start.
fn with_exif_orientation(jpeg: &[u8], orientation: u16) -> Vec<u8> {
    let body = [
        b"Exif\0\0".as_slice(),
        &pictures::exif_orientation(orientation),
    ]
    .concat();
    let len = u16::try_from(body.len() + 2).unwrap();
    [
        &jpeg[..2],
        &[0xFF, 0xE1],
        &len.to_be_bytes(),
        &body,
        &jpeg[2..],
    ]
    .concat()
}

#[test]
fn a_lossless_jpg_read_by_ffmpeg_is_turned_as_its_exif_asks() {
    let test = "a_lossless_jpg_read_by_ffmpeg_is_turned_as_its_exif_asks";
    if !support::have_ffmpeg(test) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    // A lossless JPEG (SOF3), which only FFmpeg reads, of a picture
    // marked in its top-left corner.
    let source = dir.path().join("source.png");
    corner_marked(64, 40).save(&source).unwrap();
    let plain = dir.path().join("plain.jpg");
    let made = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&source)
        .args(["-c:v", "ljpeg", "-pix_fmt", "bgr24"])
        .arg(&plain)
        .status()
        .unwrap();
    assert!(made.success());
    let turned = dir.path().join("turned.jpg");
    fs::write(
        &turned,
        with_exif_orientation(&fs::read(&plain).unwrap(), 6),
    )
    .unwrap();
    assert!(image::open(&turned).is_err(), "Kiln's reader takes it now");
    let out = Kiln::new()
        .convert(&turned, "png", &Options::default())
        .unwrap();
    let img = image::open(&out).unwrap().to_rgb8();
    // Orientation 6 turns it a quarter clockwise: the marked corner goes
    // to the top right, and the red half to the top.
    assert_eq!(img.dimensions(), (40, 64));
    assert!(
        near(img.get_pixel(39, 0).0, GREEN, 8),
        "{:?}",
        img.get_pixel(39, 0)
    );
    assert!(
        near(img.get_pixel(20, 10).0, RED, 8),
        "{:?}",
        img.get_pixel(20, 10)
    );
    assert!(
        near(img.get_pixel(20, 54).0, BLUE, 8),
        "{:?}",
        img.get_pixel(20, 54)
    );
}
