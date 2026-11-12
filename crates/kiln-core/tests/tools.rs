//! The file tools, with the fixtures.

mod pdfs;
mod pictures;
#[path = "../../../tests/support/mod.rs"]
mod support;

use std::fs;
use std::path::{Path, PathBuf};

use image::GenericImageView;
use kiln_core::{about, CancelToken, Error, FileCheck, JobError, Kiln, Options, Tool, ToolOptions};
use pictures::{
    camera_jpeg, camera_png, camera_webp, display_p3, exif_of, icc_of, ifd0_tags, jpeg_scan,
    jpeg_with, orientation_of, png_with, xmp_of, CAMERA_WORDS,
};
use support::{assert_output, files_in, fixture, probe, sample_for, secret_pdf, stage};

fn options(temp: &tempfile::TempDir) -> Options {
    Options {
        temp_root: Some(temp.path().join("work")),
        ..Options::default()
    }
}

fn run(tool: Tool, inputs: &[PathBuf], dir: &tempfile::TempDir) -> kiln_core::Result<Vec<PathBuf>> {
    run_with(tool, inputs, &options(dir), dir)
}

fn run_with(
    tool: Tool,
    inputs: &[PathBuf],
    opts: &Options,
    dir: &tempfile::TempDir,
) -> kiln_core::Result<Vec<PathBuf>> {
    let out = Kiln::new().run_tool(tool, inputs, opts);
    let work = dir.path().join("work");
    if work.exists() {
        assert!(files_in(&work).is_empty(), "work left behind");
    }
    out
}

fn name(p: &Path) -> String {
    p.file_name().unwrap().to_string_lossy().into_owned()
}

#[test]
fn tools_offered_match_the_files() {
    let kiln = Kiln::new();
    let t = |names: &[&str]| -> Vec<Tool> {
        let paths: Vec<PathBuf> = names.iter().map(PathBuf::from).collect();
        let refs: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
        kiln.tools_for(&refs)
    };
    use Tool::*;
    let none = Vec::<Tool>::new();
    // Make smaller only for formats Kiln can write (R2); Resize and Remove
    // location data for photos.
    assert_eq!(
        t(&["a.png"]),
        [Smaller, Resize, RemoveLocation, RemoveEdges]
    );
    assert_eq!(
        t(&["a.jpg"]),
        [Smaller, Resize, RemoveLocation, RemoveEdges]
    );
    assert_eq!(
        t(&["a.webp"]),
        [Smaller, Resize, RemoveLocation, RemoveEdges]
    );
    assert_eq!(t(&["a.heic"]), [Resize, RemoveLocation, RemoveEdges]);
    assert_eq!(t(&["a.gif"]), [Resize, RemoveLocation, RemoveEdges]);
    assert_eq!(t(&["a.svg"]), [RemoveEdges]);
    // No Make smaller for audio.
    assert_eq!(t(&["a.mp3"]), [RemoveQuietEnds, CutInHalf]);
    assert_eq!(t(&["a.wav"]), [RemoveQuietEnds, CutInHalf]);
    assert_eq!(t(&["a.aac"]), [RemoveQuietEnds, CutInHalf]);
    for video in ["a.mp4", "a.mov", "a.m4v", "a.mkv"] {
        assert_eq!(
            t(&[video]),
            [Smaller, RemoveEdges, RemoveQuietEnds, CutInHalf],
            "{video}"
        );
    }
    for video in ["a.avi", "a.ts", "a.webm"] {
        assert_eq!(
            t(&[video]),
            [RemoveEdges, RemoveQuietEnds, CutInHalf],
            "{video}"
        );
    }
    assert_eq!(t(&["a.pdf"]), [Smaller, SaveEachPage]);
    // Merge only for two or more.
    assert_eq!(t(&["a.mp3", "b.mp3"]), [RemoveQuietEnds, CutInHalf, Merge]);
    assert_eq!(t(&["a.mp3", "b.wav"]), [RemoveQuietEnds, CutInHalf]);
    assert_eq!(t(&["a.pdf", "b.pdf"]), [Smaller, SaveEachPage, Merge]);
    assert_eq!(t(&["a.docx", "b.docx"]), [Merge]);
    assert_eq!(t(&["a.docx"]), none);
    // A tool's name depends on the kind of file, so files of different
    // kinds share no tool.
    assert_eq!(t(&["a.png", "b.mp4"]), none);
    assert_eq!(t(&["a.png", "b.mp3"]), none);
    assert_eq!(t(&["a.png", "b.docx"]), none);
    assert_eq!(t(&["a.xyz"]), none);
}

#[test]
fn make_smaller_for_video_with_no_size_keeps_only_a_smaller_file() {
    // Make smaller for video is built, on Kiln's own encoder: with no size
    // it encodes at the Small step and keeps the result only when smaller.
    let (video_dir, video) = stage(&sample_for("mp4"));
    let original = fs::metadata(&video).unwrap().len();
    match run(Tool::Smaller, std::slice::from_ref(&video), &video_dir) {
        Ok(out) => {
            assert_eq!(out, [video_dir.path().join("sample (smaller).mp4")]);
            assert!(fs::metadata(&out[0]).unwrap().len() < original);
        }
        Err(Error::NothingToDo(m)) => {
            assert_eq!(m, "This file is already as small as Kiln can make it.");
            assert_eq!(files_in(video_dir.path()), ["sample.mp4"]);
        }
        Err(e) => panic!("{e:?}"),
    }
}

#[test]
fn crop_removes_a_plain_image_border() {
    let (dir, input) = stage(&fixture("bordered.png"));
    let out = run(Tool::RemoveEdges, &[input], &dir).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(name(&out[0]), "bordered cropped.png");
    assert_eq!(image::open(&out[0]).unwrap().dimensions(), (28, 16));
}

