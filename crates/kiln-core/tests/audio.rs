//! Sound through Kiln's own engine (kiln-media): audio files, and the sound
//! of MP4, M4V, MOV, MKV and WebM. No helper app is needed.

#[path = "../../../tests/support/mod.rs"]
mod support;

use std::fs;
use std::path::Path;

use kiln_core::{CancelToken, Error, Kiln, Options, Progress};
use support::{assert_output, files_in, fixture, probe, sample_for, stage};

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

/// Whether this PC has the Media Foundation codecs the "audio-mf" routes
/// need. Without them a test that needs them says it is skipped.
fn media_foundation(test: &str) -> bool {
    let has = Kiln::new()
        .availability(&Options::default())
        .has("audio-mf");
    if !has {
        eprintln!("{test}: skipped, Media Foundation's codecs aren't installed");
    }
    has
}

/// A 16-bit stereo WAV of `secs` seconds of a quiet tone at 48 kHz.
fn tone_wav(path: &Path, secs: u32) {
    let frames = 48_000 * secs;
    let data = frames * 4;
    let mut b = Vec::with_capacity(data as usize + 44);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&48_000u32.to_le_bytes());
    b.extend_from_slice(&(48_000u32 * 4).to_le_bytes());
    b.extend_from_slice(&4u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    for i in 0..frames {
        let s = ((i as f64 * 220.0 * std::f64::consts::TAU / 48_000.0).sin() * 8000.0) as i16;
        b.extend_from_slice(&s.to_le_bytes());
        b.extend_from_slice(&s.to_le_bytes());
    }
    fs::write(path, b).unwrap();
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
fn ogg_to_wav() {
    let (dir, input) = stage(&sample_for("ogg"));
    let out = Kiln::new().convert(&input, "wav", &options(&dir)).unwrap();
    assert_output(&out, "wav");
}

#[test]
fn a_videos_sound_drops_the_picture() {
    let (dir, input) = stage(&sample_for("webm"));
    let out = Kiln::new().convert(&input, "mp3", &options(&dir)).unwrap();
    assert_output(&out, "mp3");
    assert_eq!(probe(&out).video, 0);
    if media_foundation("a_videos_sound_drops_the_picture") {
        let (dir, input) = stage(&sample_for("mkv"));
        let out = Kiln::new().convert(&input, "m4a", &options(&dir)).unwrap();
        assert_output(&out, "m4a");
        assert_eq!(probe(&out).video, 0);
    }
}

#[test]
fn progress_rises_to_one() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("tone.wav");
    tone_wav(&input, 20);
    let mut seen = Vec::new();
    Kiln::new()
        .convert_with(
            &input,
            "ogg",
            &options(&dir),
            &mut |p| seen.push(p),
            &CancelToken::new(),
        )
        .unwrap();
    let fractions: Vec<f32> = seen
        .iter()
        .filter_map(|p| match p {
            Progress::Fraction(f) => Some(*f),
            _ => None,
        })
        .collect();
    assert!(fractions.len() > 2, "{seen:?}");
    assert_eq!(fractions.last(), Some(&1.0), "{seen:?}");
    assert!(fractions.windows(2).all(|w| w[0] <= w[1]), "{seen:?}");
    assert!(fractions.iter().all(|f| (0.0..=1.0).contains(f)));
}

#[test]
fn cancel_stops_the_job_and_cleans_up() {
    // Ten minutes of sound is long enough to cancel mid-way.
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("long.wav");
    tone_wav(&input, 600);
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
fn a_damaged_file_says_so_plainly() {
    let (dir, input) = stage(&fixture("corrupt.wav"));
    let err = Kiln::new()
        .convert(&input, "mp3", &options(&dir))
        .unwrap_err();
    assert!(matches!(err, Error::Media(_)), "{err:?}");
    assert_eq!(err.to_string(), "This WAV is damaged and can't be read.");
    assert_eq!(files_in(dir.path()), ["corrupt.wav", "work"]);
    assert_no_work_left(&dir);
}

#[test]
fn a_silent_video_has_no_sound_to_convert() {
    let (dir, input) = stage(&support::media_data("silent.mp4"));
    let err = Kiln::new()
        .convert(&input, "mp3", &options(&dir))
        .unwrap_err();
    assert_eq!(err.to_string(), "This video has no sound to convert.");
    assert_eq!(files_in(dir.path()), ["silent.mp4", "work"]);
    assert_no_work_left(&dir);
}

#[test]
fn odd_file_names_are_just_names() {
    let dir = tempfile::tempdir().unwrap();
    let name = "-y; echo hi & http:x $(ls) \u{e9}t\u{e9}.wav";
    let input = dir.path().join(name);
    fs::copy(sample_for("wav"), &input).unwrap();
    let out = Kiln::new().convert(&input, "flac", &options(&dir)).unwrap();
    assert_eq!(
        out.file_name().unwrap().to_string_lossy(),
        "-y; echo hi & http_x $(ls) \u{e9}t\u{e9}.flac"
    );
    assert_output(&out, "flac");
}

#[test]
fn the_cli_bitrate_picks_the_nearest_step() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("tone.wav");
    tone_wav(&input, 20);
    let size = |kbps: u32| {
        let out_dir = tempfile::tempdir().unwrap();
        let mut opts = options(&dir);
        opts.out_dir = Some(out_dir.path().to_path_buf());
        opts.quality.audio_kbps = Some(kbps);
        let out = Kiln::new().convert(&input, "opus", &opts).unwrap();
        fs::metadata(out).unwrap().len()
    };
    // Opus at 32 kbps against 256 kbps over 20 seconds.
    let (low, high) = (size(32), size(256));
    assert!(low * 3 < high, "{low} vs {high}");
}
