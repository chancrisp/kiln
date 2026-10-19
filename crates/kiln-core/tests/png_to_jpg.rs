use std::fs;
use std::path::{Path, PathBuf};

use image::{ColorType, GenericImageView, ImageFormat};
use kiln_core::{png_to_jpg, Error, JpegOptions};
use tempfile::TempDir;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

/// Copy a fixture into a fresh temp dir so outputs land somewhere disposable.
fn staged(name: &str) -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    fs::copy(fixture(name), &path).unwrap();
    (dir, path)
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
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
            quality: 10,
            ..JpegOptions::default()
        },
    )
    .unwrap();
    let high = png_to_jpg(
        &input,
        &JpegOptions {
            quality: 100,
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
            quality: q,
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
    assert!(matches!(err, Error::NotPng(_)));
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
        Error::NotPng(dir.path().join("a.png")),
        Error::OutputDirNotFound(dir.path().to_path_buf()),
        Error::InvalidQuality(0),
    ];
    for e in errors {
        let msg = e.to_string();
        assert!(!msg.contains('!'), "exclamation mark in {msg:?}");
        assert!(msg.ends_with('.'), "{msg:?}");
    }
}