#[test]
fn compress_and_crop_keep_orientation_and_profile() {
    let profile = display_p3();
    let dir = tempfile::tempdir().unwrap();

    // Compress turns a sideways photo upright and keeps its profile.
    let photo = image::RgbImage::from_fn(64, 32, |x, y| {
        image::Rgb([(x * 4) as u8, (y * 8) as u8, ((x * y) % 256) as u8])
    });
    let input = dir.path().join("photo.jpg");
    jpeg_with(&input, &photo, 6, Some(&profile), 100);
    let out = run(Tool::Smaller, &[input], &dir).unwrap();
    assert_eq!(name(&out[0]), "photo (smaller).jpg");
    assert_eq!(image::open(&out[0]).unwrap().dimensions(), (32, 64));
    assert_eq!(icc_of(&out[0]).as_deref(), Some(profile.as_slice()));
    assert_eq!(
        orientation_of(&out[0]),
        image::metadata::Orientation::NoTransforms
    );

    // So does Crop. This picture is stored 20 wide and 12 high with a
    // plain border and a 12 x 8 middle; shown 12 wide and 20 high.
    let mut bordered = image::RgbImage::from_pixel(20, 12, image::Rgb([240, 240, 240]));
    for y in 2..10 {
        for x in 4..16 {
            bordered.put_pixel(x, y, image::Rgb([200, 30, 30]));
        }
    }
    let input = dir.path().join("framed.png");
    png_with(&input, &bordered, 6, Some(&profile));
    let out = run(Tool::RemoveEdges, &[input], &dir).unwrap();
    assert_eq!(name(&out[0]), "framed cropped.png");
    assert_eq!(image::open(&out[0]).unwrap().dimensions(), (8, 12));
    assert_eq!(icc_of(&out[0]).as_deref(), Some(profile.as_slice()));
    assert_eq!(
        orientation_of(&out[0]),
        image::metadata::Orientation::NoTransforms
    );
}

#[test]
fn crop_says_so_when_there_is_no_border() {
    let (dir, input) = stage(&fixture("rgb_gradient.png"));
    let err = run(Tool::RemoveEdges, &[input], &dir).unwrap_err();
    assert_eq!(err.to_string(), "There's no plain border to remove.");
    assert_eq!(files_in(dir.path()), ["rgb_gradient.png", "work"]);
}

#[test]
fn a_cancelled_crop_stops_drawing_an_svg() {
    // A plain border round a red middle. Cancel stops the drawing, as it
    // does in a conversion, rather than waiting it out.
    let dir = tempfile::tempdir().unwrap();
    let svg = dir.path().join("framed.svg");
    fs::write(
        &svg,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="30"><rect width="40" height="30" fill="#eee"/><rect x="10" y="10" width="20" height="10" fill="red"/></svg>"##,
    )
    .unwrap();
    let cancel = CancelToken::new();
    cancel.cancel();
    let err = Kiln::new()
        .run_tool_with(
            Tool::RemoveEdges,
            &[svg],
            &options(&dir),
            &mut |_| {},
            &cancel,
        )
        .unwrap_err();
    assert!(matches!(err, Error::Cancelled), "{err:?}");
    // Nothing is written, and nothing is left in the work folder.
    assert_eq!(files_in(dir.path()), ["framed.svg", "work"]);
    assert!(files_in(&dir.path().join("work")).is_empty());
}

#[test]
fn crop_removes_black_bars_from_video() {
    let (dir, input) = stage(&fixture("letterbox.mp4"));
    let out = run(Tool::RemoveEdges, &[input], &dir).unwrap();
    let p = probe(&out[0]);
    assert_eq!((p.width, p.height), (Some(64), Some(32)));
    assert_eq!(p.audio, 1);
}

#[test]
fn crop_on_a_full_frame_video_has_nothing_to_do() {
    let (dir, input) = stage(&sample_for("mp4"));
    let err = run(Tool::RemoveEdges, &[input], &dir).unwrap_err();
    assert!(matches!(err, Error::NothingToDo(_)), "{err:?}");
}

#[test]
fn trim_cuts_leading_and_trailing_silence() {
    let (dir, input) = stage(&fixture("padded.wav"));
    let out = run(Tool::RemoveQuietEnds, &[input], &dir).unwrap();
    assert_eq!(name(&out[0]), "padded trimmed.wav");
    let d = probe(&out[0]).duration;
    assert!((d - 1.0).abs() < 0.1, "trimmed to {d}");
}

#[test]
fn trim_without_silence_has_nothing_to_do() {
    let (dir, input) = stage(&sample_for("wav"));
    let err = run(Tool::RemoveQuietEnds, &[input], &dir).unwrap_err();
    assert_eq!(
        err.to_string(),
        "There's no silence at the start or end to remove."
    );
}

#[test]
fn trim_cuts_black_silent_video_edges() {
    let (dir, input) = stage(&fixture("padded.mp4"));
    let out = run(Tool::RemoveQuietEnds, &[input], &dir).unwrap();
    assert_eq!(name(&out[0]), "padded trimmed.mp4");
    let p = probe(&out[0]);
    assert!((p.duration - 1.0).abs() < 0.2, "trimmed to {}", p.duration);
    assert_eq!((p.video, p.audio), (1, 1));
    assert_eq!((p.width, p.height), (Some(64), Some(48)));
}

#[test]
fn trim_on_video_without_black_edges_has_nothing_to_do() {
    let (dir, input) = stage(&sample_for("mp4"));
    let err = run(Tool::RemoveQuietEnds, &[input], &dir).unwrap_err();
    assert_eq!(
        err.to_string(),
        "There's no black, silent stretch at the start or end to remove."
    );
}

#[test]
fn split_makes_two_halves() {
    let (dir, input) = stage(&sample_for("mp3"));
    let out = run(Tool::CutInHalf, &[input], &dir).unwrap();
    let names: Vec<String> = out.iter().map(|p| name(p)).collect();
    assert_eq!(names, ["sample part 1.mp3", "sample part 2.mp3"]);
    for p in &out {
        let d = probe(p).duration;
        // MP3 frames and encoder delay add up to a tenth of a second.
        assert!((d - 0.5).abs() < 0.15, "{} lasts {d}", p.display());
    }
}

#[test]
fn split_video_keeps_both_streams() {
    let (dir, input) = stage(&sample_for("mkv"));
    let out = run(Tool::CutInHalf, &[input], &dir).unwrap();
    assert_eq!(out.len(), 2);
    for p in &out {
        let pr = probe(p);
        assert_eq!((pr.video, pr.audio), (1, 1), "{}", p.display());
    }
}

#[test]
fn merge_audio_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("one.flac");
    let b = dir.path().join("two.flac");
    fs::copy(sample_for("flac"), &a).unwrap();
    fs::copy(sample_for("flac"), &b).unwrap();
    let out = run(Tool::Merge, &[a, b], &dir).unwrap();
    assert_eq!(name(&out[0]), "one merged.flac");
    let d = probe(&out[0]).duration;
    assert!((d - 2.0).abs() < 0.15, "merged lasts {d}");
}

#[test]
fn merge_video_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.mp4");
    let b = dir.path().join("b.mp4");
    fs::copy(sample_for("mp4"), &a).unwrap();
    fs::copy(sample_for("mp4"), &b).unwrap();
    let out = run(Tool::Merge, &[a, b], &dir).unwrap();
    let p = probe(&out[0]);
    assert_eq!((p.video, p.audio), (1, 1));
    assert_eq!((p.width, p.height), (Some(64), Some(48)));
    assert!(
        (p.duration - 2.0).abs() < 0.25,
        "merged lasts {}",
        p.duration
    );
}

