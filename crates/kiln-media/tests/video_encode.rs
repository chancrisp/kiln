//! Processing and encoding decoded video (`video::VideoEncoder`): the
//! fixtures decoded by Task 4's reader, encoded on the processor or the
//! graphics card, written by Task 3's MP4 writer, and checked with ffprobe and
//! by reading the result back through Media Foundation.
//!
//! The ffprobe checks skip with a printed line when FFmpeg isn't on PATH; the
//! graphics card checks skip, and say so, when this PC has no hardware
//! encoder for the codec.

#![cfg(windows)]

mod common;

use kiln_media::video::encode::plan;
use kiln_media::video::{
    d3d_for, encoder_adapter, muxer, probe, Container, EncodePlan, EncoderChoice, Frame, Hdr,
    Packet, Pixel, ReadMode, Reader, VCodec, VideoEncoder, VideoSetting, VideoTune, TIMESCALE,
};
use kiln_media::{MediaError, Tags};
use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Command;
use windows::core::Interface;
use windows::core::GUID;
use windows::Win32::Media::MediaFoundation::{
    IMF2DBuffer, MFVideoTransferMatrix_BT2020_10, MFVideoTransferMatrix_BT601, MF_MT_YUV_MATRIX,
};

// ---------------------------------------------------------------------------
// Helpers

fn have(tool: &str) -> bool {
    Command::new(tool)
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// True when FFmpeg and ffprobe are on PATH; says so and returns false if not.
fn ffmpeg_ready() -> bool {
    let ok = have("ffmpeg") && have("ffprobe");
    if !ok {
        eprintln!("skipped: FFmpeg isn't on PATH");
    }
    ok
}

fn video(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/video")
        .join(name)
}

/// ffprobe's facts of the first picture stream (frames counted) and the
/// file's length, as `key=value`, keys without their section.
fn facts(path: &Path) -> HashMap<String, String> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-count_frames"])
        .args([
            "-show_entries",
            "stream=codec_name,profile,width,height,nb_read_frames,pix_fmt,color_transfer,color_primaries,color_space,start_time,sample_aspect_ratio:stream_side_data=rotation:format=duration",
            "-of",
            "default=nw=1",
        ])
        .arg(path)
        .output()
        .expect("ffprobe starts");
    assert!(
        out.status.success(),
        "ffprobe failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn fact<'a>(f: &'a HashMap<String, String>, key: &str) -> &'a str {
    f.get(key).map_or("", String::as_str)
}

/// What one encode came to.
struct Encoded {
    spec: kiln_media::video::StreamSpec,
    hardware: bool,
    honours_bitrate: bool,
    packets: Vec<Packet>,
}

/// The card the job would decode on for `choice`: the one whose hardware
/// encoder starts, else none (R34).
fn card(codec: VCodec, choice: EncoderChoice) -> Option<kiln_media::video::D3d> {
    encoder_adapter(codec, choice).and_then(d3d_for)
}

/// Decodes `src` (on the encoder's card, as the job does), encodes it with
/// `plan` and `choice`, and writes it to `out` as an MP4. Each decoded frame
/// is pushed and let go at once. `loops` plays the source that many times,
/// back to back.
fn encode_to(
    src: &Path,
    plan: &EncodePlan,
    choice: EncoderChoice,
    out: &Path,
    loops: u32,
) -> Result<Encoded, MediaError> {
    encode_into(src, plan, choice, out, loops, Container::Mp4)
}

/// As [`encode_to`], into container `c`.
fn encode_into(
    src: &Path,
    plan: &EncodePlan,
    choice: EncoderChoice,
    out: &Path,
    loops: u32,
    c: Container,
) -> Result<Encoded, MediaError> {
    let d3d = card(plan.codec, choice);
    let mut enc = VideoEncoder::new(plan, choice, d3d.as_ref())?;
    let mut packets = Vec::new();
    let mut offset = 0i64;
    for _ in 0..loops {
        let mut r = Reader::open(
            File::open(src).unwrap(),
            ReadMode::Decoded { d3d: d3d.clone() },
        )?;
        let mut end = 0i64;
        while let Some(f) = r.next_frame()? {
            end = end.max(f.pts + f.duration);
            let f = Frame {
                pts: f.pts + offset,
                ..f
            };
            // SAFETY: a plain call on a sample the reader gave.
            unsafe { f.sample.SetSampleTime(f.pts) }.unwrap();
            enc.push(f, &mut packets)?;
        }
        offset += end;
    }
    enc.finish(&mut packets)?;
    let spec = enc.stream();
    let mut m = muxer(c, File::create(out).unwrap())?;
    m.add_stream(spec.clone())?;
    for p in &packets {
        m.write(p.clone())?;
    }
    m.finish(&Tags::default())?;
    Ok(Encoded {
        spec,
        hardware: enc.hardware(),
        honours_bitrate: enc.honours_bitrate(),
        packets,
    })
}

fn tune_of(codec: VCodec, setting: VideoSetting) -> VideoTune {
    VideoTune {
        setting: Some(setting),
        codec,
        ..VideoTune::default()
    }
}

fn plan_for(src: &Path, tune: &VideoTune) -> EncodePlan {
    plan(&probe(src).unwrap(), tune)
}

/// True when this Windows decodes `path`'s picture; says so and returns
/// false when the Store extension it needs isn't installed.
fn decodes(path: &Path) -> bool {
    match Reader::open(File::open(path).unwrap(), ReadMode::Decoded { d3d: None }) {
        Err(MediaError::NeedsExtension { codec, .. }) => {
            eprintln!("skipped: Microsoft's {codec} Video Extensions aren't installed");
            false
        }
        _ => true,
    }
}

/// The frames Media Foundation decodes from `path`, in memory.
fn decoded_frames(path: &Path) -> usize {
    let mut r = Reader::open(File::open(path).unwrap(), ReadMode::Decoded { d3d: None }).unwrap();
    let mut n = 0;
    while r.next_frame().unwrap().is_some() {
        n += 1;
    }
    n
}

/// An 8-bit frame's planes in memory, rows packed: the luma (`w` x `h`) and
/// the interleaved chroma (`w` x `h / 2`).
struct Planes {
    w: usize,
    h: usize,
    y: Vec<u8>,
    uv: Vec<u8>,
}

impl Planes {
    fn y_at(&self, x: usize, y: usize) -> u8 {
        self.y[y * self.w + x]
    }

    /// U and V of the 2x2 block holding pixel (x, y).
    fn uv_at(&self, x: usize, y: usize) -> (u8, u8) {
        let at = (y / 2) * self.w + (x / 2) * 2;
        (self.uv[at], self.uv[at + 1])
    }

    fn y_mean(&self) -> f64 {
        self.y.iter().map(|&v| v as f64).sum::<f64>() / self.y.len() as f64
    }
}

/// The first decoded frame of `path`, decoded in memory to NV12, or to P010
/// for a deeper source, whose samples are rounded to 8 bits here.
fn first_frame(path: &Path) -> Planes {
    let mut r = Reader::open(File::open(path).unwrap(), ReadMode::Decoded { d3d: None }).unwrap();
    let f = r.next_frame().unwrap().expect("a frame");
    let fh = f.format.height as usize;
    let (w, h) = (
        f.format.shown.width as usize,
        f.format.shown.height as usize,
    );
    let deep = f.format.pixel == Pixel::P010;
    // SAFETY: the 2D buffer is locked while its rows are copied out, each
    // within the pitch it gives.
    unsafe {
        let b: IMF2DBuffer = f.sample.GetBufferByIndex(0).unwrap().cast().unwrap();
        let mut p = std::ptr::null_mut();
        let mut pitch = 0i32;
        b.Lock2D(&mut p, &mut pitch).unwrap();
        let row = |r: usize, n: usize| -> Vec<u8> {
            let bytes = if deep { 2 } else { 1 };
            let raw = std::slice::from_raw_parts(p.offset(r as isize * pitch as isize), n * bytes);
            if deep {
                // P010 keeps its 10 bits at the top of each 16.
                raw.chunks_exact(2)
                    .map(|c| (((u16::from_le_bytes([c[0], c[1]]) >> 6) + 2) >> 2).min(255) as u8)
                    .collect()
            } else {
                raw.to_vec()
            }
        };
        let mut y = Vec::with_capacity(w * h);
        for r in 0..h {
            y.extend(row(r, w));
        }
        let mut uv = Vec::with_capacity(w * h / 2);
        for r in 0..h / 2 {
            uv.extend(row(fh + r, w));
        }
        let _ = b.Unlock2D();
        Planes { w, h, y, uv }
    }
}

/// The mean U and V of the whole of `p`.
fn uv_means(p: &Planes) -> (f64, f64) {
    let n = (p.uv.len() / 2) as f64;
    let (u, v) =
        p.uv.chunks_exact(2)
            .fold((0.0, 0.0), |(u, v), c| (u + c[0] as f64, v + c[1] as f64));
    (u / n, v / n)
}

fn seconds(packets: &[Packet]) -> f64 {
    let first = packets.iter().map(|p| p.pts).min().unwrap_or(0);
    let end = packets
        .iter()
        .map(|p| p.pts + p.duration)
        .max()
        .unwrap_or(0);
    (end - first) as f64 / TIMESCALE as f64
}

/// The checks every playable H.264 or HEVC result passes: ffprobe's codec,
/// size, frame count and length, and Media Foundation decoding every frame.
fn assert_plays(out: &Path, codec: &str, size: (u32, u32), frames: usize, secs: f64) {
    let f = facts(out);
    assert_eq!(fact(&f, "codec_name"), codec, "{f:?}");
    assert_eq!(fact(&f, "width"), size.0.to_string(), "{f:?}");
    assert_eq!(fact(&f, "height"), size.1.to_string(), "{f:?}");
    assert_eq!(fact(&f, "nb_read_frames"), frames.to_string(), "{f:?}");
    let d: f64 = fact(&f, "duration").parse().unwrap();
    assert!((d - secs).abs() <= 0.04, "duration {d} s, wanted {secs}");
    // R38: the first frame shows when the source's did.
    assert_eq!(fact(&f, "start_time"), "0.000000", "{f:?}");
    assert_eq!(
        decoded_frames(out),
        frames,
        "Media Foundation decodes them all"
    );
}

// ---------------------------------------------------------------------------
// Tests

#[test]
fn h264_from_frames_plays_back() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.mp4");
    let src = video("h264.mp4");
    let p = plan_for(&src, &tune_of(VCodec::H264, VideoSetting::Quality(65)));
    assert_eq!((p.width, p.height, p.bit_depth), (320, 240, 8));
    let e = encode_to(&src, &p, EncoderChoice::Processor, &out, 1).unwrap();
    assert!(!e.hardware);
    assert_eq!(e.packets.len(), 60);
    assert!(e.packets[0].key);
    assert_plays(&out, "h264", (320, 240), 60, 2.0);
    let f = facts(&out);
    assert_eq!(fact(&f, "profile"), "High", "{f:?}");
    // R40: the colour is described even though the encoder writes no VUI;
    // R68: the fixture is untagged and 240 lines, so as BT.601.
    for key in ["color_primaries", "color_transfer", "color_space"] {
        assert_eq!(fact(&f, key), "smpte170m", "{key}: {f:?}");
    }
}

