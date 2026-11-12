//! Media Foundation: AAC in M4A and ADTS, WMA and AC3, written and read back,
//! and the formats only Media Foundation reads.
#![cfg(windows)]

mod common;

use kiln_media::mf::{aac, ac3, source, wma};
use kiln_media::{
    mp4, sym, Block, Channels, Codec, MediaError, Picture, Setting, Sink, SinkSpec, Source, Tags,
};
use std::f64::consts::PI;
use std::fs::File;
use std::path::{Path, PathBuf};

const BLOCK: usize = 8192;

fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join(name)
}

fn channels_of(count: u16) -> Channels {
    Channels {
        count,
        mask: match count {
            1 => 0x4,
            2 => 0x3,
            6 => 0x3F,
            _ => 0,
        },
    }
}

/// Something that reads like music to an encoder. Deterministic.
fn music(frames: usize, channels: usize, rate: u32) -> Vec<f32> {
    let mut state = 0x1234_5678_9ABC_DEF0u64;
    let mut out = Vec::with_capacity(frames * channels);
    for i in 0..frames {
        let t = i as f64 / rate as f64;
        for c in 0..channels {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let noise = ((state >> 40) as f64 / (1u64 << 24) as f64 - 0.5) * 0.04;
            let f = 1.0 + 0.07 * c as f64;
            let swell = 0.5 + 0.5 * (2.0 * PI * 0.37 * t + c as f64).sin();
            let v = 0.22 * (2.0 * PI * 220.0 * f * t).sin()
                + 0.16 * swell * (2.0 * PI * 554.37 * f * t).sin()
                + 0.12 * (2.0 * PI * 1318.5 * f * t + 1.3 * swell).sin()
                + noise;
            out.push(v as f32);
        }
    }
    out
}

fn blocks(samples: &[f32], channels: u16, rate: u32) -> impl Iterator<Item = Block> + '_ {
    samples
        .chunks(BLOCK * channels as usize)
        .map(move |c| Block {
            channels,
            rate,
            samples: c.to_vec(),
        })
}

fn read_all(src: &mut dyn Source) -> Vec<f32> {
    let mut out = Vec::new();
    let mut b = Block {
        channels: 0,
        rate: 0,
        samples: Vec::new(),
    };
    while src.read(&mut b).unwrap() {
        assert!(b.frames() > 0 && b.frames() <= BLOCK);
        assert_eq!(b.channels, src.info().channels.count);
        out.extend_from_slice(&b.samples);
    }
    out
}

fn spec<'a>(
    path: &Path,
    rate: u32,
    channels: u16,
    tags: &'a Tags,
    setting: &'a Setting,
) -> SinkSpec<'a> {
    SinkSpec {
        rate,
        channels: channels_of(channels),
        tags,
        setting,
        source_bits: None,
        out: File::create(path).unwrap(),
    }
}

fn write_all(mut sink: Box<dyn Sink>, samples: &[f32], channels: u16, rate: u32) {
    for b in blocks(samples, channels, rate) {
        sink.write(&b).unwrap();
    }
    sink.finish().unwrap();
}

/// A temporary folder whose name isn't ASCII, so nothing on the way may
/// depend on a narrow path.
fn tmp() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let inner = dir.path().join("t\u{f6}ne \u{6771}\u{4eac}");
    std::fs::create_dir(&inner).unwrap();
    (dir, inner)
}

fn full_tags() -> Tags {
    Tags {
        title: Some("F\u{fc}\u{df}e \u{2013} \u{6771}\u{4eac}".into()),
        artist: Some("An Artist".into()),
        album: Some("An Album".into()),
        album_artist: Some("Various".into()),
        genre: Some("Jazz".into()),
        comment: Some("A comment".into()),
        track: Some((3, Some(12))),
        disc: Some((1, Some(2))),
        year: Some("2001".into()),
        picture: Some(Picture {
            mime: "image/png".into(),
            data: std::fs::read(data("cover.png")).unwrap(),
        }),
    }
}

fn kbps(k: u32) -> Setting {
    Setting::Kbps { kbps: k }
}