#[test]
fn merge_documents_keeps_both() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("first.md");
    let b = dir.path().join("second.md");
    fs::write(&a, "# Kiln fixture\n\nPart one about glaze.\n").unwrap();
    fs::write(&b, "# Second\n\nPart two about clay.\n").unwrap();
    let out = run(Tool::Merge, &[a, b], &dir).unwrap();
    assert_output(&out[0], "md");
    let text = fs::read_to_string(&out[0]).unwrap();
    assert!(
        text.contains("Part one") && text.contains("Part two"),
        "{text}"
    );
}

#[test]
fn merge_needs_two_files_of_one_type() {
    let (dir, a) = stage(&sample_for("mp3"));
    let err = run(Tool::Merge, std::slice::from_ref(&a), &dir).unwrap_err();
    assert_eq!(
        err.to_string(),
        "Merge needs two or more files of the same type."
    );
    let b = dir.path().join("x.wav");
    fs::copy(sample_for("wav"), &b).unwrap();
    let err = run(Tool::Merge, &[a, b], &dir).unwrap_err();
    assert_eq!(err.to_string(), "Merge needs files of the same type.");
}

#[test]
fn make_smaller_is_not_offered_for_audio() {
    let (dir, input) = stage(&sample_for("wav"));
    let err = run(Tool::Smaller, &[input], &dir).unwrap_err();
    assert_eq!(err.to_string(), "Kiln can't make WAV files smaller.");
}

#[test]
fn making_a_png_smaller_never_grows_it() {
    let (dir, input) = stage(&fixture("bordered.png"));
    match run(Tool::Smaller, std::slice::from_ref(&input), &dir) {
        Ok(out) => {
            assert!(fs::metadata(&out[0]).unwrap().len() < fs::metadata(&input).unwrap().len())
        }
        Err(e) => assert!(matches!(e, Error::NothingToDo(_)), "{e:?}"),
    }
}

#[test]
fn tools_never_touch_the_originals() {
    let (dir, input) = stage(&fixture("padded.wav"));
    let before = fs::read(&input).unwrap();
    run(Tool::RemoveQuietEnds, std::slice::from_ref(&input), &dir).unwrap();
    run(Tool::CutInHalf, std::slice::from_ref(&input), &dir).unwrap();
    assert_eq!(fs::read(&input).unwrap(), before);
    assert_eq!(
        files_in(dir.path()),
        [
            "padded part 1.wav",
            "padded part 2.wav",
            "padded trimmed.wav",
            "padded.wav",
            "work"
        ]
    );
}

/// A PDF with only an owner password opens without a password, and lopdf
/// then forgets it was encrypted. The tools turn it away all the same, as
/// they do one that asks for a password: written out, it would have lost its
/// restrictions.
#[test]
fn the_pdf_tools_turn_away_protected_pdfs() {
    const PROTECTED: &str = "Kiln couldn't convert the file: the PDF is protected with a password.";
    let dir = tempfile::tempdir().unwrap();
    let path = |name: &str| dir.path().join(name);
    secret_pdf(&path("plain.pdf"), 2, None);
    secret_pdf(&path("plain 2.pdf"), 2, None);
    secret_pdf(&path("owner.pdf"), 2, Some(""));
    secret_pdf(&path("user.pdf"), 2, Some("secret"));

    // All three tools work on plain ones.
    let plain = [path("plain.pdf"), path("plain 2.pdf")];
    assert_eq!(run(Tool::SaveEachPage, &plain[..1], &dir).unwrap().len(), 2);
    assert_eq!(run(Tool::Smaller, &plain[..1], &dir).unwrap().len(), 1);
    assert_eq!(run(Tool::Merge, &plain, &dir).unwrap().len(), 1);
    let written = files_in(dir.path());

    for protected in [path("owner.pdf"), path("user.pdf")] {
        let one = std::slice::from_ref(&protected);
        for tool in [Tool::SaveEachPage, Tool::Smaller] {
            let err = run(tool, one, &dir).unwrap_err();
            assert_eq!(err.to_string(), PROTECTED, "{tool:?} on {protected:?}");
        }
        for pair in [
            [protected.clone(), path("plain.pdf")],
            [path("plain.pdf"), protected.clone()],
        ] {
            let err = run(Tool::Merge, &pair, &dir).unwrap_err();
            assert_eq!(err.to_string(), PROTECTED, "merge with {protected:?}");
        }
    }
    assert_eq!(files_in(dir.path()), written, "a refused PDF wrote nothing");
}

/// A 64 x 32 picture with something different in every pixel.
fn busy_picture() -> image::RgbImage {
    image::RgbImage::from_fn(64, 32, |x, y| {
        image::Rgb([(x * 4) as u8, (y * 8) as u8, ((x * y) % 256) as u8])
    })
}

/// Whether `bytes` holds `word`.
fn holds(bytes: &[u8], word: &str) -> bool {
    bytes.windows(word.len()).any(|w| w == word.as_bytes())
}

/// The photo at `path` says nothing of where it was taken or the camera:
/// no Exif beyond its orientation, no XMP, and none of the words.
fn assert_no_location(path: &Path) {
    let bytes = fs::read(path).unwrap();
    for word in CAMERA_WORDS {
        assert!(
            !holds(&bytes, word),
            "{} still holds {word}",
            path.display()
        );
    }
    if let Some(exif) = exif_of(path) {
        assert_eq!(ifd0_tags(&exif), [0x0112], "{}", path.display());
    }
    assert_eq!(xmp_of(path), None, "{}", path.display());
}

