//! The size estimate and the slider's labels never start the graphics
//! card's encoder (R82): they read what is already known of it, and until
//! then use the graphics card's table.
//!
//! One test only, in a test binary of its own: the answer is kept for the
//! life of the process, so nothing else may ask for it first.

#![cfg(windows)]

mod common;

use kiln_media::video::{
    self, encoder_adapter, encoder_adapter_known, tune, EncoderChoice, VCodec, VideoSetting,
    VideoTarget, VideoTune,
};
use kiln_media::Preset;
use std::path::Path;

#[test]
fn the_labels_never_start_the_graphics_card() {
    if common::no_mf() {
        return;
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/video/h264.mp4");
    let info = video::probe(&path).unwrap();
    let t = VideoTarget::Mp4;
    let choices = [EncoderChoice::Automatic, EncoderChoice::GraphicsCard];
    let codecs = [VCodec::H264, VCodec::Hevc];
    if codecs
        .iter()
        .any(|&c| encoder_adapter_known(c, EncoderChoice::GraphicsCard).is_some())
    {
        eprintln!("skipped: the graphics card is hidden, so its answer is known at once");
        return;
    }
    let q65 = |tune: &VideoTune| {
        tune::estimate_bytes(t, &VideoSetting::Quality(65), &info, tune).unwrap()
    };
    let mut before = Vec::new();
    for codec in codecs {
        for encoder in choices {
            let tune = VideoTune {
                codec,
                encoder,
                ..VideoTune::default()
            };
            // Everything the app's sheet and the job's plan work out.
            let steps = tune::steps(t, &info, &tune);
            assert!(!steps.is_empty());
            for s in &steps {
                let _ = tune::label_of(t, &s.setting, &info, &tune);
                let _ = tune::label_parts(t, &s.setting, &info, &tune);
            }
            let _ = tune::fit_under(t, 1_000_000, &info, &tune);
            let _ = tune::quality_under(t, 1_000_000, &info, &tune);
            let _ = tune::chosen_setting(t, Preset::High, &tune, &info);
            before.push((codec, encoder, q65(&tune)));
        }
    }
    for codec in codecs {
        assert_eq!(
            encoder_adapter_known(codec, EncoderChoice::GraphicsCard),
            None,
            "the estimate started the {codec:?} encoder"
        );
    }
    // Once a worker has asked, the estimate follows the answer: the card's
    // table where an encoder starts (as before), else the processor's.
    for (codec, encoder, was) in before {
        let card = encoder_adapter(codec, encoder);
        assert_eq!(encoder_adapter_known(codec, encoder), Some(card));
        let tune = VideoTune {
            codec,
            encoder,
            ..VideoTune::default()
        };
        let processor = q65(&VideoTune {
            encoder: EncoderChoice::Processor,
            ..tune
        });
        match card {
            Some(_) => assert_eq!(q65(&tune), was, "{codec:?} {encoder:?}"),
            None => {
                assert_eq!(q65(&tune), processor, "{codec:?} {encoder:?}");
                assert_ne!(was, processor, "{codec:?} {encoder:?}");
            }
        }
    }
}