/// Writes an M4A of `samples` and returns its path: 160 kbps, or 480 (the
/// same per channel pair) for 5.1.
fn write_m4a(dir: &Path, samples: &[f32], channels: u16, rate: u32, tags: &Tags) -> PathBuf {
    let path = dir.join("out.m4a");
    let k = if channels > 2 { 480 } else { 160 };
    let setting = kbps(k);
    let enc = aac::encoder(rate, channels, k).unwrap();
    let sink = mp4::m4a_sink(spec(&path, rate, channels, tags, &setting), enc).unwrap();
    write_all(sink, samples, channels, rate);
    path
}

/// The top-level boxes of an MP4 file, by name.
fn top_boxes(bytes: &[u8]) -> Vec<String> {
    let mut at = 0usize;
    let mut out = Vec::new();
    while at + 8 <= bytes.len() {
        let mut size = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as u64;
        if size == 1 {
            size = u64::from_be_bytes(bytes[at + 8..at + 16].try_into().unwrap());
        }
        out.push(String::from_utf8_lossy(&bytes[at + 4..at + 8]).into_owned());
        at += size as usize;
    }
    out
}

/// The lag (in frames, 0..max) at which `got` best lines up with `want`
/// (both mono), by normalised cross-correlation over the first `len` frames
/// of `want`.
fn best_lag(want: &[f32], got: &[f32], len: usize, max: usize) -> usize {
    let c = |lag: usize| -> f64 {
        let (mut dot, mut energy) = (0.0f64, 0.0f64);
        for (n, &w) in want.iter().take(len).enumerate() {
            let g = got.get(n + lag).copied().unwrap_or(0.0) as f64;
            dot += w as f64 * g;
            energy += g * g;
        }
        dot / energy.sqrt().max(1e-12)
    };
    (0..max)
        .max_by(|&a, &b| c(a).partial_cmp(&c(b)).unwrap())
        .unwrap()
}

fn channel(samples: &[f32], count: usize, ch: usize) -> Vec<f32> {
    samples.chunks(count).map(|f| f[ch]).collect()
}

/// The power of `freq` in `x` (Goertzel).
fn power(x: &[f32], rate: u32, freq: f64) -> f64 {
    let w = 2.0 * PI * freq / rate as f64;
    let k = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for &v in x {
        let s = v as f64 + k * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    (s1 * s1 + s2 * s2 - k * s1 * s2) / (x.len() as f64).powi(2)
}

// ---------------------------------------------------------------------------
// M4A and ADTS

#[test]
fn m4a_plays_back_with_both_readers() {
    if common::no_mf() {
        return;
    }
    let (_d, dir) = tmp();
    let frames = 44_100 * 30;
    let src = music(frames, 2, 44_100);
    let path = write_m4a(&dir, &src, 2, 44_100, &Tags::default());

    // Symphonia ignores the edit list (R14), so it hands out the priming too.
    let mut s = sym::open(File::open(&path).unwrap(), "m4a").unwrap();
    assert_eq!(s.info().codec, Codec::Aac);
    assert_eq!((s.info().rate, s.info().channels.count), (44_100, 2));
    let got = read_all(&mut *s).len() / 2 - aac::PRIMING as usize;
    assert!(got.abs_diff(frames) <= 1024, "Symphonia: {got} vs {frames}");

    let mut m = source::open(File::open(&path).unwrap()).unwrap();
    assert_eq!(m.info().codec, Codec::Aac);
    assert_eq!((m.info().rate, m.info().channels.count), (44_100, 2));
    let got = read_all(&mut *m).len() / 2;
    assert!(
        got.abs_diff(frames) <= 1024,
        "Media Foundation: {got} vs {frames}"
    );
}

#[test]
fn aac_priming_is_what_the_m4a_says() {
    if common::no_mf() {
        return;
    }
    // Decoded without the edit list, the audio lines up PRIMING frames late
    // (none: Windows' encoder takes its delay out itself).
    let (_d, dir) = tmp();
    let src = music(44_100 * 3, 2, 44_100);
    let path = write_m4a(&dir, &src, 2, 44_100, &Tags::default());
    let mut s = sym::open(File::open(&path).unwrap(), "m4a").unwrap();
    let got = read_all(&mut *s);
    // A lag up to 4,096 frames either way: compare from one second in, with
    // the decoded audio taken from 4,096 frames earlier.
    let (want, got) = (channel(&src, 2, 0), channel(&got, 2, 0));
    let lag = best_lag(&want[44_100..], &got[44_100 - 4096..], 8192, 8192) as i64 - 4096;
    assert_eq!(lag, aac::PRIMING as i64, "measured priming");
}

#[test]
fn m4a_moov_comes_first() {
    if common::no_mf() {
        return;
    }
    let (_d, dir) = tmp();
    let path = write_m4a(&dir, &music(44_100 * 2, 2, 44_100), 2, 44_100, &full_tags());
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(top_boxes(&bytes), ["ftyp", "moov", "mdat"]);
    assert_eq!(&bytes[8..12], b"M4A ");
    // The sink's temporary frames file is gone.
    let left: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
    assert_eq!(left.len(), 1, "{left:?}");
}

#[test]
fn m4a_keeps_tags_and_cover() {
    if common::no_mf() {
        return;
    }
    let (_d, dir) = tmp();
    let tags = full_tags();
    let path = write_m4a(&dir, &music(44_100, 2, 44_100), 2, 44_100, &tags);
    let s = sym::open(File::open(&path).unwrap(), "m4a").unwrap();
    assert_eq!(s.info().tags, tags);
    // And a JPEG cover is typed as one.
    let jpeg = Tags {
        picture: Some(Picture {
            mime: "image/jpeg".into(),
            data: vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, b'J', b'F', b'I', b'F', 0],
        }),
        ..Tags::default()
    };
    let path = write_m4a(&dir, &music(44_100, 2, 44_100), 2, 44_100, &jpeg);
    let s = sym::open(File::open(&path).unwrap(), "m4a").unwrap();
    assert_eq!(s.info().tags.picture, jpeg.picture);
}

