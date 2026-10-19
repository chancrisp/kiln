//! Regenerates the fixtures in tests/fixtures.
//!
//! cargo run -p kiln-core --example gen_fixtures [images|modern|media|subtitles|documents|office|ebooks]...
//!
//! With no arguments every group is made. Images are made with the image
//! crate; AVIF, JPEG XL, HEIC and APNG with FFmpeg. Audio, video and
//! documents need `ffmpeg` and `pandoc` on PATH; office files need
//! LibreOffice and are made from sample.docx; e-books need Calibre and are
//! made from sample.epub.
//!
//! Camera RAW and the read-only office types (Pages, Numbers, Keynote,
//! Visio, Publisher, WordPerfect) can't be made here. Tests that need them
//! look in the folder named by KILN_EXTRA_SAMPLES and skip when it isn't
//! set.

use std::path::{Path, PathBuf};
use std::process::Command;

use image::{GrayImage, ImageBuffer, Luma, Rgb, RgbImage, Rgba, RgbaImage};

fn main() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    std::fs::create_dir_all(&dir).unwrap();

    let groups: Vec<String> = std::env::args().skip(1).collect();
    let wanted = |g: &str| groups.is_empty() || groups.iter().any(|a| a == g);
    if wanted("images") {
        images(&dir);
    }
    if wanted("modern") {
        modern(&dir);
    }
    if wanted("subtitles") || wanted("media") {
        subtitles(&dir);
    }
    if wanted("media") {
        media(&dir);
    }
    if wanted("media") || wanted("padded-video") {
        padded_video(&dir);
    }
    if wanted("documents") {
        documents(&dir);
    }
    if wanted("office") {
        office(&dir);
    }
    if wanted("ebooks") {
        ebooks(&dir);
    }
}

fn images(dir: &Path) {
    // 64x48 RGB gradient.
    let rgb = RgbImage::from_fn(64, 48, |x, y| Rgb([(x * 4) as u8, (y * 5) as u8, 128]));
    rgb.save(dir.join("rgb_gradient.png")).unwrap();
    // The same gradient in every other raster type the image backend reads.
    for ext in ["jpg", "webp", "bmp", "gif", "tiff", "tga", "qoi"] {
        rgb.save(dir.join(format!("sample.{ext}"))).unwrap();
    }
    rgb.save_with_format(dir.join("sample.ppm"), image::ImageFormat::Pnm)
        .unwrap();
    // An icon of the same picture, written by Kiln's own encoder.
    let staging = std::env::temp_dir().join(format!("kiln-ico-{}", std::process::id()));
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::copy(dir.join("rgb_gradient.png"), staging.join("sample.png")).unwrap();
    let ico = kiln_core::Kiln::new()
        .convert(
            &staging.join("sample.png"),
            "ico",
            &kiln_core::Options::default(),
        )
        .unwrap();
    std::fs::copy(&ico, dir.join("sample.ico")).unwrap();
    // The staging folder in %TEMP% holds only the two files made above.
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::write(dir.join("sample.svg"), SVG).unwrap();

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
}

/// The fixture gradient as vector art: 64x48, with a shape and a word.
const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="48" viewBox="0 0 64 48">
  <defs>
    <linearGradient id="g" x1="0" x2="1">
      <stop offset="0" stop-color="#000080"/>
      <stop offset="1" stop-color="#ff0080"/>
    </linearGradient>
  </defs>
  <rect width="64" height="48" fill="url(#g)"/>
  <circle cx="32" cy="24" r="12" fill="#ffffff" fill-opacity="0.5"/>
</svg>
"##;

fn ffmpeg(dir: &Path, args: &[&str]) {
    let mut all = vec!["-hide_banner", "-loglevel", "error", "-y", "-bitexact"];
    all.extend_from_slice(args);
    run("ffmpeg", &all, dir);
}

/// AVIF, JPEG XL and HEIC stills of the gradient, and a one-second APNG.
fn modern(dir: &Path) {
    ffmpeg(
        dir,
        &[
            "-i",
            "rgb_gradient.png",
            "-c:v",
            "libaom-av1",
            "-still-picture",
            "1",
            "-crf",
            "20",
            "-pix_fmt",
            "yuv444p",
            "sample.avif",
        ],
    );
    ffmpeg(
        dir,
        &[
            "-i",
            "rgb_gradient.png",
            "-c:v",
            "libjxl",
            "-distance",
            "0.5",
            "sample.jxl",
        ],
    );
    ffmpeg(
        dir,
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=64x48:rate=10:duration=1",
            "-plays",
            "0",
            "-f",
            "apng",
            "sample.apng",
        ],
    );
    heic(dir);
}

