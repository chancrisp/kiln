//! Make smaller for video, on Kiln's own engine (spec 3.2): to a size, in
//! the same container, with a smaller picture or no sound when asked, and
//! never a file over the size or larger than the original (R4).
//!
//! The videos are made with FFmpeg (tests/support/clips.rs); without it, or
//! without Media Foundation's codecs, each test says it is skipped.

#![cfg(windows)]

#[path = "../../../tests/support/mod.rs"]
mod support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use kiln_core::{about, Error, Kiln, Options, Tool, ToolOptions};
use kiln_media::video;
use support::clips::{can_make_clips, clip, Clip};
use support::files_in;

/// Whether the test can run: Media Foundation's codecs and FFmpeg to make
/// its videos.
fn ready(test: &str) -> bool {
    let mf = Kiln::new()
        .availability(&Options::default())
        .has("audio-mf");
    if !mf {
        eprintln!("{test}: skipped, Media Foundation's codecs aren't installed");
        return false;
    }
    can_make_clips(test)
}

/// The clip copied into a fresh folder of its own, as `name`; an MKV is the
/// same streams put in Matroska by FFmpeg.
fn staged(kind: Clip, name: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    if name.ends_with(".mkv") {
        let made = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
            .arg(clip(kind))
            .args(["-c", "copy"])
            .arg(&path)
            .output()
            .unwrap();
        assert!(made.status.success(), "{made:?}");
    } else {
        std::fs::copy(clip(kind), &path).unwrap();
    }
    (dir, path)
}

fn options(tool: ToolOptions) -> Options {
    Options {
        tool,
        ..Options::default()
    }
}

fn to(max_bytes: u64) -> ToolOptions {
    ToolOptions {
        max_bytes: Some(max_bytes),
        ..ToolOptions::default()
    }
}

/// The file's bytes and modified time.
fn snapshot(path: &Path) -> (Vec<u8>, SystemTime) {
    let bytes = std::fs::read(path).unwrap();
    let time = std::fs::metadata(path).unwrap().modified().unwrap();
    (bytes, time)
}

fn len(path: &Path) -> u64 {
    std::fs::metadata(path).unwrap().len()
}

#[test]
fn a_video_is_made_smaller_to_the_size_asked() {
    if !ready("a_video_is_made_smaller_to_the_size_asked") {
        return;
    }
    let (dir, input) = staged(Clip::Noisy, "noisy.mp4");
    let before = snapshot(&input);
    assert!(before.0.len() > 20_000_000, "{}", before.0.len());
    let max = 10_000_000;
    let out = Kiln::new()
        .run_tool(
            Tool::Smaller,
            std::slice::from_ref(&input),
            &options(to(max)),
        )
        .unwrap();
    assert_eq!(out, [dir.path().join("noisy (smaller).mp4")]);
    let got = len(&out[0]);
    // At or under the size, and not far under it.
    assert!(got <= max && got > max / 2, "{got}");
    // Still an MP4 of the same picture.
    let info = video::probe(&out[0]).unwrap();
    assert_eq!((info.width, info.height), (960, 540));
    assert!((info.duration.unwrap() - 10.0).abs() < 0.2, "{info:?}");
    // The source is untouched.
    assert_eq!(snapshot(&input), before);
    assert_eq!(files_in(dir.path()), ["noisy (smaller).mp4", "noisy.mp4"]);
}

#[test]
fn an_mkv_stays_an_mkv_and_keeps_its_sound() {
    if !ready("an_mkv_stays_an_mkv_and_keeps_its_sound") {
        return;
    }
    let (dir, input) = staged(Clip::Film, "film.mkv");
    let original = len(&input);
    let max = original / 2;
    let out = Kiln::new()
        .run_tool(
            Tool::Smaller,
            std::slice::from_ref(&input),
            &options(to(max)),
        )
        .unwrap();
    assert_eq!(out, [dir.path().join("film (smaller).mkv")]);
    assert!(len(&out[0]) <= max, "{} over {max}", len(&out[0]));
    let tracks = video::track_counts(&out[0]).unwrap();
    assert_eq!(tracks.sound, 1);
    // Matroska's own signature.
    let head = std::fs::read(&out[0]).unwrap();
    assert_eq!(head[..4], [0x1A, 0x45, 0xDF, 0xA3]);
}

#[test]
fn sound_off_and_a_smaller_picture() {
    if !ready("sound_off_and_a_smaller_picture") {
        return;
    }
    let (dir, input) = staged(Clip::FullHd, "full.mp4");
    let before = snapshot(&input);
    let max = before.0.len() as u64 / 2;
    let opts = options(ToolOptions {
        keep_sound: Some(false),
        picture_lines: Some(720),
        ..to(max)
    });
    let out = Kiln::new()
        .run_tool(Tool::Smaller, std::slice::from_ref(&input), &opts)
        .unwrap();
    assert!(len(&out[0]) <= max);
    let info = video::probe(&out[0]).unwrap();
    assert_eq!((info.width, info.height), (1280, 720));
    assert!(info.sound.is_none() && info.sound_unreadable.is_none());
    assert_eq!(video::track_counts(&out[0]).unwrap().sound, 0);
    assert_eq!(snapshot(&input), before);

    // Already under the size, but with sound to leave out: still done.
    let out = Kiln::new()
        .run_tool(
            Tool::Smaller,
            std::slice::from_ref(&input),
            &options(ToolOptions {
                keep_sound: Some(false),
                ..to(before.0.len() as u64 * 2)
            }),
        )
        .unwrap();
    assert_eq!(out, [dir.path().join("full (smaller) (1).mp4")]);
    assert!(len(&out[0]) < before.0.len() as u64);
    assert_eq!(video::track_counts(&out[0]).unwrap().sound, 0);
}