#[test]
fn m4a_51_keeps_each_channel_where_it_was() {
    if common::no_mf() {
        return;
    }
    // A different tone in each channel, in Kiln's (WAVE) order.
    let (_d, dir) = tmp();
    let tones = [440.0, 660.0, 880.0, 55.0, 1100.0, 1320.0];
    let frames = 48_000 * 2;
    let mut src = vec![0f32; frames * 6];
    for (i, f) in src.chunks_mut(6).enumerate() {
        for (c, tone) in tones.iter().enumerate() {
            f[c] = (0.3 * (2.0 * PI * tone * i as f64 / 48_000.0).sin()) as f32;
        }
    }
    let path = write_m4a(&dir, &src, 6, 48_000, &Tags::default());
    let mut m = source::open(File::open(&path).unwrap()).unwrap();
    assert_eq!(m.info().channels.count, 6);
    let got = read_all(&mut *m);
    assert_each_tone_in_its_channel(&got, 48_000, &tones);
}

#[test]
fn m4a_51_size_matches_its_estimate() {
    if common::no_mf() {
        return;
    }
    // The bitrate tune offers for 5.1 is the total the file really has.
    use kiln_media::tune::{estimate_bytes, fit_under};
    use kiln_media::{SourceInfo, Target};
    let (_d, dir) = tmp();
    let secs = 20;
    let info = SourceInfo {
        codec: Codec::Flac,
        rate: 48_000,
        channels: channels_of(6),
        bits: Some(16),
        frames: Some(48_000 * secs),
        kbps: None,
        tags: Tags::default(),
    };
    let limit = 1_500_000;
    let setting = fit_under(Target::M4a, limit, &info).unwrap();
    let Setting::Kbps { kbps: k } = setting else {
        panic!("{setting:?}")
    };
    let estimate = estimate_bytes(Target::M4a, &setting, &info).unwrap();
    let path = dir.join("51.m4a");
    let enc = aac::encoder(48_000, 6, k).unwrap();
    let tags = Tags::default();
    let sink = mp4::m4a_sink(spec(&path, 48_000, 6, &tags, &setting), enc).unwrap();
    write_all(sink, &music(48_000 * secs as usize, 6, 48_000), 6, 48_000);
    let size = std::fs::metadata(&path).unwrap().len();
    assert!(size <= limit, "{k} kbps wrote {size} bytes, over {limit}");
    println!("5.1 at {k} kbps: {size} bytes, estimated {estimate}");
    let off = size as f64 / estimate as f64;
    assert!(
        (0.85..=1.05).contains(&off),
        "{k} kbps: {size} bytes vs {estimate} estimated"
    );
}