/// FFmpeg can read HEIC but not write it, so the HEVC picture comes from a
/// one-frame MP4 and is wrapped in HEIF boxes here.
fn heic(dir: &Path) {
    let scratch = std::env::temp_dir().join(format!("kiln-heic-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    ffmpeg(
        &scratch,
        &[
            "-i",
            &dir.join("rgb_gradient.png").to_string_lossy(),
            "-frames:v",
            "1",
            "-c:v",
            "libx265",
            "-x265-params",
            "log-level=error",
            "-pix_fmt",
            "yuv420p",
            "-crf",
            "18",
            "-tag:v",
            "hvc1",
            "frame.mp4",
        ],
    );
    let mp4 = std::fs::read(scratch.join("frame.mp4")).unwrap();
    // The scratch folder in %TEMP% holds only frame.mp4.
    let _ = std::fs::remove_dir_all(&scratch);

    let find = |tag: &[u8]| {
        let at = mp4.windows(4).position(|w| w == tag).unwrap() - 4;
        let size = u32::from_be_bytes(mp4[at..at + 4].try_into().unwrap()) as usize;
        &mp4[at..at + size]
    };
    let hvcc = find(b"hvcC").to_vec();
    let mdat = find(b"mdat");
    let picture = &mdat[8..];

    fn boxed(tag: &[u8], body: &[u8]) -> Vec<u8> {
        let mut b = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        b.extend_from_slice(tag);
        b.extend_from_slice(body);
        b
    }
    fn full(tag: &[u8], version: u8, body: &[u8]) -> Vec<u8> {
        let mut b = vec![version, 0, 0, 0];
        b.extend_from_slice(body);
        boxed(tag, &b)
    }
    let ftyp = boxed(b"ftyp", b"heic\0\0\0\0mif1heic");
    let hdlr = full(b"hdlr", 0, b"\0\0\0\0pict\0\0\0\0\0\0\0\0\0\0\0\0\0");
    let pitm = full(b"pitm", 0, &1u16.to_be_bytes());
    let infe = full(b"infe", 2, b"\0\x01\0\0hvc1\0");
    let mut iinf_body = 1u16.to_be_bytes().to_vec();
    iinf_body.extend_from_slice(&infe);
    let iinf = full(b"iinf", 0, &iinf_body);
    let mut ispe_body = 64u32.to_be_bytes().to_vec();
    ispe_body.extend_from_slice(&48u32.to_be_bytes());
    let ispe = full(b"ispe", 0, &ispe_body);
    let ipco = boxed(b"ipco", &[hvcc, ispe].concat());
    let ipma = full(b"ipma", 0, &[0, 0, 0, 1, 0, 1, 2, 0x81, 0x02]);
    let iprp = boxed(b"iprp", &[ipco, ipma].concat());
    // iloc: 4-byte offsets and lengths, one item with one extent. The
    // offset is filled in once the size of everything before mdat is known.
    let iloc_body = |offset: u32| {
        let mut b = vec![0x44, 0x00];
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&0u16.to_be_bytes());
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&offset.to_be_bytes());
        b.extend_from_slice(&(picture.len() as u32).to_be_bytes());
        b
    };
    let meta_for = |offset: u32| {
        let iloc = full(b"iloc", 0, &iloc_body(offset));
        full(
            b"meta",
            0,
            &[hdlr.clone(), pitm.clone(), iinf.clone(), iprp.clone(), iloc].concat(),
        )
    };
    let head = ftyp.len() + meta_for(0).len();
    let meta = meta_for((head + 8) as u32);
    let file = [ftyp, meta, boxed(b"mdat", picture)].concat();
    std::fs::write(dir.join("sample.heic"), file).unwrap();
}

/// Two cues, in SubRip, WebVTT and ASS.
const SRT: &str = "1
00:00:00,000 --> 00:00:00,500
Kiln fixture

2
00:00:00,500 --> 00:00:01,000
clay, glaze, fire
";

