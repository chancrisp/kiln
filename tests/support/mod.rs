//! Test helpers shared by kiln-core and kiln-cli. Included with
//! `#[path = "../../../tests/support/mod.rs"] mod support;`.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

/// The fixture used as input for an extension.
pub fn sample_for(ext: &str) -> PathBuf {
    match ext {
        "png" => fixture("rgb_gradient.png"),
        _ => fixture(&format!("sample.{ext}")),
    }
}

/// Copy a fixture into a fresh temp dir so outputs land somewhere disposable.
pub fn stage(src: &Path) -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(src.file_name().unwrap());
    fs::copy(src, &path).unwrap();
    (dir, path)
}

pub fn files_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// What ffprobe sees in a media file.
pub struct Probe {
    pub format_name: String,
    pub duration: f64,
    pub audio: usize,
    pub video: usize,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

pub fn probe(path: &Path) -> Probe {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-show_entries"])
        .arg("format=format_name,duration:stream=codec_type,width,height")
        .args(["-of", "default=noprint_wrappers=1"])
        .arg(path)
        .output()
        .expect("ffprobe runs");
    assert!(out.status.success(), "ffprobe failed on {}", path.display());
    let text = String::from_utf8_lossy(&out.stdout);
    let mut p = Probe {
        format_name: String::new(),
        duration: 0.0,
        audio: 0,
        video: 0,
        width: None,
        height: None,
    };
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        match k {
            "format_name" => p.format_name = v.to_owned(),
            "duration" => p.duration = v.parse().unwrap_or(0.0),
            "codec_type" if v == "audio" => p.audio += 1,
            "codec_type" if v == "video" => p.video += 1,
            "width" => p.width = v.parse().ok(),
            "height" => p.height = v.parse().ok(),
            _ => {}
        }
    }
    p
}

/// Assert that `path` really is a file of type `ext`, with the basic
/// properties of the one-second, 64x48 fixtures.
pub fn assert_output(path: &Path, ext: &str) {
    let bytes = fs::read(path).unwrap();
    assert!(!bytes.is_empty(), "{} is empty", path.display());
    let ctx = path.display();
    match ext {
        "jpg" => {
            assert_eq!(&bytes[..3], &[0xFF, 0xD8, 0xFF], "{ctx}");
        }
        "wav" | "mp3" | "flac" | "ogg" | "opus" | "m4a" => {
            match ext {
                "wav" => assert!(
                    bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WAVE",
                    "{ctx}"
                ),
                "flac" => assert!(bytes.starts_with(b"fLaC"), "{ctx}"),
                "ogg" => assert!(
                    bytes.starts_with(b"OggS") && contains(&bytes, b"vorbis"),
                    "{ctx}"
                ),
                "opus" => assert!(
                    bytes.starts_with(b"OggS") && contains(&bytes, b"OpusHead"),
                    "{ctx}"
                ),
                "m4a" => assert_eq!(&bytes[4..8], b"ftyp", "{ctx}"),
                "mp3" => assert!(
                    bytes.starts_with(b"ID3") || (bytes[0] == 0xFF && bytes[1] & 0xE0 == 0xE0),
                    "{ctx}"
                ),
                _ => unreachable!(),
            }
            let p = probe(path);
            assert_eq!(p.video, 0, "{ctx} has a video stream");
            assert_eq!(p.audio, 1, "{ctx} audio streams");
            assert!(
                (p.duration - 1.0).abs() < 0.15,
                "{ctx} lasts {}",
                p.duration
            );
        }
        "mp4" | "mov" | "mkv" | "webm" => {
            if matches!(ext, "mp4" | "mov") {
                assert_eq!(&bytes[4..8], b"ftyp", "{ctx}");
            } else {
                assert!(bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]), "{ctx}");
                let doctype: &[u8] = if ext == "webm" { b"webm" } else { b"matroska" };
                assert!(contains(&bytes[..64.min(bytes.len())], doctype), "{ctx}");
            }
            let p = probe(path);
            assert_eq!(p.video, 1, "{ctx} video streams");
            assert_eq!(p.audio, 1, "{ctx} audio streams");
            assert_eq!((p.width, p.height), (Some(64), Some(48)), "{ctx}");
            assert!((p.duration - 1.0).abs() < 0.2, "{ctx} lasts {}", p.duration);
        }
        "docx" => {
            assert!(bytes.starts_with(b"PK\x03\x04"), "{ctx}");
            assert!(contains(&bytes, b"word/document.xml"), "{ctx}");
        }
        "odt" => {
            assert!(bytes.starts_with(b"PK\x03\x04"), "{ctx}");
            assert!(
                contains(&bytes, b"mimetypeapplication/vnd.oasis.opendocument.text"),
                "{ctx}"
            );
        }
        "epub" => {
            assert!(bytes.starts_with(b"PK\x03\x04"), "{ctx}");
            assert!(contains(&bytes, b"mimetypeapplication/epub+zip"), "{ctx}");
        }
        "pdf" => {
            assert!(bytes.starts_with(b"%PDF-"), "{ctx}");
            assert!(contains(&bytes, b"%%EOF"), "{ctx}");
            assert!(contains(&bytes, b"/Type/Page"), "{ctx} has no pages");
        }
        // Legacy Office files are OLE compound files; the main stream's
        // name is stored in UTF-16.
        "doc" | "xls" | "ppt" => {
            assert!(
                bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]),
                "{ctx}"
            );
            let stream = match ext {
                "doc" => "WordDocument",
                "xls" => "Workbook",
                _ => "PowerPoint Document",
            };
            let utf16: Vec<u8> = stream.encode_utf16().flat_map(u16::to_le_bytes).collect();
            assert!(contains(&bytes, &utf16), "{ctx} has no {stream} stream");
        }
        "xlsx" | "pptx" => {
            assert!(bytes.starts_with(b"PK\x03\x04"), "{ctx}");
            let part: &[u8] = if ext == "xlsx" {
                b"xl/workbook.xml"
            } else {
                b"ppt/presentation.xml"
            };
            assert!(contains(&bytes, part), "{ctx}");
        }
        "ods" | "odp" => {
            assert!(bytes.starts_with(b"PK\x03\x04"), "{ctx}");
            let mime: &[u8] = if ext == "ods" {
                b"mimetypeapplication/vnd.oasis.opendocument.spreadsheet"
            } else {
                b"mimetypeapplication/vnd.oasis.opendocument.presentation"
            };
            assert!(contains(&bytes, mime), "{ctx}");
        }
        "csv" => {
            let text = String::from_utf8_lossy(&bytes);
            assert!(text.contains("1222"), "{ctx} lost a number: {text}");
            assert!(text.contains("glaze"), "{ctx} lost a word: {text}");
        }
        "md" | "rst" | "txt" | "html" | "tex" | "rtf" => {
            let text = String::from_utf8_lossy(&bytes);
            assert!(text.contains("Kiln fixture"), "{ctx} lost the heading");
            assert!(text.contains("glaze"), "{ctx} lost the list");
            match ext {
                "html" => assert!(text.contains("<html"), "{ctx}"),
                "tex" => assert!(text.contains("\\documentclass"), "{ctx}"),
                "rtf" => assert!(text.starts_with("{\\rtf"), "{ctx}"),
                _ => {}
            }
        }
        other => panic!("no check for {other} outputs; add one to tests/support"),
    }
}