/// The 16x16 block of `p` (by its top-left corner) whose colour is
/// furthest from grey: where a change of matrix moves the samples most.
fn most_saturated_block(p: &Planes) -> (usize, usize) {
    let mut best = (0, 0, -1.0f64);
    for by in (0..p.h - 15).step_by(16) {
        for bx in (0..p.w - 15).step_by(16) {
            let mut s = 0.0;
            for y in (by..by + 16).step_by(2) {
                for x in (bx..bx + 16).step_by(2) {
                    let (u, v) = p.uv_at(x, y);
                    s += (u as f64 - 128.0).abs() + (v as f64 - 128.0).abs();
                }
            }
            if s > best.2 {
                best = (bx, by, s);
            }
        }
    }
    (best.0, best.1)
}

/// The mean Y, U and V of the 16x16 block at `(bx, by)`.
fn block_means(p: &Planes, (bx, by): (usize, usize)) -> [f64; 3] {
    let mut m = [0.0; 3];
    for y in by..by + 16 {
        for x in bx..bx + 16 {
            let (u, v) = p.uv_at(x, y);
            m[0] += p.y_at(x, y) as f64;
            m[1] += u as f64;
            m[2] += v as f64;
        }
    }
    m.map(|s| s / 256.0)
}

#[test]
fn an_untagged_source_under_720_lines_is_planned_as_bt601() {
    // R68: a source that doesn't describe its colour is written as players
    // read it: BT.601 under 720 lines, BT.709 from there. A described
    // source, or HDR, keeps the description it has now.
    let sd = probe(&video("h264.mp4")).unwrap();
    assert!(
        !sd.colour_tagged,
        "the fixture has no colr and no VUI colour"
    );
    assert_eq!(sd.height, 240);
    for codec in [VCodec::H264, VCodec::Hevc] {
        let tune = tune_of(codec, VideoSetting::Quality(65));
        assert!(plan(&sd, &tune).bt601, "{codec:?}");
        let hd = kiln_media::video::VideoInfo {
            height: 720,
            ..sd.clone()
        };
        assert!(!plan(&hd, &tune).bt601, "{codec:?} at 720 lines");
        let tagged = kiln_media::video::VideoInfo {
            colour_tagged: true,
            ..sd.clone()
        };
        assert!(!plan(&tagged, &tune).bt601, "{codec:?} tagged");
        // Scaled down, the source's lines still decide.
        let small = VideoTune {
            max_lines: Some(480),
            ..tune
        };
        assert!(!plan(&hd, &small).bt601, "{codec:?} from 720 lines to 480");
    }
    let hdr = probe(&video("hevc10_hdr.mp4")).unwrap();
    assert!(hdr.colour_tagged, "the SPS's VUI describes the colour");
    // HDR known only by its static metadata says nothing of its colour, yet
    // standard range made from it is tone-mapped to BT.709: the source's
    // range decides, not the range written.
    let untagged_hdr = kiln_media::video::VideoInfo {
        colour_tagged: false,
        height: 240,
        ..hdr.clone()
    };
    for info in [&hdr, &untagged_hdr] {
        for codec in [VCodec::H264, VCodec::Hevc] {
            for encoder in [EncoderChoice::Processor, EncoderChoice::GraphicsCard] {
                let tune = VideoTune {
                    encoder,
                    ..tune_of(codec, VideoSetting::Quality(65))
                };
                let p = plan(info, &tune);
                assert!(
                    !p.bt601,
                    "{codec:?} {encoder:?} from HDR (tagged {})",
                    info.colour_tagged
                );
            }
        }
    }
}

#[test]
fn an_untagged_sd_source_keeps_its_samples_and_is_tagged_bt601() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let src = video("h264.mp4");
    let source = first_frame(&src);
    // The colour furthest from grey in testsrc2's first frame: a change of
    // matrix (BT.601 to BT.709) moves a saturated colour's Y by over 10
    // levels and its U or V as much (pure red: Y 81 to 63, U 90 to 102).
    let block = most_saturated_block(&source);
    let want = block_means(&source, block);
    let chroma = (want[1] - 128.0).abs() + (want[2] - 128.0).abs();
    assert!(chroma > 60.0, "the block is saturated: {want:?}");
    let mut ran = 0;
    for codec in [VCodec::H264, VCodec::Hevc] {
        let p = plan_for(&src, &tune_of(codec, VideoSetting::Quality(90)));
        assert!(p.bt601, "{codec:?}");
        let mut choices = vec![EncoderChoice::Processor];
        if encoder_adapter(codec, EncoderChoice::GraphicsCard).is_some() {
            choices.push(EncoderChoice::GraphicsCard);
        } else {
            eprintln!("skipped {codec:?} on the graphics card: no encoder starts");
        }
        for choice in choices {
            for (c, ext) in [(Container::Mp4, "mp4"), (Container::Mkv, "mkv")] {
                let name = format!("{codec:?} {choice:?} {ext}");
                let out = dir.path().join(format!("sd_{codec:?}_{choice:?}.{ext}"));
                let e = match encode_into(&src, &p, choice, &out, 1, c) {
                    Ok(e) => e,
                    Err(e @ MediaError::NeedsExtensionToWrite { .. }) => {
                        eprintln!("skipped {name}: {e}");
                        continue;
                    }
                    Err(e) => panic!("{name}: {e}"),
                };
                ran += 1;
                assert!(e.spec.video.as_ref().unwrap().bt601, "{name}");
                let f = facts(&out);
                for key in ["color_primaries", "color_transfer", "color_space"] {
                    assert_eq!(fact(&f, key), "smpte170m", "{name} {key}: {f:?}");
                }
                // No matrix conversion: the saturated block keeps its Y, U
                // and V to within coding loss.
                let got = block_means(&first_frame(&out), block);
                for (i, plane) in ["Y", "U", "V"].iter().enumerate() {
                    assert!(
                        (got[i] - want[i]).abs() < 3.0,
                        "{name}: the saturated block's {plane} is {} against {}",
                        got[i],
                        want[i]
                    );
                }
            }
        }
    }
    assert!(ran > 0, "no encode ran");
}

#[test]
fn hevc_on_the_graphics_card_when_present() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    if encoder_adapter(VCodec::Hevc, EncoderChoice::GraphicsCard).is_none() {
        eprintln!("skipped: this PC has no graphics card encoder for HEVC");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.mp4");
    let src = video("hevc.mp4");
    if !decodes(&src) {
        return;
    }
    let p = plan_for(&src, &tune_of(VCodec::Hevc, VideoSetting::Quality(65)));
    let e = encode_to(&src, &p, EncoderChoice::GraphicsCard, &out, 1).unwrap();
    assert!(e.hardware);
    assert_plays(&out, "hevc", (320, 240), 60, 2.0);
}

#[test]
fn h264_on_the_graphics_card_when_present() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    if encoder_adapter(VCodec::H264, EncoderChoice::GraphicsCard).is_none() {
        eprintln!("skipped: this PC has no graphics card encoder for H.264");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.mp4");
    let src = video("h264.mp4");
    let p = plan_for(&src, &tune_of(VCodec::H264, VideoSetting::Quality(65)));
    let e = encode_to(&src, &p, EncoderChoice::Automatic, &out, 1).unwrap();
    assert!(e.hardware);
    assert!(e.honours_bitrate);
    assert_plays(&out, "h264", (320, 240), 60, 2.0);
}

#[test]
fn rotated_source_comes_out_upright() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    let src = video("rot90.mp4");
    let before = first_frame(&src);
    for choice in [EncoderChoice::Processor, EncoderChoice::Automatic] {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out.mp4");
        let p = plan_for(&src, &tune_of(VCodec::H264, VideoSetting::Quality(80)));
        assert_eq!((p.width, p.height, p.rotate), (240, 320, 90));
        encode_to(&src, &p, choice, &out, 1).unwrap();
        assert_plays(&out, "h264", (240, 320), 60, 2.0);
        let f = facts(&out);
        assert!(!f.contains_key("rotation"), "{choice:?}: {f:?}");
        // The picture is the source's turned 90 degrees clockwise: output
        // (x, y) is source (y, h - 1 - x).
        let after = first_frame(&out);
        assert_eq!((after.w, after.h), (240, 320));
        let (mut cw, mut ccw) = (0.0, 0.0);
        for y in 0..after.h {
            for x in 0..after.w {
                let got = after.y_at(x, y) as f64;
                cw += (got - before.y_at(y, before.h - 1 - x) as f64).abs();
                ccw += (got - before.y_at(before.w - 1 - y, x) as f64).abs();
            }
        }
        let n = (after.w * after.h) as f64;
        let (cw, ccw) = (cw / n, ccw / n);
        assert!(
            cw < 6.0 && ccw > 3.0 * cw,
            "{choice:?}: mean difference from clockwise {cw:.1}, counterclockwise {ccw:.1}"
        );
    }
}

