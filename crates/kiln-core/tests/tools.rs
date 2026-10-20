//! Compress, crop, trim, split and merge, with the fixtures.

#[path = "../../../tests/support/mod.rs"]
mod support;

use std::fs;
use std::path::{Path, PathBuf};

use image::GenericImageView;
use kiln_core::{Error, Kiln, Options, Tool};
use support::{assert_output, files_in, fixture, probe, sample_for, stage};

fn options(temp: &tempfile::TempDir) -> Options {
    Options {
        temp_root: Some(temp.path().join("work")),
        ..Options::default()
    }
}

fn run(tool: Tool, inputs: &[PathBuf], dir: &tempfile::TempDir) -> kiln_core::Result<Vec<PathBuf>> {
    let out = Kiln::new().run_tool(tool, inputs, &options(dir));
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
    assert_eq!(t(&["a.png"]), [Compress, Crop]);
    assert_eq!(t(&["a.mp3"]), [Compress, Trim, Split]);
    assert_eq!(t(&["a.wav"]), [Trim, Split]);
    assert_eq!(t(&["a.mp4"]), [Compress, Crop, Trim, Split]);
    // Compress only where it has re-encode settings.
    assert_eq!(t(&["a.avi"]), [Crop, Trim, Split]);
    assert_eq!(t(&["a.ts"]), [Crop, Trim, Split]);
    assert_eq!(t(&["a.aac"]), [Trim, Split]);
    assert_eq!(t(&["a.jpg"]), [Compress, Crop]);
    assert_eq!(t(&["a.gif"]), [Crop]);
    assert_eq!(t(&["a.mp3", "b.mp3"]), [Compress, Trim, Split, Merge]);
    assert_eq!(t(&["a.mp3", "b.wav"]), [Trim, Split]);
    assert_eq!(t(&["a.docx", "b.docx"]), [Merge]);
    assert_eq!(t(&["a.docx"]), Vec::<Tool>::new());
    // Compress works on each file in turn, so mixed types still share it.
    assert_eq!(t(&["a.png", "b.mp3"]), [Compress]);
    assert_eq!(t(&["a.png", "b.docx"]), Vec::<Tool>::new());
    assert_eq!(t(&["a.xyz"]), Vec::<Tool>::new());
}

#[test]
fn crop_removes_a_plain_image_border() {
    let (dir, input) = stage(&fixture("bordered.png"));
    let out = run(Tool::Crop, &[input], &dir).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(name(&out[0]), "bordered cropped.png");
    assert_eq!(image::open(&out[0]).unwrap().dimensions(), (28, 16));
}

#[test]
fn crop_says_so_when_there_is_no_border() {
    let (dir, input) = stage(&fixture("rgb_gradient.png"));
    let err = run(Tool::Crop, &[input], &dir).unwrap_err();
    assert_eq!(err.to_string(), "There's no plain border to crop.");
    assert_eq!(files_in(dir.path()), ["rgb_gradient.png", "work"]);
}

#[test]
fn crop_removes_black_bars_from_video() {
    let (dir, input) = stage(&fixture("letterbox.mp4"));
    let out = run(Tool::Crop, &[input], &dir).unwrap();
    let p = probe(&out[0]);
    assert_eq!((p.width, p.height), (Some(64), Some(32)));
    assert_eq!(p.audio, 1);
}

#[test]
fn crop_on_a_full_frame_video_has_nothing_to_do() {
    let (dir, input) = stage(&sample_for("mp4"));
    let err = run(Tool::Crop, &[input], &dir).unwrap_err();
    assert!(matches!(err, Error::NothingToDo(_)), "{err:?}");
}

#[test]
fn trim_cuts_leading_and_trailing_silence() {
    let (dir, input) = stage(&fixture("padded.wav"));
    let out = run(Tool::Trim, &[input], &dir).unwrap();
    assert_eq!(name(&out[0]), "padded trimmed.wav");
    let d = probe(&out[0]).duration;
    assert!((d - 1.0).abs() < 0.1, "trimmed to {d}");
}

#[test]
fn trim_without_silence_has_nothing_to_do() {
    let (dir, input) = stage(&sample_for("wav"));
    let err = run(Tool::Trim, &[input], &dir).unwrap_err();
    assert_eq!(
        err.to_string(),
        "There's no silence at the start or end to trim."
    );
}

#[test]
fn trim_cuts_black_silent_video_edges() {
    let (dir, input) = stage(&fixture("padded.mp4"));
    let out = run(Tool::Trim, &[input], &dir).unwrap();
    assert_eq!(name(&out[0]), "padded trimmed.mp4");
    let p = probe(&out[0]);
    assert!((p.duration - 1.0).abs() < 0.2, "trimmed to {}", p.duration);
    assert_eq!((p.video, p.audio), (1, 1));
    assert_eq!((p.width, p.height), (Some(64), Some(48)));
}

#[test]
fn trim_on_video_without_black_edges_has_nothing_to_do() {
    let (dir, input) = stage(&sample_for("mp4"));
    let err = run(Tool::Trim, &[input], &dir).unwrap_err();
    assert_eq!(
        err.to_string(),
        "There's no black, silent stretch at the start or end to trim."
    );
}

#[test]
fn split_makes_two_halves() {
    let (dir, input) = stage(&sample_for("mp3"));
    let out = run(Tool::Split, &[input], &dir).unwrap();
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
    let out = run(Tool::Split, &[input], &dir).unwrap();
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
fn compress_makes_media_smaller() {
    for ext in ["mp4", "flac", "m4a"] {
        let (dir, input) = stage(&sample_for(ext));
        let before = fs::metadata(&input).unwrap().len();
        match run(Tool::Compress, std::slice::from_ref(&input), &dir) {
            Ok(out) => {
                assert_eq!(name(&out[0]), format!("sample compressed.{ext}"));
                assert!(fs::metadata(&out[0]).unwrap().len() < before);
                assert_output(&out[0], ext);
            }
            // Tiny fixtures can already be minimal; that must be said plainly.
            Err(e) => assert_eq!(
                e.to_string(),
                "This file is already as small as Kiln can make it."
            ),
        }
    }
}

#[test]
fn compress_a_wav_is_not_offered() {
    let (dir, input) = stage(&sample_for("wav"));
    let err = run(Tool::Compress, &[input], &dir).unwrap_err();
    assert_eq!(err.to_string(), "Kiln can't compress WAV files.");
}

#[test]
fn compressing_a_png_never_grows_it() {
    let (dir, input) = stage(&fixture("bordered.png"));
    match run(Tool::Compress, std::slice::from_ref(&input), &dir) {
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
    run(Tool::Trim, std::slice::from_ref(&input), &dir).unwrap();
    run(Tool::Split, std::slice::from_ref(&input), &dir).unwrap();
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
