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
        .arg("format=format_name,duration:stream=index,codec_type,width,height")
        .args(["-of", "compact"])
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
    // A transport stream lists its streams again under its program, so
    // each stream counts once, by its index.
    let mut seen = std::collections::HashSet::new();
    for line in text.lines() {
        let fields: Vec<(&str, &str)> = line.split('|').filter_map(|f| f.split_once('=')).collect();
        let index = fields.iter().find(|(k, _)| *k == "index").map(|f| f.1);
        if index.is_some_and(|i| !seen.insert(i.to_owned())) {
            continue;
        }
        for (k, v) in fields {
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
    }
    p
}

/// Types no free tool can write, so their samples can't be generated:
/// camera RAW and the read-only office types. Tests find them in the
/// folder named by KILN_EXTRA_SAMPLES and skip them when it isn't set.
pub const EXTERNAL: [&str; 16] = [
    "dng", "cr2", "cr3", "nef", "arw", "orf", "rw2", "raf", "pef", "srw", "pages", "numbers",
    "key", "vsdx", "pub", "wpd",
];

/// The sample for an external type, if one is available.
pub fn external_sample(ext: &str) -> Option<PathBuf> {
    let dir = std::env::var_os("KILN_EXTRA_SAMPLES")?;
    let path = Path::new(&dir).join(format!("sample.{ext}"));
    path.is_file().then_some(path)
}

/// Inputs that are the 64x48 gradient, so their image outputs are too.
const GRADIENT: [&str; 13] = [
    "png", "jpg", "webp", "bmp", "gif", "tiff", "tga", "qoi", "ppm", "svg", "avif", "jxl", "heic",
];

/// Inputs that become one image per page or slide.
pub fn paged(ext: &str) -> bool {
    matches!(
        ext,
        "pdf" | "pptx" | "ppt" | "odp" | "key" | "vsdx" | "pub" | "docx" | "odt" | "doc"
    )
}

/// Assert that `path` really is a file of type `ext`, with the basic
/// properties of the one-second, 64x48 fixtures.
pub fn assert_output(path: &Path, ext: &str) {
    assert_converted(path, None, ext);
}