#[test]
fn hdr_to_h264_is_bt709_8bit() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    let src = video("hevc10_hdr.mp4");
    if !decodes(&src) {
        return;
    }
    let p = plan_for(&src, &tune_of(VCodec::H264, VideoSetting::Quality(80)));
    assert_eq!((p.hdr_out, p.bit_depth), (Hdr::None, 8));
    for choice in [EncoderChoice::Processor, EncoderChoice::Automatic] {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out.mp4");
        encode_to(&src, &p, choice, &out, 1).unwrap();
        let f = facts(&out);
        assert_eq!(fact(&f, "codec_name"), "h264", "{f:?}");
        assert_eq!(fact(&f, "pix_fmt"), "yuv420p", "{f:?}");
        assert_eq!(fact(&f, "color_transfer"), "bt709", "{choice:?}: {f:?}");
        assert_eq!(fact(&f, "color_primaries"), "bt709", "{choice:?}: {f:?}");
        // Tone-mapped, not passed through: 121 untouched, 145 on AMD's
        // curve, 204 on NVIDIA's. R88: 165 holds only for this whole-frame
        // mean of testsrc2's bars, far outside BT.709, which the processor
        // in software maps through RGB (Task 10c) and so clips channel by
        // channel (175). Neutral pictures keep the card's brightness to 3
        // levels (`neutral_blocks_keep_their_brightness_on_the_processor`),
        // and colours inside BT.709 their source's, block by block
        // (`an_in_gamut_hdr_clip_on_the_processor_matches_its_source`).
        let mean = first_frame(&out).y_mean();
        let least = if choice == EncoderChoice::Processor {
            165.0
        } else {
            185.0
        };
        assert!(mean > least, "{choice:?}: luma mean {mean:.1}");
        eprintln!("{choice:?}: {f:?}, luma mean {mean:.1}");
    }
}

#[test]
fn hdr_to_hevc_stays_pq_10bit() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    if encoder_adapter(VCodec::Hevc, EncoderChoice::GraphicsCard).is_none() {
        eprintln!("skipped: this PC has no graphics card encoder for HEVC");
        return;
    }
    let src = video("hevc10_hdr.mp4");
    if !decodes(&src) {
        return;
    }
    let p = plan_for(&src, &tune_of(VCodec::Hevc, VideoSetting::Quality(80)));
    assert_eq!((p.hdr_out, p.bit_depth), (Hdr::Pq, 10));
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.mp4");
    let e = encode_to(&src, &p, EncoderChoice::Automatic, &out, 1).unwrap();
    assert!(e.hardware);
    let v = e.spec.video.clone().unwrap();
    assert_eq!((v.hdr, v.bit_depth), (Hdr::Pq, 10));
    let f = facts(&out);
    assert_eq!(fact(&f, "codec_name"), "hevc", "{f:?}");
    assert_eq!(fact(&f, "profile"), "Main 10", "{f:?}");
    assert_eq!(fact(&f, "pix_fmt"), "yuv420p10le", "{f:?}");
    assert_eq!(fact(&f, "color_transfer"), "smpte2084", "{f:?}");
    assert_eq!(fact(&f, "color_primaries"), "bt2020", "{f:?}");
    assert_eq!(e.packets.len(), 60);
    assert_eq!(decoded_frames(&out), 60);
    // R40: the source's HDR10 metadata goes with it.
    let info = probe(&src).unwrap();
    assert!(info.mastering.is_some() && info.content_light.is_some());
    assert_eq!(
        (v.mastering, v.content_light),
        (info.mastering, info.content_light)
    );
    let bytes = std::fs::read(&out).unwrap();
    for b in [b"mdcv", b"clli"] {
        assert!(
            bytes.windows(4).any(|w| w == b),
            "{}",
            String::from_utf8_lossy(b)
        );
    }
}

/// Windows' HEVC software encoder writes 8-bit Main only: on the processor,
/// HDR to HEVC is tone-mapped to 8-bit standard range, as for H.264, and the
/// stream says so.
#[test]
fn hdr_to_hevc_on_the_processor_is_8bit_standard_range() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    let src = video("hevc10_hdr.mp4");
    if !decodes(&src) {
        return;
    }
    let p = plan_for(&src, &tune_of(VCodec::Hevc, VideoSetting::Quality(80)));
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.mp4");
    let e = encode_to(&src, &p, EncoderChoice::Processor, &out, 1).unwrap();
    assert!(!e.hardware);
    let v = e.spec.video.clone().unwrap();
    assert_eq!((v.hdr, v.bit_depth), (Hdr::None, 8));
    let f = facts(&out);
    assert_eq!(fact(&f, "codec_name"), "hevc", "{f:?}");
    assert_eq!(fact(&f, "profile"), "Main", "{f:?}");
    assert_eq!(fact(&f, "pix_fmt"), "yuv420p", "{f:?}");
    assert_ne!(fact(&f, "color_transfer"), "smpte2084", "{f:?}");
    assert_eq!(decoded_frames(&out), 60);
    // Tone-mapped through RGB in software, as for H.264: 175, with 121
    // untouched. 165 is the bars' whole-frame mean only (R88; see
    // `hdr_to_h264_is_bt709_8bit`).
    let mean = first_frame(&out).y_mean();
    assert!(mean > 165.0, "luma mean {mean:.1}");
}

/// Pearson's correlation of `a` and `b`: NaN when either is flat.
fn correlation(a: &[f64], b: &[f64]) -> f64 {
    let n = a.len() as f64;
    let (ma, mb) = (a.iter().sum::<f64>() / n, b.iter().sum::<f64>() / n);
    let cov: f64 = a.iter().zip(b).map(|(x, y)| (x - ma) * (y - mb)).sum();
    let va: f64 = a.iter().map(|x| (x - ma).powi(2)).sum();
    let vb: f64 = b.iter().map(|y| (y - mb).powi(2)).sum();
    cov / (va * vb).sqrt()
}

/// Each 16x16 block's corner and mean Y, U and V, row by row.
type Blocks = Vec<((usize, usize), [f64; 3])>;

/// The mean Y, U and V of each 16x16 block of `p` with its corner, row by
/// row.
fn blocks(p: &Planes) -> Vec<((usize, usize), [f64; 3])> {
    let mut out = Vec::new();
    for by in (0..p.h - 15).step_by(16) {
        for bx in (0..p.w - 15).step_by(16) {
            out.push(((bx, by), block_means(p, (bx, by))));
        }
    }
    out
}

/// How far a colour is from grey: |U - 128| + |V - 128|.
fn chroma(m: &[f64; 3]) -> f64 {
    (m[1] - 128.0).abs() + (m[2] - 128.0).abs()
}

/// Task 10c: Microsoft's video processor in software (`Processor`, or no
/// graphics card) writes only the luma of a YUV picture it makes from a
/// 10-bit or HDR one, and leaves the chroma plane as it was given: U and V
/// near 0, a green picture. HDR tone-mapped to standard range on the
/// processor keeps its colours: across testsrc2's blocks, the output's U and
/// V follow the source's, and every strongly coloured block stays coloured.
/// (The bars are far outside BT.709, so a few change side of grey when
/// mapped into it, on the graphics card too; the correlation is 0.94 for U
/// and 0.77 for V on the processor, 0.94 and 0.84 on the card, and the
/// least colour a block keeps is 35 on the processor, 43 on the card.)
#[test]
fn hdr_to_sdr_on_the_processor_keeps_its_colour() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    let src = video("hevc10_hdr.mp4");
    if !decodes(&src) {
        return;
    }
    let source = blocks(&first_frame(&src));
    let dir = tempfile::tempdir().unwrap();
    for codec in [VCodec::H264, VCodec::Hevc] {
        let tune = VideoTune {
            encoder: EncoderChoice::Processor,
            ..tune_of(codec, VideoSetting::Quality(90))
        };
        let p = plan_for(&src, &tune);
        assert_eq!((p.hdr_out, p.bit_depth), (Hdr::None, 8), "{codec:?}");
        let out = dir.path().join(format!("{codec:?}.mp4"));
        let e = encode_to(&src, &p, EncoderChoice::Processor, &out, 1).unwrap();
        assert!(!e.hardware);
        let got = blocks(&first_frame(&out));
        for i in [1, 2] {
            let plane = |b: &[((usize, usize), [f64; 3])]| -> Vec<f64> {
                b.iter().map(|(_, m)| m[i]).collect()
            };
            let r = correlation(&plane(&source), &plane(&got));
            assert!(
                r > 0.6,
                "{codec:?}: the blocks' {} follow the source's by {r:.3}",
                ["Y", "U", "V"][i]
            );
        }
        for ((at, s), (_, g)) in source.iter().zip(&got) {
            if chroma(s) > 60.0 {
                assert!(
                    chroma(g) > 20.0,
                    "{codec:?}: the block at {at:?} is {g:?}, from {s:?}"
                );
            }
        }
    }
}

/// Makes `out` with FFmpeg from a lavfi source `input` (10 frames at
/// 320x240) through `filter`, as 10-bit HEVC (lossless, with `x265`'s
/// colour) or, when `x265` is `None`, 8-bit H.264 tagged by `x264`.
/// False when FFmpeg lacks a part.
fn make_clip(out: &Path, input: &str, filter: &str, x265: Option<&str>, x264: &str) -> bool {
    let mut c = Command::new("ffmpeg");
    c.args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg(format!("{input}=size=320x240:rate=30"))
        .args(["-frames:v", "10", "-vf", filter]);
    match x265 {
        Some(params) => c
            .args(["-c:v", "libx265", "-tag:v", "hvc1", "-x265-params"])
            .arg(format!("log-level=error:lossless=1:{params}")),
        None => c
            .args(["-c:v", "libx264", "-crf", "1", "-x264-params"])
            .arg(x264),
    };
    c.arg(out).status().is_ok_and(|s| s.success())
}

/// The first frame FFmpeg makes from a lavfi source `input` (320x240)
/// through `filter`, as 8-bit NV12.
fn ffmpeg_frame(input: &str, filter: &str) -> Planes {
    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-f", "lavfi", "-i"])
        .arg(format!("{input}=size=320x240:rate=30"))
        .args(["-frames:v", "1", "-vf"])
        .arg(format!("{filter},format=nv12"))
        .args(["-f", "rawvideo", "-"])
        .output()
        .unwrap();
    let (w, h) = (320, 240);
    assert_eq!(out.stdout.len(), w * h * 3 / 2, "ffmpeg: {:?}", out.status);
    Planes {
        w,
        h,
        y: out.stdout[..w * h].to_vec(),
        uv: out.stdout[w * h..].to_vec(),
    }
}

/// The matrix of an HDR10 frame, which Microsoft's HEVC decoder leaves
/// unstated (Kiln fills it in the same way).
const PQ_MATRIX: [(GUID, u32); 1] = [(MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT2020_10.0 as u32)];

/// HDR10 tagging for x265.
const PQ_X265: &str = "colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:range=limited";

/// testsrc2 at half its RGB levels: the in-gamut HDR clip below is made
/// from it, and is compared with it.
const HALF: &str = "format=gbrp,colorlevels=romax=0.5:gomax=0.5:bomax=0.5";

