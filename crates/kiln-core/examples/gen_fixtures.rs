//! Regenerates the fixtures in tests/fixtures.
//!
//! cargo run -p kiln-core --example gen_fixtures
//!
//! Images are made with the image crate. Audio, video and documents need
//! `ffmpeg` and `pandoc` on PATH.

use std::path::{Path, PathBuf};
use std::process::Command;

use image::{GrayImage, ImageBuffer, Luma, Rgb, RgbImage, Rgba, RgbaImage};

fn main() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    std::fs::create_dir_all(&dir).unwrap();

    // 64x48 RGB gradient.
    let rgb = RgbImage::from_fn(64, 48, |x, y| Rgb([(x * 4) as u8, (y * 5) as u8, 128]));
    rgb.save(dir.join("rgb_gradient.png")).unwrap();

    // 32x32: left half fully transparent, right half opaque dark blue.
    let rgba = RgbaImage::from_fn(32, 32, |x, _| {
        if x < 16 {
            Rgba([200, 30, 30, 0])
        } else {
            Rgba([20, 40, 120, 255])
        }
    });
    rgba.save(dir.join("rgba_half_transparent.png")).unwrap();

    // 40x20 8-bit grayscale ramp.
    let gray = GrayImage::from_fn(40, 20, |x, _| Luma([(x * 6) as u8]));
    gray.save(dir.join("gray8.png")).unwrap();

    // 24x24 16-bit RGB, flat mid-green.
    let deep: ImageBuffer<Rgb<u16>, Vec<u16>> =
        ImageBuffer::from_pixel(24, 24, Rgb([10_000, 40_000, 20_000]));
    deep.save(dir.join("rgb16.png")).unwrap();

    // A valid PNG signature followed by garbage.
    let bytes = std::fs::read(dir.join("rgb_gradient.png")).unwrap();
    std::fs::write(dir.join("truncated.png"), &bytes[..bytes.len() / 3]).unwrap();

    // 48x36 with a plain light border around a darker 28x16 block.
    let bordered = RgbImage::from_fn(48, 36, |x, y| {
        if (10..38).contains(&x) && (10..26).contains(&y) {
            Rgb([40, 60, 90])
        } else {
            Rgb([235, 230, 220])
        }
    });
    bordered.save(dir.join("bordered.png")).unwrap();

    // Text with a .png extension.
    std::fs::write(dir.join("not_really.png"), b"this is plain text\n").unwrap();

    media(&dir);
    documents(&dir);
}

fn run(program: &str, args: &[&str], cwd: &Path) {
    let status = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .status()
        .unwrap_or_else(|e| panic!("couldn't start {program}: {e}"));
    assert!(status.success(), "{program} {args:?} failed");
}

/// One second of audio and video, tiny and deterministic.
fn media(dir: &Path) {
    let ff = |args: &[&str]| {
        let mut all = vec!["-hide_banner", "-loglevel", "error", "-y", "-bitexact"];
        all.extend_from_slice(args);
        run("ffmpeg", &all, dir);
    };
    let tone = "sine=frequency=440:sample_rate=22050:duration=1";
    ff(&[
        "-f",
        "lavfi",
        "-i",
        tone,
        "-ac",
        "1",
        "-c:a",
        "pcm_s16le",
        "sample.wav",
    ]);
    ff(&[
        "-i",
        "sample.wav",
        "-c:a",
        "libmp3lame",
        "-b:a",
        "32k",
        "sample.mp3",
    ]);
    ff(&["-i", "sample.wav", "-c:a", "flac", "sample.flac"]);
    ff(&[
        "-i",
        "sample.wav",
        "-c:a",
        "libvorbis",
        "-q:a",
        "0",
        "sample.ogg",
    ]);
    ff(&[
        "-i",
        "sample.wav",
        "-c:a",
        "libopus",
        "-b:a",
        "16k",
        "sample.opus",
    ]);
    ff(&[
        "-i",
        "sample.wav",
        "-c:a",
        "aac",
        "-b:a",
        "32k",
        "sample.m4a",
    ]);

    let picture = "testsrc=size=64x48:rate=10:duration=1";
    let h264 = ["-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", "40"];
    for (name, audio) in [
        ("sample.mp4", ["-c:a", "aac", "-b:a", "32k"]),
        ("sample.mov", ["-c:a", "aac", "-b:a", "32k"]),
        ("sample.mkv", ["-c:a", "libopus", "-b:a", "16k"]),
    ] {
        let mut args = vec!["-f", "lavfi", "-i", picture, "-f", "lavfi", "-i", tone];
        args.extend_from_slice(&h264);
        args.extend_from_slice(&audio);
        args.extend_from_slice(&["-shortest", name]);
        ff(&args);
    }
    ff(&[
        "-f",
        "lavfi",
        "-i",
        picture,
        "-f",
        "lavfi",
        "-i",
        tone,
        "-c:v",
        "libvpx-vp9",
        "-b:v",
        "0",
        "-crf",
        "50",
        "-deadline",
        "realtime",
        "-c:a",
        "libopus",
        "-b:a",
        "16k",
        "-shortest",
        "sample.webm",
    ]);

    // Letterboxed: 64x48 frame with 64x32 picture and black bars.
    ff(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc=size=64x32:rate=10:duration=1",
        "-f",
        "lavfi",
        "-i",
        tone,
        "-vf",
        "pad=64:48:0:8:black",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        "-crf",
        "30",
        "-c:a",
        "aac",
        "-b:a",
        "32k",
        "-shortest",
        "letterbox.mp4",
    ]);
    // Half a second of silence either side of one second of tone.
    ff(&[
        "-f",
        "lavfi",
        "-i",
        "anullsrc=r=22050:cl=mono:d=0.5",
        "-f",
        "lavfi",
        "-i",
        tone,
        "-f",
        "lavfi",
        "-i",
        "anullsrc=r=22050:cl=mono:d=0.5",
        "-filter_complex",
        "[0:a][1:a][2:a]concat=n=3:v=0:a=1",
        "-c:a",
        "pcm_s16le",
        "padded.wav",
    ]);

    // Text with a media extension.
    std::fs::write(dir.join("corrupt.wav"), b"RIFF but not really\n").unwrap();
}

const MARKDOWN: &str = "---
title: Kiln fixture
---

# Kiln fixture

A small document with *emphasis*, **strong text** and a list:

- clay
- glaze
- fire

| Cone | Temperature |
|------|-------------|
| 06   | 999 C       |
| 6    | 1222 C      |
";

fn documents(dir: &Path) {
    std::fs::write(dir.join("sample.md"), MARKDOWN).unwrap();
    for (out, to) in [
        ("sample.html", "html"),
        ("sample.docx", "docx"),
        ("sample.odt", "odt"),
        ("sample.epub", "epub"),
        ("sample.rst", "rst"),
        ("sample.tex", "latex"),
    ] {
        run(
            "pandoc",
            &[
                "--standalone",
                "-f",
                "markdown",
                "-t",
                to,
                "-o",
                out,
                "sample.md",
            ],
            dir,
        );
    }
}
