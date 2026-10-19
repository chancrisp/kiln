//! The FFmpeg backend, with the audio and video fixtures.

#[path = "../../../tests/support/mod.rs"]
mod support;

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use kiln_core::{CancelToken, Engines, Error, Kiln, Options, Progress};
use support::{assert_output, files_in, probe, sample_for, stage};

/// Options with a private temp root, so tests can check it is cleaned up.
fn options(temp: &tempfile::TempDir) -> Options {
    Options {
        temp_root: Some(temp.path().join("work")),
        ..Options::default()
    }
}

fn assert_no_work_left(temp: &tempfile::TempDir) {
    let work = temp.path().join("work");
    if work.exists() {
        assert!(
            files_in(&work).is_empty(),
            "left behind: {:?}",
            files_in(&work)
        );
    }
}

#[test]
fn wav_to_mp3() {
    let (dir, input) = stage(&sample_for("wav"));
    let out = Kiln::new().convert(&input, "mp3", &options(&dir)).unwrap();
    assert_eq!(out, dir.path().join("sample.mp3"));
    assert_output(&out, "mp3");
    assert_no_work_left(&dir);
}

#[test]
fn flac_to_opus() {
    let (dir, input) = stage(&sample_for("flac"));
    let out = Kiln::new().convert(&input, "opus", &options(&dir)).unwrap();
    assert_output(&out, "opus");
}

#[test]
fn mp4_to_webm_keeps_size_and_audio() {
    let (dir, input) = stage(&sample_for("mp4"));
    let out = Kiln::new().convert(&input, "webm", &options(&dir)).unwrap();
    assert_output(&out, "webm");
}

#[test]
fn video_to_audio_drops_the_picture() {
    let (dir, input) = stage(&sample_for("mkv"));
    let out = Kiln::new().convert(&input, "m4a", &options(&dir)).unwrap();
    assert_output(&out, "m4a");
    assert_eq!(probe(&out).video, 0);
}

#[test]
fn progress_rises_to_one() {
    let (dir, input) = stage(&sample_for("mov"));
    let mut seen = Vec::new();
    Kiln::new()
        .convert_with(
            &input,
            "mp4",
            &options(&dir),
            &mut |p| seen.push(p),
            &CancelToken::new(),
        )
        .unwrap();
    let fractions: Vec<f32> = seen
        .iter()
        .filter_map(|p| match p {
            Progress::Fraction(f) => Some(*f),
            Progress::Indeterminate => None,
        })
        .collect();
    assert_eq!(fractions.first(), Some(&0.0), "{seen:?}");
    assert_eq!(fractions.last(), Some(&1.0), "{seen:?}");
    assert!(fractions.windows(2).all(|w| w[0] <= w[1]), "{seen:?}");
    assert!(fractions.iter().all(|f| (0.0..=1.0).contains(f)));
}

#[test]
fn cancel_stops_the_job_and_cleans_up() {
    // A minute of audio is long enough to cancel mid-way.
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("long.wav");
    let ok = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
        .arg("sine=frequency=220:sample_rate=48000:duration=600")
        .arg(&input)
        .status()
        .unwrap();
    assert!(ok.success());

    let cancel = CancelToken::new();
    let trigger = cancel.clone();
    let mut calls = 0;
    let err = Kiln::new()
        .convert_with(
            &input,
            "mp3",
            &options(&dir),
            &mut |_| {
                calls += 1;
                if calls == 3 {
                    trigger.cancel();
                }
            },
            &cancel,
        )
        .unwrap_err();
    assert!(matches!(err, Error::Cancelled), "{err:?}");
    assert_eq!(files_in(dir.path()), ["long.wav", "work"]);
    assert_no_work_left(&dir);
}

#[test]
fn missing_ffmpeg_says_where_to_get_it() {
    let (dir, input) = stage(&sample_for("wav"));
    let opts = Options {
        engines: Engines {
            ffmpeg: Some(dir.path().join("no-such-dir").join("ffmpeg")),
            ..Engines::default()
        },
        ..options(&dir)
    };
    let err = Kiln::new().convert(&input, "mp3", &opts).unwrap_err();
    assert_eq!(
        err.to_string(),
        "FFmpeg isn't installed. Get it from Settings > Engines."
    );
    assert_eq!(files_in(dir.path()), ["sample.wav", "work"]);
    assert_no_work_left(&dir);
}

#[test]
fn corrupt_media_reports_the_first_stderr_line() {
    let (dir, input) = stage(&support::fixture("corrupt.wav"));
    let err = Kiln::new()
        .convert(&input, "mp3", &options(&dir))
        .unwrap_err();
    match &err {
        Error::EngineFailed {
            engine,
            summary,
            details,
        } => {
            assert_eq!(*engine, "FFmpeg");
            assert!(!summary.is_empty() && !summary.contains('\n'));
            assert!(details.contains(summary.as_str()));
        }
        other => panic!("expected EngineFailed, got {other:?}"),
    }
    assert!(err
        .to_string()
        .starts_with("FFmpeg couldn't convert the file: "));
    assert_eq!(files_in(dir.path()), ["corrupt.wav", "work"]);
    assert_no_work_left(&dir);
}

#[test]
fn hostile_file_names_are_just_names() {
    // Looks like options, protocols and shell syntax. None of it may act.
    let dir = tempfile::tempdir().unwrap();
    let name = "-y; echo hi & http:x $(ls).wav";
    let input = dir.path().join(name);
    fs::copy(sample_for("wav"), &input).unwrap();
    let out = Kiln::new().convert(&input, "flac", &options(&dir)).unwrap();
    assert_eq!(
        out.file_name().unwrap().to_string_lossy(),
        "-y; echo hi & http_x $(ls).flac"
    );
    assert_output(&out, "flac");
}

/// Find ffmpeg on PATH the way the OS would, without a shell tool.
fn ffmpeg_on_path() -> PathBuf {
    let exe = format!("ffmpeg{}", std::env::consts::EXE_SUFFIX);
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join(&exe))
        .find(|p| p.is_file())
        .expect("ffmpeg must be on PATH for these tests")
}

#[test]
fn a_configured_engine_path_is_used() {
    let (dir, input) = stage(&sample_for("ogg"));
    let opts = Options {
        engines: Engines {
            ffmpeg: Some(ffmpeg_on_path()),
            ..Engines::default()
        },
        ..options(&dir)
    };
    let out = Kiln::new().convert(&input, "wav", &opts).unwrap();
    assert_output(&out, "wav");
}
