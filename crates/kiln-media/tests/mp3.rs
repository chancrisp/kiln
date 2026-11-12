//! MP3 written through `lame_enc.dll`, read back with Symphonia.
//!
//! The DLL sits next to the test binary: kiln-media has kiln-lame as a
//! dev-dependency, so cargo builds it into `target/<profile>/deps`.

use kiln_media::mp3;
use kiln_media::{
    sym, Block, Channels, MediaError, Picture, Process, Setting, SinkSpec, Source, Tags,
};
use std::f64::consts::PI;
use std::fs::File;
use std::path::{Path, PathBuf};

const BLOCK: usize = 8192;
const FRAME: u64 = 1152;

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
            3 => 0x7,
            6 => 0x3F,
            8 => 0x63F,
            _ => 0,
        },
    }
}

/// Something that reads like music to an encoder: a few unrelated tones that
/// swell and fade, with a little noise on top. Deterministic.
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
                + 0.07 * (1.0 - swell) * (2.0 * PI * 3135.9 * f * t).sin()
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

fn empty_block() -> Block {
    Block {
        channels: 0,
        rate: 0,
        samples: Vec::new(),
    }
}

fn read_all(src: &mut dyn Source) -> kiln_media::Result<Vec<f32>> {
    let mut out = Vec::new();
    let mut b = empty_block();
    while src.read(&mut b)? {
        out.extend_from_slice(&b.samples);
    }
    Ok(out)
}

fn spec<'a>(
    path: &Path,
    rate: u32,
    channels: Channels,
    tags: &'a Tags,
    setting: &'a Setting,
) -> SinkSpec<'a> {
    SinkSpec {
        rate,
        channels,
        tags,
        setting,
        source_bits: None,
        out: File::create(path).unwrap(),
    }
}

