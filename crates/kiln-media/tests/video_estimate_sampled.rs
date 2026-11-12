//! The size estimate, scaled by a short sample encode (spec 3.3): on the
//! generated clips (a screen recording, film and noise) the median error
//! against a real encode is at most 15%, and none is off by more than 2x.
//!
//! The clips are made with FFmpeg (tests/support/clips.rs); without it, or
//! without Media Foundation, the test says it is skipped.

#![cfg(windows)]

#[path = "../../../tests/support/clips.rs"]
mod clips;
mod common;

use clips::{can_make_clips, clip, Clip};
use kiln_media::video::{self, convert, tune, VideoSetting, VideoTarget, VideoTune};
use kiln_media::Preset;

/// The quality step the estimate is checked at: High.
const STEP: u8 = 65;

#[test]
fn a_sample_brings_the_estimate_within_reach_of_the_real_size() {
    let test = "a_sample_brings_the_estimate_within_reach_of_the_real_size";
    if common::no_mf() || !can_make_clips(test) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let t = VideoTarget::Mp4;
    let tune = VideoTune {
        setting: Some(VideoSetting::Quality(STEP)),
        ..VideoTune::default()
    };
    let s = VideoSetting::Quality(STEP);
    let mut errors = Vec::new();
    eprintln!("clip     | table      | sampled    | actual     | table err | sampled err");
    for kind in Clip::ESTIMATED {
        let path = clip(kind);
        let info = video::probe(&path).unwrap();
        // Asked first, so the table is the card's or the processor's as the
        // job will find.
        let _ = video::encoder_adapter(tune.codec, tune.encoder);
        let table = tune::estimate_bytes(t, &s, &info, &tune).unwrap();
        let measured = video::sample(&path, &tune, &|| false).unwrap().unwrap();
        assert_eq!(measured.quality, STEP);
        // Kept: asked again, the same answer at once.
        assert_eq!(video::sampled(&path, &tune), Some(measured));
        let sampled = tune::estimate_measured(t, &s, &info, &tune, Some(&measured)).unwrap();

        let out = dir.path().join(format!("{}.mp4", kind.name()));
        convert(&path, &out, t, Preset::High, &tune, &mut |_| {}, &|| false).unwrap();
        let actual = std::fs::metadata(&out).unwrap().len();
        let err = |e: u64| (e as f64 - actual as f64) / actual as f64;
        eprintln!(
            "{:<8} | {table:>10} | {sampled:>10} | {actual:>10} | {:>+8.1}% | {:>+8.1}%",
            kind.name(),
            err(table) * 100.0,
            err(sampled) * 100.0,
        );
        let ratio = sampled as f64 / actual as f64;
        assert!(
            (0.5..=2.0).contains(&ratio),
            "{}: estimate {sampled}, actual {actual}",
            kind.name()
        );
        errors.push(err(sampled).abs());
    }
    errors.sort_by(f64::total_cmp);
    let median = errors[errors.len() / 2];
    assert!(median <= 0.15, "median error {median:.3}: {errors:?}");
}

#[test]
fn the_estimate_without_a_sample_is_the_table_s() {
    // Nothing measured, nothing scaled.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/video/h264.mp4");
    let info = video::probe(&path).unwrap();
    let tune = VideoTune::default();
    let t = VideoTarget::Mp4;
    for s in [
        VideoSetting::Quality(45),
        VideoSetting::Original,
        VideoSetting::Kbps(900),
    ] {
        assert_eq!(
            tune::estimate_measured(t, &s, &info, &tune, None),
            tune::estimate_bytes(t, &s, &info, &tune)
        );
    }
    // A sample's scale applies to the quality steps only.
    let m = video::Measured {
        quality: 65,
        kbps: 0.0,
        scale: 0.5,
    };
    let q = VideoSetting::Quality(45);
    let plain = tune::estimate_bytes(t, &q, &info, &tune).unwrap();
    let scaled = tune::estimate_measured(t, &q, &info, &tune, Some(&m)).unwrap();
    assert!(scaled < plain, "{scaled} {plain}");
    let k = VideoSetting::Kbps(900);
    assert_eq!(
        tune::estimate_measured(t, &k, &info, &tune, Some(&m)),
        tune::estimate_bytes(t, &k, &info, &tune)
    );
}

#[test]
fn a_short_file_is_sampled_whole_and_a_cancel_keeps_nothing() {
    if common::no_mf() {
        return;
    }
    // Two seconds: all of it. A copy of its own, so no other test's sample
    // of it is kept.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("short.mp4");
    let from = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/video/h264.mp4");
    std::fs::copy(from, &path).unwrap();
    let tune = VideoTune::default();
    let cancelled = video::sample(&path, &tune, &|| true);
    assert!(
        matches!(cancelled, Err(kiln_media::MediaError::Cancelled)),
        "{cancelled:?}"
    );
    assert_eq!(video::sampled(&path, &tune), None);
    let m = video::sample(&path, &tune, &|| false).unwrap().unwrap();
    // Unset, the step is High's.
    assert_eq!(m.quality, STEP);
    assert!(m.kbps > 0.0 && m.scale > 0.0, "{m:?}");
    assert_eq!(video::sampled(&path, &tune), Some(m));
    // Another codec is another sample.
    let hevc = VideoTune {
        codec: video::VCodec::Hevc,
        ..tune
    };
    assert_eq!(video::sampled(&path, &hevc), None);
    // A changed file is sampled again.
    std::fs::write(&path, b"no longer that video").unwrap();
    assert_eq!(video::sampled(&path, &tune), None);
}

#[test]
fn a_file_with_no_picture_in_its_middle_gives_no_sample() {
    let test = "a_file_with_no_picture_in_its_middle_gives_no_sample";
    if common::no_mf() || !can_make_clips(test) {
        return;
    }
    // Ten seconds with frames only in the first and the last: its middle
    // two hold no frame.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gap.mp4");
    let made = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "lavfi", "-i", "testsrc2=s=320x240:r=30:d=10"])
        .args([
            "-vf",
            "select='lt(t,1)+gte(t,9)'",
            "-fps_mode",
            "passthrough",
        ])
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(made.status.success(), "{made:?}");
    let info = video::probe(&path).unwrap();
    assert!(info.duration.is_some_and(|d| d > 6.0), "{info:?}");
    let tune = VideoTune::default();
    // Not a damaged file: only nothing to measure, which the estimate
    // does without.
    let got = video::sample(&path, &tune, &|| false);
    assert!(matches!(got, Ok(None)), "{got:?}");
    assert_eq!(video::sampled(&path, &tune), None);
}
