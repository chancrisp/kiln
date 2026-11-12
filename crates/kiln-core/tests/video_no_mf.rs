//! MP4, MOV, M4V and MKV on a Windows without Media Foundation (an N
//! edition without the Media Feature Pack), as debug builds can pretend
//! with `KILN_HIDE_MF` (R59): with FFmpeg every job is FFmpeg's, with the
//! job's own settings (R57); without it the row asks for the pack.
//!
//! One test only, in a test binary of its own: the variable is read once
//! per process, before anything else asks Media Foundation what it has.

#![cfg(windows)]

#[path = "../../../tests/support/mod.rs"]
mod support;

use std::path::PathBuf;

use kiln_core::{Engines, Error, JobError, Kiln, Options};
use kiln_media::video::{self, VideoCodec, VideoSetting, VideoTune};
use support::{files_in, fixture, stage};

fn options(temp: &tempfile::TempDir) -> Options {
    Options {
        temp_root: Some(temp.path().join("work")),
        ..Options::default()
    }
}

fn no_ffmpeg(temp: &tempfile::TempDir) -> Options {
    Options {
        engines: Engines {
            ffmpeg: Some(temp.path().join("no-such-dir").join("ffmpeg.exe")),
            ..Engines::default()
        },
        ..options(temp)
    }
}

#[test]
fn without_media_foundation_ffmpeg_does_every_video_job() {
    if !cfg!(debug_assertions) {
        eprintln!("skipped: KILN_HIDE_MF works in debug builds only");
        return;
    }
    std::env::set_var("KILN_HIDE_MF", "1");
    let kiln = Kiln::new();
    let dir = tempfile::tempdir().unwrap();
    assert!(!kiln.availability(&options(&dir)).has("audio-mf"));

    // Neither: the pairs are blocked, and the row asks for the pack.
    let neither = kiln.availability(&no_ffmpeg(&dir));
    assert!(!neither.has("video"));
    let (usable, blocked) = kiln.targets_split(&fixture("sample.mp4"), &neither);
    assert!(!usable.iter().any(|f| f.ext == "mkv"), "{usable:?}");
    assert!(blocked
        .iter()
        .any(|(f, helper)| f.ext == "mkv" && helper == "audio-mf"));
    // A job sent anyway says the same, and leaves nothing.
    let (dir, input) = stage(&fixture("sample.mkv"));
    let err = kiln.convert(&input, "mp4", &no_ffmpeg(&dir)).unwrap_err();
    assert!(
        matches!(err, Error::EngineMissing { engine: "audio-mf" }),
        "{err:?}"
    );
    assert_eq!(
        JobError::from(err).message,
        "Needs Windows' Media Feature Pack, which isn't installed."
    );
    assert_eq!(files_in(dir.path()), ["sample.mkv", "work"]);

    // With FFmpeg: available, and FFmpeg converts with the job's settings.
    let found = kiln_core::find_on_path(&std::env::var_os("PATH").unwrap_or_default(), "ffmpeg");
    if found.is_none() {
        eprintln!("the FFmpeg half: skipped, FFmpeg isn't on PATH");
        return;
    }
    assert!(kiln.availability(&options(&dir)).has("video"));
    let (usable, _) =
        kiln.targets_split(&fixture("sample.mp4"), &kiln.availability(&options(&dir)));
    assert!(usable.iter().any(|f| f.ext == "mkv"), "{usable:?}");
    let (dir, input) = stage(&fixture("sample.mkv"));
    let mut opts = options(&dir);
    opts.quality.video = VideoTune {
        setting: Some(VideoSetting::Quality(45)),
        max_lines: Some(24),
        ..VideoTune::default()
    };
    let out: PathBuf = kiln.convert(&input, "mp4", &opts).unwrap();
    let v = video::probe(&out).unwrap();
    assert_eq!(v.codec, VideoCodec::H264);
    assert_eq!((v.width, v.height), (32, 24));
    assert!(v.sound.is_some());

    // Fit under a limit the source already keeps to: FFmpeg copies it
    // rather than refusing (R60).
    let src = support::media_data("video/h264.mp4");
    let source = std::fs::metadata(&src).unwrap().len();
    let (dir, input) = stage(&src);
    let mut opts = options(&dir);
    opts.quality.video.fit_bytes = Some(source * 2);
    let out = kiln.convert(&input, "mov", &opts).unwrap();
    let written = std::fs::metadata(&out).unwrap().len();
    assert!(written <= source * 2, "{written}");
    assert!(
        written.abs_diff(source) < source / 10,
        "{written} from {source}"
    );
    let v = video::probe(&out).unwrap();
    assert_eq!((v.codec, v.width, v.height), (VideoCodec::H264, 320, 240));

    // Black bars come from FFmpeg's detector, which needs FFmpeg.
    let bars = |opts: &Options| {
        kiln_core::inspect(
            &kiln,
            &fixture("letterbox.mp4"),
            opts,
            &kiln_core::CancelToken::new(),
        )
        .unwrap()
        .bars
    };
    assert_eq!(bars(&options(&dir)), Some(kiln_core::Bars::AboveBelow));
    assert_eq!(bars(&no_ffmpeg(&dir)), None);
}