#[test]
fn adts_decodes() {
    if common::no_mf() {
        return;
    }
    let (_d, dir) = tmp();
    let path = dir.join("out.aac");
    let frames = 44_100 * 5;
    let setting = kbps(128);
    let tags = Tags::default();
    let enc = aac::encoder(44_100, 2, 128).unwrap();
    let sink = mp4::adts_sink(spec(&path, 44_100, 2, &tags, &setting), enc).unwrap();
    write_all(sink, &music(frames, 2, 44_100), 2, 44_100);
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[..2], &[0xFF, 0xF1]);
    let mut s = sym::open(File::open(&path).unwrap(), "aac").unwrap();
    assert_eq!(s.info().codec, Codec::Aac);
    assert_eq!((s.info().rate, s.info().channels.count), (44_100, 2));
    let got = read_all(&mut *s).len() / 2;
    assert!(got.abs_diff(frames) <= 3 * 1024, "{got} vs {frames}");
}

#[test]
fn aac_input_limits_are_encoder_errors() {
    if common::no_mf() {
        return;
    }
    assert!(matches!(
        aac::encoder(32_000, 2, 128),
        Err(MediaError::Encoder(_))
    ));
    assert!(matches!(
        aac::encoder(44_100, 8, 128),
        Err(MediaError::Encoder(_))
    ));
    assert!(matches!(
        aac::encoder(44_100, 2, 256),
        Err(MediaError::Encoder(_))
    ));
}

#[test]
fn samples_past_full_scale_clamp() {
    if common::no_mf() {
        return;
    }
    // Twice full scale must neither fail nor wrap round.
    let (_d, dir) = tmp();
    let loud: Vec<f32> = music(44_100, 2, 44_100).iter().map(|s| s * 8.0).collect();
    let path = write_m4a(&dir, &loud, 2, 44_100, &Tags::default());
    let mut s = sym::open(File::open(&path).unwrap(), "m4a").unwrap();
    let got = read_all(&mut *s);
    let peak = got.iter().fold(0f32, |m, s| m.max(s.abs()));
    assert!(peak > 0.5 && peak < 1.5, "peak {peak}");
}

// ---------------------------------------------------------------------------
// WMA

#[test]
fn wma_bitrates_include_128_for_stereo_44k() {
    let list = wma::bitrates(44_100, 2);
    assert!(list.contains(&128), "{list:?}");
    assert!(list.windows(2).all(|w| w[0] < w[1]), "{list:?}");
}

#[test]
fn wma_round_trip() {
    if common::no_mf() {
        return;
    }
    let (_d, dir) = tmp();
    let path = dir.join("out.wma");
    let frames = 44_100 * 30;
    let tags = Tags {
        // A title that really holds "; " comes back whole.
        title: Some("F\u{fc}\u{df}e; F\u{fc}\u{df}e \u{6771}\u{4eac}".into()),
        artist: Some("An Artist".into()),
        picture: full_tags().picture,
        ..Tags::default()
    };
    let setting = kbps(128);
    let sink = wma::sink(spec(&path, 44_100, 2, &tags, &setting)).unwrap();
    write_all(sink, &music(frames, 2, 44_100), 2, 44_100);

    let mut m = source::open(File::open(&path).unwrap()).unwrap();
    assert_eq!(m.info().codec, Codec::Wma);
    assert_eq!((m.info().rate, m.info().channels.count), (44_100, 2));
    assert_eq!(m.info().tags.title, tags.title);
    assert_eq!(m.info().tags.artist, tags.artist);
    assert_eq!(m.info().tags.picture, tags.picture);
    let k = m.info().kbps.unwrap();
    assert!((120..=136).contains(&k), "{k} kbps");
    let got = read_all(&mut *m).len() / 2;
    // One WMA packet at 44.1 kHz is 2,048 frames.
    assert!(got.abs_diff(frames) <= 2048, "{got} vs {frames}");
}

