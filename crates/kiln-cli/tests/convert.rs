use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use image::{GenericImageView, ImageFormat};
use predicates::prelude::*;
use tempfile::TempDir;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

fn staged(name: &str) -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    fs::copy(fixture(name), &path).unwrap();
    (dir, path)
}

fn kiln() -> Command {
    Command::cargo_bin("kiln").unwrap()
}

fn load_jpeg(path: &Path) -> image::DynamicImage {
    let bytes = fs::read(path).unwrap();
    assert_eq!(image::guess_format(&bytes).unwrap(), ImageFormat::Jpeg);
    image::load_from_memory_with_format(&bytes, ImageFormat::Jpeg).unwrap()
}

/// Every supported pair, with each spelling of the target users might type.
#[test]
fn png_to_jpg_for_every_spelling() {
    for format in ["jpg", "JPG", "jpeg", ".jpg"] {
        let (dir, input) = staged("rgb_gradient.png");
        let expected = dir.path().join("rgb_gradient.jpg");
        kiln()
            .args(["convert"])
            .arg(&input)
            .arg(format)
            .assert()
            .success()
            .stdout(format!("{}\n", expected.display()))
            .stderr("");
        let img = load_jpeg(&expected);
        assert_eq!(img.dimensions(), (64, 48), "format {format}");
    }
}

#[test]
fn png_fixtures_all_convert() {
    for name in [
        "rgb_gradient.png",
        "rgba_half_transparent.png",
        "gray8.png",
        "rgb16.png",
    ] {
        let (dir, input) = staged(name);
        kiln()
            .arg("convert")
            .arg(&input)
            .arg("jpg")
            .assert()
            .success();
        let stem = name.trim_end_matches(".png");
        let src = image::open(&input).unwrap();
        let out = load_jpeg(&dir.path().join(format!("{stem}.jpg")));
        assert_eq!(out.dimensions(), src.dimensions(), "{name}");
    }
}

#[test]
fn second_run_does_not_overwrite() {
    let (dir, input) = staged("rgb_gradient.png");
    kiln()
        .arg("convert")
        .arg(&input)
        .arg("jpg")
        .assert()
        .success();
    kiln()
        .arg("convert")
        .arg(&input)
        .arg("jpg")
        .assert()
        .success()
        .stdout(predicate::str::ends_with("rgb_gradient (1).jpg\n"));
    assert!(dir.path().join("rgb_gradient.jpg").is_file());
    assert!(dir.path().join("rgb_gradient (1).jpg").is_file());
}

#[test]
fn out_dir_flag_is_respected() {
    let (_src, input) = staged("rgb_gradient.png");
    let dest = tempfile::tempdir().unwrap();
    kiln()
        .arg("convert")
        .arg(&input)
        .arg("jpg")
        .arg("--out-dir")
        .arg(dest.path())
        .assert()
        .success();
    load_jpeg(&dest.path().join("rgb_gradient.jpg"));
}

#[test]
fn quality_flag_changes_size() {
    let (dir, input) = staged("rgb_gradient.png");
    let a = tempfile::tempdir_in(dir.path()).unwrap();
    let b = tempfile::tempdir_in(dir.path()).unwrap();
    for (q, d) in [("5", &a), ("100", &b)] {
        kiln()
            .arg("convert")
            .arg(&input)
            .arg("jpg")
            .args(["-q", q, "-o"])
            .arg(d.path())
            .assert()
            .success();
    }
    let small = fs::metadata(a.path().join("rgb_gradient.jpg"))
        .unwrap()
        .len();
    let large = fs::metadata(b.path().join("rgb_gradient.jpg"))
        .unwrap()
        .len();
    assert!(small < large);
}

#[test]
fn quality_out_of_range_is_a_usage_error() {
    let (_dir, input) = staged("rgb_gradient.png");
    kiln()
        .arg("convert")
        .arg(&input)
        .args(["jpg", "-q", "0"])
        .assert()
        .code(2);
}

#[test]
fn unsupported_target_names_the_options() {
    let (dir, input) = staged("rgb_gradient.png");
    kiln()
        .arg("convert")
        .arg(&input)
        .arg("mp4")
        .assert()
        .code(2)
        .stdout("")
        .stderr("kiln: Can't convert PNG to MP4. Supported formats: BMP, GIF, JPG, TIFF, WEBP.\n");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn unknown_target_is_a_usage_error() {
    let (_dir, input) = staged("rgb_gradient.png");
    kiln()
        .arg("convert")
        .arg(&input)
        .arg("heic")
        .assert()
        .code(2)
        .stderr("kiln: HEIC isn't a format Kiln knows.\n");
}

#[test]
fn formats_lists_compatible_outputs() {
    kiln()
        .args(["formats", "song.flac"])
        .assert()
        .success()
        .stdout("M4A\nMP3\nOGG\nOPUS\nWAV\n");
    kiln()
        .args(["formats", "notes.xyz"])
        .assert()
        .code(2)
        .stderr("kiln: Kiln can't convert XYZ files.\n");
}

#[test]
fn missing_input_fails_with_a_plain_message() {
    let dir = tempfile::tempdir().unwrap();
    kiln()
        .arg("convert")
        .arg(dir.path().join("ghost.png"))
        .arg("jpg")
        .assert()
        .code(1)
        .stderr(predicate::str::ends_with("ghost.png doesn't exist.\n"));
}

#[test]
fn non_png_input_is_rejected() {
    let (dir, input) = staged("not_really.png");
    kiln()
        .arg("convert")
        .arg(&input)
        .arg("jpg")
        .assert()
        .code(1)
        .stderr(predicate::str::ends_with("isn't a PNG file.\n"));
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn corrupt_input_fails_and_leaves_nothing() {
    let (dir, input) = staged("truncated.png");
    kiln()
        .arg("convert")
        .arg(&input)
        .arg("jpg")
        .assert()
        .code(1)
        .stderr(predicate::str::starts_with("kiln: Couldn't read the image"));
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn messages_never_shout() {
    let (_dir, input) = staged("rgb_gradient.png");
    for args in [vec!["webp"], vec!["jpg"]] {
        kiln()
            .arg("convert")
            .arg(&input)
            .args(&args)
            .assert()
            .stderr(predicate::str::contains("!").not())
            .stdout(predicate::str::contains("!").not());
    }
}

#[test]
fn help_lists_convert() {
    kiln()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("convert"));
}
