//! Each encoder's own settings (R85, Task 10d): every encoder on this PC is
//! given the settings chosen for it and reads each one back as set, its
//! stream shows them, and a bitrate still lands near its target with them.
//!
//! The graphics card checks skip, and say so, when this PC has no hardware
//! encoder for the codec; the HEVC processor checks skip without the HEVC
//! Video Extensions.

#![cfg(windows)]

mod common;

use kiln_media::video::encode::plan;
use kiln_media::video::encoder_settings::{key_interval, settings_for, EncoderFamily};
use kiln_media::video::{
    d3d_for, encoder_adapter, probe, Frame, Packet, ReadMode, Reader, VCodec, VideoEncoder,
    VideoSetting, VideoTune, TIMESCALE,
};
use kiln_media::MediaError;
use std::fs::File;
use std::path::{Path, PathBuf};

const NVIDIA: u32 = 0x10DE;

fn video(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/video")
        .join(name)
}

/// Every codec and encoder choice, with the family the encoder Kiln starts
/// for it on this PC is expected to be, or `None` when there is none.
fn encoders() -> Vec<(VCodec, EncoderChoiceAndFamily)> {
    let mut list = Vec::new();
    for codec in [VCodec::H264, VCodec::Hevc] {
        let processor = match codec {
            VCodec::H264 => EncoderFamily::MicrosoftH264,
            VCodec::Hevc => EncoderFamily::MicrosoftHevc,
        };
        list.push((
            codec,
            (kiln_media::video::EncoderChoice::Processor, processor),
        ));
        let card = kiln_media::video::EncoderChoice::GraphicsCard;
        match encoder_adapter(codec, card).and_then(d3d_for) {
            Some(d) if d.adapter().vendor == NVIDIA => {
                list.push((codec, (card, EncoderFamily::Nvidia)))
            }
            Some(_) => list.push((codec, (card, EncoderFamily::Other))),
            None => eprintln!("skipped: no graphics card encoder for {codec:?}"),
        }
    }
    list
}

type EncoderChoiceAndFamily = (kiln_media::video::EncoderChoice, EncoderFamily);

/// The encoder for `setting` on `src`, or `None` (said) when this PC has
/// none for it.
fn start(
    src: &Path,
    codec: VCodec,
    choice: kiln_media::video::EncoderChoice,
    setting: VideoSetting,
) -> Option<(VideoEncoder, Option<kiln_media::video::D3d>)> {
    let tune = VideoTune {
        setting: Some(setting),
        codec,
        encoder: choice,
        ..VideoTune::default()
    };
    let p = plan(&probe(src).unwrap(), &tune);
    let d3d = encoder_adapter(codec, choice).and_then(d3d_for);
    match VideoEncoder::new(&p, choice, d3d.as_ref()) {
        Ok(e) => Some((e, d3d)),
        Err(e @ MediaError::NeedsExtensionToWrite { .. }) => {
            eprintln!("skipped {codec:?} on {choice:?}: {e}");
            None
        }
        Err(e) => panic!("{codec:?} on {choice:?}: {e}"),
    }
}

/// `src` decoded `loops` times back to back through `enc`.
fn encode(
    src: &Path,
    enc: &mut VideoEncoder,
    d3d: Option<kiln_media::video::D3d>,
    loops: u32,
) -> Vec<Packet> {
    let mut packets = Vec::new();
    let mut offset = 0i64;
    for _ in 0..loops {
        let mut r = Reader::open(
            File::open(src).unwrap(),
            ReadMode::Decoded { d3d: d3d.clone() },
        )
        .unwrap();
        let mut end = 0i64;
        while let Some(f) = r.next_frame().unwrap() {
            end = end.max(f.pts + f.duration);
            let f = Frame {
                pts: f.pts + offset,
                ..f
            };
            // SAFETY: a plain call on a sample the reader gave.
            unsafe { f.sample.SetSampleTime(f.pts) }.unwrap();
            enc.push(f, &mut packets).unwrap();
        }
        offset += end;
    }
    enc.finish(&mut packets).unwrap();
    packets
}

#[test]
fn every_encoder_reads_back_the_settings_chosen_for_it() {
    if common::no_mf() {
        return;
    }
    let src = video("h264.mp4");
    for (codec, (choice, family)) in encoders() {
        for setting in [VideoSetting::Quality(65), VideoSetting::Kbps(800)] {
            let Some((e, _)) = start(&src, codec, choice, setting) else {
                continue;
            };
            let chosen = settings_for(family);
            assert_eq!(e.encoder_settings(), chosen, "{codec:?} on {choice:?}");
            let back = e.settings_read_back();
            let mut want = vec![("key frame interval", Some(key_interval(&chosen, 30.0)))];
            if let Some(b) = chosen.b_frames {
                want.push(("B-frames", Some(b)));
            }
            if let Some(c) = chosen.cabac {
                want.push(("CABAC", Some(u32::from(c))));
            }
            if let Some(q) = chosen.quality_vs_speed {
                want.push(("quality vs speed", Some(q)));
            }
            // An unmeasured encoder may refuse the B-frames it is offered.
            if family == EncoderFamily::Other {
                want.retain(|(name, _)| *name != "B-frames");
            }
            let got: Vec<_> = back
                .iter()
                .filter(|(name, _)| want.iter().any(|(w, _)| w == name))
                .copied()
                .collect();
            assert_eq!(got, want, "{codec:?} on {choice:?} with {setting:?}");
        }
    }
}