#[test]
fn removing_location_data_keeps_the_picture() {
    type Make = fn(&Path, &image::RgbImage, &[u8]);
    let profile = display_p3();
    let dir = tempfile::tempdir().unwrap();
    for (file, make) in [
        ("photo.jpg", camera_jpeg as Make),
        ("photo.png", camera_png),
        ("photo.webp", camera_webp),
    ] {
        let input = dir.path().join(file);
        make(&input, &busy_picture(), &profile);
        let before = fs::read(&input).unwrap();
        // The photo says where it was taken and what took it.
        assert!(
            holds(&before, "Canon") && holds(&before, "xmpmeta"),
            "{file}"
        );
        assert_eq!(ifd0_tags(&exif_of(&input).unwrap()).len(), 5, "{file}");
        let modified = fs::metadata(&input).unwrap().modified().unwrap();

        let out = run(Tool::RemoveLocation, std::slice::from_ref(&input), &dir).unwrap();
        let ext = file.rsplit('.').next().unwrap();
        assert_eq!(name(&out[0]), format!("photo (no location).{ext}"));
        assert_no_location(&out[0]);
        // The colour profile and the turn stay.
        assert_eq!(
            icc_of(&out[0]).as_deref(),
            Some(profile.as_slice()),
            "{file}"
        );
        assert_eq!(
            orientation_of(&out[0]),
            image::metadata::Orientation::Rotate90,
            "{file}"
        );
        // The pixels are the same: the JPEG's coded picture byte for byte,
        // the others decoded.
        let after = fs::read(&out[0]).unwrap();
        if ext == "jpg" {
            assert_eq!(jpeg_scan(&after), jpeg_scan(&before));
        } else {
            let pixels = |p: &Path| image::open(p).unwrap().to_rgba8();
            assert_eq!(pixels(&out[0]), pixels(&input), "{file}");
        }
        // The photo itself is untouched.
        assert_eq!(fs::read(&input).unwrap(), before, "{file}");
        assert_eq!(fs::metadata(&input).unwrap().modified().unwrap(), modified);
    }
    // A second run never writes over the first.
    let again = run(Tool::RemoveLocation, &[dir.path().join("photo.jpg")], &dir).unwrap();
    assert_eq!(name(&again[0]), "photo (no location) (1).jpg");
}

#[test]
fn a_photo_with_nothing_to_remove_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let photo = dir.path().join("plain.jpg");
    image::DynamicImage::ImageRgb8(busy_picture())
        .save(&photo)
        .unwrap();
    let err = run(Tool::RemoveLocation, &[photo], &dir).unwrap_err();
    assert_eq!(
        err.to_string(),
        "There's no location data or camera details to remove."
    );
    assert_eq!(files_in(dir.path()), ["plain.jpg", "work"]);
}

#[test]
fn removing_location_data_from_other_photos_saves_them_again() {
    // A TIFF is written again with the same pixels.
    let (dir, input) = stage(&sample_for("tiff"));
    let out = run(Tool::RemoveLocation, std::slice::from_ref(&input), &dir).unwrap();
    assert_eq!(name(&out[0]), "sample (no location).tiff");
    let pixels = |p: &Path| image::open(p).unwrap().to_rgba8();
    assert_eq!(pixels(&out[0]), pixels(&input));

    // Kiln can't write HEIC, so a HEIC comes out as a JPG.
    let (dir, input) = stage(&sample_for("heic"));
    match run(Tool::RemoveLocation, &[input], &dir) {
        Ok(out) => {
            assert_eq!(name(&out[0]), "sample (no location).jpg");
            assert_eq!(image::open(&out[0]).unwrap().dimensions(), (64, 48));
            assert_no_location(&out[0]);
        }
        Err(Error::NeedsExtension { message, .. }) => {
            eprintln!("HEIC: skipped, nothing here reads it ({message})")
        }
        Err(e) => panic!("{e:?}"),
    }
}

/// Options for Make smaller to `max` bytes, with the long side at most
/// `long_side`, and location data removed when `remove_location` is set.
fn sized(
    dir: &tempfile::TempDir,
    max: u64,
    long_side: Option<u32>,
    remove_location: bool,
) -> Options {
    Options {
        tool: ToolOptions {
            max_bytes: Some(max),
            long_side,
            remove_location,
            ..ToolOptions::default()
        },
        ..options(dir)
    }
}

/// A photo-like picture, `w` x `h`: smooth light with a fine grain.
fn grainy(w: u32, h: u32) -> image::RgbImage {
    let mut seed = 0x2545_F491u32;
    image::RgbImage::from_fn(w, h, |x, y| {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        let grain = (seed >> 16) as i32 % 64 - 32;
        let c = |base: u32| (base as i32 + grain).clamp(0, 255) as u8;
        image::Rgb([
            c(x * 200 / w + 20),
            c(y * 200 / h + 30),
            c((x + y) * 100 / (w + h) + 60),
        ])
    })
}

fn size_of(path: &Path) -> u64 {
    fs::metadata(path).unwrap().len()
}

#[test]
fn a_photo_is_made_smaller_to_the_size_asked() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("big.jpg");
    jpeg_with(&input, &grainy(2400, 1600), 1, None, 96);
    let size = size_of(&input);
    assert!(
        (2_500_000..4_000_000).contains(&size),
        "the photo is {size} bytes"
    );
    let before = fs::read(&input).unwrap();
    let modified = fs::metadata(&input).unwrap().modified().unwrap();

    let out = run_with(
        Tool::Smaller,
        std::slice::from_ref(&input),
        &sized(&dir, 1_000_000, None, false),
        &dir,
    )
    .unwrap();
    assert_eq!(name(&out[0]), "big (smaller).jpg");
    let made = size_of(&out[0]);
    assert!((700_001..=1_000_000).contains(&made), "made {made} bytes");
    assert_eq!(
        image::guess_format(&fs::read(&out[0]).unwrap()).unwrap(),
        image::ImageFormat::Jpeg
    );

    // With Resize as well, and a second run never writes over the first.
    let out = run_with(
        Tool::Smaller,
        std::slice::from_ref(&input),
        &sized(&dir, 500_000, Some(1920), false),
        &dir,
    )
    .unwrap();
    assert_eq!(name(&out[0]), "big (smaller) (1).jpg");
    assert!(size_of(&out[0]) <= 500_000, "made {}", size_of(&out[0]));
    let (w, h) = image::open(&out[0]).unwrap().dimensions();
    assert!(w.max(h) <= 1920, "{w} x {h}");

    assert_eq!(fs::read(&input).unwrap(), before);
    assert_eq!(fs::metadata(&input).unwrap().modified().unwrap(), modified);
}

