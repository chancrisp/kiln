//! Video among MP4, MOV, M4V and MKV through Kiln's own engine
//! (`kiln_media::video`), with FFmpeg only for what Windows can't read.
//! FFmpeg is hidden by pointing Kiln at an FFmpeg that isn't there.

#![cfg(windows)]

#[path = "../../../tests/support/mod.rs"]
mod support;

use std::path::{Path, PathBuf};

use kiln_core::quality::{self, Knob, Quality};
use kiln_core::{CancelToken, Engines, Error, Kiln, Options, Preset, Progress, Registry};
use kiln_media::video::{
    self, tune, SoundChoice, VCodec, VideoCodec, VideoSetting, VideoTarget, VideoTune,
};
use support::{files_in, fixture, media_data, stage};

/// Options with a private temp root, so tests can check it is cleaned up.
fn options(temp: &tempfile::TempDir) -> Options {
    Options {
        temp_root: Some(temp.path().join("work")),
        ..Options::default()
    }
}

/// `options`, with FFmpeg (and FFprobe beside it) somewhere it isn't.
fn no_ffmpeg(temp: &tempfile::TempDir) -> Options {
    Options {
        engines: Engines {
            ffmpeg: Some(temp.path().join("no-such-dir").join("ffmpeg.exe")),
            ..Engines::default()
        },
        ..options(temp)
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

/// A video from kiln-media's test data.
fn data(name: &str) -> PathBuf {
    media_data(&format!("video/{name}"))
}

/// Whether FFmpeg is on PATH; a test that needs it says it is skipped.
fn ffmpeg(test: &str) -> bool {
    let has =
        kiln_core::find_on_path(&std::env::var_os("PATH").unwrap_or_default(), "ffmpeg").is_some();
    if !has {
        eprintln!("{test}: skipped, FFmpeg isn't on PATH");
    }
    has
}

/// Whether Windows has the Media Foundation codecs (an N edition without
/// the Media Feature Pack has none); a test that needs them says it is
/// skipped.
fn media_foundation(test: &str) -> bool {
    let has = Kiln::new()
        .availability(&Options::default())
        .has("audio-mf");
    if !has {
        eprintln!("{test}: skipped, Media Foundation's codecs aren't installed");
    }
    has
}

#[test]
fn mkv_to_mp4_converts_with_no_ffmpeg() {
    if !media_foundation("mkv_to_mp4_converts_with_no_ffmpeg") {
        return;
    }
    let (dir, input) = stage(&fixture("sample.mkv"));
    let opts = no_ffmpeg(&dir);
    let kiln = Kiln::new();
    assert!(kiln.availability(&opts).has("video"));
    assert!(!kiln.availability(&opts).has("ffmpeg"));
    let mut seen = vec![];
    let out = kiln
        .convert_with(
            &input,
            "mp4",
            &opts,
            &mut |p| seen.push(p),
            &CancelToken::new(),
        )
        .unwrap();
    assert_eq!(out, dir.path().join("sample.mp4"));
    let bytes = std::fs::read(&out).unwrap();
    assert_eq!(&bytes[4..8], b"ftyp");
    // The H.264 picture is copied; the Opus sound becomes AAC.
    let v = video::probe(&out).unwrap();
    assert_eq!(v.codec, VideoCodec::H264);
    assert_eq!((v.width, v.height), (64, 48));
    assert_eq!(
        v.sound.as_ref().map(|s| s.codec.clone()),
        Some(kiln_media::Codec::Aac)
    );
    // Progress rises to one and never goes back; a copy has no note.
    let fractions: Vec<f32> = seen
        .iter()
        .filter_map(|p| match p {
            Progress::Fraction(f) => Some(*f),
            _ => None,
        })
        .collect();
    assert_eq!(fractions.last(), Some(&1.0), "{seen:?}");
    assert!(fractions.windows(2).all(|w| w[0] <= w[1]), "{seen:?}");
    assert!(
        !seen.iter().any(|p| matches!(p, Progress::Note(_))),
        "{seen:?}"
    );
    assert_eq!(files_in(dir.path()), ["sample.mkv", "sample.mp4", "work"]);
    assert_no_work_left(&dir);
}

#[test]
fn every_pair_of_the_four_converts() {
    if !media_foundation("every_pair_of_the_four_converts") {
        return;
    }
    let has_ffprobe =
        kiln_core::find_on_path(&std::env::var_os("PATH").unwrap_or_default(), "ffprobe").is_some();
    for from in ["mp4", "mov", "m4v", "mkv"] {
        for to in ["mp4", "mov", "m4v", "mkv"] {
            if from == to {
                continue;
            }
            let (dir, input) = stage(&fixture(&format!("sample.{from}")));
            let out = Kiln::new()
                .convert(&input, to, &no_ffmpeg(&dir))
                .unwrap_or_else(|e| panic!("{from} -> {to}: {e}"));
            let v = video::probe(&out).unwrap_or_else(|e| panic!("{from} -> {to}: {e}"));
            assert_eq!(v.codec, VideoCodec::H264, "{from} -> {to}");
            assert!(v.sound.is_some(), "{from} -> {to}");
            // What kiln-cli's every-pair test checks, with FFprobe.
            if has_ffprobe {
                support::assert_converted(&out, Some(from), to);
            }
            assert_no_work_left(&dir);
        }
    }
}

#[test]
fn an_encode_on_the_processor_writes_h264() {
    if !media_foundation("an_encode_on_the_processor_writes_h264") {
        return;
    }
    let (dir, input) = stage(&data("h264.mp4"));
    let mut opts = no_ffmpeg(&dir);
    opts.quality.video = VideoTune {
        setting: Some(VideoSetting::Quality(45)),
        max_lines: Some(120),
        encoder: kiln_media::video::EncoderChoice::Processor,
        ..VideoTune::default()
    };
    let mut seen = vec![];
    let out = Kiln::new()
        .convert_with(
            &input,
            "mkv",
            &opts,
            &mut |p| seen.push(p),
            &CancelToken::new(),
        )
        .unwrap();
    let v = video::probe(&out).unwrap();
    assert_eq!(v.codec, VideoCodec::H264);
    assert_eq!((v.width, v.height), (160, 120));
    // Processor was chosen, so being on it is no news.
    assert!(
        !seen.iter().any(|p| matches!(p, Progress::Note(_))),
        "{seen:?}"
    );
    assert_no_work_left(&dir);
}

#[test]
fn cancel_stops_a_video_job_and_cleans_up() {
    if !media_foundation("cancel_stops_a_video_job_and_cleans_up") {
        return;
    }
    let (dir, input) = stage(&data("h264.mp4"));
    let mut opts = no_ffmpeg(&dir);
    opts.quality.video.setting = Some(VideoSetting::Quality(50));
    opts.quality.video.encoder = kiln_media::video::EncoderChoice::Processor;
    let cancel = CancelToken::new();
    let trigger = cancel.clone();
    let err = Kiln::new()
        .convert_with(
            &input,
            "mov",
            &opts,
            &mut |p| {
                if matches!(p, Progress::Fraction(f) if f > 0.0) {
                    trigger.cancel();
                }
            },
            &cancel,
        )
        .unwrap_err();
    assert!(matches!(err, Error::Cancelled), "{err:?}");
    assert_eq!(files_in(dir.path()), ["h264.mp4", "work"]);
    assert_no_work_left(&dir);
}

#[test]
fn prores_falls_back_to_ffmpeg_when_present() {
    if !ffmpeg("prores_falls_back_to_ffmpeg_when_present") {
        return;
    }
    let src = data("prores.mov");
    let info = video::probe(&src).unwrap();
    assert!(
        matches!(info.codec, VideoCodec::Other(_)),
        "{:?}",
        info.codec
    );
    let (dir, input) = stage(&src);
    let out = Kiln::new().convert(&input, "mp4", &options(&dir)).unwrap();
    // formats.toml's MP4 arguments: libx264 and AAC.
    let v = video::probe(&out).unwrap();
    assert_eq!(v.codec, VideoCodec::H264);
    assert_eq!((v.width, v.height), (160, 120));
    assert!(v.sound.is_some());
    assert_no_work_left(&dir);
}

/// Converts the ProRes fixture to `to` through the FFmpeg fallback with
/// `video` as the job's video settings, in a folder of its own.
fn prores_with(to: &str, video: VideoTune) -> (tempfile::TempDir, kiln_core::Result<PathBuf>) {
    let (dir, input) = stage(&data("prores.mov"));
    let mut opts = options(&dir);
    opts.quality.video = video;
    let out = Kiln::new().convert(&input, to, &opts);
    (dir, out)
}

#[test]
fn the_ffmpeg_fallback_follows_the_video_settings() {
    if !ffmpeg("the_ffmpeg_fallback_follows_the_video_settings") {
        return;
    }
    // Quality 45 at 60 lines: scaled down, sides even, as libx264.
    let (dir, out) = prores_with(
        "mp4",
        VideoTune {
            setting: Some(VideoSetting::Quality(45)),
            max_lines: Some(60),
            ..VideoTune::default()
        },
    );
    let v = video::probe(&out.unwrap()).unwrap();
    assert_eq!((v.width, v.height), (80, 60));
    assert_eq!(v.codec, VideoCodec::H264);
    assert_no_work_left(&dir);
    // A limit at or over the source enlarges nothing.
    let (_dir, out) = prores_with(
        "mp4",
        VideoTune {
            setting: Some(VideoSetting::Quality(45)),
            max_lines: Some(720),
            ..VideoTune::default()
        },
    );
    assert_eq!(video::probe(&out.unwrap()).unwrap().height, 120);
    // A lower step is a higher CRF: a smaller file.
    let size = |q: u8| {
        let (_dir, out) = prores_with(
            "mkv",
            VideoTune {
                setting: Some(VideoSetting::Quality(q)),
                ..VideoTune::default()
            },
        );
        std::fs::metadata(out.unwrap()).unwrap().len()
    };
    assert!(size(20) < size(90));
    // HEVC is libx265, and the sound its chosen bitrate.
    let (_dir, out) = prores_with(
        "mkv",
        VideoTune {
            setting: Some(VideoSetting::Quality(60)),
            codec: VCodec::Hevc,
            sound: SoundChoice::Kbps(96),
            ..VideoTune::default()
        },
    );
    let v = video::probe(&out.unwrap()).unwrap();
    assert_eq!(v.codec, VideoCodec::Hevc);
    // A frame rate limit drops frames.
    let (_dir, out) = prores_with(
        "mp4",
        VideoTune {
            setting: Some(VideoSetting::Quality(45)),
            max_fps: Some(5),
            ..VideoTune::default()
        },
    );
    let fps = video::probe(&out.unwrap()).unwrap().fps.unwrap();
    assert!((fps - 5.0).abs() < 0.1, "{fps}");
}

#[test]
fn hevc_into_m4v_on_the_fallback_converts() {
    if !ffmpeg("hevc_into_m4v_on_the_fallback_converts") {
        return;
    }
    let (dir, out) = prores_with(
        "m4v",
        VideoTune {
            setting: Some(VideoSetting::Quality(60)),
            codec: VCodec::Hevc,
            ..VideoTune::default()
        },
    );
    let out = out.unwrap();
    assert_eq!(out.extension().unwrap(), "m4v");
    let v = video::probe(&out).unwrap();
    assert_eq!(v.codec, VideoCodec::Hevc);
    assert!(v.sound.is_some());
    assert_no_work_left(&dir);
}

#[test]
fn an_odd_line_limit_rounds_down_to_even() {
    if !ffmpeg("an_odd_line_limit_rounds_down_to_even") {
        return;
    }
    let (_dir, out) = prores_with(
        "mp4",
        VideoTune {
            setting: Some(VideoSetting::Quality(45)),
            max_lines: Some(61),
            ..VideoTune::default()
        },
    );
    let v = video::probe(&out.unwrap()).unwrap();
    assert_eq!((v.width, v.height), (80, 60));
}

#[test]
fn a_fit_under_the_fallback_cant_reach_is_refused() {
    if !ffmpeg("a_fit_under_the_fallback_cant_reach_is_refused") {
        return;
    }
    let (dir, out) = prores_with(
        "mp4",
        VideoTune {
            fit_bytes: Some(1_000),
            ..VideoTune::default()
        },
    );
    let err = out.unwrap_err();
    assert!(err.to_string().starts_with("MP4 can't go under "), "{err}");
    assert_eq!(files_in(dir.path()), ["prores.mov", "work"]);
    assert_no_work_left(&dir);
    // A limit it can keep to is kept to.
    let limit = 400_000;
    let (dir, out) = prores_with(
        "mp4",
        VideoTune {
            fit_bytes: Some(limit),
            ..VideoTune::default()
        },
    );
    let out = out.unwrap();
    assert!(std::fs::metadata(&out).unwrap().len() <= limit);
    assert_no_work_left(&dir);
}

#[test]
fn prores_without_ffmpeg_says_why() {
    let (dir, input) = stage(&data("prores.mov"));
    let err = Kiln::new()
        .convert(&input, "mp4", &no_ffmpeg(&dir))
        .unwrap_err();
    assert!(
        matches!(err, Error::EngineMissing { engine: "FFmpeg" }),
        "{err:?}"
    );
    // The row's sentence says what is missing. FFmpeg comes with Kiln,
    // so it sends no one to get it.
    let row = kiln_core::JobError::from(err);
    assert_eq!(row.message, "Needs FFmpeg, which isn't installed.");
    assert_eq!(row.store_product, None);
    assert_eq!(files_in(dir.path()), ["prores.mov", "work"]);
    assert_no_work_left(&dir);
}

#[test]
fn sound_the_target_cant_hold_goes_to_ffmpeg() {
    if !media_foundation("sound_the_target_cant_hold_goes_to_ffmpeg") {
        return;
    }
    // DTS can't go into MP4 and Kiln can't read it to encode it (R50).
    let (dir, input) = stage(&data("dts.mkv"));
    let err = Kiln::new()
        .convert(&input, "mp4", &no_ffmpeg(&dir))
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "This file's audio is DTS, which Kiln can't read yet."
    );
    assert!(matches!(err, Error::Media(_)), "{err:?}");
    assert_eq!(files_in(dir.path()), ["dts.mkv", "work"]);
    assert_no_work_left(&dir);
    if !ffmpeg("sound_the_target_cant_hold_goes_to_ffmpeg") {
        return;
    }
    let out = Kiln::new().convert(&input, "mp4", &options(&dir)).unwrap();
    let v = video::probe(&out).unwrap();
    assert_eq!(v.codec, VideoCodec::H264);
    assert_eq!(
        v.sound.as_ref().map(|s| s.codec.clone()),
        Some(kiln_media::Codec::Aac)
    );
    assert_no_work_left(&dir);
}

#[test]
fn ac3_in_mkv_is_flagged_before_conversion() {
    let test = "ac3_in_mkv_is_flagged_before_conversion";
    if !media_foundation(test) || !ffmpeg(test) {
        return;
    }
    // R91: part 1 names AC-3 and E-AC-3 in Matroska but can't decode them
    // (R50). The facts say so before any conversion, as they do for DTS,
    // and with FFmpeg MP4 goes through it (R52).
    let dir = tempfile::tempdir().unwrap();
    let src = data("h264.mp4");
    let make = |codec: &str| {
        let out = dir.path().join(format!("{codec}.mkv"));
        let done = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-i"])
            .arg(&src)
            .args(["-map", "0:v", "-map", "0:a", "-c:v", "copy", "-c:a", codec])
            .arg(&out)
            .status()
            .unwrap();
        assert!(done.success(), "{codec}");
        out
    };
    let facts = |p: &Path| {
        kiln_core::inspect(&Kiln::new(), p, &no_ffmpeg(&dir), &CancelToken::new()).unwrap()
    };
    for (codec, name) in [("ac3", "AC3"), ("eac3", "EAC3")] {
        let input = make(codec);
        assert_eq!(
            facts(&input).audio_problem.as_deref(),
            Some(format!("This file's audio is {name}, which Kiln can't read yet.").as_str()),
            "{codec}"
        );
        let err = Kiln::new()
            .convert(&input, "mp4", &no_ffmpeg(&dir))
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("This file's audio is {name}, which Kiln can't read yet.")
        );
        let out = Kiln::new().convert(&input, "mp4", &options(&dir)).unwrap();
        assert_eq!(
            video::probe(&out).unwrap().sound.map(|s| s.codec),
            Some(kiln_media::Codec::Aac),
            "{codec}"
        );
    }
    // DTS is flagged the same way, and AAC isn't flagged at all.
    assert_eq!(
        facts(&data("dts.mkv")).audio_problem.as_deref(),
        Some("This file's audio is DTS, which Kiln can't read yet.")
    );
    assert_eq!(facts(&data("h264.mkv")).audio_problem, None);
    assert_no_work_left(&dir);
}