/// An HDR10 clip made with FFmpeg's zscale from testsrc2 at half its RGB
/// levels, with BT.2408's reference white (203 nits) as its white: its
/// colours lie inside BT.709 and below the tone map's knee, so tone-mapped
/// to standard range it is near the 8-bit picture it was made from.
/// `None` without libx265 or zscale.
fn in_gamut_pq_clip(dir: &Path) -> Option<PathBuf> {
    let out = dir.join("pq.mp4");
    let filter = format!(
        "{HALF},zscale=tin=bt709:pin=bt709:rin=pc:t=smpte2084:m=bt2020nc:p=bt2020:r=tv:npl=203,format=yuv420p10le"
    );
    make_clip(&out, "testsrc2", &filter, Some(PQ_X265), "").then_some(out)
}

/// R88: an in-gamut HDR clip tone-mapped on the processor is the 8-bit
/// picture it was made from, block by block: Y within 16 levels and U and V
/// within 10, and on average within 2. The tone map isn't the identity
/// below its knee (the graphics card's blocks are as far: Y 15, U 11, V
/// 16); measured on the processor, Y 15, U 9 and V 8 at most, and means
/// within 1.
#[test]
fn an_in_gamut_hdr_clip_on_the_processor_matches_its_source() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let Some(src) = in_gamut_pq_clip(dir.path()) else {
        eprintln!("skipped: FFmpeg has no libx265 or zscale");
        return;
    };
    if !decodes(&src) {
        return;
    }
    assert_eq!(probe(&src).unwrap().hdr, Hdr::Pq);
    let want = blocks(&ffmpeg_frame(
        "testsrc2",
        &format!("{HALF},zscale=tin=bt709:pin=bt709:rin=pc:t=bt709:m=bt709:p=bt709:r=tv"),
    ));
    let tune = VideoTune {
        encoder: EncoderChoice::Processor,
        ..tune_of(VCodec::H264, VideoSetting::Quality(90))
    };
    let out = dir.path().join("out.mp4");
    encode_to(
        &src,
        &plan_for(&src, &tune),
        EncoderChoice::Processor,
        &out,
        1,
    )
    .unwrap();
    let got = blocks(&first_frame(&out));
    let mut sums = [0.0; 3];
    for ((at, w), (_, g)) in want.iter().zip(&got) {
        for (i, limit) in [16.0, 10.0, 10.0].into_iter().enumerate() {
            assert!(
                (g[i] - w[i]).abs() <= limit,
                "the block at {at:?} is {g:?}, its source {w:?}"
            );
            sums[i] += g[i] - w[i];
        }
    }
    for (i, sum) in sums.into_iter().enumerate() {
        let mean = sum / want.len() as f64;
        assert!(
            mean.abs() <= 2.0,
            "{} is {mean:+.1} on average",
            ["Y", "U", "V"][i]
        );
    }
}

/// Task 10c: HDR tone-mapped to standard range looks the same on the
/// processor as on the graphics card: the mean Y, U and V within 3 levels,
/// and each block's within 16 (measured: Y 4.5, U 7.1 and V 13.5 at most,
/// on the strongest colours' edges).
#[test]
fn hdr_to_sdr_colour_on_the_processor_is_the_cards() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    if encoder_adapter(VCodec::H264, EncoderChoice::GraphicsCard).is_none() {
        eprintln!("skipped: this PC has no graphics card encoder for H.264");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let Some(src) = in_gamut_pq_clip(dir.path()) else {
        eprintln!("skipped: FFmpeg has no libx265 or zscale");
        return;
    };
    if !decodes(&src) {
        return;
    }
    let mut means = Vec::new();
    let mut pictures = Vec::new();
    for choice in [EncoderChoice::Processor, EncoderChoice::GraphicsCard] {
        let tune = VideoTune {
            encoder: choice,
            ..tune_of(VCodec::H264, VideoSetting::Quality(90))
        };
        let p = plan_for(&src, &tune);
        let out = dir.path().join(format!("{choice:?}.mp4"));
        let e = encode_to(&src, &p, choice, &out, 1).unwrap();
        assert_eq!(e.hardware, choice == EncoderChoice::GraphicsCard);
        let pic = first_frame(&out);
        let (u, v) = uv_means(&pic);
        means.push([pic.y_mean(), u, v]);
        pictures.push(blocks(&pic));
    }
    let (processor, card) = (means[0], means[1]);
    for i in 0..3 {
        assert!(
            (processor[i] - card[i]).abs() <= 3.0,
            "mean Y, U and V: processor {processor:?}, graphics card {card:?}"
        );
    }
    for ((at, p), (_, c)) in pictures[0].iter().zip(&pictures[1]) {
        assert!(
            p.iter().zip(c).all(|(a, b)| (a - b).abs() <= 16.0),
            "the block at {at:?}: processor {p:?}, graphics card {c:?}"
        );
    }
}

/// What Microsoft's video processor makes in software from `f` (decoded in
/// memory, its picture the whole frame, with the attributes `colour` set on
/// its type): NV12 BT.709 in limited range, the same size, written into a
/// buffer that held `fill` before. From PQ BT.2020 Kiln doesn't take this
/// YUV output (Task 10c): its luma is whole, its chroma isn't written.
fn software_processor_nv12(f: &Frame, fill: u8, colour: &[(GUID, u32)]) -> Planes {
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    let (w, h) = (f.format.width, f.format.height);
    // SAFETY: plain Media Foundation calls with types and samples made here;
    // the output buffer is locked while it is filled and while it is read.
    unsafe {
        let t: IMFTransform =
            CoCreateInstance(&CLSID_VideoProcessorMFT, None, CLSCTX_INPROC_SERVER).unwrap();
        t.GetAttributes()
            .unwrap()
            .SetUINT32(&MF_XVP_DISABLE_FRC, 1)
            .unwrap();
        let input = MFCreateMediaType().unwrap();
        f.format.media_type.CopyAllItems(&input).unwrap();
        for (key, v) in colour {
            input.SetUINT32(key, *v).unwrap();
        }
        let output = MFCreateMediaType().unwrap();
        output
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .unwrap();
        output.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12).unwrap();
        for (key, v) in [
            (&MF_MT_FRAME_SIZE, ((w as u64) << 32) | h as u64),
            (&MF_MT_FRAME_RATE, (30 << 32) | 1),
            (&MF_MT_PIXEL_ASPECT_RATIO, (1 << 32) | 1),
        ] {
            output.SetUINT64(key, v).unwrap();
        }
        for (key, v) in [
            (&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32),
            (&MF_MT_DEFAULT_STRIDE, w),
            (&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32),
            (&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32),
            (&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32),
            (&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32),
        ] {
            output.SetUINT32(key, v).unwrap();
        }
        t.SetInputType(0, &input, 0).unwrap();
        t.SetOutputType(0, &output, 0).unwrap();
        let size = t.GetOutputStreamInfo(0).unwrap().cbSize;
        t.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
            .unwrap();
        t.ProcessInput(0, &f.sample, 0).unwrap();
        let buffer = MFCreateMemoryBuffer(size).unwrap();
        let mut p = std::ptr::null_mut();
        buffer.Lock(&mut p, None, None).unwrap();
        std::ptr::write_bytes(p, fill, size as usize);
        buffer.Unlock().unwrap();
        let sample = MFCreateSample().unwrap();
        sample.AddBuffer(&buffer).unwrap();
        let mut db = [MFT_OUTPUT_DATA_BUFFER {
            dwStreamID: 0,
            pSample: std::mem::ManuallyDrop::new(Some(sample)),
            dwStatus: 0,
            pEvents: std::mem::ManuallyDrop::new(None),
        }];
        let mut status = 0;
        t.ProcessOutput(0, &mut db, &mut status).unwrap();
        drop(std::mem::ManuallyDrop::take(&mut db[0].pSample));
        drop(std::mem::ManuallyDrop::take(&mut db[0].pEvents));
        let mut length = 0;
        buffer.Lock(&mut p, None, Some(&mut length)).unwrap();
        let bytes = std::slice::from_raw_parts(p, length as usize).to_vec();
        buffer.Unlock().unwrap();
        let _ = t.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
        let (w, h) = (w as usize, h as usize);
        Planes {
            w,
            h,
            y: bytes[..w * h].to_vec(),
            uv: bytes[w * h..w * h * 3 / 2].to_vec(),
        }
    }
}

/// Task 10c's root cause, kept to run by hand: Microsoft's video processor
/// in software, from a 10-bit PQ frame to NV12 BT.709, writes the luma and
/// leaves every chroma byte as the buffer held it (here 77). Kiln goes
/// through RGB instead (`Detour`). Should this ever fail, Windows has
/// changed and the detour may no longer be needed.
#[test]
#[ignore = "a probe of Windows' video processor, not of Kiln"]
fn the_software_processor_leaves_a_ten_bit_pictures_chroma_alone() {
    if common::no_mf() {
        return;
    }
    let src = video("hevc10_hdr.mp4");
    if !decodes(&src) {
        return;
    }
    let mut r = Reader::open(File::open(&src).unwrap(), ReadMode::Decoded { d3d: None }).unwrap();
    let f = r.next_frame().unwrap().expect("a frame");
    assert_eq!(f.format.pixel, Pixel::P010);
    let out = software_processor_nv12(&f, 77, &PQ_MATRIX);
    assert!(out.uv.iter().all(|&b| b == 77), "the chroma was written");
    let changed = out.y.iter().filter(|&&b| b != 77).count();
    assert!(
        changed > out.y.len() * 9 / 10,
        "the luma: {changed} written"
    );
}

/// An 8-bit clip of testsrc2 at half its RGB levels (inside every gamut
/// here), made with zscale in the matrix and primaries `zscale` and tagged
/// with `x264`'s; and the first frame of the same picture made BT.709 by
/// FFmpeg, block by block. `None` without libx264 or zscale.
fn half_clip(dir: &Path, name: &str, zscale: &str, x264: &str) -> Option<(PathBuf, Blocks)> {
    let src = dir.join(format!("{name}.mp4"));
    let filter =
        format!("{HALF},zscale=tin=bt709:pin=bt709:rin=pc:t=bt709:{zscale}:r=tv,format=yuv420p");
    if !make_clip(&src, "testsrc2", &filter, None, x264) {
        return None;
    }
    let bt709 = blocks(&ffmpeg_frame(
        "testsrc2",
        &format!("{HALF},zscale=tin=bt709:pin=bt709:rin=pc:t=bt709:m=bt709:p=bt709:r=tv"),
    ));
    Some((src, bt709))
}