#[test]
fn wma_echo_title_stays_whole() {
    if common::no_mf() {
        return;
    }
    let (_d, dir) = tmp();
    let path = dir.join("echo.wma");
    let tags = Tags {
        title: Some("Echo; Echo".into()),
        ..Tags::default()
    };
    let setting = kbps(128);
    let sink = wma::sink(spec(&path, 44_100, 2, &tags, &setting)).unwrap();
    write_all(sink, &music(44_100, 2, 44_100), 2, 44_100);
    let m = source::open(File::open(&path).unwrap()).unwrap();
    assert_eq!(m.info().tags.title.as_deref(), Some("Echo; Echo"));
}

/// Writes nothing through `sink` and finishes it.
fn finish_empty(sink: Box<dyn Sink>) -> kiln_media::Result<()> {
    sink.finish()
}

/// Finishes `sink` with nothing written and returns the error's text.
fn empty_error(sink: Box<dyn Sink>) -> String {
    match finish_empty(sink) {
        Err(MediaError::Encoder(why)) => why,
        other => panic!("expected the no-audio error, got {other:?}"),
    }
}

#[test]
fn wma_zero_length_finishes() {
    if common::no_mf() {
        return;
    }
    // No audio is a clear Encoder error (Media Foundation would fail with
    // MF_E_SINK_NO_SAMPLES_PROCESSED); the caller deletes the partial output.
    let (_d, dir) = tmp();
    let setting = kbps(128);
    let tags = Tags::default();
    let sink = wma::sink(spec(&dir.join("empty.wma"), 44_100, 2, &tags, &setting)).unwrap();
    assert_eq!(empty_error(sink), "WMA can't be written with no audio.");
}

#[test]
fn m4a_aac_and_ac3_zero_length_fail_the_same_way() {
    if common::no_mf() {
        return;
    }
    let (_d, dir) = tmp();
    let tags = Tags::default();
    let setting = kbps(160);
    let enc = aac::encoder(44_100, 2, 160).unwrap();
    let sink = mp4::m4a_sink(spec(&dir.join("e.m4a"), 44_100, 2, &tags, &setting), enc).unwrap();
    assert_eq!(empty_error(sink), "M4A can't be written with no audio.");
    // The sink's temporary frames file is gone too.
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    let enc = aac::encoder(44_100, 2, 160).unwrap();
    let sink = mp4::adts_sink(spec(&dir.join("e.aac"), 44_100, 2, &tags, &setting), enc).unwrap();
    assert_eq!(empty_error(sink), "AAC can't be written with no audio.");
    let setting = kbps(256);
    let sink = ac3::sink(spec(&dir.join("e.ac3"), 48_000, 2, &tags, &setting)).unwrap();
    assert_eq!(empty_error(sink), "AC3 can't be written with no audio.");
}

#[test]
fn wma_fixture_reads_with_its_tags() {
    if common::no_mf() {
        return;
    }
    let mut m = source::open(File::open(data("tone.wma")).unwrap()).unwrap();
    assert_eq!(m.info().codec, Codec::Wma);
    assert_eq!((m.info().rate, m.info().channels.count), (44_100, 2));
    assert_eq!(m.info().tags.title.as_deref(), Some("Tone"));
    assert_eq!(m.info().tags.artist.as_deref(), Some("Kiln"));
    let got = read_all(&mut *m);
    assert!(
        (got.len() / 2).abs_diff(44_100 * 3) <= 2048,
        "{}",
        got.len() / 2
    );
    let left = channel(&got, 2, 0);
    assert!(power(&left, 44_100, 440.0) > 100.0 * power(&left, 44_100, 1000.0));
}

// ---------------------------------------------------------------------------
// AC3

#[test]
fn ac3_bitrates_are_what_windows_takes() {
    assert_eq!(
        ac3::bitrates(48_000, 1),
        [64, 80, 96, 112, 128, 160, 192, 224, 256]
    );
    assert_eq!(
        ac3::bitrates(48_000, 2),
        [128, 160, 192, 224, 256, 320, 384, 448]
    );
    assert!(ac3::bitrates(48_000, 6).is_empty());
}