#[test]
fn the_video_knob_is_offered_for_the_four_pairs() {
    let r = Registry::builtin();
    let four = ["mp4", "m4v", "mov", "mkv"];
    for from in four {
        for to in four {
            if from == to {
                continue;
            }
            let (f, t) = (r.format(from).unwrap(), r.format(to).unwrap());
            assert_eq!(quality::knobs(f, t), [Knob::VideoTune], "{from} -> {to}");
        }
    }
    // FFmpeg's pairs keep the preset and their rows.
    for (from, to) in [("mp4", "webm"), ("avi", "mp4"), ("webm", "mkv")] {
        let k = quality::knobs(r.format(from).unwrap(), r.format(to).unwrap());
        assert!(k.contains(&Knob::Preset), "{from} -> {to}: {k:?}");
        assert!(!k.contains(&Knob::VideoTune), "{from} -> {to}: {k:?}");
    }
}

/// What kiln-media's own estimate is for `path` written as `t` with `q`,
/// the setting chosen as the job chooses it.
fn media_estimate(path: &Path, t: VideoTarget, q: &Quality) -> Option<u64> {
    let info = video::probe(path).unwrap();
    let tune = quality::video_tune(q);
    let preset = match q.preset {
        Preset::Small => kiln_media::Preset::Small,
        Preset::High => kiln_media::Preset::High,
        Preset::Best => kiln_media::Preset::Best,
    };
    let s = tune::chosen_setting(t, preset, &tune, &info).ok()?;
    tune::estimate_bytes(t, &s, &info, &tune)
}

