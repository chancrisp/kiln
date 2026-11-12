//! Kiln's own video engine as the environment changes it: with FFmpeg
//! taken off PATH, with the HEVC extension hidden (`KILN_HIDE_HEVC`), and
//! with the graphics cards' encoders hidden (`KILN_HIDE_HW_ENCODERS`).
//!
//! One test only, in a test binary of its own: the variables change what
//! every job in the process sees, so nothing may run beside it.

#![cfg(windows)]

#[path = "../../../tests/support/mod.rs"]
mod support;

use std::ffi::OsString;
use std::path::PathBuf;

use kiln_core::{
    inspect, Bars, CancelToken, Error, JobError, Kiln, Options, Progress, ON_THE_PROCESSOR,
};
use kiln_media::video::{self, EncoderChoice, VCodec, VideoCodec, VideoSetting};
use support::{files_in, fixture, media_data, stage};

fn options(temp: &tempfile::TempDir) -> Options {
    Options {
        temp_root: Some(temp.path().join("work")),
        ..Options::default()
    }
}

fn data(name: &str) -> PathBuf {
    media_data(&format!("video/{name}"))
}

/// PATH without the folders that hold an ffmpeg.exe.
fn path_without_ffmpeg(path: &OsString) -> OsString {
    let kept: Vec<PathBuf> = std::env::split_paths(path)
        .filter(|dir| !dir.join("ffmpeg.exe").is_file() && !dir.join("ffprobe.exe").is_file())
        .collect();
    std::env::join_paths(kept).unwrap()
}

#[test]
fn video_as_the_environment_changes() {
    if !Kiln::new()
        .availability(&Options::default())
        .has("audio-mf")
    {
        eprintln!("skipped: Media Foundation's codecs aren't installed");
        return;
    }
    let had_ffmpeg =
        kiln_core::find_on_path(&std::env::var_os("PATH").unwrap_or_default(), "ffmpeg").is_some();

    // 1. FFmpeg off PATH: the facts and the conversion are Kiln's own.
    //    Kiln reads PATH afresh from the registry, which still names
    //    FFmpeg's folder, so it is asked to use only its own.
    std::env::set_var("KILN_TEST_PROCESS_PATH", "1");
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::set_var("PATH", path_without_ffmpeg(&path));
    let kiln = Kiln::new();
    let facts = inspect(
        &kiln,
        &fixture("sample.mp4"),
        &Options::default(),
        &CancelToken::new(),
    )
    .unwrap();
    let m = facts.media.as_ref().unwrap();
    assert_eq!((m.width, m.height), (Some(64), Some(48)));
    assert!(m.duration.is_some() && m.has_audio);
    assert_eq!(facts.bars, Some(Bars::None));
    assert!(facts.estimates.contains_key("mp4"));
    let (dir, input) = stage(&fixture("sample.mkv"));
    assert!(!kiln.availability(&options(&dir)).has("ffmpeg"));
    let out = kiln.convert(&input, "mp4", &options(&dir)).unwrap();
    assert_eq!(video::probe(&out).unwrap().codec, VideoCodec::H264);

    // 2. HEVC without its extension, and no FFmpeg: the row says what to
    //    get, with the Store product for Get it. Copying needs no decoder.
    if cfg!(debug_assertions) {
        std::env::set_var("KILN_HIDE_HEVC", "1");
        let (dir, input) = stage(&data("hevc.mp4"));
        let mut opts = options(&dir);
        opts.quality.video.setting = Some(VideoSetting::Quality(50));
        let err = kiln.convert(&input, "mkv", &opts).unwrap_err();
        assert!(
            matches!(
                err,
                Error::NeedsExtension {
                    product: "9N4WGH0Z6VHQ",
                    ..
                }
            ),
            "{err:?}"
        );
        let row = JobError::from(err);
        assert_eq!(
            row.message,
            "Needs Microsoft's HEVC Video Extensions to read HEVC video."
        );
        assert_eq!(row.store_product, Some("9N4WGH0Z6VHQ"));
        assert_eq!(files_in(dir.path()), ["hevc.mp4", "work"]);
        let copied = kiln.convert(&input, "mkv", &options(&dir)).unwrap();
        assert_eq!(video::probe(&copied).unwrap().codec, VideoCodec::Hevc);

        // With FFmpeg back, FFmpeg converts it.
        std::env::set_var("PATH", &path);
        if had_ffmpeg {
            let (dir, input) = stage(&data("hevc.mp4"));
            let mut opts = options(&dir);
            opts.quality.video.setting = Some(VideoSetting::Quality(50));
            let out = kiln.convert(&input, "mov", &opts).unwrap();
            let v = video::probe(&out).unwrap();
            assert_eq!(v.codec, VideoCodec::H264, "formats.toml's MOV arguments");
        } else {
            eprintln!("FFmpeg fallback for HEVC: skipped, FFmpeg isn't on PATH");
        }
        std::env::remove_var("KILN_HIDE_HEVC");
    } else {
        eprintln!("KILN_HIDE_HEVC: skipped, it works in debug builds only");
    }
    std::env::set_var("PATH", &path);

    // 3. The processor note (R58): only when Automatic meant the picture
    //    for a card whose encoder refused it. A PC with no card encoder,
    //    Processor chosen, or a copy says nothing.
    if cfg!(debug_assertions) {
        let run = |encoder: EncoderChoice, setting: Option<VideoSetting>| {
            let (dir, input) = stage(&data("h264.mp4"));
            let mut opts = options(&dir);
            opts.quality.video.encoder = encoder;
            opts.quality.video.setting = setting;
            let mut notes = vec![];
            kiln.convert_with(
                &input,
                "mkv",
                &opts,
                &mut |p| {
                    if let Progress::Note(n) = p {
                        notes.push(n);
                    }
                },
                &CancelToken::new(),
            )
            .unwrap();
            notes
        };
        let encode = Some(VideoSetting::Quality(40));
        assert_eq!(ON_THE_PROCESSOR, "Encoded on the processor.");
        std::env::set_var("KILN_HIDE_HW_ENCODERS", "1");
        assert!(run(EncoderChoice::Automatic, encode).is_empty(), "no card");
        assert!(run(EncoderChoice::Processor, encode).is_empty());
        assert!(run(EncoderChoice::Automatic, None).is_empty(), "a copy");
        std::env::remove_var("KILN_HIDE_HW_ENCODERS");
        let card =
            kiln_media::video::encoder_adapter(VCodec::H264, EncoderChoice::Automatic).is_some();
        if card {
            // A card whose encoder fails before its first packet.
            std::env::set_var("KILN_FAIL_HW_ENCODER", "1");
            assert_eq!(run(EncoderChoice::Automatic, encode), [ON_THE_PROCESSOR]);
            assert!(run(EncoderChoice::Processor, encode).is_empty());
            std::env::remove_var("KILN_FAIL_HW_ENCODER");
            // A card that does the work says nothing either.
            assert!(run(EncoderChoice::Automatic, encode).is_empty());
        } else {
            eprintln!("the fallback note: skipped, no hardware H.264 encoder");
        }
    } else {
        eprintln!("KILN_HIDE_HW_ENCODERS: skipped, it works in debug builds only");
    }
}
