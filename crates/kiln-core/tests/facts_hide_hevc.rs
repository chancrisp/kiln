//! A video's facts name the Store extension Windows needs to read it (R56),
//! on a Windows without the HEVC Video Extensions, as debug builds can
//! pretend with `KILN_HIDE_HEVC`.
//!
//! One test only, in a test binary of its own: the variable changes what
//! every reader in the process sees, so nothing may run beside it.

#![cfg(windows)]

#[path = "../../../tests/support/mod.rs"]
mod support;

use kiln_core::{inspect, CancelToken, Kiln, Options};
use kiln_media::video::VideoCodec;
use support::media_data;

#[test]
fn facts_name_the_missing_hevc_extension() {
    if !Kiln::new()
        .availability(&Options::default())
        .has("audio-mf")
    {
        eprintln!("skipped: Media Foundation's codecs aren't installed");
        return;
    }
    if !cfg!(debug_assertions) {
        eprintln!("skipped: KILN_HIDE_HEVC works in debug builds only");
        return;
    }
    let hevc = media_data("video/hevc.mp4");
    let look = || {
        inspect(
            &Kiln::new(),
            &hevc,
            &Options::default(),
            &CancelToken::new(),
        )
        .unwrap()
        .media
        .unwrap()
    };
    std::env::set_var("KILN_HIDE_HEVC", "1");
    let hidden = look();
    // H.264 needs no extension, hidden HEVC or not.
    let h264 = inspect(
        &Kiln::new(),
        &media_data("video/h264.mp4"),
        &Options::default(),
        &CancelToken::new(),
    )
    .unwrap()
    .media
    .unwrap();
    std::env::remove_var("KILN_HIDE_HEVC");
    assert_eq!(hidden.video_codec, Some(VideoCodec::Hevc));
    assert_eq!(hidden.needs_extension, Some(("HEVC", "9N4WGH0Z6VHQ")));
    assert_eq!(h264.needs_extension, None);
    // The same check the reader makes: with the extension (installed here
    // when HEVC decodes), nothing is missing.
    let shown = look();
    let decodes = kiln_media::video::Reader::open(
        std::fs::File::open(&hevc).unwrap(),
        kiln_media::video::ReadMode::Decoded { d3d: None },
    )
    .is_ok();
    assert_eq!(shown.needs_extension.is_none(), decodes);
}