/// Like [`assert_output`], knowing the input type: an image of a PDF page
/// has its own size, video made from a GIF has no sound, and a type read
/// from an external sample has unknown content.
pub fn assert_converted(path: &Path, from: Option<&str>, ext: &str) {
    let bytes = fs::read(path).unwrap();
    assert!(!bytes.is_empty(), "{} is empty", path.display());
    let ctx = path.display();
    let from_ext = from.unwrap_or("");
    let external = EXTERNAL.contains(&from_ext);
    let kind = from.and_then(|f| kiln_core::Registry::builtin().format(f).map(|f| f.kind));
    let from_sheet = kind == Some(kiln_core::Kind::Spreadsheet);
    let silent_source = matches!(from_ext, "gif" | "apng");
    match ext {
        "png" | "jpg" | "webp" | "bmp" | "gif" | "tiff" | "ico" | "tga" | "qoi" | "ppm" => {
            let format = match ext {
                "png" => image::ImageFormat::Png,
                "jpg" => image::ImageFormat::Jpeg,
                "webp" => image::ImageFormat::WebP,
                "bmp" => image::ImageFormat::Bmp,
                "gif" => image::ImageFormat::Gif,
                "ico" => image::ImageFormat::Ico,
                "tga" => image::ImageFormat::Tga,
                "qoi" => image::ImageFormat::Qoi,
                "ppm" => image::ImageFormat::Pnm,
                _ => image::ImageFormat::Tiff,
            };
            if ext != "tga" {
                assert_eq!(image::guess_format(&bytes).ok(), Some(format), "{ctx}");
            }
            let img = image::load_from_memory_with_format(&bytes, format).unwrap();
            let size = (img.width(), img.height());
            if from.is_none()
                || GRADIENT.contains(&from_ext)
                || kind == Some(kiln_core::Kind::Video)
            {
                let want = if ext == "ico" { (64, 64) } else { (64, 48) };
                assert_eq!(size, want, "{ctx}");
            } else if from_ext == "ico" {
                assert_eq!(size, (64, 64), "{ctx}");
            } else {
                assert!(size.0 > 0 && size.1 > 0, "{ctx}");
            }
        }
        "avif" => {
            assert_eq!(&bytes[4..12], b"ftypavif", "{ctx}");
            let p = probe(path);
            assert!(p.video >= 1, "{ctx}");
            if GRADIENT.contains(&from_ext) {
                assert_eq!((p.width, p.height), (Some(64), Some(48)), "{ctx}");
            }
        }
        "jxl" => {
            assert!(
                bytes.starts_with(&[0xFF, 0x0A]) || bytes.starts_with(b"\0\0\0\x0cJXL "),
                "{ctx}"
            );
        }
        "apng" => {
            assert!(bytes.starts_with(&[0x89, b'P', b'N', b'G']), "{ctx}");
            assert!(contains(&bytes, b"acTL"), "{ctx} isn't animated");
        }
        "wav" | "mp3" | "flac" | "ogg" | "opus" | "m4a" | "aac" | "aiff" | "wma" | "ac3" | "wv" => {
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
                "aac" => assert!(bytes[0] == 0xFF && bytes[1] & 0xF0 == 0xF0, "{ctx}"),
                "aiff" => assert!(
                    bytes.starts_with(b"FORM") && &bytes[8..12] == b"AIFF",
                    "{ctx}"
                ),
                "wma" => assert!(bytes.starts_with(&ASF), "{ctx}"),
                "ac3" => assert!(bytes.starts_with(&[0x0B, 0x77]), "{ctx}"),
                "wv" => assert!(bytes.starts_with(b"wvpk"), "{ctx}"),
                _ => unreachable!(),
            }
            let p = probe(path);
            assert_eq!(p.video, 0, "{ctx} has a video stream");
            assert_eq!(p.audio, 1, "{ctx} audio streams");
            // Raw AAC has no header with its length, so FFprobe guesses
            // from the bitrate.
            let slack = if ext == "aac" { 0.25 } else { 0.15 };
            assert!(
                (p.duration - 1.0).abs() < slack,
                "{ctx} lasts {}",
                p.duration
            );
        }
        "mp4" | "m4v" | "mov" | "3gp" | "mkv" | "webm" | "avi" | "wmv" | "flv" | "mpg" | "ts"
        | "ogv" => {
            match ext {
                "mp4" | "m4v" | "mov" | "3gp" => assert_eq!(&bytes[4..8], b"ftyp", "{ctx}"),
                "mkv" | "webm" => {
                    assert!(bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]), "{ctx}");
                    let doctype: &[u8] = if ext == "webm" { b"webm" } else { b"matroska" };
                    assert!(contains(&bytes[..64.min(bytes.len())], doctype), "{ctx}");
                }
                "avi" => assert!(
                    bytes.starts_with(b"RIFF") && &bytes[8..12] == b"AVI ",
                    "{ctx}"
                ),
                "wmv" => assert!(bytes.starts_with(&ASF), "{ctx}"),
                "flv" => assert!(bytes.starts_with(b"FLV"), "{ctx}"),
                "mpg" => assert!(bytes.starts_with(&[0, 0, 1, 0xBA]), "{ctx}"),
                "ts" => assert!(bytes[0] == 0x47 && bytes[188] == 0x47, "{ctx}"),
                "ogv" => assert!(
                    bytes.starts_with(b"OggS") && contains(&bytes, b"theora"),
                    "{ctx}"
                ),
                _ => unreachable!(),
            }
            let p = probe(path);
            assert_eq!(p.video, 1, "{ctx} video streams");
            assert_eq!((p.width, p.height), (Some(64), Some(48)), "{ctx}");
            if silent_source {
                assert_eq!(p.audio, 0, "{ctx} audio streams");
                assert!(p.duration > 0.0, "{ctx} lasts {}", p.duration);
            } else {
                assert_eq!(p.audio, 1, "{ctx} audio streams");
                assert!(
                    (p.duration - 1.0).abs() < 0.25,
                    "{ctx} lasts {}",
                    p.duration
                );
            }
        }
        "srt" | "vtt" | "ass" => {
            let text = String::from_utf8_lossy(&bytes);
            assert!(text.contains("glaze"), "{ctx} lost a cue: {text}");
            match ext {
                "srt" => assert!(text.contains("-->"), "{ctx}"),
                "vtt" => assert!(
                    text.trim_start_matches('\u{feff}').starts_with("WEBVTT"),
                    "{ctx}"
                ),
                _ => assert!(
                    text.contains("[Script Info]") && text.contains("Dialogue:"),
                    "{ctx}"
                ),
            }
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
        "mobi" | "azw3" => {
            assert!(bytes.len() > 68 && &bytes[60..68] == b"BOOKMOBI", "{ctx}");
        }
        "pdf" => {
            assert!(bytes.starts_with(b"%PDF-"), "{ctx}");
            assert!(contains(&bytes, b"%%EOF"), "{ctx}");
            // Pages may sit in compressed object streams, so parse the file.
            let doc = lopdf::Document::load_mem(&bytes)
                .unwrap_or_else(|e| panic!("{ctx} doesn't parse: {e}"));
            assert!(!doc.get_pages().is_empty(), "{ctx} has no pages");
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
            if !external {
                let text = String::from_utf8_lossy(&bytes);
                assert!(text.contains("1222"), "{ctx} lost a number: {text}");
                assert!(text.contains("glaze"), "{ctx} lost a word: {text}");
            }
        }
        "md" | "rst" | "txt" | "html" | "tex" | "rtf" | "org" | "adoc" | "typ" | "wiki"
        | "textile" | "dbk" | "fb2" | "ipynb" => {
            let text = String::from_utf8_lossy(&bytes);
            if external {
                // Downloaded samples hold their own text.
            } else if from_sheet {
                assert!(text.contains("1222"), "{ctx} lost a number");
            } else {
                assert!(text.contains("Kiln fixture"), "{ctx} lost the heading");
            }
            if !external {
                assert!(text.contains("glaze"), "{ctx} lost the list");
            }
            match ext {
                "html" => assert!(text.to_ascii_lowercase().contains("<html"), "{ctx}"),
                "tex" => assert!(text.contains("\\documentclass"), "{ctx}"),
                "rtf" => assert!(text.starts_with("{\\rtf"), "{ctx}"),
                "dbk" | "fb2" => assert!(text.trim_start().starts_with('<'), "{ctx}"),
                "ipynb" => assert!(text.contains("\"cells\""), "{ctx}"),
                _ => {}
            }
        }
        other => panic!("no check for {other} outputs; add one to tests/support"),
    }
}

/// The ASF header GUID that starts WMA and WMV files.
const ASF: [u8; 4] = [0x30, 0x26, 0xB2, 0x75];