#[test]
fn estimate_matches_media_estimate() {
    let r = Registry::builtin();
    let kiln = Kiln::new();
    let dir = tempfile::tempdir().unwrap();
    let opts = no_ffmpeg(&dir);
    let tunes = [
        Quality::default(),
        Quality {
            preset: Preset::Small,
            ..Quality::default()
        },
        Quality {
            preset: Preset::Best,
            ..Quality::default()
        },
        Quality {
            video: VideoTune {
                setting: Some(VideoSetting::Quality(70)),
                codec: VCodec::Hevc,
                ..VideoTune::default()
            },
            ..Quality::default()
        },
        Quality {
            video: VideoTune {
                fit_bytes: Some(600_000),
                ..VideoTune::default()
            },
            ..Quality::default()
        },
        Quality {
            size: kiln_core::Limit::Max(120),
            video: VideoTune {
                sound: SoundChoice::Kbps(96),
                ..VideoTune::default()
            },
            ..Quality::default()
        },
    ];
    for name in [
        "h264.mp4",
        "h264.mkv",
        "rot90.mp4",
        "hevc10_hdr.mp4",
        "dts.mkv",
    ] {
        let path = data(name);
        let info = kiln.probe(&path, &opts).unwrap();
        assert!(info.video_codec.is_some(), "{name}");
        for (to, t) in [("mp4", VideoTarget::Mp4), ("mkv", VideoTarget::Mkv)] {
            for q in &tunes {
                let ours = quality::estimate(r.format(to).unwrap(), q, &info);
                let theirs = media_estimate(&path, t, q);
                assert_eq!(ours, theirs, "{name} -> {to} with {q:?}");
                // Every one has a size, a copy from MKV too (R54).
                assert!(ours.is_some(), "{name} -> {to} with {q:?}");
            }
        }
    }
    // A fragmented MP4 has no length, so no estimate (R22).
    let mut fragmented = kiln.probe(&data("h264.mp4"), &opts).unwrap();
    fragmented.duration = None;
    assert_eq!(
        quality::estimate(r.format("mkv").unwrap(), &Quality::default(), &fragmented),
        None
    );
    assert!(!quality::fit_under_offered(&fragmented));
}