#[test]
fn the_size_a_photo_will_come_to_is_worked_out_without_writing_anything() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("big.jpg");
    jpeg_with(&input, &grainy(2400, 1600), 1, None, 96);
    let size = size_of(&input);
    let before = fs::read(&input).unwrap();
    let kiln = Kiln::new();
    let cancel = CancelToken::new();

    let n = kiln
        .smaller_size(&input, &sized(&dir, 1_000_000, None, false), &cancel)
        .unwrap();
    assert!((700_001..=1_000_000).contains(&n), "{n} bytes");
    // The same as the tool makes.
    let out = run_with(
        Tool::Smaller,
        std::slice::from_ref(&input),
        &sized(&dir, 1_000_000, None, false),
        &dir,
    )
    .unwrap();
    assert_eq!(size_of(&out[0]), n);
    fs::remove_file(&out[0]).unwrap();

    // Out of reach: the smallest it gets to.
    match kiln.smaller_size(&input, &sized(&dir, 1_000, None, false), &cancel) {
        Err(Error::OutOfReach {
            max_bytes: 1_000,
            smallest,
        }) => assert!(smallest > 1_000, "{smallest}"),
        other => panic!("{other:?}"),
    }
    // Already under the size.
    let err = kiln
        .smaller_size(&input, &sized(&dir, size, None, false), &cancel)
        .unwrap_err();
    assert!(matches!(err, Error::NothingToDo(_)), "{err:?}");
    // Cancelled.
    let stopped = CancelToken::new();
    stopped.cancel();
    let err = kiln
        .smaller_size(&input, &sized(&dir, 500_000, None, false), &stopped)
        .unwrap_err();
    assert!(matches!(err, Error::Cancelled), "{err:?}");

    // Nothing was written: not even a work folder.
    assert_eq!(files_in(dir.path()), ["big.jpg", "work"]);
    assert!(files_in(&dir.path().join("work")).is_empty());
    assert_eq!(fs::read(&input).unwrap(), before);
}

#[test]
fn a_jpg_kiln_cant_read_itself_is_estimated_and_made_smaller_through_ffmpeg() {
    // Decision D21: a JPG Kiln's own reader can't read is read by
    // FFmpeg when it is there, for the estimate as for the job. A lossless
    // JPEG (SOF3) is such a file.
    let test = "a_jpg_kiln_cant_read_itself_is_estimated_and_made_smaller_through_ffmpeg";
    if !support::have_ffmpeg(test) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("lossless.jpg");
    let made = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-f", "lavfi", "-i"])
        .arg("testsrc2=size=1280x720:rate=1")
        .args(["-frames:v", "1", "-c:v", "ljpeg", "-pix_fmt", "bgr24"])
        .arg(&input)
        .status()
        .unwrap();
    assert!(made.success());
    assert!(image::open(&input).is_err(), "Kiln's reader takes it now");
    let size = size_of(&input);
    let max = size / 4;
    let kiln = Kiln::new();
    let cancel = CancelToken::new();

    let n = kiln
        .smaller_size(&input, &sized(&dir, max, None, false), &cancel)
        .unwrap();
    assert!(n <= max, "{n} bytes over {max}");
    // The work folder FFmpeg's picture went to is gone.
    assert!(files_in(&dir.path().join("work")).is_empty());
    // The tool makes the same.
    let out = run_with(
        Tool::Smaller,
        std::slice::from_ref(&input),
        &sized(&dir, max, None, false),
        &dir,
    )
    .unwrap();
    assert_eq!(size_of(&out[0]), n);
    assert_eq!(image::open(&out[0]).unwrap().dimensions(), (1280, 720));
    fs::remove_file(&out[0]).unwrap();

    // Without FFmpeg, the estimate fails as before, plainly: the dialog
    // says Kiln couldn't work out the size.
    let mut no_ffmpeg = sized(&dir, max, None, false);
    no_ffmpeg.engines.ffmpeg = Some(dir.path().join("gone").join("ffmpeg.exe"));
    let err = kiln.smaller_size(&input, &no_ffmpeg, &cancel).unwrap_err();
    assert!(matches!(err, Error::Decode { .. }), "{err:?}");
    assert!(files_in(&dir.path().join("work")).is_empty());
}

#[test]
fn a_png_made_smaller_has_fewer_pixels_and_stays_a_png() {
    let profile = display_p3();
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("shot.png");
    png_with(&input, &grainy(300, 200), 1, Some(&profile));
    let out = run_with(
        Tool::Smaller,
        std::slice::from_ref(&input),
        &sized(&dir, 40_000, None, false),
        &dir,
    )
    .unwrap();
    assert_eq!(name(&out[0]), "shot (smaller).png");
    assert!(size_of(&out[0]) <= 40_000, "made {}", size_of(&out[0]));
    assert_eq!(
        image::guess_format(&fs::read(&out[0]).unwrap()).unwrap(),
        image::ImageFormat::Png
    );
    let (w, h) = image::open(&out[0]).unwrap().dimensions();
    assert!(w < 300 && h < 200, "{w} x {h}");
    assert!((w as f32 / h as f32 - 1.5).abs() < 0.05, "{w} x {h}");
    assert_eq!(icc_of(&out[0]).as_deref(), Some(profile.as_slice()));
}

#[test]
fn a_webp_is_made_smaller_too() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("pic.webp");
    image::DynamicImage::ImageRgb8(grainy(600, 400))
        .save(&input)
        .unwrap();
    let out = run_with(
        Tool::Smaller,
        std::slice::from_ref(&input),
        &sized(&dir, 30_000, None, false),
        &dir,
    )
    .unwrap();
    assert_eq!(name(&out[0]), "pic (smaller).webp");
    assert!(size_of(&out[0]) <= 30_000, "made {}", size_of(&out[0]));
    assert_eq!(
        image::guess_format(&fs::read(&out[0]).unwrap()).unwrap(),
        image::ImageFormat::WebP
    );
}

#[test]
fn a_size_out_of_reach_saves_nothing_and_says_how_small_it_can_get() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("photo.jpg");
    jpeg_with(&input, &busy_picture(), 1, Some(&display_p3()), 100);
    let err = run_with(
        Tool::Smaller,
        std::slice::from_ref(&input),
        &sized(&dir, 1_000, None, false),
        &dir,
    )
    .unwrap_err();
    match &err {
        Error::OutOfReach {
            max_bytes: 1_000,
            smallest,
        } => assert!(*smallest > 1_000, "{smallest}"),
        other => panic!("{other:?}"),
    }
    let said = err.to_string();
    assert!(
        said.starts_with("Can't get this under 1 KB. The smallest Kiln can make is about "),
        "{said}"
    );
    assert_eq!(files_in(dir.path()), ["photo.jpg", "work"]);

    // A size the photo is already under is turned down.
    let size = size_of(&input);
    let err = run_with(
        Tool::Smaller,
        std::slice::from_ref(&input),
        &sized(&dir, size + 1, None, false),
        &dir,
    )
    .unwrap_err();
    assert!(matches!(err, Error::NothingToDo(_)), "{err:?}");
    assert_eq!(
        err.to_string(),
        format!("This file is already smaller than {}.", about(size + 1))
    );
    assert_eq!(files_in(dir.path()), ["photo.jpg", "work"]);
    // So is one with a long side the photo is already within.
    let err = run_with(
        Tool::Smaller,
        std::slice::from_ref(&input),
        &sized(&dir, size + 1, Some(64), false),
        &dir,
    )
    .unwrap_err();
    assert!(matches!(err, Error::NothingToDo(_)), "{err:?}");
    assert_eq!(files_in(dir.path()), ["photo.jpg", "work"]);

    // With a long side the photo is over, it is still brought down to it,
    // and comes out smaller than it was.
    let out = run_with(
        Tool::Smaller,
        std::slice::from_ref(&input),
        &sized(&dir, size + 1, Some(32), false),
        &dir,
    )
    .unwrap();
    assert_eq!(image::open(&out[0]).unwrap().dimensions(), (32, 16));
    assert!(size_of(&out[0]) < size);
    assert_eq!(fs::read(&input).unwrap().len() as u64, size);
}

