//! `kiln suggest`: the printed suggestions for fixtures.

#[path = "../../../tests/support/mod.rs"]
mod support;

use std::fs;
use std::process::Output;

use assert_cmd::Command;
use support::fixture;
use tempfile::TempDir;

/// Copy the named fixtures into one fresh temp dir and run `kiln suggest`
/// on the copies.
fn suggest(names: &[&str]) -> Output {
    let pairs: Vec<(&str, &str)> = names.iter().map(|n| (*n, *n)).collect();
    suggest_as(&pairs)
}

/// Like `suggest`, staging each fixture under a name of its own:
/// (fixture, staged name).
fn suggest_as(files: &[(&str, &str)]) -> Output {
    let dir = TempDir::new().unwrap();
    let mut cmd = Command::cargo_bin("kiln").unwrap();
    cmd.arg("suggest");
    for (name, staged) in files {
        let path = dir.path().join(staged);
        fs::copy(fixture(name), &path).unwrap();
        cmd.arg(&path);
    }
    cmd.output().unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn one_line_per_file_then_the_set() {
    let out = suggest(&["photo.png", "sample.mov"]);
    assert!(out.status.success());
    assert_eq!(
        stdout(&out),
        "photo.png    JPG  A photo. JPG keeps the look at about 4.2 KB.\n\
         sample.mov   MP4  Plays on any phone, PC or browser.\n"
    );
}

#[test]
fn a_file_with_nothing_to_suggest() {
    let out = suggest(&["sample.jpg"]);
    assert!(out.status.success());
    assert_eq!(stdout(&out), "sample.jpg   No suggestion.\n");
}

#[test]
fn two_pdfs_suggest_merge() {
    let out = stdout(&suggest_as(&[
        ("sample.pdf", "sample.pdf"),
        ("sample.pdf", "sample 2.pdf"),
    ]));
    assert!(
        out.ends_with("(all)          Merge  One PDF, in the order shown.\n"),
        "{out}"
    );
}

#[test]
fn a_scanned_pdf_gets_no_merge() {
    // The spec: a scanned PDF gets no suggestion, alone or in a set.
    let out = stdout(&suggest(&["sample.pdf", "scan.pdf"]));
    assert_eq!(
        out,
        "sample.pdf   No suggestion.\nscan.pdf     No suggestion.\n"
    );
}

#[test]
fn a_missing_file_exits_2() {
    let out = Command::cargo_bin("kiln")
        .unwrap()
        .args(["suggest", "nope.png"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "kiln: nope.png doesn't exist.\n"
    );
    assert!(out.stdout.is_empty());
}
