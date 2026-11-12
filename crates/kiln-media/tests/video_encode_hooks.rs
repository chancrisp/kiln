//! The video encoder's debug-only hooks: `KILN_FAIL_HW_ENCODER` makes a
//! hardware encoder fail on its first frame, `KILN_MUTE_HW_ENCODER` makes
//! one say nothing, and `KILN_HIDE_HW_ENCODERS` acts as if this PC had none.
//!
//! In a test binary of their own, one test at a time: the variables change
//! what every encoder in the process sees.

#![cfg(windows)]

mod common;

use kiln_media::video::encode::plan;
use kiln_media::video::{
    d3d_for, encoder_adapter, muxer, probe, Container, EncodePlan, EncoderChoice, Hdr, ReadMode,
    Reader, StreamSpec, VCodec, VideoEncoder, VideoSetting, VideoTune,
};
use kiln_media::{MediaError, Tags};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use windows::core::Interface;
use windows::Win32::Media::MediaFoundation::IMF2DBuffer;

/// One test at a time: they set process-wide variables.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// Sets `name` for as long as it lives.
struct Var(&'static str);

impl Var {
    fn set(name: &'static str) -> Var {
        std::env::set_var(name, "1");
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

fn plan_of(src: &Path, codec: VCodec) -> EncodePlan {
    let tune = VideoTune {
        setting: Some(VideoSetting::Quality(65)),
        codec,
        ..VideoTune::default()
    };
    plan(&probe(src).unwrap(), &tune)
}

/// Decodes `src` on the card the job would use for Automatic, encodes it
/// with Automatic and writes an MP4 to `out`. Gives whether the hardware
/// encoder wrote it, the stream, and the packet count.
fn encode_automatic(src: &Path, p: &EncodePlan, out: &Path) -> (bool, StreamSpec, usize) {
    let d3d = encoder_adapter(p.codec, EncoderChoice::Automatic).and_then(d3d_for);
    let mut enc = VideoEncoder::new(p, EncoderChoice::Automatic, d3d.as_ref()).unwrap();
    let mut r = Reader::open(File::open(src).unwrap(), ReadMode::Decoded { d3d }).unwrap();
    let mut packets = Vec::new();
    while let Some(f) = r.next_frame().unwrap() {
        enc.push(f, &mut packets).unwrap();
    }
    enc.finish(&mut packets).unwrap();
    let spec = enc.stream();
    let mut m = muxer(Container::Mp4, File::create(out).unwrap()).unwrap();
    m.add_stream(spec.clone()).unwrap();
    let n = packets.len();
    for p in packets {
        m.write(p).unwrap();
    }
    m.finish(&Tags::default()).unwrap();
    (enc.hardware(), spec, n)
}

/// The frames Media Foundation decodes from `path`.
fn decoded_frames(path: &Path) -> usize {
    let mut r = Reader::open(File::open(path).unwrap(), ReadMode::Decoded { d3d: None }).unwrap();
    let mut n = 0;
    while r.next_frame().unwrap().is_some() {
        n += 1;
    }
    n
}

/// The mean Y, U and V of the first frame of `path`, decoded in memory to
/// NV12, as a whole and in each 16x16 block, row by row.
fn first_frame_means(path: &Path) -> ([f64; 3], Vec<[f64; 3]>) {
    let mut r = Reader::open(File::open(path).unwrap(), ReadMode::Decoded { d3d: None }).unwrap();
    let f = r.next_frame().unwrap().expect("a frame");
    let (w, h) = (
        f.format.shown.width as usize,
        f.format.shown.height as usize,
    );
    let fh = f.format.height as usize;
    // SAFETY: the 2D buffer is locked while its rows are read, each within
    // the pitch it gives.
    let (y, uv) = unsafe {
        let b: IMF2DBuffer = f.sample.GetBufferByIndex(0).unwrap().cast().unwrap();
        let mut p = std::ptr::null_mut();
        let mut pitch = 0i32;
        b.Lock2D(&mut p, &mut pitch).unwrap();
        let line =
            |row: usize| std::slice::from_raw_parts(p.offset(row as isize * pitch as isize), w);
        let y: Vec<u8> = (0..h).flat_map(|row| line(row).to_vec()).collect();
        let uv: Vec<u8> = (0..h / 2).flat_map(|row| line(fh + row).to_vec()).collect();
        let _ = b.Unlock2D();
        (y, uv)
    };
    let mean = |pick: &dyn Fn(usize, usize) -> u8,
                xs: std::ops::Range<usize>,
                ys: std::ops::Range<usize>| {
        let n = (xs.len() * ys.len()) as f64;
        ys.flat_map(|j| xs.clone().map(move |i| (i, j)))
            .map(|(i, j)| pick(i, j) as f64)
            .sum::<f64>()
            / n
    };
    let luma = |x: usize, y_: usize| y[y_ * w + x];
    let u = |x: usize, y_: usize| uv[(y_ / 2) * w + (x / 2) * 2];
    let v = |x: usize, y_: usize| uv[(y_ / 2) * w + (x / 2) * 2 + 1];
    let at = |xs: std::ops::Range<usize>, ys: std::ops::Range<usize>| {
        [
            mean(&luma, xs.clone(), ys.clone()),
            mean(&u, xs.clone(), ys.clone()),
            mean(&v, xs, ys),
        ]
    };
    let mut blocks = Vec::new();
    for by in (0..h - 15).step_by(16) {
        for bx in (0..w - 15).step_by(16) {
            blocks.push(at(bx..bx + 16, by..by + 16));
        }
    }
    (at(0..w, 0..h), blocks)
}

/// ffprobe's count of the frames in `path`, when FFmpeg is on PATH.
fn ffprobe_frames(path: &Path) -> Option<String> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-count_frames"])
        .args(["-show_entries", "stream=nb_read_frames", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[test]
fn a_cancel_is_heard_while_a_silent_card_encoder_is_waited_on() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if common::no_mf() {
        return;
    }
    if !cfg!(debug_assertions) {
        eprintln!("skipped: KILN_MUTE_HW_ENCODER works in debug builds only");
        return;
    }
    if encoder_adapter(VCodec::H264, EncoderChoice::GraphicsCard).is_none() {
        eprintln!("skipped: this PC has no graphics card encoder to wait on");
        return;
    }
    // Final review item 8: an encoder that has stopped answering is waited
    // on for 30 s before it is called stopped; a cancel ends the wait at
    // once, both for a frame and for the drain, and Automatic doesn't take
    // it for a failure to fall back from.
    let _mute = Var::set("KILN_MUTE_HW_ENCODER");
    let src = video("h264.mp4");
    let p = plan_of(&src, VCodec::H264);
    let d3d = encoder_adapter(p.codec, EncoderChoice::GraphicsCard).and_then(d3d_for);
    let after = |ms: u64| {
        let start = std::time::Instant::now();
        move || start.elapsed() >= std::time::Duration::from_millis(ms)
    };
    for choice in [EncoderChoice::GraphicsCard, EncoderChoice::Automatic] {
        let mut enc = VideoEncoder::new(&p, choice, d3d.as_ref()).unwrap();
        assert!(enc.hardware(), "{choice:?}");
        let mut r = Reader::open(
            File::open(&src).unwrap(),
            ReadMode::Decoded { d3d: d3d.clone() },
        )
        .unwrap();
        let f = r.next_frame().unwrap().unwrap();
        let mut out = Vec::new();
        let asked = std::time::Instant::now();
        let got = enc.push_watched(f, &mut out, &after(100));
        assert!(
            matches!(got, Err(MediaError::Cancelled)),
            "{choice:?}: {got:?}"
        );
        assert!(asked.elapsed() < std::time::Duration::from_secs(5));
        assert!(!enc.fell_back(), "{choice:?}");
        let asked = std::time::Instant::now();
        let got = enc.finish_watched(&mut out, &after(100));
        assert!(
            matches!(got, Err(MediaError::Cancelled)),
            "{choice:?}: {got:?}"
        );
        assert!(asked.elapsed() < std::time::Duration::from_secs(5));
    }
}

#[test]
fn hardware_refusal_falls_back_to_the_processor() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if common::no_mf() {
        return;
    }
    if !cfg!(debug_assertions) {
        eprintln!("skipped: KILN_FAIL_HW_ENCODER works in debug builds only");
        return;
    }
    if encoder_adapter(VCodec::H264, EncoderChoice::Automatic).is_none() {
        eprintln!("skipped: this PC has no graphics card encoder to fail");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    // What the card makes of the HDR clip in standard range when nothing
    // fails, to compare the fallback's picture with.
    let hdr = video("hevc10_hdr.mp4");
    let card = Reader::open(File::open(&hdr).unwrap(), ReadMode::Decoded { d3d: None })
        .is_ok()
        .then(|| {
            let sdr = EncodePlan {
                bit_depth: 8,
                hdr_out: Hdr::None,
                setting: VideoSetting::Quality(90),
                ..plan_of(&hdr, VCodec::Hevc)
            };
            let out = dir.path().join("card.mp4");
            assert!(encode_automatic(&hdr, &sdr, &out).0);
            first_frame_means(&out)
        });
    let _fail = Var::set("KILN_FAIL_HW_ENCODER");

    // H.264: the frames the hardware encoder failed on go to the software
    // encoder, and every one is written.
    let out = dir.path().join("h264.mp4");
    let src = video("h264.mp4");
    let (hardware, _, packets) = encode_automatic(&src, &plan_of(&src, VCodec::H264), &out);
    assert!(!hardware);
    assert_eq!(packets, 60);
    assert_eq!(decoded_frames(&out), 60);
    if let Some(n) = ffprobe_frames(&out) {
        assert_eq!(n, "60");
    }

    // 10-bit HDR to HEVC: the software encoder takes 8 bits only, so the
    // frames already made in 10-bit PQ are brought over to standard range.
    let out = dir.path().join("hevc.mp4");
    let src = hdr;
    let Some((_, card)) = card else {
        eprintln!("skipped the HEVC half: Windows can't decode HEVC");
        return;
    };
    let p = EncodePlan {
        setting: VideoSetting::Quality(90),
        ..plan_of(&src, VCodec::Hevc)
    };
    assert_eq!((p.hdr_out, p.bit_depth), (Hdr::Pq, 10));
    let (hardware, spec, packets) = encode_automatic(&src, &p, &out);
    assert!(!hardware);
    let v = spec.video.unwrap();
    assert_eq!((v.hdr, v.bit_depth), (Hdr::None, 8));
    assert_eq!(packets, 60);
    assert_eq!(decoded_frames(&out), 60);
    // Tone-mapped, not left dark (121): the frames brought over are mapped
    // on the card, as the rest are (the probe's 204), and in colour (in
    // software they had U and V near 0). Block by block the first frame is
    // the card's own standard-range picture of the clip: two thirds of the
    // blocks to 3 levels, and all to 30. The frames brought over are
    // processed twice (to 10-bit PQ, then to standard range), which clips
    // testsrc2's bars far outside BT.709 a second time; measured, 209 of
    // 300 blocks within 3 and the worst 27.7 (a blue bar's V).
    let ([y, _, _], got) = first_frame_means(&out);
    assert!(y > 185.0, "luma mean {y:.1}");
    let mut close = 0;
    for (i, (g, c)) in got.iter().zip(&card).enumerate() {
        let worst = g
            .iter()
            .zip(c)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f64::max);
        assert!(worst <= 30.0, "block {i}: {g:?}, the card's {c:?}");
        close += usize::from(worst <= 3.0);
    }
    assert!(
        close * 3 >= got.len() * 2,
        "{close} of {} blocks within 3 levels of the card's",
        got.len()
    );
}

/// An HDR10 clip of testsrc2 at half its RGB levels, made with zscale and
/// libx265 at 640x360 (its colours inside BT.709, below the tone map's
/// knee), in `dir`. `None` without FFmpeg, libx265 or zscale.
fn in_gamut_pq_clip(dir: &Path) -> Option<PathBuf> {
    let out = dir.join("pq.mp4");
    let ok = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg("testsrc2=size=640x360:rate=30")
        .args(["-frames:v", "30", "-vf"])
        .arg(
            "format=gbrp,colorlevels=romax=0.5:gomax=0.5:bomax=0.5,\
             zscale=tin=bt709:pin=bt709:rin=pc:t=smpte2084:m=bt2020nc:p=bt2020:r=tv:npl=203,\
             format=yuv420p10le",
        )
        .args(["-c:v", "libx265", "-tag:v", "hvc1", "-x265-params"])
        .arg(
            "log-level=error:lossless=1:colorprim=bt2020:transfer=smpte2084:\
             colormatrix=bt2020nc:range=limited",
        )
        .arg(&out)
        .status()
        .is_ok_and(|s| s.success());
    ok.then_some(out)
}

/// R58 with Task 10c: on an in-gamut HDR clip, the frames the failed
/// hardware encoder held, brought over to standard range, are the card's
/// own standard-range picture of the clip. The reference is that picture
/// made by the card's processor once and written by the same software
/// encoder (itself within 2 levels a block of the card's encode; measured
/// 0.9). Against it, the first frame's luma is within 3 a block (measured
/// 1.4), each mean within 1 (measured 0.3), and 85% of the blocks within 4
/// in Y, U and V (measured 91%). The held frames are processed twice (to
/// 10-bit PQ, then to standard range), so the chroma is filtered twice:
/// on testsrc2's sharp colour edges U within 10 and V within 16 a block
/// (measured 14.5 in V).
#[test]
fn a_hardware_refusal_keeps_an_in_gamut_hdr_clips_colour() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if common::no_mf() {
        return;
    }
    if !cfg!(debug_assertions) {
        eprintln!("skipped: KILN_FAIL_HW_ENCODER works in debug builds only");
        return;
    }
    if encoder_adapter(VCodec::Hevc, EncoderChoice::Automatic).is_none() {
        eprintln!("skipped: this PC has no graphics card encoder for HEVC to fail");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let Some(src) = in_gamut_pq_clip(dir.path()) else {
        eprintln!("skipped: FFmpeg, libx265 or zscale is missing");
        return;
    };
    if Reader::open(File::open(&src).unwrap(), ReadMode::Decoded { d3d: None }).is_err() {
        eprintln!("skipped: Windows can't decode HEVC");
        return;
    }
    let p = EncodePlan {
        setting: VideoSetting::Quality(90),
        ..plan_of(&src, VCodec::Hevc)
    };
    assert_eq!((p.hdr_out, p.bit_depth), (Hdr::Pq, 10));
    let sdr = EncodePlan {
        bit_depth: 8,
        hdr_out: Hdr::None,
        ..p
    };
    let card = dir.path().join("card.mp4");
    assert!(encode_automatic(&src, &sdr, &card).0);
    let (_, card) = first_frame_means(&card);

    let _fail = Var::set("KILN_FAIL_HW_ENCODER");
    // The standard-range plan falls back with the same plan: the frames
    // held are the card processor's, written by the software encoder.
    let once = dir.path().join("once.mp4");
    assert!(!encode_automatic(&src, &sdr, &once).0);
    let (_, once) = first_frame_means(&once);
    let largest = |a: &[[f64; 3]], b: &[[f64; 3]]| {
        let mut m = [0.0f64; 3];
        for (x, y) in a.iter().zip(b) {
            for i in 0..3 {
                m[i] = m[i].max((x[i] - y[i]).abs());
            }
        }
        m
    };
    let off = largest(&once, &card);
    assert!(
        off.iter().all(|&d| d <= 2.0),
        "the software encoder's blocks are up to {off:?} from the card's"
    );

    let out = dir.path().join("fallback.mp4");
    let (hardware, spec, packets) = encode_automatic(&src, &p, &out);
    assert!(!hardware);
    let v = spec.video.unwrap();
    assert_eq!((v.hdr, v.bit_depth), (Hdr::None, 8));
    assert_eq!(packets, 30);
    let (_, got) = first_frame_means(&out);
    let off = largest(&got, &once);
    assert!(
        off[0] <= 3.0 && off[1] <= 10.0 && off[2] <= 16.0,
        "the blocks are up to {off:?} from the card's picture"
    );
    let n = got.len() as f64;
    for i in 0..3 {
        let mean = got.iter().zip(&once).map(|(g, o)| g[i] - o[i]).sum::<f64>() / n;
        assert!(
            mean.abs() <= 1.0,
            "{} is {mean:+.2} on average",
            ["Y", "U", "V"][i]
        );
    }
    let close = got
        .iter()
        .zip(&once)
        .filter(|(g, o)| g.iter().zip(*o).all(|(a, b)| (a - b).abs() <= 4.0))
        .count();
    assert!(
        close * 100 >= got.len() * 85,
        "{close} of {} blocks within 4 levels",
        got.len()
    );
}

#[test]
fn graphics_card_with_no_encoder_says_so() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if common::no_mf() {
        return;
    }
    if !cfg!(debug_assertions) {
        eprintln!("skipped: KILN_HIDE_HW_ENCODERS works in debug builds only");
        return;
    }
    // The answer is stored per codec once found: asked first with the hook
    // off, it must still give way to the hook.
    for codec in [VCodec::H264, VCodec::Hevc] {
        let _ = encoder_adapter(codec, EncoderChoice::Automatic);
    }
    let _hide = Var::set("KILN_HIDE_HW_ENCODERS");
    let src = video("h264.mp4");
    for (codec, name) in [(VCodec::H264, "H.264"), (VCodec::Hevc, "HEVC")] {
        assert_eq!(encoder_adapter(codec, EncoderChoice::GraphicsCard), None);
        let p = plan_of(&src, codec);
        match VideoEncoder::new(&p, EncoderChoice::GraphicsCard, None) {
            Err(e @ MediaError::NoHardwareEncoder { .. }) => {
                assert!(matches!(e, MediaError::NoHardwareEncoder { codec } if codec == name));
                assert_eq!(
                    e.to_string(),
                    format!(
                        "This PC has no graphics card encoder for {name}. Choose Automatic or Processor."
                    )
                );
            }
            Err(e) => panic!("{name}: {e}"),
            Ok(_) => panic!("{name}: a graphics card encoder with every one hidden"),
        }
        // Automatic carries on with the processor.
        let e = VideoEncoder::new(&p, EncoderChoice::Automatic, None).unwrap();
        assert!(!e.hardware());
    }
}

/// R39: with no hardware encoder and no HEVC Video Extensions, HEVC can't be
/// written, and says what is missing.
#[test]
fn hevc_with_no_encoder_says_what_is_missing() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if common::no_mf() {
        return;
    }
    if !cfg!(debug_assertions) {
        eprintln!("skipped: the hiding variables work in debug builds only");
        return;
    }
    let _hide = Var::set("KILN_HIDE_HW_ENCODERS");
    let _hevc = Var::set("KILN_HIDE_HEVC");
    let p = plan_of(&video("h264.mp4"), VCodec::Hevc);
    for choice in [EncoderChoice::Processor, EncoderChoice::Automatic] {
        match VideoEncoder::new(&p, choice, None) {
            Err(e @ MediaError::NeedsExtensionToWrite { .. }) => {
                assert!(matches!(
                    e,
                    MediaError::NeedsExtensionToWrite {
                        codec: "HEVC",
                        product: "9N4WGH0Z6VHQ"
                    }
                ));
                assert_eq!(
                    e.to_string(),
                    "Needs Microsoft's HEVC Video Extensions to write HEVC video."
                );
            }
            Err(e) => panic!("{choice:?}: {e}"),
            Ok(_) => panic!("{choice:?}: an HEVC encoder with every one hidden"),
        }
    }
    // H.264 needs no extension.
    let p = plan_of(&video("h264.mp4"), VCodec::H264);
    assert!(VideoEncoder::new(&p, EncoderChoice::Processor, None).is_ok());
}