/// The mean difference in Y, U and V between blocks of two pictures.
fn mean_off(a: &[((usize, usize), [f64; 3])], b: &[((usize, usize), [f64; 3])]) -> [f64; 3] {
    let mut m = [0.0f64; 3];
    for ((_, x), (_, y)) in a.iter().zip(b) {
        for i in 0..3 {
            m[i] += (x[i] - y[i]).abs() / a.len() as f64;
        }
    }
    m
}

/// The largest difference in Y, U and V between blocks of two pictures.
fn worst(a: &[((usize, usize), [f64; 3])], b: &[((usize, usize), [f64; 3])]) -> [f64; 3] {
    let mut m = [0.0f64; 3];
    for ((_, x), (_, y)) in a.iter().zip(b) {
        for i in 0..3 {
            m[i] = m[i].max((x[i] - y[i]).abs());
        }
    }
    m
}

/// SMPTE 170M's tags for x264 and its matrix and primaries for zscale.
const BT601: (&str, &str) = (
    "m=170m:p=170m",
    "colorprim=smpte170m:transfer=smpte170m:colormatrix=smpte170m",
);

/// Task 10c, round 2: Microsoft's video processor in software, from an
/// 8-bit picture tagged BT.601 (SMPTE 170M) to NV12 BT.709, copies the
/// samples as they are: the matrix and primaries aren't converted, so the
/// picture is then read as BT.709 in the wrong colours. Kiln goes through
/// RGB instead (`needs_detour`). Should this ever fail, Windows has changed.
#[test]
#[ignore = "a probe of Windows' video processor, not of Kiln"]
fn the_software_processor_copies_bt601_samples_unconverted() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let Some((src, bt709)) = half_clip(dir.path(), "bt601", BT601.0, BT601.1) else {
        eprintln!("skipped: FFmpeg has no libx264 or zscale");
        return;
    };
    let mut r = Reader::open(File::open(&src).unwrap(), ReadMode::Decoded { d3d: None }).unwrap();
    let f = r.next_frame().unwrap().expect("a frame");
    // SAFETY: plain attribute reads.
    let stated = unsafe { f.format.media_type.GetUINT32(&MF_MT_YUV_MATRIX) }.ok();
    assert_eq!(stated, Some(MFVideoTransferMatrix_BT601.0 as u32));
    let out = blocks(&software_processor_nv12(&f, 0, &[]));
    drop(f);
    drop(r);
    let source = blocks(&first_frame(&src));
    assert_eq!(worst(&out, &source), [0.0; 3], "the samples were changed");
    let off = worst(&out, &bt709);
    assert!(off.iter().any(|&d| d > 8.0), "BT.709 is only {off:?} away");
}

/// Task 10c, round 2: an 8-bit source tagged BT.601 (SMPTE 170M or
/// BT.470 BG, as SD and PAL video is) is written as BT.709 on the
/// processor, its samples converted. Each block is within 4 levels of the
/// graphics card's (measured 3.4). Against FFmpeg's own BT.709 picture its
/// luma is within 8 levels a block and 2.5 on average (measured 6.0 and
/// 1.9; copied unconverted, as the processor did straight to NV12, 15 and
/// 7.4). Its U and V are within 16 (measured 15.2 in PAL's V, where the
/// card is as far: Windows and zscale take BT.470 BG's primaries apart).
#[test]
fn a_tagged_bt601_source_on_the_processor_is_converted_to_bt709() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let card = encoder_adapter(VCodec::H264, EncoderChoice::GraphicsCard).is_some();
    for (name, zscale, x264) in [
        ("170m", BT601.0, BT601.1),
        (
            "470bg",
            "m=470bg:p=bt470bg",
            "colorprim=bt470bg:transfer=smpte170m:colormatrix=bt470bg",
        ),
    ] {
        let Some((src, bt709)) = half_clip(dir.path(), name, zscale, x264) else {
            eprintln!("skipped: FFmpeg has no libx264 or zscale");
            return;
        };
        let info = probe(&src).unwrap();
        assert!(info.colour_tagged && info.height < 720, "{name}");
        let mut pictures = Vec::new();
        for choice in [EncoderChoice::Processor, EncoderChoice::GraphicsCard] {
            if choice == EncoderChoice::GraphicsCard && !card {
                eprintln!("skipped the card: no H.264 encoder starts on it");
                continue;
            }
            let tune = VideoTune {
                encoder: choice,
                ..tune_of(VCodec::H264, VideoSetting::Quality(90))
            };
            let p = plan_for(&src, &tune);
            assert!(!p.bt601, "a tagged source is written as BT.709");
            let out = dir.path().join(format!("{name}_{choice:?}.mp4"));
            encode_to(&src, &p, choice, &out, 1).unwrap();
            pictures.push(blocks(&first_frame(&out)));
        }
        let (off, mean) = (worst(&pictures[0], &bt709), mean_off(&pictures[0], &bt709));
        assert!(
            off[0] <= 8.0 && mean[0] <= 2.5 && off[1] <= 16.0 && off[2] <= 16.0,
            "{name}: the processor's blocks are up to {off:?} from BT.709's, {mean:?} on average"
        );
        if let [processor, card] = &pictures[..] {
            let off = worst(processor, card);
            assert!(
                off.iter().all(|&d| d <= 4.0),
                "{name}: the processor's blocks are up to {off:?} from the card's"
            );
        }
    }
}

/// R88: grey stays grey and as bright as Microsoft's own tone map makes it.
/// SMPTE's HD bars (grey, white and black patches among the colours) are
/// tagged PQ as the HDR fixture is. On every flat neutral block the
/// processor's Y is within 3 levels of what Microsoft's processor writes
/// itself (its luma, which is whole; the graphics card's is the same to a
/// level), and its U and V within 2 of grey.
#[test]
fn neutral_blocks_keep_their_brightness_on_the_processor() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("bars.mp4");
    if !make_clip(&src, "smptehdbars", "format=yuv420p10le", Some(PQ_X265), "") {
        eprintln!("skipped: FFmpeg has no libx265");
        return;
    }
    if !decodes(&src) {
        return;
    }
    let mut r = Reader::open(File::open(&src).unwrap(), ReadMode::Decoded { d3d: None }).unwrap();
    let f = r.next_frame().unwrap().expect("a frame");
    let reference = software_processor_nv12(&f, 0, &PQ_MATRIX);
    drop(f);
    drop(r);
    let source = first_frame(&src);
    let tune = VideoTune {
        encoder: EncoderChoice::Processor,
        ..tune_of(VCodec::H264, VideoSetting::Quality(90))
    };
    let out = dir.path().join("out.mp4");
    encode_to(
        &src,
        &plan_for(&src, &tune),
        EncoderChoice::Processor,
        &out,
        1,
    )
    .unwrap();
    let got = first_frame(&out);
    let (mut neutral, mut bright) = (0, 0);
    for (at, s) in blocks(&source) {
        let flat = (at.1..at.1 + 16)
            .all(|y| (at.0..at.0 + 16).all(|x| (source.y_at(x, y) as f64 - s[0]).abs() <= 2.0));
        if !flat || chroma(&s) >= 10.0 {
            continue;
        }
        neutral += 1;
        let (g, want) = (block_means(&got, at), block_means(&reference, at));
        bright += usize::from(want[0] >= 185.0);
        assert!(
            (g[0] - want[0]).abs() <= 3.0
                && (g[1] - 128.0).abs() <= 2.0
                && (g[2] - 128.0).abs() <= 2.0,
            "the neutral block at {at:?} is {g:?}; Microsoft's own luma {:.1}",
            want[0]
        );
    }
    assert!(
        neutral >= 40 && bright >= 8,
        "{neutral} neutral blocks, {bright} bright"
    );
}

/// Task 10c: an 8-bit BT.2020 standard-range picture goes through RGB on
/// the processor too; straight to NV12 BT.709 its chroma isn't written. The
/// clip is testsrc2 at half its levels (inside BT.709) made BT.2020 with
/// zscale. Block by block: Y within 2 levels of Microsoft's own luma of it
/// (measured 1.0); Y, U and V within 20, 20 and 24 of FFmpeg's BT.709
/// picture of it (measured 18, 18.4 and 20.1; Windows' BT.2020 conversion
/// isn't zscale's, and the graphics card's is as far, at 18, 16.3 and
/// 22.3; unwritten chroma is 100 away). With a graphics card, each block is
/// within 12 of the card's (measured 10.0) and the means within 3.
#[test]
fn an_8bit_bt2020_source_on_the_processor_keeps_its_colour() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("wide.mp4");
    let filter = format!(
        "{HALF},zscale=tin=bt709:pin=bt709:rin=pc:t=bt709:m=bt2020nc:p=bt2020:r=tv,format=yuv420p"
    );
    if !make_clip(
        &src,
        "testsrc2",
        &filter,
        None,
        "colorprim=bt2020:transfer=bt709:colormatrix=bt2020nc",
    ) {
        eprintln!("skipped: FFmpeg has no libx264 or zscale");
        return;
    }
    let info = probe(&src).unwrap();
    assert_eq!((info.bit_depth, info.hdr), (8, Hdr::None));
    let bt709 = blocks(&ffmpeg_frame(
        "testsrc2",
        &format!("{HALF},zscale=tin=bt709:pin=bt709:rin=pc:t=bt709:m=bt709:p=bt709:r=tv"),
    ));
    let own = {
        let mut r =
            Reader::open(File::open(&src).unwrap(), ReadMode::Decoded { d3d: None }).unwrap();
        let f = r.next_frame().unwrap().expect("a frame");
        blocks(&software_processor_nv12(&f, 0, &[]))
    };
    let mut pictures = Vec::new();
    for choice in [EncoderChoice::Processor, EncoderChoice::GraphicsCard] {
        if choice == EncoderChoice::GraphicsCard && encoder_adapter(VCodec::H264, choice).is_none()
        {
            eprintln!("skipped the card: no H.264 encoder starts on it");
            continue;
        }
        let tune = VideoTune {
            encoder: choice,
            ..tune_of(VCodec::H264, VideoSetting::Quality(90))
        };
        let out = dir.path().join(format!("{choice:?}.mp4"));
        encode_to(&src, &plan_for(&src, &tune), choice, &out, 1).unwrap();
        pictures.push(first_frame(&out));
    }
    let got = blocks(&pictures[0]);
    let luma = worst(&got, &own)[0];
    assert!(luma <= 2.0, "Y is up to {luma} from Microsoft's own");
    let off = worst(&got, &bt709);
    assert!(
        off[0] <= 20.0 && off[1] <= 20.0 && off[2] <= 24.0,
        "the blocks are up to {off:?} from FFmpeg's BT.709 picture"
    );
    if let [processor, card] = &pictures[..] {
        let off = worst(&got, &blocks(card));
        assert!(
            off.iter().all(|&d| d <= 12.0),
            "the blocks are up to {off:?} from the card's"
        );
        let mean = |p: &Planes| {
            let (u, v) = uv_means(p);
            [p.y_mean(), u, v]
        };
        let (p, c) = (mean(processor), mean(card));
        assert!(
            p.iter().zip(&c).all(|(a, b)| (a - b).abs() <= 3.0),
            "mean Y, U and V: processor {p:?}, graphics card {c:?}"
        );
    }
}

