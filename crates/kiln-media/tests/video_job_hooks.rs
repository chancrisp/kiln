//! The video job under the debug-only hooks: `KILN_FAIL_WRITE` set to an
//! HRESULT makes every packet write fail with it, as Media Foundation would
//! report it, and `KILN_FAIL_HW_ENCODER` makes a hardware encoder fail on its
//! first frame.
//!
//! In a test binary of its own, one test at a time: the variable changes
//! what every job in the process sees.

#![cfg(all(windows, debug_assertions))]

mod common;

use kiln_media::video::{
    convert, encoder_adapter, EncoderChoice, VCodec, VideoSetting, VideoTarget, VideoTune,
};
use kiln_media::{MediaError, Preset};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// One test at a time: they set a process-wide variable.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// Sets `name` to `value` for as long as it lives.
struct Var(&'static str);

impl Var {
    fn set(name: &'static str, value: &str) -> Var {
        std::env::set_var(name, value);
        Var(name)
    }
}

impl Drop for Var {
    fn drop(&mut self) {
        std::env::remove_var(self.0);
    }
}

fn video(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/video")
        .join(name)
}

#[test]
fn a_full_disk_is_an_io_error() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    if common::no_mf() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let encode = VideoTune {
        setting: Some(VideoSetting::Quality(65)),
        ..VideoTune::default()
    };
    // ERROR_DISK_FULL and ERROR_HANDLE_DISK_FULL, as HRESULTs.
    for (hr, os) in [("0x80070070", 112), ("0x80070027", 39)] {
        let _full = Var::set("KILN_FAIL_WRITE", hr);
        for (name, target, tune) in [
            ("copy.mkv", VideoTarget::Mkv, VideoTune::default()),
            ("encode.mp4", VideoTarget::Mp4, encode),
        ] {
            let out = dir.path().join(format!("{os}-{name}"));
            let r = convert(
                &video("h264.mp4"),
                &out,
                target,
                Preset::High,
                &tune,
                &mut |_| {},
                &|| false,
            );
            match r {
                Err(MediaError::Io(e)) => assert_eq!(e.raw_os_error(), Some(os), "{name}"),
                other => panic!("{hr} {name}: {other:?}"),
            }
            assert!(!out.exists());
        }
    }
    // Nothing is left: no output, no `.part`, no temporary sample file.
    let left: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn automatic_falls_back_to_the_processor_before_the_first_packet() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    if common::no_mf() {
        return;
    }
    if encoder_adapter(VCodec::H264, EncoderChoice::Automatic).is_none() {
        eprintln!("skipped: no hardware H.264 encoder");
        return;
    }
    let _fail = Var::set("KILN_FAIL_HW_ENCODER", "1");
    let dir = tempfile::tempdir().unwrap();
    let encode = |encoder| VideoTune {
        setting: Some(VideoSetting::Quality(65)),
        encoder,
        ..VideoTune::default()
    };
    // Automatic: the software encoder takes over, and the job says so.
    let out = dir.path().join("automatic.mp4");
    let report = convert(
        &video("h264.mp4"),
        &out,
        VideoTarget::Mp4,
        Preset::High,
        &encode(EncoderChoice::Automatic),
        &mut |_| {},
        &|| false,
    )
    .unwrap();
    assert!(!report.video_copied && !report.hardware, "{report:?}");
    // R58: a fallback from the card, so the job says it fell back.
    assert!(report.fell_back, "{report:?}");
    assert!(out.exists());
    // Graphics card only: the failure is the job's, and nothing is left.
    let out = dir.path().join("card.mp4");
    let r = convert(
        &video("h264.mp4"),
        &out,
        VideoTarget::Mp4,
        Preset::High,
        &encode(EncoderChoice::GraphicsCard),
        &mut |_| {},
        &|| false,
    );
    assert!(r.is_err(), "{r:?}");
    assert!(!out.exists());
    let left: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .filter(|n| n != "automatic.mp4")
        .collect();
    assert!(left.is_empty(), "{left:?}");
}