fn subtitles(dir: &Path) {
    std::fs::write(dir.join("sample.srt"), SRT).unwrap();
    ffmpeg(dir, &["-i", "sample.srt", "-c:s", "webvtt", "sample.vtt"]);
    ffmpeg(dir, &["-i", "sample.srt", "-c:s", "ass", "sample.ass"]);
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
    ff(&[
        "-i",
        "sample.wav",
        "-c:a",
        "aac",
        "-b:a",
        "32k",
        "sample.aac",
    ]);
    ff(&["-i", "sample.wav", "-c:a", "pcm_s16be", "sample.aiff"]);
    ff(&[
        "-i",
        "sample.wav",
        "-c:a",
        "wmav2",
        "-b:a",
        "32k",
        "sample.wma",
    ]);
    ff(&[
        "-i",
        "sample.wav",
        "-c:a",
        "ac3",
        "-b:a",
        "64k",
        "-ar",
        "48000",
        "sample.ac3",
    ]);
    ff(&["-i", "sample.wav", "-c:a", "wavpack", "sample.wv"]);

    let picture = "testsrc=size=64x48:rate=10:duration=1";
    let h264 = ["-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", "40"];
    // MP4 and MKV carry the subtitle fixture as a text track.
    for (name, audio, subs) in [
        (
            "sample.mp4",
            ["-c:a", "aac", "-b:a", "32k"],
            Some("mov_text"),
        ),
        ("sample.mov", ["-c:a", "aac", "-b:a", "32k"], None),
        (
            "sample.mkv",
            ["-c:a", "libopus", "-b:a", "16k"],
            Some("srt"),
        ),
        ("sample.m4v", ["-c:a", "aac", "-b:a", "32k"], None),
        ("sample.flv", ["-c:a", "aac", "-b:a", "32k"], None),
        ("sample.ts", ["-c:a", "aac", "-b:a", "32k"], None),
        ("sample.3gp", ["-c:a", "aac", "-b:a", "32k"], None),
    ] {
        let mut args = vec!["-f", "lavfi", "-i", picture, "-f", "lavfi", "-i", tone];
        if subs.is_some() {
            args.extend_from_slice(&["-i", "sample.srt", "-map", "0", "-map", "1", "-map", "2"]);
        }
        args.extend_from_slice(&h264);
        args.extend_from_slice(&audio);
        if let Some(codec) = subs {
            args.extend_from_slice(&["-c:s", codec]);
        }
        args.extend_from_slice(&["-shortest", name]);
        ff(&args);
    }
    for (name, codecs) in [
        (
            "sample.avi",
            &[
                "-c:v",
                "mpeg4",
                "-q:v",
                "8",
                "-c:a",
                "libmp3lame",
                "-b:a",
                "32k",
            ][..],
        ),
        (
            "sample.wmv",
            &["-c:v", "wmv2", "-q:v", "8", "-c:a", "wmav2", "-b:a", "32k"][..],
        ),
        (
            "sample.ogv",
            &[
                "-c:v",
                "libtheora",
                "-q:v",
                "4",
                "-c:a",
                "libvorbis",
                "-q:a",
                "0",
            ][..],
        ),
    ] {
        let mut args = vec!["-f", "lavfi", "-i", picture, "-f", "lavfi", "-i", tone];
        args.extend_from_slice(codecs);
        args.extend_from_slice(&["-shortest", name]);
        ff(&args);
    }
    // MPEG-2 only allows broadcast frame rates.
    ff(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc=size=64x48:rate=25:duration=1",
        "-f",
        "lavfi",
        "-i",
        tone,
        "-f",
        "dvd",
        "-c:v",
        "mpeg2video",
        "-q:v",
        "8",
        "-c:a",
        "mp2",
        "-b:a",
        "64k",
        "-ar",
        "48000",
        "-shortest",
        "sample.mpg",
    ]);
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