#[test]
fn the_saved_audio_default_stays_out_of_video() {
    // R55: Settings' audio bitrate is for audio files; a video's sound is
    // re-encoded only when this job asks.
    let saved = Quality {
        audio_kbps: Some(96),
        ..Quality::default()
    };
    assert_eq!(quality::video_tune(&saved).sound, SoundChoice::Keep);
    let asked = Quality {
        video: VideoTune {
            sound: SoundChoice::Kbps(96),
            ..VideoTune::default()
        },
        ..saved
    };
    assert_eq!(quality::video_tune(&asked).sound, SoundChoice::Kbps(96));
    if !media_foundation("the_saved_audio_default_stays_out_of_video") {
        return;
    }
    // The 64 kbps AAC sound is copied whatever was saved; asked for 96, it
    // is re-encoded, which makes a bigger file.
    let size = |q: Quality| {
        let (dir, input) = stage(&data("h264.mp4"));
        let mut opts = no_ffmpeg(&dir);
        opts.quality = q;
        let out = Kiln::new().convert(&input, "mov", &opts).unwrap();
        std::fs::metadata(out).unwrap().len()
    };
    let source = std::fs::metadata(data("h264.mp4")).unwrap().len();
    let (kept, encoded) = (size(saved), size(asked));
    assert!(kept.abs_diff(source) < 2_000, "{kept} from {source}");
    assert!(encoded > kept + 3_000, "{encoded} against {kept}");
}