/// Encodes `samples` and returns the file's path and bytes.
fn encode(
    dir: &Path,
    samples: &[f32],
    channels: u16,
    rate: u32,
    tags: &Tags,
    setting: &Setting,
) -> (PathBuf, Vec<u8>) {
    let path = dir.join("out.mp3");
    let mut sink = mp3::sink(spec(&path, rate, channels_of(channels), tags, setting)).unwrap();
    for b in blocks(samples, channels, rate) {
        sink.write(&b).unwrap();
    }
    sink.finish().unwrap();
    let bytes = std::fs::read(&path).unwrap();
    (path, bytes)
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

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Where the audio starts: after an ID3v2 tag, if there is one.
fn audio_start(bytes: &[u8]) -> usize {
    if bytes.starts_with(b"ID3") {
        let size = bytes[6..10]
            .iter()
            .fold(0usize, |n, b| (n << 7) | (*b as usize & 0x7F));
        10 + size
    } else {
        0
    }
}

const V2: Setting = Setting::LameVbr { tenths: 20 };

// ---------------------------------------------------------------------------
// The six from the plan

#[test]
fn v2_decodes_and_keeps_length() {
    // 30 s of 44.1 kHz stereo.
    let frames = 44_100 * 30;
    let samples = music(frames, 2, 44100);
    let dir = tempfile::tempdir().unwrap();
    let (path, _) = encode(dir.path(), &samples, 2, 44100, &Tags::default(), &V2);

    let mut back = sym::open(File::open(&path).unwrap(), "mp3").unwrap();
    assert_eq!(back.info().rate, 44100);
    assert_eq!(back.info().channels.count, 2);
    let stated = back.info().frames;
    let got = read_all(&mut *back).unwrap();
    let decoded = (got.len() / 2) as u64;
    // The LAME tag states the encoder delay and padding, so with gapless
    // decoding the length is the source's to within one MP3 frame.
    assert!(
        decoded.abs_diff(frames as u64) <= FRAME,
        "decoded {decoded} frames, source {frames}"
    );
    assert!(
        stated.unwrap().abs_diff(frames as u64) <= FRAME,
        "the file states {stated:?} frames"
    );
}

#[test]
fn has_a_lame_tag() {
    let frames = 44_100 * 30;
    let samples = music(frames, 2, 44100);
    let dir = tempfile::tempdir().unwrap();
    let tags = Tags {
        title: Some("A title".into()),
        ..Tags::default()
    };
    let (_, bytes) = encode(dir.path(), &samples, 2, 44100, &tags, &V2);

    let start = audio_start(&bytes);
    assert!(start > 10, "no ID3 tag in front");
    // A frame header: sync, then MPEG-1 Layer III.
    assert_eq!(bytes[start], 0xFF);
    assert_eq!(bytes[start + 1] & 0xFE, 0xFA);
    let first = &bytes[start..start + 400];
    // MPEG-1 stereo: the tag sits after the 4-byte header and 32 bytes of side info.
    let at = find(first, b"Xing").expect("a Xing tag in a VBR file");
    assert_eq!(at, 36);
    assert!(find(first, b"LAME").is_some(), "no LAME in the tag");
    // The tag was rewritten at the end: it counts the frames.
    let flags = u32::from_be_bytes(first[at + 4..at + 8].try_into().unwrap());
    assert_eq!(flags & 1, 1, "the tag states no frame count");
    let counted = u32::from_be_bytes(first[at + 8..at + 12].try_into().unwrap()) as u64;
    let expected = (frames as u64).div_ceil(FRAME);
    assert!(
        counted.abs_diff(expected) <= 3,
        "the tag counts {counted} frames, expected about {expected}"
    );
}

#[test]
fn cbr_is_labelled_info() {
    let samples = music(44_100 * 3, 2, 44100);
    let dir = tempfile::tempdir().unwrap();
    let (_, bytes) = encode(
        dir.path(),
        &samples,
        2,
        44100,
        &Tags::default(),
        &Setting::LameCbr { kbps: 128 },
    );
    assert_eq!(audio_start(&bytes), 0);
    let first = &bytes[..400];
    assert_eq!(find(first, b"Info"), Some(36));
    assert!(find(first, b"LAME").is_some());
}

#[test]
fn cbr_320_is_320() {
    let samples = music(44_100 * 30, 2, 44100);
    let dir = tempfile::tempdir().unwrap();
    let (_, bytes) = encode(
        dir.path(),
        &samples,
        2,
        44100,
        &Tags::default(),
        &Setting::LameCbr { kbps: 320 },
    );
    let want = 320_000.0 * 30.0 / 8.0;
    let got = bytes.len() as f64;
    assert!(
        (got - want).abs() <= want * 0.02,
        "{got} bytes, expected about {want}"
    );
}

#[test]
fn keeps_tags_and_cover() {
    let samples = music(44_100 * 2, 2, 44100);
    let dir = tempfile::tempdir().unwrap();
    let tags = full_tags();
    let (path, bytes) = encode(dir.path(), &samples, 2, 44100, &tags, &V2);
    assert!(bytes.starts_with(b"ID3\x04"));
    let back = sym::open(File::open(&path).unwrap(), "mp3").unwrap();
    assert_eq!(back.info().tags, tags);
}

#[test]
fn the_dll_cargo_built_loads() {
    // With lame_enc.dll where cargo built it, the sink starts. A missing DLL
    // is checked in the crate itself (`mp3::tests::missing_dll_is_lame_missing`),
    // where the override that points the loader at another folder is `cfg(test)`.
    let dir = tempfile::tempdir().unwrap();
    let tags = Tags::default();
    let path = dir.path().join("a.mp3");
    let ok = mp3::sink(spec(&path, 44100, channels_of(2), &tags, &V2));
    assert!(ok.is_ok(), "{:?}", ok.err());
}

#[test]
fn surround_and_192k_still_encode() {
    // 7.1 at 192 kHz, brought to what the sink takes: Task 11's convert() will
    // build this same Process.
    let rate = 192_000u32;
    let frames = rate as usize * 2;
    let samples = music(frames, 8, rate);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.mp3");
    let tags = Tags::default();
    let mut sink = mp3::sink(spec(&path, 48_000, channels_of(2), &tags, &V2)).unwrap();
    let mut process = Process::new(rate, channels_of(8), 48_000, channels_of(2), None).unwrap();
    let mut converted = empty_block();
    for b in blocks(&samples, 8, rate) {
        process.run(&b, &mut converted);
        if converted.frames() > 0 {
            sink.write(&converted).unwrap();
        }
    }
    process.flush(&mut converted);
    if converted.frames() > 0 {
        sink.write(&converted).unwrap();
    }
    sink.finish().unwrap();

    let mut back = sym::open(File::open(&path).unwrap(), "mp3").unwrap();
    assert_eq!(back.info().rate, 48_000);
    assert_eq!(back.info().channels.count, 2);
    let got = read_all(&mut *back).unwrap();
    let decoded = (got.len() / 2) as u64;
    assert!(
        decoded.abs_diff(96_000) <= FRAME,
        "decoded {decoded} frames, expected about 96,000"
    );
    let peak = got.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(peak > 0.1, "the MP3 is silent");
}

// ---------------------------------------------------------------------------
// Beyond the plan

#[test]
fn the_sink_takes_two_channels_and_48k_at_most() {
    let dir = tempfile::tempdir().unwrap();
    let tags = Tags::default();
    let path = dir.path().join("a.mp3");
    for (rate, count) in [(96_000u32, 2u16), (192_000, 1), (48_000, 3), (44_100, 6)] {
        let r = mp3::sink(spec(&path, rate, channels_of(count), &tags, &V2));
        assert!(
            matches!(r, Err(MediaError::Encoder(_))),
            "{rate} Hz, {count} channels was accepted"
        );
    }
    let wrong = Setting::Kbps { kbps: 128 };
    let r = mp3::sink(spec(&path, 44100, channels_of(2), &tags, &wrong));
    assert!(matches!(r, Err(MediaError::Encoder(_))));
}

#[test]
fn mono_keeps_its_audio() {
    let samples = music(44_100 * 3, 1, 44100);
    let dir = tempfile::tempdir().unwrap();
    let (path, _) = encode(dir.path(), &samples, 1, 44100, &Tags::default(), &V2);
    let mut back = sym::open(File::open(&path).unwrap(), "mp3").unwrap();
    assert_eq!(back.info().channels.count, 1);
    let got = read_all(&mut *back).unwrap();
    assert!((got.len() as u64).abs_diff(44_100 * 3) <= FRAME);
    // Not every other sample, which is what reading mono as stereo would do.
    let rms =
        |v: &[f32]| (v.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / v.len() as f64).sqrt();
    let want = rms(&samples);
    let have = rms(&got);
    assert!((have / want - 1.0).abs() < 0.1, "rms {have} against {want}");
}

#[test]
fn abr_lands_near_its_bitrate() {
    let samples = music(44_100 * 20, 2, 44100);
    let dir = tempfile::tempdir().unwrap();
    let (_, bytes) = encode(
        dir.path(),
        &samples,
        2,
        44100,
        &Tags::default(),
        &Setting::LameAbr { kbps: 128 },
    );
    let kbps = bytes.len() as f64 * 8.0 / 20.0 / 1000.0;
    assert!((kbps - 128.0).abs() <= 128.0 * 0.15, "{kbps} kbps");
}

#[test]
fn no_tags_means_no_id3() {
    let samples = music(44_100, 2, 44100);
    let dir = tempfile::tempdir().unwrap();
    let (_, bytes) = encode(dir.path(), &samples, 2, 44100, &Tags::default(), &V2);
    assert_eq!(bytes[0], 0xFF, "the file does not start with a frame");
    assert_eq!(bytes[1] & 0xE0, 0xE0);
}

#[test]
fn loud_and_broken_samples_are_clamped() {
    // Past full scale, and not numbers at all: neither may wrap or fail.
    let mut samples = music(44_100, 2, 44100);
    for (i, s) in samples.iter_mut().enumerate() {
        match i % 4000 {
            0 => *s = 5.0,
            1 => *s = -5.0,
            2 => *s = f32::NAN,
            3 => *s = f32::INFINITY,
            _ => {}
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let (path, _) = encode(dir.path(), &samples, 2, 44100, &Tags::default(), &V2);
    let mut back = sym::open(File::open(&path).unwrap(), "mp3").unwrap();
    let got = read_all(&mut *back).unwrap();
    assert!(got.iter().all(|s| s.is_finite()));
}

#[test]
fn an_empty_conversion_still_finishes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.mp3");
    let tags = Tags::default();
    let sink = mp3::sink(spec(&path, 44100, channels_of(2), &tags, &V2)).unwrap();
    sink.finish().unwrap();
    assert!(path.exists());
}

#[test]
fn rates_lame_accepts_all_encode() {
    let dir = tempfile::tempdir().unwrap();
    for rate in [8_000u32, 22_050, 32_000, 48_000] {
        let samples = music(rate as usize, 2, rate);
        let (path, _) = encode(dir.path(), &samples, 2, rate, &Tags::default(), &V2);
        let back = sym::open(File::open(&path).unwrap(), "mp3").unwrap();
        assert_eq!(back.info().rate, rate);
    }
}
