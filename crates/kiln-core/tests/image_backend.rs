//! The image backend, through the full `Kiln::convert` path.

#[path = "../../../tests/support/mod.rs"]
mod support;

use std::fs;
use std::path::{Path, PathBuf};

use image::{ColorType, GenericImageView, ImageFormat};
use kiln_core::{Error, Kiln, Options};
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
    for to in ["webp", "bmp", "tiff"] {
        let out = convert(&input, to).unwrap();
        assert_eq!(image::open(&out).unwrap().to_rgb8(), src, "{to}");
    }
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
        assert!(close(p[1], 155, 4), "{to}: {p:?}");
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