/// Half a second of black, silent video either side of one second of test
/// picture and tone, for Trim.
fn padded_video(dir: &Path) {
    let gap = "color=black:size=64x48:rate=10:duration=0.5";
    let quiet = "anullsrc=r=22050:cl=mono:d=0.5";
    let mut args = vec!["-hide_banner", "-loglevel", "error", "-y", "-bitexact"];
    for src in [
        gap,
        quiet,
        "testsrc=size=64x48:rate=10:duration=1",
        "sine=frequency=440:sample_rate=22050:duration=1",
        gap,
        quiet,
    ] {
        args.extend_from_slice(&["-f", "lavfi", "-i", src]);
    }
    args.extend_from_slice(&[
        "-filter_complex",
        "[0:v][1:a][2:v][3:a][4:v][5:a]concat=n=3:v=1:a=1[v][a]",
        "-map",
        "[v]",
        "-map",
        "[a]",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        "-crf",
        "40",
        "-c:a",
        "aac",
        "-b:a",
        "32k",
        "padded.mp4",
    ]);
    run("ffmpeg", &args, dir);
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
        ("sample.txt", "plain"),
        ("sample.org", "org"),
        ("sample.adoc", "asciidoc"),
        ("sample.typ", "typst"),
        ("sample.wiki", "mediawiki"),
        ("sample.textile", "textile"),
        ("sample.dbk", "docbook"),
        ("sample.fb2", "fb2"),
        ("sample.ipynb", "ipynb"),
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

const CSV: &str = "Cone,Temperature,Glaze\n06,999,clay\n6,1222,glaze\n";

/// Two slides in flat ODF, the source for the presentation fixtures.
const SLIDES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
  xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0"
  xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"
  xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0"
  xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0"
  xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0"
  xmlns:presentation="urn:oasis:names:tc:opendocument:xmlns:presentation:1.0"
  office:version="1.3" office:mimetype="application/vnd.oasis.opendocument.presentation">
 <office:automatic-styles>
  <style:page-layout style:name="PM1">
   <style:page-layout-properties fo:page-width="28cm" fo:page-height="15.75cm" style:print-orientation="landscape"/>
  </style:page-layout>
 </office:automatic-styles>
 <office:master-styles>
  <style:master-page style:name="Default" style:page-layout-name="PM1"/>
 </office:master-styles>
 <office:body>
  <office:presentation>
   <draw:page draw:name="Kiln" draw:master-page-name="Default">
    <draw:frame svg:x="2cm" svg:y="2cm" svg:width="24cm" svg:height="3cm">
     <draw:text-box><text:p>Kiln fixture</text:p></draw:text-box>
    </draw:frame>
    <draw:frame svg:x="2cm" svg:y="6cm" svg:width="24cm" svg:height="6cm">
     <draw:text-box><text:p>clay, glaze, fire</text:p></draw:text-box>
    </draw:frame>
   </draw:page>
   <draw:page draw:name="Cones" draw:master-page-name="Default">
    <draw:frame svg:x="2cm" svg:y="2cm" svg:width="24cm" svg:height="3cm">
     <draw:text-box><text:p>Cone 6 is 1222 C</text:p></draw:text-box>
    </draw:frame>
   </draw:page>
  </office:presentation>
 </office:body>
</office:document>
"#;

/// Word, spreadsheet and slide fixtures, written by LibreOffice.
fn office(dir: &Path) {
    let soffice = kiln_core::find_libreoffice().expect("LibreOffice is installed");
    let scratch = std::env::temp_dir().join(format!("kiln-office-fixtures-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    std::fs::write(dir.join("sample.csv"), CSV).unwrap();
    std::fs::write(scratch.join("sample.fodp"), SLIDES).unwrap();

    let profile = format!(
        "-env:UserInstallation=file:///{}",
        scratch
            .join("profile")
            .to_string_lossy()
            .replace('\\', "/")
            .replace(' ', "%20")
    );
    let convert = |input: &Path, to: &str, infilter: Option<&str>| {
        let mut cmd = Command::new(&soffice);
        cmd.arg(&profile)
            .args(["--headless", "--norestore", "--nolockcheck"])
            .env("SAL_DISABLE_DEFAULTPRINTER", "1")
            .env("SAL_DISABLE_PRINTERLIST", "1");
        if let Some(f) = infilter {
            cmd.arg(format!("--infilter={f}"));
        }
        let status = cmd
            .args(["--convert-to", to, "--outdir"])
            .arg(dir)
            .arg(input)
            .status()
            .expect("LibreOffice starts");
        assert!(status.success(), "LibreOffice couldn't write {to}");
    };

    let docx = dir.join("sample.docx");
    convert(&docx, "doc:MS Word 97", None);
    convert(&docx, "rtf:Rich Text Format", None);
    convert(&docx, "pdf", None);
    let csv = dir.join("sample.csv");
    for to in [
        "xlsx:Calc MS Excel 2007 XML",
        "xls:MS Excel 97",
        "ods:calc8",
    ] {
        convert(&csv, to, Some("CSV:44,34,76,1"));
    }
    let slides = scratch.join("sample.fodp");
    for to in [
        "pptx:Impress MS PowerPoint 2007 XML",
        "ppt:MS PowerPoint 97",
        "odp:impress8",
    ] {
        convert(&slides, to, None);
    }
    // The scratch folder in %TEMP% holds only what this function wrote.
    let _ = std::fs::remove_dir_all(&scratch);
}

/// MOBI and AZW3 from sample.epub, written by Calibre.
fn ebooks(dir: &Path) {
    let convert = kiln_core::find_calibre().expect("Calibre is installed");
    for out in ["sample.mobi", "sample.azw3"] {
        let status = Command::new(&convert)
            .arg(dir.join("sample.epub"))
            .arg(dir.join(out))
            .stdout(std::process::Stdio::null())
            .status()
            .expect("ebook-convert starts");
        assert!(status.success(), "Calibre couldn't write {out}");
    }
}