/// Task 10c: a 10-bit ramp keeps its steps through the processor's 8-bit
/// RGB detour as well as FFmpeg's own 10-to-8-bit conversion of it does:
/// as many distinct Y codes but 3, and no step wider than 2 codes.
/// (Measured: 219 codes and steps of 1, against FFmpeg's 220 and 1.)
#[test]
fn a_ten_bit_ramp_keeps_its_steps_on_the_processor() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("ramp.mp4");
    let made = Command::new("ffmpeg")
        .args(["-v", "error", "-f", "lavfi", "-i"])
        .arg(
            "nullsrc=s=1024x64:r=30:d=0.4,format=yuv420p10le,geq=lum='64+X*876/1023':cb=512:cr=512",
        )
        .args(["-c:v", "libx265", "-tag:v", "hvc1"])
        .args(["-x265-params", "log-level=error:lossless=1"])
        .arg(&src)
        .status()
        .is_ok_and(|s| s.success());
    if !made {
        eprintln!("skipped: FFmpeg has no libx265");
        return;
    }
    if !decodes(&src) {
        return;
    }
    let row = |path: &Path| -> Vec<u8> {
        let out = Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(path)
            .args(["-frames:v", "1"])
            .args(["-pix_fmt", "yuv420p", "-f", "rawvideo", "-"])
            .output()
            .unwrap();
        assert_eq!(out.stdout.len(), 1024 * 64 * 3 / 2);
        out.stdout[32 * 1024..33 * 1024].to_vec()
    };
    let steps = |r: &[u8]| -> (usize, i32) {
        let mut codes = r.to_vec();
        codes.sort_unstable();
        codes.dedup();
        let widest = r
            .windows(2)
            .map(|w| (w[1] as i32 - w[0] as i32).abs())
            .max()
            .unwrap();
        (codes.len(), widest)
    };
    let (ffmpeg_codes, _) = steps(&row(&src));
    let tune = VideoTune {
        encoder: EncoderChoice::Processor,
        ..tune_of(VCodec::H264, VideoSetting::Quality(90))
    };
    let p = plan_for(&src, &tune);
    assert_eq!(p.bit_depth, 8);
    let out = dir.path().join("out.mp4");
    encode_to(&src, &p, EncoderChoice::Processor, &out, 1).unwrap();
    let (codes, widest) = steps(&row(&out));
    assert!(
        codes + 3 >= ffmpeg_codes && widest <= 2,
        "{codes} codes, steps up to {widest}; FFmpeg's {ffmpeg_codes}"
    );
}

/// ffprobe's colour of `file`'s picture stream copied out bare, as
/// `[range, matrix, transfer, primaries]`: what its SPS's VUI says, with no
/// container to go by. Every frame of the bare stream must decode.
fn stream_colour(file: &Path, codec: VCodec, frames: usize) -> [String; 4] {
    let dir = tempfile::tempdir().unwrap();
    let format = match codec {
        VCodec::H264 => "h264",
        VCodec::Hevc => "hevc",
    };
    let raw = dir.path().join(format!("raw.{format}"));
    let copied = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-i"])
        .arg(file)
        .args(["-c:v", "copy", "-an", "-f", format])
        .arg(&raw)
        .status()
        .is_ok_and(|s| s.success());
    assert!(copied, "{}", file.display());
    let f = facts(&raw);
    assert_eq!(fact(&f, "nb_read_frames"), frames.to_string(), "{f:?}");
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries"])
        .args(["stream=color_range,color_space,color_transfer,color_primaries"])
        .args(["-of", "default=nw=1"])
        .arg(&raw)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let get = |key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(&format!("{key}=")))
            .unwrap_or("")
            .to_string()
    };
    [
        get("color_range"),
        get("color_space"),
        get("color_transfer"),
        get("color_primaries"),
    ]
}

/// R84: every encoded H.264 and HEVC stream, on each encoder, says its
/// colour in its SPS's VUI as the container does (R40, R68): BT.709 from
/// 720 lines, BT.601 (SMPTE 170M) for an untagged source under 720, HDR
/// kept as PQ, and HDR tone-mapped as BT.709, all limited range.
/// Microsoft's software encoders write no colour description of their own,
/// and NVIDIA's writes BT.709's transfer for BT.601.
#[test]
fn every_stream_says_its_colour_as_the_container_does() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let hd = dir.path().join("hd.mp4");
    let made = Command::new("ffmpeg")
        .args(["-v", "error", "-f", "lavfi"])
        .args(["-i", "testsrc2=size=1280x720:rate=30", "-frames:v", "10"])
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
        .arg(&hd)
        .status()
        .is_ok_and(|s| s.success());
    if !made {
        eprintln!("skipped: FFmpeg has no libx264");
        return;
    }
    let hdr = video("hevc10_hdr.mp4");
    let bt709 = ["tv", "bt709", "bt709", "bt709"];
    let bt601 = ["tv", "smpte170m", "smpte170m", "smpte170m"];
    let pq = ["tv", "bt2020nc", "smpte2084", "bt2020"];
    let mut ran = 0;
    for codec in [VCodec::H264, VCodec::Hevc] {
        for choice in [EncoderChoice::Processor, EncoderChoice::GraphicsCard] {
            if choice == EncoderChoice::GraphicsCard && encoder_adapter(codec, choice).is_none() {
                eprintln!("skipped {codec:?} on the graphics card: no encoder starts");
                continue;
            }
            let hdr_kept = codec == VCodec::Hevc && choice == EncoderChoice::GraphicsCard;
            let cases = [
                ("SDR from 720 lines", hd.as_path(), bt709),
                ("SDR under 720 lines", &video("h264.mp4"), bt601),
                ("HDR", &hdr, if hdr_kept { pq } else { bt709 }),
            ];
            for (name, src, want) in cases {
                if !decodes(src) {
                    continue;
                }
                let tune = VideoTune {
                    encoder: choice,
                    ..tune_of(codec, VideoSetting::Quality(65))
                };
                let p = plan_for(src, &tune);
                for (c, ext) in [(Container::Mp4, "mp4"), (Container::Mkv, "mkv")] {
                    let what = format!("{name}, {codec:?} on {choice:?}, {ext}");
                    let out = dir.path().join(format!("{codec:?}_{choice:?}_{ran}.{ext}"));
                    let e = match encode_into(src, &p, choice, &out, 1, c) {
                        Ok(e) => e,
                        Err(e @ MediaError::NeedsExtensionToWrite { .. }) => {
                            eprintln!("skipped {what}: {e}");
                            continue;
                        }
                        Err(e) => panic!("{what}: {e}"),
                    };
                    ran += 1;
                    let f = facts(&out);
                    let container = ["color_space", "color_transfer", "color_primaries"]
                        .map(|key| fact(&f, key));
                    assert_eq!(container, want[1..], "{what}: the container");
                    let stream = stream_colour(&out, codec, e.packets.len());
                    assert_eq!(stream, want, "{what}: the stream");
                }
            }
        }
    }
    assert!(ran > 0, "no encode ran");
}

/// A 10-bit standard-range source on the processor keeps its colour, as an
/// 8-bit one does: Task 10c's lost chroma came from the bit depth as well as
/// from HDR. Untagged and under 720 lines, its samples are kept (R68).
#[test]
fn a_ten_bit_sd_source_on_the_processor_keeps_its_colour() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("ten.mp4");
    let made = Command::new("ffmpeg")
        .args(["-v", "error", "-f", "lavfi"])
        .args(["-i", "testsrc2=size=320x240:rate=30", "-frames:v", "10"])
        .args(["-c:v", "libx265", "-pix_fmt", "yuv420p10le"])
        .args(["-x265-params", "log-level=error", "-tag:v", "hvc1"])
        .arg(&src)
        .status()
        .is_ok_and(|s| s.success());
    if !made {
        eprintln!("skipped: FFmpeg has no libx265");
        return;
    }
    if !decodes(&src) {
        return;
    }
    let info = probe(&src).unwrap();
    assert_eq!((info.bit_depth, info.hdr), (10, Hdr::None));
    let source = first_frame(&src);
    let block = most_saturated_block(&source);
    let want = block_means(&source, block);
    let chroma = (want[1] - 128.0).abs() + (want[2] - 128.0).abs();
    assert!(chroma > 60.0, "the block is saturated: {want:?}");
    let mut ran = 0;
    for codec in [VCodec::H264, VCodec::Hevc] {
        let tune = VideoTune {
            encoder: EncoderChoice::Processor,
            ..tune_of(codec, VideoSetting::Quality(90))
        };
        let p = plan(&info, &tune);
        assert!(p.bt601 && p.bit_depth == 8, "{codec:?}: {p:?}");
        let out = dir.path().join(format!("{codec:?}.mp4"));
        match encode_to(&src, &p, EncoderChoice::Processor, &out, 1) {
            Ok(_) => ran += 1,
            Err(e @ MediaError::NeedsExtensionToWrite { .. }) => {
                eprintln!("skipped {codec:?}: {e}");
                continue;
            }
            Err(e) => panic!("{codec:?}: {e}"),
        }
        let got = block_means(&first_frame(&out), block);
        for (i, plane) in ["Y", "U", "V"].iter().enumerate() {
            assert!(
                (got[i] - want[i]).abs() < 3.0,
                "{codec:?}: the saturated block's {plane} is {:.1} against {:.1}",
                got[i],
                want[i]
            );
        }
    }
    assert!(ran > 0, "no encode ran");
}