#[test]
fn ac3_stereo_round_trip() {
    if common::no_mf() {
        return;
    }
    let (_d, dir) = tmp();
    let path = dir.join("out.ac3");
    let frames = 48_000 * 10;
    let setting = kbps(256);
    let tags = Tags::default();
    let sink = ac3::sink(spec(&path, 48_000, 2, &tags, &setting)).unwrap();
    write_all(sink, &music(frames, 2, 48_000), 2, 48_000);
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[..2], &[0x0B, 0x77]);
    // 256 kbps is 1,024 bytes per 1,536-frame AC3 frame.
    assert_eq!(bytes.len() % 1024, 0);

    let mut m = source::open(File::open(&path).unwrap()).unwrap();
    assert_eq!(m.info().codec, Codec::Ac3);
    assert_eq!((m.info().rate, m.info().channels.count), (48_000, 2));
    let got = read_all(&mut *m).len() / 2;
    assert!(got.abs_diff(frames) <= 1536, "{got} vs {frames}");
}

#[test]
fn ac3_mono_writes() {
    if common::no_mf() {
        return;
    }
    let (_d, dir) = tmp();
    let path = dir.join("mono.ac3");
    let setting = kbps(96);
    let tags = Tags::default();
    let sink = ac3::sink(spec(&path, 48_000, 1, &tags, &setting)).unwrap();
    write_all(sink, &music(48_000, 1, 48_000), 1, 48_000);
    let m = source::open(File::open(&path).unwrap()).unwrap();
    assert_eq!(m.info().channels.count, 1);
}

#[test]
fn ac3_51_round_trip() {
    if common::no_mf() {
        return;
    }
    let (_d, dir) = tmp();
    let path = dir.join("out51.ac3");
    let frames = 48_000 * 10;
    let setting = kbps(448);
    let tags = Tags::default();
    let sink = ac3::sink(spec(&path, 48_000, 6, &tags, &setting)).unwrap();
    write_all(sink, &music(frames, 6, 48_000), 6, 48_000);
    let mut m = source::open(File::open(&path).unwrap()).unwrap();
    assert_eq!(m.info().channels.count, 6);
    let got = read_all(&mut *m).len() / 6;
    // Kiln's surround encoder keeps every sample: 256 to 1,791 more (R27).
    assert!(
        got >= frames + 256 && got - frames <= 1536 + 256,
        "{got} vs {frames}"
    );
}

#[test]
fn ac3_surround_encodes_3_to_6_channels() {
    if common::no_mf() {
        return;
    }
    // Kiln's own encoder: each default layout decodes with its channels.
    let (_d, dir) = tmp();
    let setting = kbps(384);
    let tags = Tags::default();
    for channels in [3u16, 4, 5, 6] {
        let path = dir.join(format!("{channels}.ac3"));
        let frames = 48_000;
        let sink = ac3::sink(spec(&path, 48_000, channels, &tags, &setting)).unwrap();
        write_all(
            sink,
            &music(frames, channels as usize, 48_000),
            channels,
            48_000,
        );
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..2], &[0x0B, 0x77]);
        // 384 kbps is 1,536 bytes per syncframe.
        assert_eq!(bytes.len() % 1536, 0);
        let mut m = source::open(File::open(&path).unwrap()).unwrap();
        assert_eq!(m.info().codec, Codec::Ac3);
        assert_eq!(m.info().channels.count, channels, "{channels} ch");
        let got = read_all(&mut *m).len() / channels as usize;
        assert!(
            got >= frames + 256 && got - frames <= 1536 + 256,
            "{channels} ch: {got} vs {frames}"
        );
    }
    let r = ac3::sink(spec(&dir.join("71.ac3"), 48_000, 8, &tags, &setting));
    match r {
        Err(MediaError::Encoder(why)) => assert_eq!(why, "AC3 takes up to 5.1 channels."),
        Err(e) => panic!("expected the too-many-channels error, got {e:?}"),
        Ok(_) => panic!("expected the too-many-channels error, got a sink"),
    }
}