#[test]
fn a_size_out_of_reach_saves_nothing_and_says_how_small_it_can_get() {
    if !ready("a_size_out_of_reach_saves_nothing_and_says_how_small_it_can_get") {
        return;
    }
    let (dir, input) = staged(Clip::Film, "film.mp4");
    let before = snapshot(&input);
    let kiln = Kiln::new();
    // Ten seconds can't go under 50 KB: the picture's least bitrate alone
    // is more.
    let err = kiln
        .run_tool(
            Tool::Smaller,
            std::slice::from_ref(&input),
            &options(to(50_000)),
        )
        .unwrap_err();
    let smallest = match &err {
        Error::OutOfReach {
            max_bytes: 50_000,
            smallest,
        } => *smallest,
        other => panic!("{other:?}"),
    };
    assert!(smallest > 50_000, "{smallest}");
    assert_eq!(
        err.to_string(),
        format!(
            "Can't get this under 50 KB. The smallest Kiln can make is about {}.",
            about(smallest)
        )
    );
    // A size at or over the file's: left alone, with nothing else asked
    // that would change it (a picture already under 720 lines).
    let over = before.0.len() as u64 + 1;
    for lines in [None, Some(720)] {
        let err = kiln
            .run_tool(
                Tool::Smaller,
                std::slice::from_ref(&input),
                &options(ToolOptions {
                    picture_lines: lines,
                    ..to(over)
                }),
            )
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("This file is already smaller than {}.", about(over))
        );
    }
    assert_eq!(files_in(dir.path()), ["film.mp4"]);
    assert_eq!(snapshot(&input), before);
}

#[test]
fn a_video_cancelled_part_way_leaves_nothing() {
    if !ready("a_video_cancelled_part_way_leaves_nothing") {
        return;
    }
    let (dir, input) = staged(Clip::Noisy, "noisy.mp4");
    let before = snapshot(&input);
    let cancel = kiln_core::CancelToken::new();
    let mut heard = 0;
    let err = Kiln::new()
        .run_tool_with(
            Tool::Smaller,
            std::slice::from_ref(&input),
            &options(to(10_000_000)),
            &mut |p| {
                // Cancelled once the encode has begun, not before.
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
    assert_eq!(files_in(dir.path()), ["noisy.mp4"]);
    assert_eq!(snapshot(&input), before);
}

#[test]
fn a_video_of_no_known_length_is_turned_down_before_it_is_encoded() {
    if !ready("a_video_of_no_known_length_is_turned_down_before_it_is_encoded") {
        return;
    }
    // Matroska written to a pipe has no length in its header.
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("piped.mkv");
    let made = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(clip(Clip::Film))
        .args(["-c", "copy", "-f", "matroska", "pipe:1"])
        .output()
        .unwrap();
    assert!(made.status.success(), "{made:?}");
    std::fs::write(&input, &made.stdout).unwrap();
    let info = video::probe(&input).unwrap();
    assert_eq!(info.duration, None, "{info:?}");
    let before = snapshot(&input);
    let mut heard = Vec::new();
    let err = Kiln::new()
        .run_tool_with(
            Tool::Smaller,
            std::slice::from_ref(&input),
            &options(to(len(&input) / 2)),
            &mut |p| {
                if let kiln_core::Progress::Fraction(f) = p {
                    heard.push(f)
                }
            },
            &kiln_core::CancelToken::new(),
        )
        .unwrap_err();
    assert!(matches!(err, Error::NothingToDo(_)), "{err:?}");
    assert_eq!(err.to_string(), "Couldn't tell how long this file is.");
    // Nothing was encoded.
    assert!(!heard.iter().any(|&f| f > 0.0), "{heard:?}");
    assert_eq!(files_in(dir.path()), ["piped.mkv"]);
    assert_eq!(snapshot(&input), before);
}

#[test]
fn the_size_a_video_will_come_to_is_worked_out_without_writing_anything() {
    if !ready("the_size_a_video_will_come_to_is_worked_out_without_writing_anything") {
        return;
    }
    let (dir, input) = staged(Clip::Noisy, "noisy.mp4");
    let before = snapshot(&input);
    let kiln = Kiln::new();
    let cancel = kiln_core::CancelToken::new();
    let max = 10_000_000;
    let n = kiln
        .smaller_size(&input, &options(to(max)), &cancel)
        .unwrap();
    // About the size asked, never over it.
    assert!(n <= max && n > max / 2, "{n}");
    // Out of reach: how small it can get.
    match kiln.smaller_size(&input, &options(to(50_000)), &cancel) {
        Err(Error::OutOfReach {
            max_bytes: 50_000,
            smallest,
        }) => assert!(smallest > 50_000, "{smallest}"),
        other => panic!("{other:?}"),
    }
    // Already under it.
    let err = kiln
        .smaller_size(&input, &options(to(before.0.len() as u64 * 2)), &cancel)
        .unwrap_err();
    assert!(matches!(err, Error::NothingToDo(_)), "{err:?}");
    assert_eq!(snapshot(&input), before);
    assert_eq!(files_in(dir.path()), ["noisy.mp4"]);
}