#[test]
fn odd_source_encodes_even() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    let src = video("odd.mp4");
    if !decodes(&src) {
        return;
    }
    let p = plan_for(&src, &tune_of(VCodec::H264, VideoSetting::Quality(80)));
    assert_eq!((p.width, p.height), (884, 1918));
    for choice in [EncoderChoice::Processor, EncoderChoice::Automatic] {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out.mp4");
        encode_to(&src, &p, choice, &out, 1).unwrap();
        let f = facts(&out);
        assert_eq!((fact(&f, "width"), fact(&f, "height")), ("884", "1918"));
        // The last column and row carry the picture, not padding: their
        // colour is their neighbours'. Green padding is U and V near 0.
        let pic = first_frame(&out);
        let (w, h) = (pic.w, pic.h);
        let edge = |at: &dyn Fn(usize) -> (usize, usize), n: usize| -> (f64, f64) {
            let (mut u, mut v) = (0.0, 0.0);
            for i in 0..n {
                let (x, y) = at(i);
                let (a, b) = pic.uv_at(x, y);
                u += a as f64;
                v += b as f64;
            }
            (u / n as f64, v / n as f64)
        };
        let right = edge(&|i| (w - 1, i), h);
        let right_in = edge(&|i| (w - 4, i), h);
        let bottom = edge(&|i| (i, h - 1), w);
        let bottom_in = edge(&|i| (i, h - 4), w);
        for (name, a, b) in [("right", right, right_in), ("bottom", bottom, bottom_in)] {
            assert!(
                (a.0 - b.0).abs() <= 10.0 && (a.1 - b.1).abs() <= 10.0,
                "{choice:?}: {name} edge U/V {a:?}, beside it {b:?}"
            );
        }
    }
}

#[test]
fn frame_rate_cap_drops_frames() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.mp4");
    let src = video("h264.mp4");
    let tune = VideoTune {
        max_fps: Some(15),
        ..tune_of(VCodec::H264, VideoSetting::Quality(65))
    };
    let p = plan_for(&src, &tune);
    assert_eq!((p.fps, p.max_fps), (Some(15.0), Some(15.0)));
    let e = encode_to(&src, &p, EncoderChoice::Processor, &out, 1).unwrap();
    assert_eq!(e.packets.len(), 30);
    assert_plays(&out, "h264", (320, 240), 30, 2.0);
}

#[test]
fn a_frame_rate_at_the_limit_keeps_every_frame() {
    let info = probe(&video("h264.mp4")).unwrap();
    let tune = VideoTune {
        max_fps: Some(30),
        ..VideoTune::default()
    };
    let p = plan(&info, &tune);
    assert_eq!(p.max_fps, None);
    assert_eq!(p.setting, VideoSetting::Quality(65));
    let original = VideoTune {
        setting: Some(VideoSetting::Original),
        ..VideoTune::default()
    };
    assert_eq!(plan(&info, &original).setting, VideoSetting::Quality(90));
}

#[test]
fn kbps_lands_near_the_target_on_the_graphics_card() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    if encoder_adapter(VCodec::H264, EncoderChoice::GraphicsCard).is_none() {
        eprintln!("skipped: this PC has no graphics card encoder for H.264");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.mp4");
    let src = video("h264.mp4");
    let p = plan_for(&src, &tune_of(VCodec::H264, VideoSetting::Kbps(1500)));
    // The 2 s clip five times over: 10 s, five key-frame intervals.
    let e = encode_to(&src, &p, EncoderChoice::GraphicsCard, &out, 5).unwrap();
    assert!(e.hardware && e.honours_bitrate);
    assert_eq!(e.packets.len(), 300);
    let secs = seconds(&e.packets);
    let bytes: usize = e.packets.iter().map(|p| p.data.len()).sum();
    let kbps = bytes as f64 * 8.0 / secs / 1000.0;
    eprintln!("NVIDIA H.264 at Kbps(1500): {kbps:.0} kbps over {secs:.2} s");
    assert!(
        (1125.0..=1875.0).contains(&kbps),
        "{kbps:.0} kbps for a 1500 kbps target"
    );
    assert_plays(&out, "h264", (320, 240), 300, 10.0);
}

#[test]
fn the_software_h264_encoder_follows_a_bitrate_in_cbr() {
    if common::no_mf() {
        return;
    }
    let src = video("h264.mp4");
    let p = plan_for(&src, &tune_of(VCodec::H264, VideoSetting::Kbps(800)));
    let e = VideoEncoder::new(&p, EncoderChoice::Processor, None).unwrap();
    assert!(!e.hardware());
    // R49: CBR, which it follows.
    assert!(e.honours_bitrate());
    let p = plan_for(&src, &tune_of(VCodec::Hevc, VideoSetting::Kbps(800)));
    match VideoEncoder::new(&p, EncoderChoice::Processor, None) {
        Ok(e) => assert!(e.honours_bitrate()),
        // The HEVC software encoder comes with the HEVC Video Extensions.
        Err(e) => eprintln!("skipped the HEVC half: {e}"),
    }
}

#[test]
fn processor_choice_has_no_card() {
    assert_eq!(
        encoder_adapter(VCodec::H264, EncoderChoice::Processor),
        None
    );
    assert_eq!(
        card(VCodec::Hevc, EncoderChoice::Processor).map(|_| ()),
        None
    );
}

/// The card is found once per codec: asked again, the answer is the same
/// and comes without starting an encoder.
#[test]
fn the_encoder_card_is_found_once() {
    for codec in [VCodec::H264, VCodec::Hevc] {
        let t = std::time::Instant::now();
        let first = encoder_adapter(codec, EncoderChoice::Automatic);
        let found = t.elapsed();
        let t = std::time::Instant::now();
        for _ in 0..10 {
            assert_eq!(encoder_adapter(codec, EncoderChoice::Automatic), first);
            assert_eq!(encoder_adapter(codec, EncoderChoice::GraphicsCard), first);
        }
        let again = t.elapsed();
        eprintln!("{codec:?}: {first:?} found in {found:?}, 20 more in {again:?}");
        assert!(again.as_millis() < 25, "{codec:?}: 20 asks took {again:?}");
    }
}

// ---------------------------------------------------------------------------
// Fix round 1

/// Letting go of a hardware encoder mid-stream, with the reader still open,
/// used to crash the process now and then (NVIDIA's transform ran a work item
/// after it was freed). A crash here takes the whole test binary down.
#[test]
fn dropping_a_hardware_encoder_mid_stream_is_safe() {
    if common::no_mf() {
        return;
    }
    let mut ran = false;
    for (codec, name) in [(VCodec::Hevc, "hevc.mp4"), (VCodec::H264, "h264.mp4")] {
        let Some(d3d) = card(codec, EncoderChoice::GraphicsCard) else {
            eprintln!("skipped {codec:?}: no graphics card encoder for it");
            continue;
        };
        let src = video(name);
        if !decodes(&src) {
            continue;
        }
        ran = true;
        let p = plan_for(&src, &tune_of(codec, VideoSetting::Quality(65)));
        for round in 0..10 {
            let mut enc = VideoEncoder::new(&p, EncoderChoice::GraphicsCard, Some(&d3d)).unwrap();
            let mut r = Reader::open(
                File::open(&src).unwrap(),
                ReadMode::Decoded {
                    d3d: Some(d3d.clone()),
                },
            )
            .unwrap();
            let mut packets = Vec::new();
            for _ in 0..5 {
                let f = r.next_frame().unwrap().unwrap();
                enc.push(f, &mut packets).unwrap();
            }
            if round % 2 == 1 {
                enc.finish(&mut packets).unwrap();
            }
            drop(enc);
            drop(r);
        }
    }
    if !ran {
        eprintln!("skipped: this PC has no graphics card encoder");
    }
}

/// The decoded frames' times of `path`, in order.
fn frame_times(path: &Path) -> Vec<(i64, i64)> {
    let mut r = Reader::open(File::open(path).unwrap(), ReadMode::Decoded { d3d: None }).unwrap();
    let mut v = Vec::new();
    while let Some(f) = r.next_frame().unwrap() {
        v.push((f.pts, f.duration));
    }
    v
}

/// R38: every encoder path gives each packet its frame's time, from the
/// first (no frame late), for a steady and a variable rate, and under a
/// frame-rate limit each packet lasts until the next frame kept. From
/// Matroska, the frames' times are its blocks' (R78), so the first packet
/// is exactly at the first block.
#[test]
fn packets_keep_the_frames_times_on_every_path() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    let paths = [
        (VCodec::H264, EncoderChoice::Processor),
        (VCodec::Hevc, EncoderChoice::Processor),
        (VCodec::H264, EncoderChoice::GraphicsCard),
        (VCodec::Hevc, EncoderChoice::GraphicsCard),
    ];
    for (codec, choice) in paths {
        if choice == EncoderChoice::GraphicsCard && card(codec, choice).is_none() {
            eprintln!("skipped {codec:?} on the graphics card: there is no encoder for it");
            continue;
        }
        for name in ["h264.mp4", "vfr.mp4", "h264.mkv"] {
            let src = video(name);
            let want = frame_times(&src);
            if name.ends_with(".mkv") {
                let mut r = Reader::open(File::open(&src).unwrap(), ReadMode::Compressed).unwrap();
                let mut blocks = Vec::new();
                while let Some(p) = r.next_packet().unwrap() {
                    if p.stream == 0 {
                        blocks.push(p.pts);
                    }
                }
                blocks.sort_unstable();
                let frames: Vec<i64> = want.iter().map(|f| f.0).collect();
                assert_eq!(frames, blocks, "{name}: the frames take the blocks' times");
            }
            let dir = tempfile::tempdir().unwrap();
            let out = dir.path().join("out.mp4");
            let p = plan_for(&src, &tune_of(codec, VideoSetting::Quality(65)));
            let e = match encode_to(&src, &p, choice, &out, 1) {
                Err(e @ MediaError::NeedsExtensionToWrite { .. }) => {
                    eprintln!("skipped {codec:?} {choice:?}: {e}");
                    continue;
                }
                r => r.unwrap(),
            };
            let mut got: Vec<(i64, i64)> = e.packets.iter().map(|p| (p.pts, p.duration)).collect();
            got.sort();
            assert_eq!(got, want, "{codec:?} {choice:?} {name}");
            assert_eq!(
                e.packets[0].pts, want[0].0,
                "{codec:?} {choice:?} {name}: the first packet"
            );
            let f = facts(&out);
            assert_eq!(
                fact(&f, "start_time"),
                "0.000000",
                "{codec:?} {choice:?} {name}"
            );
        }
    }

    // Under a 15 fps limit, every other frame of a 30 fps source, each
    // lasting the gap to the next kept one.
    let src = video("h264.mp4");
    let tune = VideoTune {
        max_fps: Some(15),
        ..tune_of(VCodec::H264, VideoSetting::Quality(65))
    };
    let p = plan_for(&src, &tune);
    let dir = tempfile::tempdir().unwrap();
    let e = encode_to(
        &src,
        &p,
        EncoderChoice::Processor,
        &dir.path().join("cap.mp4"),
        1,
    )
    .unwrap();
    let mut got: Vec<(i64, i64)> = e.packets.iter().map(|p| (p.pts, p.duration)).collect();
    got.sort();
    let kept: Vec<i64> = frame_times(&src).iter().step_by(2).map(|t| t.0).collect();
    assert_eq!(got.iter().map(|g| g.0).collect::<Vec<_>>(), kept);
    for w in got.windows(2) {
        assert_eq!(
            w[0].0 + w[0].1,
            w[1].0,
            "a gap that isn't the packet's length"
        );
    }
    assert_eq!(got.last().unwrap().1, 666_667, "the last lasts one step");
}