#[test]
fn bars_count_from_the_first_frame() {
    if !ffmpeg("bars_count_from_the_first_frame") {
        return;
    }
    // A letterboxed picture whose first frame is at 10 s.
    let dir = tempfile::tempdir().unwrap();
    let late = dir.path().join("late.mkv");
    let made = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
        .arg("testsrc2=size=64x36:rate=10:duration=1")
        .args(["-vf", "pad=64:48:0:6:black", "-output_ts_offset", "10"])
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
        .arg(&late)
        .status()
        .unwrap();
    assert!(made.success());
    let facts =
        kiln_core::inspect(&Kiln::new(), &late, &no_ffmpeg(&dir), &CancelToken::new()).unwrap();
    assert_eq!(facts.bars, Some(kiln_core::Bars::AboveBelow));
}

#[test]
fn the_video_engine_is_used_for_the_four_and_only_them() {
    let kiln = Kiln::new();
    let r = kiln.registry();
    let backend = |a: &str, b: &str| r.backend(r.format(a).unwrap(), r.format(b).unwrap());
    assert_eq!(backend("mp4", "mkv"), Some("video"));
    assert_eq!(backend("mkv", "mov"), Some("video"));
    assert_eq!(backend("avi", "mp4"), Some("ffmpeg"));
    assert_eq!(backend("mp4", "gif"), Some("ffmpeg"));
}

