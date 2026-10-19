//! `kiln tool`: one run per tool, checking outputs and messages.

#[path = "../../../tests/support/mod.rs"]
mod support;

use std::fs;

use assert_cmd::Command;
use predicates::prelude::*;
use support::{fixture, probe, sample_for, stage};

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
fn compress_a_video() {
    let (dir, input) = stage(&sample_for("mov"));
    let out = kiln()
        .args(["tool", "compress"])
        .arg(&input)
        .output()
        .unwrap();
    if out.status.success() {
        assert!(dir.path().join("sample compressed.mov").is_file());
    } else {
        assert_eq!(
            String::from_utf8_lossy(&out.stderr),
            "kiln: This file is already as small as Kiln can make it.\n"
        );
    }
}

#[test]
fn unknown_tool_lists_the_real_ones() {
    kiln()
        .args(["tool", "explode", "x.png"])
        .assert()
        .code(2)
        .stderr("kiln: explode isn't a tool. Tools: compress, crop, trim, split, merge.\n");
}

#[test]
fn tool_that_does_not_apply_is_a_usage_error() {
    let (_dir, input) = stage(&sample_for("docx"));
    kiln()
        .args(["tool", "crop"])
        .arg(&input)
        .assert()
        .code(2)
        .stderr("kiln: Kiln can't crop DOCX files.\n");
}

#[test]
fn nothing_to_do_is_a_plain_failure() {
    let (dir, input) = stage(&sample_for("png"));
    kiln()
        .args(["tool", "crop"])
        .arg(&input)
        .assert()
        .code(1)
        .stderr("kiln: There's no plain border to crop.\n");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}