#[test]
fn making_a_photo_smaller_keeps_its_details_unless_asked_not_to() {
    type Make = fn(&Path, &image::RgbImage, &[u8]);
    let profile = display_p3();
    let dir = tempfile::tempdir().unwrap();
    for (file, make) in [
        ("photo.jpg", camera_jpeg as Make),
        ("photo.png", camera_png),
        ("photo.webp", camera_webp),
    ] {
        let input = dir.path().join(file);
        make(&input, &busy_picture(), &profile);
        let max = size_of(&input) - 1;

        // Kept: the camera, the place and the colour profile, with the
        // turn now in the pixels.
        let out = run_with(
            Tool::Smaller,
            std::slice::from_ref(&input),
            &sized(&dir, max, None, false),
            &dir,
        )
        .unwrap();
        assert!(size_of(&out[0]) <= max, "{file}");
        assert_eq!(ifd0_tags(&exif_of(&out[0]).unwrap()).len(), 5, "{file}");
        let bytes = fs::read(&out[0]).unwrap();
        assert!(holds(&bytes, "Canon") && holds(&bytes, "SN-0042"), "{file}");
        let xmp = String::from_utf8(xmp_of(&out[0]).unwrap()).unwrap();
        assert!(xmp.contains("tiff:Orientation=\"1\""), "{file}: {xmp}");
        assert_eq!(
            orientation_of(&out[0]),
            image::metadata::Orientation::NoTransforms,
            "{file}"
        );
        let (w, h) = image::open(&out[0]).unwrap().dimensions();
        assert!(w < h, "{file} is upright: {w} x {h}");
        assert_eq!(
            icc_of(&out[0]).as_deref(),
            Some(profile.as_slice()),
            "{file}"
        );

        // Removed, when asked.
        let out = run_with(
            Tool::Smaller,
            std::slice::from_ref(&input),
            &sized(&dir, max, None, true),
            &dir,
        )
        .unwrap();
        assert_no_location(&out[0]);
        assert_eq!(
            icc_of(&out[0]).as_deref(),
            Some(profile.as_slice()),
            "{file}"
        );
    }
}

/// Options for Resize with the long side at `side`.
fn long_side(dir: &tempfile::TempDir, side: u32) -> Options {
    Options {
        tool: ToolOptions {
            long_side: Some(side),
            ..ToolOptions::default()
        },
        ..options(dir)
    }
}

#[test]
fn resize_brings_the_long_side_down_and_never_up() {
    let profile = display_p3();
    let dir = tempfile::tempdir().unwrap();
    // Stored 64 wide and 32 high, shown 32 wide and 64 high.
    let input = dir.path().join("photo.jpg");
    jpeg_with(&input, &busy_picture(), 6, Some(&profile), 95);
    let before = fs::read(&input).unwrap();
    let modified = fs::metadata(&input).unwrap().modified().unwrap();
    let one = std::slice::from_ref(&input);

    let out = run_with(Tool::Resize, one, &long_side(&dir, 40), &dir).unwrap();
    assert_eq!(name(&out[0]), "photo (40 px).jpg");
    assert_eq!(image::open(&out[0]).unwrap().dimensions(), (20, 40));
    assert_eq!(
        image::guess_format(&fs::read(&out[0]).unwrap()).unwrap(),
        image::ImageFormat::Jpeg
    );
    assert_eq!(icc_of(&out[0]).as_deref(), Some(profile.as_slice()));
    assert_eq!(
        orientation_of(&out[0]),
        image::metadata::Orientation::NoTransforms
    );
    // A second run never writes over the first.
    let out = run_with(Tool::Resize, one, &long_side(&dir, 40), &dir).unwrap();
    assert_eq!(name(&out[0]), "photo (40 px) (1).jpg");

    // A PNG stays a PNG.
    let png = dir.path().join("shot.png");
    png_with(&png, &grainy(300, 200), 1, Some(&profile));
    let out = run_with(Tool::Resize, &[png], &long_side(&dir, 150), &dir).unwrap();
    assert_eq!(name(&out[0]), "shot (150 px).png");
    assert_eq!(image::open(&out[0]).unwrap().dimensions(), (150, 100));
    assert_eq!(
        image::guess_format(&fs::read(&out[0]).unwrap()).unwrap(),
        image::ImageFormat::Png
    );
    assert_eq!(icc_of(&out[0]).as_deref(), Some(profile.as_slice()));

    // Never larger.
    let err = run_with(Tool::Resize, one, &long_side(&dir, 64), &dir).unwrap_err();
    assert!(matches!(err, Error::NothingToDo(_)), "{err:?}");
    assert_eq!(
        err.to_string(),
        "This picture's long side is already 64 px."
    );
    // And never without a size.
    let err = run(Tool::Resize, one, &dir).unwrap_err();
    assert!(matches!(err, Error::ToolUnavailable(_)), "{err:?}");
    assert_eq!(err.to_string(), "Resize needs a size for the long side.");

    assert_eq!(fs::read(&input).unwrap(), before);
    assert_eq!(fs::metadata(&input).unwrap().modified().unwrap(), modified);
}

#[test]
fn resizing_a_heic_saves_a_jpg() {
    let (dir, input) = stage(&sample_for("heic"));
    match run_with(Tool::Resize, &[input], &long_side(&dir, 32), &dir) {
        Ok(out) => {
            assert_eq!(name(&out[0]), "sample (32 px).jpg");
            assert_eq!(image::open(&out[0]).unwrap().dimensions(), (32, 24));
        }
        Err(Error::NeedsExtension { message, .. }) => {
            eprintln!("HEIC: skipped, nothing here reads it ({message})")
        }
        Err(e) => panic!("{e:?}"),
    }
}