/// R35: the plan says what will be written. HEVC from an HDR source keeps
/// its HDR only where a hardware encoder will write it.
#[test]
fn the_plan_follows_the_encoder_for_hdr_hevc() {
    if common::no_mf() {
        return;
    }
    let info = probe(&video("hevc10_hdr.mp4")).unwrap();
    let plan_with = |codec, encoder| {
        plan(
            &info,
            &VideoTune {
                codec,
                encoder,
                ..VideoTune::default()
            },
        )
    };
    let p = plan_with(VCodec::Hevc, EncoderChoice::Processor);
    assert_eq!((p.hdr_out, p.bit_depth, p.mastering), (Hdr::None, 8, None));
    let p = plan_with(VCodec::Hevc, EncoderChoice::GraphicsCard);
    assert_eq!((p.hdr_out, p.bit_depth), (Hdr::Pq, 10));
    assert_eq!(
        (p.mastering, p.content_light),
        (info.mastering, info.content_light)
    );
    let p = plan_with(VCodec::Hevc, EncoderChoice::Automatic);
    let hardware = encoder_adapter(VCodec::Hevc, EncoderChoice::Automatic).is_some();
    assert_eq!(
        (p.hdr_out, p.bit_depth),
        if hardware {
            (Hdr::Pq, 10)
        } else {
            (Hdr::None, 8)
        }
    );
    let p = plan_with(VCodec::H264, EncoderChoice::GraphicsCard);
    assert_eq!((p.hdr_out, p.bit_depth, p.mastering), (Hdr::None, 8, None));
}

/// R87: 10-bit HDR is kept only by a card's own video processor. Given a
/// 10-bit plan and no card (frames in memory), the encoder writes 8-bit
/// standard range, tone-mapped as on the processor, and its stream says
/// so; the picture keeps its colour.
#[test]
fn hdr_is_kept_only_with_a_cards_processor() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    let src = video("hevc10_hdr.mp4");
    if !decodes(&src) {
        return;
    }
    let ten = EncodePlan {
        hdr_out: Hdr::Pq,
        bit_depth: 10,
        ..plan_for(&src, &tune_of(VCodec::Hevc, VideoSetting::Quality(80)))
    };
    let mut enc = VideoEncoder::new(&ten, EncoderChoice::Automatic, None).unwrap();
    let mut r = Reader::open(File::open(&src).unwrap(), ReadMode::Decoded { d3d: None }).unwrap();
    let mut packets = Vec::new();
    while let Some(f) = r.next_frame().unwrap() {
        enc.push(f, &mut packets).unwrap();
    }
    enc.finish(&mut packets).unwrap();
    let spec = enc.stream();
    let v = spec.video.clone().unwrap();
    assert_eq!((v.hdr, v.bit_depth), (Hdr::None, 8));
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.mp4");
    let mut m = muxer(Container::Mp4, File::create(&out).unwrap()).unwrap();
    m.add_stream(spec).unwrap();
    for p in packets {
        m.write(p).unwrap();
    }
    m.finish(&Tags::default()).unwrap();
    let f = facts(&out);
    assert_eq!(fact(&f, "pix_fmt"), "yuv420p", "{f:?}");
    assert_eq!(fact(&f, "color_transfer"), "bt709", "{f:?}");
    let (u, v) = uv_means(&first_frame(&out));
    assert!(
        (u - 128.0).abs() < 40.0 && (v - 128.0).abs() < 40.0,
        "mean U {u:.1}, V {v:.1}"
    );
}

/// Makes a 1440x1080 clip with 4:3 pixels (shown 1920x1080) in `dir`, as
/// `name` (its extension picks the container). `None` without FFmpeg's
/// libx264. With `in_stream` the pixel aspect is in the H.264 stream's VUI
/// as well as the container; without, in the container only (`pasp`, or
/// the Matroska display size), so the decoder says square.
fn anamorphic(dir: &Path, name: &str, in_stream: bool) -> Option<PathBuf> {
    let out = dir.join(name);
    let source = |to: &Path, filter: &[&str]| {
        Command::new("ffmpeg")
            .args(["-v", "error", "-f", "lavfi"])
            .args(["-i", "testsrc2=size=1440x1080:rate=30", "-frames:v", "15"])
            .args(filter)
            .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
            .arg(to)
            .status()
            .is_ok_and(|s| s.success())
    };
    if in_stream {
        return source(&out, &["-vf", "setsar=4/3"]).then_some(out);
    }
    let square = dir.join(format!("square_{name}"));
    let made = source(&square, &[])
        && Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(&square)
            .args(["-c", "copy", "-aspect", "16:9"])
            .arg(&out)
            .status()
            .is_ok_and(|s| s.success());
    made.then_some(out)
}

/// The mean luma of rows `rows` of `p`.
fn rows_mean(p: &Planes, rows: std::ops::Range<usize>) -> f64 {
    let n = rows.len() * p.w;
    rows.flat_map(|y| (0..p.w).map(move |x| (x, y)))
        .map(|(x, y)| p.y_at(x, y) as f64)
        .sum::<f64>()
        / n as f64
}

/// R37: anamorphic pixels stay as they are, in both containers, encoded or
/// copied: no bars, and the pixel aspect written.
#[test]
fn anamorphic_sources_keep_their_pixel_aspect() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let made = [
        anamorphic(dir.path(), "ana.mp4", true),
        anamorphic(dir.path(), "ana.mkv", true),
        anamorphic(dir.path(), "box.mp4", false),
        anamorphic(dir.path(), "box.mkv", false),
    ];
    let [Some(mp4), Some(mkv), Some(box_mp4), Some(box_mkv)] = made else {
        eprintln!("skipped: FFmpeg has no libx264");
        return;
    };
    // From the MP4's pasp, and from the Matroska display size, with the
    // stream saying so too or not.
    for f in [&mp4, &mkv, &box_mp4, &box_mkv] {
        assert_eq!(probe(f).unwrap().sar, (4, 3), "{}", f.display());
    }
    let source = first_frame(&mp4);
    let (top, bottom) = (rows_mean(&source, 0..4), rows_mean(&source, 1076..1080));

    for (src, c, ext) in [
        (&mp4, Container::Mp4, "mp4"),
        (&mkv, Container::Mkv, "mkv"),
        (&box_mp4, Container::Mp4, "mp4"),
        (&box_mkv, Container::Mkv, "mkv"),
    ] {
        for (choice, max_lines) in [
            (EncoderChoice::Processor, None),
            (EncoderChoice::Automatic, None),
            (EncoderChoice::Automatic, Some(540)),
        ] {
            let tune = VideoTune {
                max_lines,
                ..tune_of(VCodec::H264, VideoSetting::Quality(65))
            };
            let p = plan_for(src, &tune);
            assert_eq!(p.sar, (4, 3));
            let stem = src.file_stem().unwrap().to_string_lossy();
            let out = dir
                .path()
                .join(format!("out_{stem}_{choice:?}_{max_lines:?}.{ext}"));
            let e = encode_into(src, &p, choice, &out, 1, c).unwrap();
            assert_eq!(e.spec.video.as_ref().unwrap().sar, (4, 3));
            let f = facts(&out);
            let size = (p.width.to_string(), p.height.to_string());
            assert_eq!(
                (fact(&f, "width"), fact(&f, "height")),
                (size.0.as_str(), size.1.as_str())
            );
            assert_eq!(
                fact(&f, "sample_aspect_ratio"),
                "4:3",
                "{ext} {choice:?}: {f:?}"
            );
            // R40, in both containers.
            for key in ["color_primaries", "color_transfer", "color_space"] {
                assert_eq!(fact(&f, key), "bt709", "{ext} {choice:?} {key}: {f:?}");
            }
            if c == Container::Mp4 {
                let bytes = std::fs::read(&out).unwrap();
                assert!(
                    bytes.windows(4).any(|w| w == b"pasp"),
                    "{choice:?}: no pasp"
                );
            }
            // No bars: the top and bottom rows are the picture's.
            let pic = first_frame(&out);
            let h = pic.h;
            let (t, b) = (rows_mean(&pic, 0..4), rows_mean(&pic, h - 4..h));
            assert!(
                (t - top).abs() < 12.0 && (b - bottom).abs() < 12.0,
                "{ext} {choice:?} {max_lines:?}: top {t:.1} (source {top:.1}), bottom {b:.1} (source {bottom:.1})"
            );
        }
    }

    // A copy keeps the pixel aspect too.
    let mut r = Reader::open(File::open(&mp4).unwrap(), ReadMode::Compressed).unwrap();
    let spec = r.streams()[0].spec.clone();
    assert_eq!(spec.video.as_ref().unwrap().sar, (4, 3));
    let out = dir.path().join("copy.mp4");
    let mut m = muxer(Container::Mp4, File::create(&out).unwrap()).unwrap();
    m.add_stream(spec).unwrap();
    while let Some(p) = r.next_packet().unwrap() {
        if p.stream == 0 {
            m.write(p).unwrap();
        }
    }
    m.finish(&Tags::default()).unwrap();
    assert_eq!(fact(&facts(&out), "sample_aspect_ratio"), "4:3");
}
