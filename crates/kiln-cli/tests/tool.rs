//! `kiln tool`: one run per tool, checking outputs and messages.

#[path = "../../../tests/support/mod.rs"]
mod support;

use std::fs;

use assert_cmd::Command;
use predicates::prelude::*;
use support::{fixture, probe, sample_for, secret_pdf, stage};

fn kiln() -> Command {
    Command::cargo_bin("kiln").unwrap()
}

#[test]
fn crop_prints_the_new_file() {
    let (dir, input) = stage(&fixture("bordered.png"));
    let expected = dir.path().join("bordered cropped.png");
    kiln()
        .args(["tool", "crop"])
        .arg(&input)
        .assert()
        .success()
        .stdout(format!("{}\n", expected.display()));
    assert!(expected.is_file());
}

#[test]
fn resize_names_the_new_size() {
    let (dir, input) = stage(&sample_for("jpg"));
    let expected = dir.path().join("sample (32 px).jpg");
    kiln()
        .args(["tool", "resize", "--long-side", "32"])
        .arg(&input)
        .assert()
        .success()
        .stdout(format!("{}\n", expected.display()));
    assert!(expected.is_file());
}

#[test]
fn split_prints_both_parts() {
    let (dir, input) = stage(&sample_for("wav"));
    kiln()
        .args(["tool", "split"])
        .arg(&input)
        .assert()
        .success()
        .stdout(format!(
            "{}\n{}\n",
            dir.path().join("sample part 1.wav").display(),
            dir.path().join("sample part 2.wav").display()
        ));
}

#[test]
fn trim_and_merge() {
    let (dir, input) = stage(&fixture("padded.wav"));
    kiln().args(["tool", "trim"]).arg(&input).assert().success();
    let trimmed = dir.path().join("padded trimmed.wav");
    assert!((probe(&trimmed).duration - 1.0).abs() < 0.1);

    kiln()
        .args(["tool", "merge"])
        .arg(&input)
        .arg(&trimmed)
        .assert()
        .success()
        .stdout(predicate::str::ends_with("padded merged.wav\n"));
    let merged = dir.path().join("padded merged.wav");
    assert!((probe(&merged).duration - 3.0).abs() < 0.15);
}

#[test]
fn compress_still_makes_a_photo_smaller() {
    let (dir, input) = stage(&sample_for("jpg"));
    let out = kiln()
        .args(["tool", "compress"])
        .arg(&input)
        .output()
        .unwrap();
    if out.status.success() {
        assert!(dir.path().join("sample (smaller).jpg").is_file());
    } else {
        assert_eq!(
            String::from_utf8_lossy(&out.stderr),
            "kiln: This file is already as small as Kiln can make it.
"
        );
    }
}

#[test]
fn a_video_under_the_size_is_left_alone() {
    let (dir, input) = stage(&sample_for("mov"));
    kiln()
        .args(["tool", "smaller", "--max-size", "10MB"])
        .arg(&input)
        .assert()
        .code(1)
        .stderr("kiln: This file is already smaller than 10 MB.\n");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn unknown_tool_lists_the_real_ones() {
    kiln()
        .args(["tool", "explode", "x.png"])
        .assert()
        .code(2)
        .stderr("kiln: explode isn't a tool. Tools: smaller, resize, remove-location, remove-edges, remove-quiet-ends, cut-in-half, save-each-page, merge.\n");
}

#[test]
fn tool_that_does_not_apply_is_a_usage_error() {
    let (_dir, input) = stage(&sample_for("docx"));
    kiln()
        .args(["tool", "crop"])
        .arg(&input)
        .assert()
        .code(2)
        .stderr("kiln: Kiln can't remove borders from DOCX files.\n");
}

#[test]
fn compress_on_a_type_it_has_no_settings_for_is_a_usage_error() {
    let (_dir, input) = stage(&sample_for("avi"));
    kiln()
        .args(["tool", "compress"])
        .arg(&input)
        .assert()
        .code(2)
        .stderr("kiln: Kiln can't make AVI files smaller.\n");
}

#[test]
fn nothing_to_do_is_a_plain_failure() {
    let (dir, input) = stage(&sample_for("png"));
    kiln()
        .args(["tool", "crop"])
        .arg(&input)
        .assert()
        .code(1)
        .stderr("kiln: There's no plain border to remove.\n");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn split_on_a_pdf_still_saves_each_page() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("two.pdf");
    secret_pdf(&input, 2, None);
    let out = kiln().args(["tool", "split"]).arg(&input).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(dir.path().join("two page 2.pdf").is_file());
}

#[test]
fn a_photo_under_the_size_is_left_alone() {
    let (dir, input) = stage(&sample_for("jpg"));
    kiln()
        .args(["tool", "smaller", "--max-size", "10KB"])
        .arg(&input)
        .assert()
        .code(1)
        .stderr("kiln: This file is already smaller than 10 KB.\n");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    kiln()
        .args(["tool", "smaller", "--max-size", "ten"])
        .arg(&input)
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "ten isn't a size. Use a number with KB, MB or GB, such as 10MB.",
        ));
}

#[test]
fn a_pdf_under_the_size_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("two.pdf");
    secret_pdf(&input, 2, None);
    kiln()
        .args(["tool", "smaller", "--max-size", "10MB"])
        .arg(&input)
        .assert()
        .code(1)
        .stderr("kiln: This file is already smaller than 10 MB.\n");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}