/// The words in the PDF at `path`, read as the text conversion reads them.
fn pdf_words(path: &Path) -> String {
    pdf_extract::extract_text_from_mem(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn a_scanned_pdf_is_made_smaller_to_the_size_asked() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("scan.pdf");
    pdfs::scan_pdf(&input, 3, 1200, 1600);
    let size = size_of(&input);
    assert!(size > 2_000_000, "the scan is {size} bytes");
    let words = pdf_words(&input);
    assert!(words.contains("Scanned page 3"), "{words:?}");
    let before = fs::read(&input).unwrap();
    let modified = fs::metadata(&input).unwrap().modified().unwrap();

    let out = run_with(
        Tool::Smaller,
        std::slice::from_ref(&input),
        &sized(&dir, 1_000_000, None, false),
        &dir,
    )
    .unwrap();
    assert_eq!(out, [dir.path().join("scan (smaller).pdf")]);
    let got = size_of(&out[0]);
    assert!(got <= 1_000_000, "{got} bytes");
    eprintln!("scan: {size} bytes made {got} bytes for 1 MB");
    let doc = lopdf::Document::load(&out[0]).unwrap();
    assert_eq!(doc.get_pages().len(), 3);
    let words = pdf_words(&out[0]);
    for n in 1..=3 {
        assert!(words.contains(&format!("Scanned page {n}")), "{words:?}");
    }
    // The photos are still photos, as JPG.
    let photos: Vec<_> = doc
        .objects
        .values()
        .filter_map(|o| o.as_stream().ok())
        .filter(|s| pdfs::filter_of(s) == b"DCTDecode")
        .collect();
    assert_eq!(photos.len(), 3);

    assert_eq!(fs::read(&input).unwrap(), before);
    assert_eq!(fs::metadata(&input).unwrap().modified().unwrap(), modified);
    assert_eq!(
        files_in(dir.path()),
        ["scan (smaller).pdf", "scan.pdf", "work"]
    );
}

#[test]
fn a_pdf_of_text_out_of_reach_saves_nothing_and_says_how_small_it_can_get() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("words.pdf");
    pdfs::text_pdf(&input, 6);
    let size = size_of(&input);
    let err = run_with(
        Tool::Smaller,
        std::slice::from_ref(&input),
        &sized(&dir, 1_000, None, false),
        &dir,
    )
    .unwrap_err();
    match &err {
        // Tidied, it is smaller than it was, but nowhere near 1 KB.
        Error::OutOfReach {
            max_bytes: 1_000,
            smallest,
        } => {
            eprintln!("words: {size} bytes, smallest {smallest} for 1 KB");
            assert!((1_000..size).contains(smallest), "{smallest} of {size}")
        }
        other => panic!("{other:?}"),
    }
    let said = err.to_string();
    assert!(
        said.starts_with("Can't get this under 1 KB. The smallest Kiln can make is about "),
        "{said}"
    );
    assert_eq!(files_in(dir.path()), ["words.pdf", "work"]);

    // A size it is already under is turned down.
    let err = run_with(
        Tool::Smaller,
        std::slice::from_ref(&input),
        &sized(&dir, size, None, false),
        &dir,
    )
    .unwrap_err();
    assert!(matches!(err, Error::NothingToDo(_)), "{err:?}");
    assert_eq!(
        err.to_string(),
        format!("This file is already smaller than {}.", about(size))
    );
    assert_eq!(files_in(dir.path()), ["words.pdf", "work"]);
}

#[test]
fn the_size_a_pdf_will_come_to_is_worked_out_without_writing_anything() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("scan.pdf");
    pdfs::scan_pdf(&input, 3, 1200, 1600);
    let kiln = Kiln::new();
    let cancel = CancelToken::new();
    let n = kiln
        .smaller_size(&input, &sized(&dir, 1_000_000, None, false), &cancel)
        .unwrap();
    assert!(n <= 1_000_000, "{n} bytes");
    let out = run_with(
        Tool::Smaller,
        std::slice::from_ref(&input),
        &sized(&dir, 1_000_000, None, false),
        &dir,
    )
    .unwrap();
    assert_eq!(size_of(&out[0]), n);
    fs::remove_file(&out[0]).unwrap();

    let words = dir.path().join("words.pdf");
    pdfs::text_pdf(&words, 6);
    match kiln.smaller_size(&words, &sized(&dir, 1_000, None, false), &cancel) {
        Err(Error::OutOfReach {
            max_bytes: 1_000,
            smallest,
        }) => assert!(smallest > 1_000, "{smallest}"),
        other => panic!("{other:?}"),
    }
    let mut names = files_in(dir.path());
    names.retain(|n| n != "work");
    assert_eq!(names, ["scan.pdf", "words.pdf"]);
}

#[test]
fn making_a_pdf_smaller_keeps_masks_fonts_and_drawing() {
    use pdfs::{filter_of, find_stream, is_image, MASKED, PHOTO};
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("mixed.pdf");
    pdfs::mixed_pdf(&input);
    let size = size_of(&input);
    let was = lopdf::Document::load(&input).unwrap();
    let photo_len = find_stream(&was, |s| is_image(s, PHOTO, b"DeviceRGB"))
        .content
        .len() as u64;
    let before = fs::read(&input).unwrap();

    // Reachable only by making the Flate photo a JPG.
    let max = size - photo_len / 2;
    let out = run_with(
        Tool::Smaller,
        std::slice::from_ref(&input),
        &sized(&dir, max, None, false),
        &dir,
    )
    .unwrap();
    assert_eq!(out, [dir.path().join("mixed (smaller).pdf")]);
    assert!(size_of(&out[0]) <= max);
    eprintln!("mixed: {size} bytes made {} for {max}", size_of(&out[0]));
    let now = lopdf::Document::load(&out[0]).unwrap();

    // The Flate photo without a mask became a JPG, at its size.
    let photo = find_stream(&now, |s| is_image(s, PHOTO, b"DeviceRGB"));
    assert_eq!(filter_of(photo), b"DCTDecode");
    assert!(photo.dict.get(b"DecodeParms").is_err());

    // The masked photo and its mask are as they were, byte for byte.
    let masked = |doc: &lopdf::Document| -> (Vec<u8>, Vec<u8>) {
        let s = find_stream(doc, |s| is_image(s, MASKED, b"DeviceRGB"));
        let mask_id = s.dict.get(b"SMask").unwrap().as_reference().unwrap();
        let mask = doc.get_object(mask_id).unwrap().as_stream().unwrap();
        assert_eq!(filter_of(mask), b"FlateDecode");
        (s.content.clone(), mask.content.clone())
    };
    assert_eq!(masked(&now), masked(&was));
    // So are the font program and the page's drawing.
    let font = |doc: &lopdf::Document| find_stream(doc, |s| s.dict.has(b"Length1")).content.clone();
    assert_eq!(font(&now), font(&was));
    assert_eq!(pdfs::page_content(&now), pdfs::page_content(&was));
    let contents = |doc: &lopdf::Document| {
        let (_, page) = doc.get_pages().into_iter().next().unwrap();
        let id = doc.get_page_contents(page)[0];
        doc.get_object(id)
            .unwrap()
            .as_stream()
            .unwrap()
            .content
            .clone()
    };
    assert_eq!(contents(&now), contents(&was));
    assert!(pdf_words(&out[0]).contains("Drawn words"));

    assert_eq!(fs::read(&input).unwrap(), before);
}