#[test]
fn a_videos_sound_and_subtitle_tracks_are_counted_and_said() {
    if !support::have_ffmpeg("a_videos_sound_and_subtitle_tracks_are_counted_and_said") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let kiln = Kiln::new();
    let both = [
        "Keeps the first of 2 sound tracks.",
        "Subtitles aren't kept.",
    ];
    for (name, subtitles) in [("two.mkv", "srt"), ("two.mp4", "mov_text")] {
        let file = support::video_with_tracks(dir.path(), name, subtitles);
        let info = kiln.probe(&file, &Options::default()).unwrap();
        assert_eq!((info.sound_tracks, info.subtitle_tracks), (2, 1), "{name}");
        // The picture's facts are still its own.
        assert_eq!((info.width, info.height), (Some(64), Some(48)), "{name}");
        // A video target says both; a sound target only the sound; a picture
        // or a subtitle file neither.
        let target = if name.ends_with("mkv") { "mp4" } else { "mkv" };
        assert_eq!(kiln.drops_reading(&file, target), both, "{name}");
        assert_eq!(kiln.drops_reading(&file, "webm"), both, "{name}");
        assert_eq!(
            kiln.drops_reading(&file, "mp3"),
            ["Keeps the first of 2 sound tracks."],
            "{name}"
        );
        assert!(kiln.drops_reading(&file, "gif").is_empty(), "{name}");
        assert!(kiln.drops_reading(&file, "srt").is_empty(), "{name}");
    }
    // A video the headers can't be read of here is read by FFprobe.
    let avi = dir.path().join("two.avi");
    let made = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "lavfi", "-i", "testsrc=size=64x48:rate=10:duration=1"])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=1"])
        .args(["-f", "lavfi", "-i", "sine=frequency=880:duration=1"])
        .args(["-map", "0:v", "-map", "1:a", "-map", "2:a"])
        .args(["-c:v", "mpeg4", "-c:a", "pcm_s16le"])
        .arg(&avi)
        .status()
        .unwrap();
    assert!(made.success());
    let info = kiln.probe(&avi, &Options::default()).unwrap();
    assert_eq!((info.sound_tracks, info.subtitle_tracks), (2, 0));
    assert_eq!(
        kiln.drops_reading(&avi, "mp4"),
        ["Keeps the first of 2 sound tracks."]
    );
    // A job cancelled while it is looked at is looked at no further.
    let cancelled = CancelToken::new();
    cancelled.cancel();
    assert!(kiln
        .drops_reading_with(&avi, "mp4", &Options::default(), &cancelled)
        .is_empty());
    // With no FFmpeg to ask there is nothing to go by, so nothing is said.
    let opts = Options {
        engines: Engines {
            ffmpeg: Some(dir.path().join("no-such-dir").join("ffmpeg.exe")),
            ..Engines::default()
        },
        ..Options::default()
    };
    assert!(kiln
        .drops_reading_with(&avi, "mp4", &opts, &CancelToken::new())
        .is_empty());
    // A file that isn't a video gives no tracks and says nothing.
    let other = dir.path().join("note.txt");
    std::fs::write(&other, "words").unwrap();
    assert!(kiln.drops_reading(&other, "docx").is_empty());
    assert!(kiln
        .drops_reading(&dir.path().join("gone.mkv"), "mp4")
        .is_empty());
}

