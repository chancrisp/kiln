//! Sound with a gap in its timestamps keeps its length: the gap is filled
//! with silence, so it stays in step with the picture (ENG-09).
//!
//! The Opus case builds its own file with Kiln's encoder and Matroska writer,
//! so it always runs. The AAC cases need FFmpeg to make their files, and skip
//! with a printed line when it isn't on PATH.

use kiln_media::ogg::OpusPackets;
use kiln_media::video::{muxer, Container, Mux, Packet};
use kiln_media::{
    convert, open_source, probe, Block, Channels, Preset, Setting, Tags, Target, Tune,
};
use std::fs::File;
use std::path::Path;
use std::process::Command;

const RATE: u32 = 48_000;
/// How far from 10 s the converted sound may be.
const SLACK_S: f64 = 0.05;

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

/// `seconds` of a 440 Hz sine, stereo, in blocks of 8,192 frames.
fn sine(seconds: u32) -> Vec<Block> {
    let total = (seconds * RATE) as usize;
    let mut blocks = Vec::new();
    let mut at = 0;
    while at < total {
        let n = 8192.min(total - at);
        let mut samples = Vec::with_capacity(n * 2);
        for i in at..at + n {
            let v = 0.5 * (std::f32::consts::TAU * 440.0 * i as f32 / RATE as f32).sin();
            samples.extend_from_slice(&[v, v]);
        }
        blocks.push(Block {
            channels: 2,
            rate: RATE,
            samples,
        });
        at += n;
    }
    blocks
}

/// 10 s of sine as Opus in Matroska, without the packets that start from
/// `cut.0` to `cut.1` seconds, when given.
fn opus_mkv(path: &Path, cut: Option<(f64, f64)>) {
    let stereo = Channels {
        count: 2,
        mask: 0x3,
    };
    let setting = Setting::Opus {
        kbps: 96,
        speech: false,
    };
    let mut enc = OpusPackets::new(stereo, &setting, 0).unwrap();
    let mut packets: Vec<Packet> = Vec::new();
    for b in sine(10) {
        enc.push(&b, &mut packets).unwrap();
    }
    enc.finish(&mut packets).unwrap();
    if let Some((from, to)) = cut {
        let (from, to) = ((from * RATE as f64) as i64, (to * RATE as f64) as i64);
        let before = packets.len();
        packets.retain(|p| p.pts < from || p.pts >= to);
        assert!(before - packets.len() > 90, "the cut took too few packets");
    }
    let mut m: Box<dyn Mux> = muxer(Container::Mkv, File::create(path).unwrap()).unwrap();
    m.add_stream(enc.stream()).unwrap();
    for p in packets {
        m.write(p).unwrap();
    }
    m.finish(&Tags::default()).unwrap();
}

/// Converts `input` to a WAV beside it and returns the WAV's path.
fn to_wav(input: &Path) -> std::path::PathBuf {
    let wav = input.with_extension("wav");
    convert(
        input,
        &wav,
        Target::Wav,
        Preset::High,
        &Tune::default(),
        &mut |_| {},
        &|| false,
    )
    .unwrap();
    wav
}

fn seconds_of(wav: &Path) -> f64 {
    let info = probe(wav).unwrap();
    info.frames.expect("a WAV states its length") as f64 / info.rate as f64
}

/// The mean absolute level of the first channel from `from` to `to` seconds.
fn level(samples: &[f32], channels: usize, rate: u32, from: f64, to: f64) -> f32 {
    let (a, b) = ((from * rate as f64) as usize, (to * rate as f64) as usize);
    let part: Vec<f32> = (a..b).map(|i| samples[i * channels].abs()).collect();
    part.iter().sum::<f32>() / part.len() as f32
}

fn samples_of(wav: &Path) -> (Vec<f32>, usize, u32) {
    let mut src = open_source(wav).unwrap();
    let (channels, rate) = (src.info().channels.count as usize, src.info().rate);
    let mut all = Vec::new();
    let mut b = Block {
        channels: 0,
        rate: 0,
        samples: Vec::new(),
    };
    while src.read(&mut b).unwrap() {
        all.extend_from_slice(&b.samples);
    }
    (all, channels, rate)
}

/// 4 s of sound, 2 s of nothing, then 4 s more: the sound's length is that of
/// the whole, and the gap is silent.
fn assert_gap_filled(wav: &Path) {
    let secs = seconds_of(wav);
    assert!((secs - 10.0).abs() < SLACK_S, "{secs} s, not 10 s");
    let (samples, channels, rate) = samples_of(wav);
    assert!(level(&samples, channels, rate, 1.0, 3.0) > 0.03);
    assert!(level(&samples, channels, rate, 4.2, 5.8) < 0.001);
    assert!(level(&samples, channels, rate, 6.2, 9.8) > 0.03);
}

#[test]
fn a_gapped_source_keeps_its_length() {
    let dir = tempfile::tempdir().unwrap();

    // Without the cut the file is 10 s: the baseline for the one with it.
    let whole = dir.path().join("whole.mkv");
    opus_mkv(&whole, None);
    let secs = seconds_of(&to_wav(&whole));
    assert!((secs - 10.0).abs() < SLACK_S, "{secs} s, not 10 s");

    let gapped = dir.path().join("gapped.mkv");
    opus_mkv(&gapped, Some((4.0, 6.0)));
    assert_gap_filled(&to_wav(&gapped));
}

/// An AAC file with the sound from 4 s to 6 s missing, made by FFmpeg: its
/// encoder is given no frames for that stretch, so the packets' times jump.
fn ffmpeg_aac(path: &Path) {
    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg("sine=frequency=440:sample_rate=48000:duration=10")
        .args([
            "-af",
            "aselect='lt(t,4)+gte(t,6)'",
            "-ac",
            "2",
            "-c:a",
            "aac",
        ])
        .args(["-b:a", "128k"])
        .arg(path)
        .output()
        .expect("ffmpeg starts");
    assert!(
        out.status.success(),
        "ffmpeg failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // The packets' times jump over the gap, or this proves nothing.
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "a:0", "-show_entries"])
        .args(["packet=pts_time", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .expect("ffprobe starts");
    let times: Vec<f64> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().trim_end_matches(',').parse().ok())
        .collect();
    let widest = times.windows(2).map(|w| w[1] - w[0]).fold(0.0, f64::max);
    assert!(
        widest > 1.9,
        "no gap in {path:?}: the widest step is {widest}"
    );
}

#[test]
fn a_gapped_aac_in_matroska_keeps_its_length() {
    if !ffmpeg_ready() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let gapped = dir.path().join("gapped.mkv");
    ffmpeg_aac(&gapped);
    assert_gap_filled(&to_wav(&gapped));
}

#[test]
fn a_gapped_aac_in_an_mp4_keeps_its_length() {
    if !ffmpeg_ready() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let gapped = dir.path().join("gapped.m4a");
    ffmpeg_aac(&gapped);
    assert_gap_filled(&to_wav(&gapped));
}