#[test]
fn a_pdf_with_no_size_keeps_its_flate_photos_as_they_are() {
    // As Compress did: with no size, only JPG photos are saved again, so a
    // lossless picture never becomes a JPG unasked.
    use pdfs::{filter_of, find_stream, is_image, PHOTO};
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("mixed.pdf");
    pdfs::mixed_pdf(&input);
    let before = fs::read(&input).unwrap();
    match run_with(
        Tool::Smaller,
        std::slice::from_ref(&input),
        &options(&dir),
        &dir,
    ) {
        Ok(out) => {
            let now = lopdf::Document::load(&out[0]).unwrap();
            let photo = find_stream(&now, |s| is_image(s, PHOTO, b"DeviceRGB"));
            assert_eq!(filter_of(photo), b"FlateDecode");
        }
        Err(Error::NothingToDo(_)) => {}
        Err(e) => panic!("{e:?}"),
    }
    assert_eq!(fs::read(&input).unwrap(), before);
}

#[test]
fn a_pdf_cancelled_part_way_leaves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("mixed.pdf");
    pdfs::mixed_pdf(&input);
    let before = fs::read(&input).unwrap();
    let cancel = CancelToken::new();
    let mut heard = 0;
    let err = Kiln::new()
        .run_tool_with(
            Tool::Smaller,
            std::slice::from_ref(&input),
            &sized(&dir, size_of(&input) / 2, None, false),
            &mut |p| {
                // Cancelled once the work has begun, not before.
                if matches!(p, kiln_core::Progress::Fraction(f) if f > 0.0 && f < 1.0) {
                    heard += 1;
                    cancel.cancel();
                }
            },
            &cancel,
        )
        .unwrap_err();
    assert!(matches!(err, Error::Cancelled), "{err:?}");
    assert!(heard > 0);
    assert_eq!(files_in(dir.path()), ["mixed.pdf", "work"]);
    assert!(files_in(&dir.path().join("work")).is_empty());
    assert_eq!(fs::read(&input).unwrap(), before);
}

/// A JPG built in code at `path`, whole, and its bytes.
fn whole_jpeg(path: &Path) -> Vec<u8> {
    jpeg_with(path, &busy_picture(), 1, None, 90);
    fs::read(path).unwrap()
}

#[test]
fn a_jpg_cut_short_is_refused_wherever_it_is_read() {
    // Decision D26: a JPG with no end marker is refused before it is
    // decoded, saying it looks cut short. Kiln's reader fills what is
    // missing with grey, which would be saved as if whole.
    let dir = tempfile::tempdir().unwrap();
    let whole = dir.path().join("whole.jpg");
    let bytes = whole_jpeg(&whole);
    fs::remove_file(&whole).unwrap();
    let cut = dir.path().join("cut.jpg");
    fs::write(&cut, &bytes[..bytes.len() / 2]).unwrap();
    assert!(image::open(&cut).is_ok(), "Kiln's reader refuses it itself");
    let kiln = Kiln::new();
    let cancel = CancelToken::new();
    let refused = |what: &str, err: Error| {
        assert!(matches!(err, Error::CutShort { .. }), "{what}: {err:?}");
        assert_eq!(
            JobError::from(err).message,
            "This JPG looks cut short. Try copying or downloading it again.",
            "{what}"
        );
    };

    // On adding, the row's look finds it.
    let size = fs::metadata(&cut).unwrap().len();
    assert_eq!(
        kiln_core::check_file(&cut, Some("jpg"), size),
        FileCheck::CutShort
    );
    // Conversion, to a picture and to a PDF.
    for to in ["png", "webp", "pdf"] {
        refused(to, kiln.convert(&cut, to, &options(&dir)).unwrap_err());
    }
    // Make smaller, Resize and Remove location data.
    for tool in [Tool::Smaller, Tool::Resize, Tool::RemoveLocation] {
        let opts = sized(&dir, size / 4, Some(16), false);
        let err = run_with(tool, std::slice::from_ref(&cut), &opts, &dir).unwrap_err();
        refused(&format!("{tool:?}"), err);
    }
    // Make smaller's estimate, and the facts' estimates.
    // Asked for a size under the file's own, so it has to be read.
    let opts = sized(&dir, size / 4, None, false);
    refused(
        "estimate",
        kiln.smaller_size(&cut, &opts, &cancel).unwrap_err(),
    );
    let facts = kiln_core::inspect(&kiln, &cut, &options(&dir), &cancel).unwrap();
    assert_eq!(facts.picture, None);
    assert!(facts.estimates.is_empty(), "{:?}", facts.estimates);

    let work = dir.path().join("work");
    if work.exists() {
        assert!(files_in(&work).is_empty());
    }
    assert!(
        files_in(dir.path())
            .iter()
            .all(|f| f == "cut.jpg" || f == "work"),
        "{:?}",
        files_in(dir.path())
    );
}

#[test]
fn a_whole_jpg_converts_as_before_with_or_without_bytes_after_its_end() {
    let dir = tempfile::tempdir().unwrap();
    let whole = dir.path().join("whole.jpg");
    let bytes = whole_jpeg(&whole);
    // Some cameras add data after the end marker: a phone's motion photo
    // puts a whole video there.
    let mut video = b"\0\0\0\x18ftypmp42".to_vec();
    video.extend((0..200_000u32).map(|i| (i * 7 % 251) as u8));
    let trailed = dir.path().join("motion.jpg");
    fs::write(&trailed, [bytes.as_slice(), &video].concat()).unwrap();
    let kiln = Kiln::new();
    for input in [&whole, &trailed] {
        let size = fs::metadata(input).unwrap().len();
        assert_eq!(
            kiln_core::check_file(input, Some("jpg"), size),
            FileCheck::Ok
        );
        let out = kiln.convert(input, "png", &options(&dir)).unwrap();
        assert_eq!(image::open(&out).unwrap().dimensions(), (64, 32));
        let n = kiln
            .smaller_size(
                input,
                &sized(&dir, size / 2, None, false),
                &CancelToken::new(),
            )
            .unwrap();
        assert!(n > 0);
    }
}