#[test]
fn kilns_own_engines_keep_the_first_sound_track_and_no_subtitles() {
    // The notes are true only if every engine that can run the route does
    // what they say.
    if !support::have_ffmpeg("kilns_own_engines_keep_the_first_sound_track_and_no_subtitles")
        || !media_foundation("kilns_own_engines_keep_the_first_sound_track_and_no_subtitles")
    {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mkv = support::video_with_tracks(dir.path(), "two.mkv", "srt");
    let opts = no_ffmpeg(&dir);
    let kiln = Kiln::new();

    // Video among the four.
    let mp4 = kiln.convert(&mkv, "mp4", &opts).unwrap();
    assert_eq!(video::track_counts(&mp4), Some(counts(1, 0)));
    let sound = kiln_media::video::probe(&mp4).unwrap().sound.unwrap();
    assert_eq!(sound.channels.count, 1, "the mono track is the first");

    // Sound from a video, by the engine that writes it itself and by the
    // one that writes through Media Foundation.
    for target in ["mp3", "m4a"] {
        let sound = kiln.convert(&mkv, target, &opts).unwrap();
        let heard = kiln_media::probe(&sound).unwrap();
        assert_eq!(heard.channels.count, 1, "{target}");
    }
}

fn counts(sound: u16, subtitles: u16) -> video::TrackCounts {
    video::TrackCounts { sound, subtitles }
}

#[test]
fn a_first_sound_kilns_engines_skip_gets_no_first_of_note() {
    // Kiln's sound engine reads the first sound track whose codec it can
    // name, so with AC-3 first it reads the second, and its video engine
    // starts from what the sound engine reads. A missing note is better
    // than a false one.
    let test = "a_first_sound_kilns_engines_skip_gets_no_first_of_note";
    if !support::have_ffmpeg(test) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let kiln = Kiln::new();
    let first_of_two = ["Keeps the first of 2 sound tracks."];
    // `probe` says whether they read a later track, for the sheet's notes.
    let later = |file: &std::path::Path| kiln.probe(file, &Options::default()).unwrap().later_sound;
    for ext in ["mkv", "mp4"] {
        let video = if ext == "mkv" { "mp4" } else { "mkv" };
        // The plain case: two it can name, and the first is read.
        let plain = support::video_with_sound(dir.path(), &format!("plain.{ext}"), &["aac", "aac"]);
        assert!(!later(&plain), "{ext}");
        assert_eq!(kiln.drops_reading(&plain, "mp3"), first_of_two, "{ext}");
        assert_eq!(kiln.drops_reading(&plain, "m4a"), first_of_two, "{ext}");
        assert_eq!(kiln.drops_reading(&plain, video), first_of_two, "{ext}");
        // One it can name, then one it can't: the first is still read.
        let last = support::video_with_sound(dir.path(), &format!("last.{ext}"), &["aac", "ac3"]);
        assert!(!later(&last), "{ext}");
        assert_eq!(kiln.drops_reading(&last, "mp3"), first_of_two, "{ext}");
        assert_eq!(kiln.drops_reading(&last, video), first_of_two, "{ext}");

        // AC-3 first: the sound engines (MP3 and the Media Foundation ones)
        // and the video engine read the AAC second.
        let skipped =
            support::video_with_sound(dir.path(), &format!("skipped.{ext}"), &["ac3", "aac"]);
        assert!(later(&skipped), "{ext}");
        assert!(kiln.drops_reading(&skipped, "mp3").is_empty(), "{ext}");
        assert!(kiln.drops_reading(&skipped, "m4a").is_empty(), "{ext}");
        assert!(kiln.drops_reading(&skipped, video).is_empty(), "{ext}");
        // FFmpeg reads the first, whatever it is.
        assert_eq!(kiln.drops_reading(&skipped, "webm"), first_of_two, "{ext}");
    }

    // The premises, run for real. If an engine comes to read the first
    // track, its line above goes back to saying so.
    let skipped = dir.path().join("skipped.mkv");
    let opts = options(&dir);
    let mp3 = kiln.convert(&skipped, "mp3", &opts).unwrap();
    assert_eq!(kiln_media::probe(&mp3).unwrap().channels.count, 2, "mp3");
    let mp4 = kiln.convert(&skipped, "mp4", &opts).unwrap();
    let sound = video::probe(&mp4).unwrap().sound.unwrap();
    assert_eq!(sound.channels.count, 2, "mp4");
    // FFmpeg takes the first (mono) track.
    let webm = kiln.convert(&skipped, "webm", &opts).unwrap();
    assert_eq!(kiln_media::probe(&webm).unwrap().channels.count, 1, "webm");
}