#[test]
fn ac3_input_limits_are_encoder_errors() {
    if common::no_mf() {
        return;
    }
    let (_d, dir) = tmp();
    let setting = kbps(256);
    let tags = Tags::default();
    let err = |rate, channels| {
        ac3::sink(spec(&dir.join("x.ac3"), rate, channels, &tags, &setting))
            .err()
            .unwrap()
    };
    assert!(matches!(err(44_100, 2), MediaError::Encoder(_)));
    assert!(matches!(err(44_100, 6), MediaError::Encoder(_)));
    assert!(matches!(err(48_000, 7), MediaError::Encoder(_)));
    // A bitrate Windows won't take for stereo.
    let setting = kbps(640);
    let r = ac3::sink(spec(&dir.join("y.ac3"), 48_000, 2, &tags, &setting));
    assert!(matches!(r, Err(MediaError::Encoder(_))));
}

fn assert_each_tone_in_its_channel(got: &[f32], rate: u32, tones: &[f64]) {
    let count = tones.len();
    for (c, &tone) in tones.iter().enumerate() {
        let x = channel(got, count, c);
        let mine = power(&x, rate, tone);
        for (o, &other) in tones.iter().enumerate() {
            if o != c {
                let theirs = power(&x, rate, other);
                assert!(
                    mine > 30.0 * theirs,
                    "channel {c}: its own {tone} Hz {mine:.2e}, channel {o}'s {other} Hz {theirs:.2e}"
                );
            }
        }
    }
}

#[test]
fn ac3_51_fixture_reads_in_wave_order() {
    if common::no_mf() {
        return;
    }
    // tone51.ac3 holds FL 440, FR 660, FC 880, LFE 55, BL 1100, BR 1320 Hz.
    let mut m = source::open(File::open(data("tone51.ac3")).unwrap()).unwrap();
    assert_eq!(m.info().codec, Codec::Ac3);
    assert_eq!((m.info().rate, m.info().channels.count), (48_000, 6));
    let mask = m.info().channels.mask;
    assert!(mask == 0x3F || mask == 0x60F, "mask {mask:#x}");
    let got = read_all(&mut *m);
    assert!(
        (got.len() / 6).abs_diff(48_000) <= 1536,
        "{}",
        got.len() / 6
    );
    assert_each_tone_in_its_channel(&got, 48_000, &[440.0, 660.0, 880.0, 55.0, 1100.0, 1320.0]);
}

#[test]
fn ac3_fixture_reads() {
    if common::no_mf() {
        return;
    }
    let mut m = source::open(File::open(data("tone.ac3")).unwrap()).unwrap();
    assert_eq!(m.info().codec, Codec::Ac3);
    assert_eq!((m.info().rate, m.info().channels.count), (48_000, 2));
    assert_eq!(m.info().kbps, Some(192));
    let got = read_all(&mut *m);
    assert!(
        (got.len() / 2).abs_diff(48_000) <= 1536,
        "{}",
        got.len() / 2
    );
}

// ---------------------------------------------------------------------------
// HE-AAC

#[test]
#[ignore = "no local HE-AAC encoder; fixture needs owner download"]
fn heaac_reads_through_media_foundation() {
    let path = data("he-aac.m4a");
    match sym::open(File::open(&path).unwrap(), "m4a") {
        Err(MediaError::Unsupported { codec }) => assert_eq!(codec, "HE-AAC"),
        Err(e) => panic!("expected Unsupported, got {e:?}"),
        Ok(s) => assert_eq!(s.info().codec, Codec::HeAac),
    }
    let mut m = source::open(File::open(&path).unwrap()).unwrap();
    assert_eq!(m.info().codec, Codec::HeAac);
    assert!(!read_all(&mut *m).is_empty());
}

#[test]
fn media_foundation_reports_byte_progress() {
    if common::no_mf() {
        return;
    }
    // Progress by bytes reaches the whole file by the end.
    let (_d, dir) = tmp();
    let from = data("tone.ac3");
    let path = dir.join("copy.ac3");
    std::fs::copy(&from, &path).unwrap();
    let file = File::open(&path).unwrap();
    let mut m = source::open(file).unwrap();
    assert!(!read_all(&mut *m).is_empty());
    assert!(m.position().total_bytes > 0);
    assert_eq!(m.position().bytes, m.position().total_bytes);
}