#[test]
fn the_streams_show_the_settings() {
    if common::no_mf() {
        return;
    }
    let src = video("h264.mp4");
    for (codec, (choice, family)) in encoders() {
        let Some((mut e, d3d)) = start(&src, codec, choice, VideoSetting::Quality(65)) else {
            continue;
        };
        // 10 s at 30 frames a second.
        let packets = encode(&src, &mut e, d3d, 5);
        assert_eq!(packets.len(), 300, "{codec:?} on {choice:?}");
        let mut shown: Vec<&Packet> = packets.iter().collect();
        shown.sort_by_key(|p| p.pts);
        let keys: Vec<usize> = shown
            .iter()
            .enumerate()
            .filter(|(_, p)| p.key)
            .map(|(i, _)| i)
            .collect();
        let interval = key_interval(&settings_for(family), 30.0) as usize;
        // A key frame at least every interval, from the first frame; the
        // encoder may add one at a cut, which this clip has where it loops.
        assert_eq!(keys.first(), Some(&0), "{codec:?} on {choice:?}");
        assert!(
            keys.windows(2).all(|w| w[1] - w[0] <= interval),
            "{codec:?} on {choice:?}: key frames at {keys:?}, interval {interval}"
        );
        if family != EncoderFamily::Other {
            assert!(
                keys.len() <= 300 / interval + 5,
                "{codec:?} on {choice:?}: key frames at {keys:?}, interval {interval}"
            );
        }
        // No B-frames where none are asked for: decode order is display
        // order.
        if settings_for(family).b_frames == Some(0) || family == EncoderFamily::Nvidia {
            assert!(
                packets.windows(2).all(|w| w[0].pts < w[1].pts),
                "{codec:?} on {choice:?} reorders frames"
            );
        }
        // CABAC: the H.264 picture parameter set's entropy_coding_mode_flag.
        if settings_for(family).cabac == Some(true) {
            let spec = e.stream();
            let kiln_media::video::StreamCodec::H264 { avcc } = &spec.codec else {
                panic!("not H.264");
            };
            assert!(pps_says_cabac(avcc), "{codec:?} on {choice:?} writes CAVLC");
        }
    }
}

/// Whether the first PPS in an `avcC` record says CABAC: its
/// `entropy_coding_mode_flag`, after two Exp-Golomb numbers.
fn pps_says_cabac(avcc: &[u8]) -> bool {
    // avcC: 5 bytes, SPS count, the SPSs, PPS count, then each PPS's
    // length and bytes.
    let mut i = 5;
    let sps = (avcc[i] & 0x1F) as usize;
    i += 1;
    for _ in 0..sps {
        let n = u16::from_be_bytes([avcc[i], avcc[i + 1]]) as usize;
        i += 2 + n;
    }
    i += 1;
    let n = u16::from_be_bytes([avcc[i], avcc[i + 1]]) as usize;
    let pps = &avcc[i + 2 + 1..i + 2 + n];
    let mut bits = pps
        .iter()
        .flat_map(|b| (0..8).rev().map(move |s| (b >> s) & 1 == 1));
    let mut ue = || {
        let mut zeros = 0;
        while !bits.next().unwrap() {
            zeros += 1;
        }
        let mut v = 0u32;
        for _ in 0..zeros {
            v = (v << 1) | u32::from(bits.next().unwrap());
        }
        (1 << zeros) - 1 + v
    };
    let _pps_id = ue();
    let _sps_id = ue();
    bits.next().unwrap()
}

#[test]
fn a_bitrate_lands_near_its_target_with_the_settings() {
    if common::no_mf() {
        return;
    }
    let src = video("h264.mp4");
    for (codec, (choice, _)) in encoders() {
        let Some((mut e, d3d)) = start(&src, codec, choice, VideoSetting::Kbps(800)) else {
            continue;
        };
        assert!(e.honours_bitrate());
        let packets = encode(&src, &mut e, d3d, 5);
        let first = packets.iter().map(|p| p.pts).min().unwrap();
        let end = packets.iter().map(|p| p.pts + p.duration).max().unwrap();
        let secs = (end - first) as f64 / TIMESCALE as f64;
        let bytes: usize = packets.iter().map(|p| p.data.len()).sum();
        let kbps = bytes as f64 * 8.0 / secs / 1000.0;
        eprintln!("{codec:?} on {choice:?} at Kbps(800): {kbps:.0} kbps over {secs:.2} s");
        assert!(
            (600.0..=1000.0).contains(&kbps),
            "{codec:?} on {choice:?}: {kbps:.0} kbps for an 800 kbps target"
        );
    }
}
