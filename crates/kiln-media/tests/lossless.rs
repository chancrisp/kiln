//! FLAC and WavPack, written and read back. Lossless paths must be bit-exact.

use kiln_media::pcm::wav;
use kiln_media::tags::vorbis;
use kiln_media::{
    flac, sym, wavpack, Block, Channels, Codec, MediaError, Picture, Setting, Sink, SinkSpec,
    Source, Tags,
};
use std::fs::File;
use std::path::{Path, PathBuf};

const BLOCK: usize = 8192;

fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join(name)
}

/// Integer samples over the whole range (both extremes included), as the f32
/// a decoder would give: `v / 2^(bits-1)`.
fn sweep(bits: u32, count: usize) -> Vec<f32> {
    let scale = (1u64 << (bits - 1)) as f64;
    let max = scale as i64 - 1;
    let min = -(scale as i64);
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    (0..count)
        .map(|i| {
            let v = match i {
                0 => min,
                1 => max,
                2 => 0,
                3 => -1,
                _ => {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    (state % (max - min + 1) as u64) as i64 + min
                }
            };
            (v as f64 / scale) as f32
        })
        .collect()
}

/// A tone with a little noise: something a lossless coder can shrink.
fn tone(bits: u32, frames: usize, channels: usize) -> Vec<f32> {
    let scale = (1u64 << (bits - 1)) as f64;
    let mut state = 12345u64;
    let mut out = Vec::with_capacity(frames * channels);
    for i in 0..frames {
        for c in 0..channels {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let noise = ((state >> 40) % 64) as f64 - 32.0;
            let t = i as f64 / 44100.0;
            let v = (0.5
                * (2.0 * std::f64::consts::PI * (440.0 + 110.0 * c as f64) * t).sin()
                * (scale - 1.0)
                + noise)
                .round();
            out.push((v / scale) as f32);
        }
    }
    out
}

fn channels_of(count: u16) -> Channels {
    Channels {
        count,
        mask: match count {
            1 => 0x4,
            2 => 0x3,
            8 => 0x63F,
            _ => 0,
        },
    }
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

fn read_all(src: &mut dyn Source) -> kiln_media::Result<Vec<f32>> {
    let mut out = Vec::new();
    let mut b = Block {
        channels: 0,
        rate: 0,
        samples: Vec::new(),
    };
    while src.read(&mut b)? {
        assert!(b.frames() > 0 && b.frames() <= BLOCK);
        assert_eq!(b.channels, src.info().channels.count);
        out.extend_from_slice(&b.samples);
    }
    assert_eq!(b.frames(), 0, "the final read left samples in the block");
    Ok(out)
}

fn spec<'a>(
    path: &Path,
    rate: u32,
    channels: Channels,
    tags: &'a Tags,
    setting: &'a Setting,
    source_bits: Option<u16>,
) -> SinkSpec<'a> {
    SinkSpec {
        rate,
        channels,
        tags,
        setting,
        source_bits,
        out: File::create(path).unwrap(),
    }
}

fn write_with(mut sink: Box<dyn Sink>, samples: &[f32], channels: u16, rate: u32) {
    for b in blocks(samples, channels, rate) {
        sink.write(&b).unwrap();
    }
    sink.finish().unwrap();
}

fn write_wav(path: &Path, bits: u16, rate: u32, channels: Channels, samples: &[f32]) {
    let tags = Tags::default();
    let setting = Setting::Pcm { bits, float: false };
    let s = wav::sink(spec(path, rate, channels, &tags, &setting, None)).unwrap();
    write_with(s, samples, channels.count, rate);
}

fn flac_level(level: u8) -> Setting {
    Setting::FlacLevel { level, bits: None }
}

fn wv_mode(mode: u8) -> Setting {
    Setting::WavPack { mode, bits: None }
}

/// Converts a WAV file the way a conversion would: read it, hand its blocks
/// and its bit depth to the sink.
fn convert(
    wav_path: &Path,
    out: &Path,
    tags: &Tags,
    setting: &Setting,
    make: fn(SinkSpec) -> kiln_media::Result<Box<dyn Sink>>,
) {
    let mut src = wav::open(File::open(wav_path).unwrap()).unwrap();
    let info = src.info().clone();
    let mut sink = make(spec(
        out,
        info.rate,
        info.channels,
        tags,
        setting,
        info.bits,
    ))
    .unwrap();
    let mut b = Block {
        channels: 0,
        rate: 0,
        samples: Vec::new(),
    };
    while src.read(&mut b).unwrap() {
        sink.write(&b).unwrap();
    }
    sink.finish().unwrap();
}

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

struct Case {
    name: &'static str,
    bits: u16,
    rate: u32,
    channels: u16,
    frames: usize,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "16-bit stereo 44.1k",
            bits: 16,
            rate: 44100,
            channels: 2,
            frames: 20_000,
        },
        Case {
            name: "24-bit 7.1 96k",
            bits: 24,
            rate: 96000,
            channels: 8,
            frames: 20_000,
        },
        Case {
            name: "24-bit mono 192k",
            bits: 24,
            rate: 192000,
            channels: 1,
            frames: 20_000,
        },
        Case {
            name: "16-bit mono 8k",
            bits: 16,
            rate: 8000,
            channels: 1,
            frames: 20_000,
        },
    ]
}

#[test]
fn flac_round_trip_is_bit_exact() {
    for c in cases() {
        let dir = tmp();
        let (wav_path, out) = (dir.path().join("in.wav"), dir.path().join("out.flac"));
        let samples = sweep(c.bits as u32, c.frames * c.channels as usize);
        write_wav(&wav_path, c.bits, c.rate, channels_of(c.channels), &samples);
        convert(
            &wav_path,
            &out,
            &Tags::default(),
            &flac_level(5),
            flac::sink,
        );

        let mut back = sym::open(File::open(&out).unwrap(), "flac").unwrap();
        assert_eq!(back.info().codec, Codec::Flac, "{}", c.name);
        assert_eq!(back.info().rate, c.rate, "{}", c.name);
        assert_eq!(back.info().channels.count, c.channels, "{}", c.name);
        assert_eq!(back.info().bits, Some(c.bits), "{}", c.name);
        assert_eq!(back.info().frames, Some(c.frames as u64), "{}", c.name);
        let got = read_all(&mut *back).unwrap();
        assert!(got == samples, "{}: samples differ", c.name);
    }
}

#[test]
fn wavpack_round_trip_is_bit_exact() {
    for c in cases() {
        let dir = tmp();
        let (wav_path, out) = (dir.path().join("in.wav"), dir.path().join("out.wv"));
        let samples = sweep(c.bits as u32, c.frames * c.channels as usize);
        write_wav(&wav_path, c.bits, c.rate, channels_of(c.channels), &samples);
        convert(
            &wav_path,
            &out,
            &Tags::default(),
            &wv_mode(1),
            wavpack::sink,
        );

        let mut back = wavpack::open(File::open(&out).unwrap()).unwrap();
        let info = back.info().clone();
        assert_eq!(info.codec, Codec::WavPack, "{}", c.name);
        assert_eq!(info.rate, c.rate, "{}", c.name);
        assert_eq!(info.channels.count, c.channels, "{}", c.name);
        assert_eq!(info.bits, Some(c.bits), "{}", c.name);
        assert_eq!(info.frames, Some(c.frames as u64), "{}", c.name);
        assert!(info.kbps.is_some(), "{}", c.name);
        if c.channels == 8 {
            assert_eq!(info.channels.mask, 0x63F);
        }
        let got = read_all(&mut *back).unwrap();
        assert!(got == samples, "{}: samples differ", c.name);
        let at = back.position();
        assert_eq!(at.frames, c.frames as u64);
        assert!(at.bytes > 0 && at.bytes <= at.total_bytes);
    }
}

#[test]
fn wavpack_modes_are_all_lossless() {
    let samples = tone(16, 30_000, 2);
    for mode in 0..=3u8 {
        let dir = tmp();
        let path = dir.path().join("a.wv");
        let s = wavpack::sink(spec(
            &path,
            44100,
            channels_of(2),
            &Tags::default(),
            &wv_mode(mode),
            Some(16),
        ))
        .unwrap();
        write_with(s, &samples, 2, 44100);
        let got = read_all(&mut *wavpack::open(File::open(&path).unwrap()).unwrap()).unwrap();
        assert!(got == samples, "mode {mode}");
    }
}

#[test]
fn flac_levels_change_size_not_sound() {
    let samples = tone(16, 3 * 44100, 2);
    let dir = tmp();
    let mut sizes = Vec::new();
    for level in [0u8, 8] {
        let path = dir.path().join(format!("l{level}.flac"));
        let setting = flac_level(level);
        let s = flac::sink(spec(
            &path,
            44100,
            channels_of(2),
            &Tags::default(),
            &setting,
            Some(16),
        ))
        .unwrap();
        write_with(s, &samples, 2, 44100);
        sizes.push(std::fs::metadata(&path).unwrap().len());
        let got = read_all(&mut *sym::open(File::open(&path).unwrap(), "flac").unwrap()).unwrap();
        assert!(got == samples, "level {level}");
    }
    assert!(
        sizes[1] <= sizes[0],
        "level 8 {} > level 0 {}",
        sizes[1],
        sizes[0]
    );
    assert!(sizes[0] < samples.len() as u64 * 2, "FLAC didn't compress");
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

/// The body of the first metadata block of `kind` in a FLAC file.
fn flac_block(bytes: &[u8], kind: u8) -> Option<Vec<u8>> {
    assert_eq!(&bytes[..4], b"fLaC");
    let mut at = 4;
    loop {
        let head = bytes[at];
        let len = u32::from_be_bytes([0, bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize;
        let body = &bytes[at + 4..at + 4 + len];
        if head & 0x7F == kind {
            return Some(body.to_vec());
        }
        if head & 0x80 != 0 {
            return None;
        }
        at += 4 + len;
    }
}

#[test]
fn flac_keeps_tags_and_cover() {
    let dir = tmp();
    let path = dir.path().join("t.flac");
    let tags = full_tags();
    let setting = flac_level(5);
    let samples = tone(16, 5000, 2);
    let s = flac::sink(spec(
        &path,
        44100,
        channels_of(2),
        &tags,
        &setting,
        Some(16),
    ))
    .unwrap();
    write_with(s, &samples, 2, 44100);

    let back = sym::open(File::open(&path).unwrap(), "flac").unwrap();
    assert_eq!(back.info().tags, tags);

    // The PICTURE block is the layout Task 8 reuses for Ogg.
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(
        flac_block(&bytes, 6).unwrap(),
        vorbis::picture_block(tags.picture.as_ref().unwrap())
    );
}

#[test]
fn flac_with_no_tags_has_no_comment_block() {
    let dir = tmp();
    let path = dir.path().join("t.flac");
    let setting = flac_level(5);
    let s = flac::sink(spec(
        &path,
        44100,
        channels_of(2),
        &Tags::default(),
        &setting,
        None,
    ))
    .unwrap();
    write_with(s, &tone(16, 1000, 2), 2, 44100);
    let back = sym::open(File::open(&path).unwrap(), "flac").unwrap();
    assert_eq!(back.info().tags, Tags::default());
}

#[test]
fn flac_names_an_unusual_speaker_layout() {
    let dir = tmp();
    let path = dir.path().join("t.flac");
    let setting = flac_level(5);
    let odd = Channels {
        count: 6,
        mask: 0x60F, // 5.1 with side speakers
    };
    let s = flac::sink(spec(
        &path,
        48000,
        odd,
        &Tags::default(),
        &setting,
        Some(16),
    ))
    .unwrap();
    write_with(s, &tone(16, 1000, 6), 6, 48000);
    let bytes = std::fs::read(&path).unwrap();
    let comments = flac_block(&bytes, 4).unwrap();
    let text = String::from_utf8_lossy(&comments);
    assert!(
        text.contains("WAVEFORMATEXTENSIBLE_CHANNEL_MASK=0x60F"),
        "{text}"
    );
}

#[test]
fn wavpack_keeps_tags() {
    let dir = tmp();
    let path = dir.path().join("t.wv");
    let tags = full_tags();
    let samples = tone(16, 5000, 2);
    let s = wavpack::sink(spec(
        &path,
        44100,
        channels_of(2),
        &tags,
        &wv_mode(1),
        Some(16),
    ))
    .unwrap();
    write_with(s, &samples, 2, 44100);

    let mut back = wavpack::open(File::open(&path).unwrap()).unwrap();
    assert_eq!(back.info().tags, tags);
    // The tag sits after the audio and doesn't get in its way.
    assert!(read_all(&mut *back).unwrap() == samples);
    let bytes = std::fs::read(&path).unwrap();
    assert!(bytes.windows(8).any(|w| w == b"APETAGEX"));
    assert_eq!(&bytes[..4], b"wvpk");
}

#[test]
fn bit_depth_follows_the_setting_then_the_source() {
    // The depth in a FLAC's STREAMINFO.
    fn flac_bits(path: &Path) -> u16 {
        let b = std::fs::read(path).unwrap();
        let info = flac_block(&b, 0).unwrap();
        ((((info[12] & 1) as u16) << 4) | (info[13] >> 4) as u16) + 1
    }
    let dir = tmp();
    let samples = sweep(16, 2000);
    let cases: [(Option<u16>, Option<u16>, u16); 6] = [
        (None, Some(24), 24),
        (None, Some(16), 16),
        (None, None, 16),         // unknown, or a float source
        (None, Some(32), 24),     // capped
        (Some(24), Some(16), 24), // the setting wins
        (Some(16), Some(24), 16),
    ];
    for (i, (setting_bits, source_bits, want)) in cases.into_iter().enumerate() {
        let f = dir.path().join(format!("{i}.flac"));
        let setting = Setting::FlacLevel {
            level: 5,
            bits: setting_bits,
        };
        let s = flac::sink(spec(
            &f,
            44100,
            channels_of(1),
            &Tags::default(),
            &setting,
            source_bits,
        ))
        .unwrap();
        write_with(s, &samples, 1, 44100);
        assert_eq!(flac_bits(&f), want, "flac {i}");

        let w = dir.path().join(format!("{i}.wv"));
        let setting = Setting::WavPack {
            mode: 1,
            bits: setting_bits,
        };
        let s = wavpack::sink(spec(
            &w,
            44100,
            channels_of(1),
            &Tags::default(),
            &setting,
            source_bits,
        ))
        .unwrap();
        write_with(s, &samples, 1, 44100);
        let back = wavpack::open(File::open(&w).unwrap()).unwrap();
        assert_eq!(back.info().bits, Some(want), "wavpack {i}");
    }
}

#[test]
fn samples_past_full_scale_clamp() {
    let loud = vec![2.0f32, -2.0, f32::INFINITY, f32::NEG_INFINITY, 0.5, -0.5];
    let want = [32767.0 / 32768.0, -1.0, 32767.0 / 32768.0, -1.0, 0.5, -0.5];
    let dir = tmp();
    let f = dir.path().join("c.flac");
    let w = dir.path().join("c.wv");
    let s = flac::sink(spec(
        &f,
        44100,
        channels_of(1),
        &Tags::default(),
        &flac_level(5),
        Some(16),
    ))
    .unwrap();
    write_with(s, &loud, 1, 44100);
    let got = read_all(&mut *sym::open(File::open(&f).unwrap(), "flac").unwrap()).unwrap();
    assert_eq!(got, want);
    let s = wavpack::sink(spec(
        &w,
        44100,
        channels_of(1),
        &Tags::default(),
        &wv_mode(1),
        Some(16),
    ))
    .unwrap();
    write_with(s, &loud, 1, 44100);
    let got = read_all(&mut *wavpack::open(File::open(&w).unwrap()).unwrap()).unwrap();
    assert_eq!(got, want);
}

#[test]
fn flac_takes_up_to_eight_channels() {
    let dir = tmp();
    let setting = flac_level(5);
    let nine = Channels { count: 9, mask: 0 };
    let r = flac::sink(spec(
        &dir.path().join("n.flac"),
        44100,
        nine,
        &Tags::default(),
        &setting,
        None,
    ));
    assert!(matches!(r, Err(MediaError::Encoder(_))));
}

#[test]
fn wrong_setting_is_refused() {
    let dir = tmp();
    let pcm = Setting::Pcm {
        bits: 16,
        float: false,
    };
    let r = flac::sink(spec(
        &dir.path().join("a.flac"),
        44100,
        channels_of(2),
        &Tags::default(),
        &pcm,
        None,
    ));
    assert!(matches!(r, Err(MediaError::Encoder(_))));
    let r = wavpack::sink(spec(
        &dir.path().join("a.wv"),
        44100,
        channels_of(2),
        &Tags::default(),
        &pcm,
        None,
    ));
    assert!(matches!(r, Err(MediaError::Encoder(_))));
}

#[test]
fn an_unfinished_sink_can_be_dropped() {
    let dir = tmp();
    let samples = tone(16, 20_000, 2);
    let setting = flac_level(5);
    let mut s = flac::sink(spec(
        &dir.path().join("a.flac"),
        44100,
        channels_of(2),
        &Tags::default(),
        &setting,
        None,
    ))
    .unwrap();
    s.write(&blocks(&samples, 2, 44100).next().unwrap())
        .unwrap();
    drop(s);
    let mut s = wavpack::sink(spec(
        &dir.path().join("a.wv"),
        44100,
        channels_of(2),
        &Tags::default(),
        &wv_mode(1),
        None,
    ))
    .unwrap();
    s.write(&blocks(&samples, 2, 44100).next().unwrap())
        .unwrap();
    drop(s);
}

#[test]
fn an_empty_conversion_still_makes_a_file() {
    let dir = tmp();
    let f = dir.path().join("e.flac");
    let s = flac::sink(spec(
        &f,
        44100,
        channels_of(2),
        &Tags::default(),
        &flac_level(5),
        None,
    ))
    .unwrap();
    s.finish().unwrap();
    // STREAMINFO: a 20-bit rate, then 3 + 5 bits, then a 36-bit sample count.
    let bytes = std::fs::read(&f).unwrap();
    let info = flac_block(&bytes, 0).unwrap();
    let rate = (info[10] as u32) << 12 | (info[11] as u32) << 4 | (info[12] as u32) >> 4;
    let samples = u64::from_be_bytes(info[10..18].try_into().unwrap()) & 0xF_FFFF_FFFF;
    assert_eq!((rate, samples), (44100, 0));
}

#[test]
fn wavpack_reads_a_file_another_encoder_wrote() {
    // tests/data/tone.wv comes from FFmpeg: 1 s of a 440 Hz tone, same in both channels.
    let mut src = wavpack::open(File::open(data("tone.wv")).unwrap()).unwrap();
    let info = src.info().clone();
    assert_eq!(info.codec, Codec::WavPack);
    assert_eq!((info.rate, info.channels.count), (44100, 2));
    assert_eq!(info.frames, Some(44100));
    let got = read_all(&mut *src).unwrap();
    assert_eq!(got.len(), 44100 * 2);
    assert!(got.iter().any(|&s| s.abs() > 0.05));
    assert!(got.chunks(2).all(|f| f[0] == f[1]));
}

#[test]
fn damaged_wavpack_is_reported_as_damaged() {
    let dir = tmp();
    let path = dir.path().join("d.wv");
    let samples = sweep(16, 40_000);
    let s = wavpack::sink(spec(
        &path,
        44100,
        channels_of(1),
        &Tags::default(),
        &wv_mode(1),
        Some(16),
    ))
    .unwrap();
    write_with(s, &samples, 1, 44100);
    let bytes = std::fs::read(&path).unwrap();

    // Not WavPack at all.
    let junk = dir.path().join("junk.wv");
    std::fs::write(&junk, vec![7u8; 4096]).unwrap();
    assert!(matches!(
        wavpack::open(File::open(&junk).unwrap()),
        Err(MediaError::Damaged { .. })
    ));

    // Cut in half: the header promises more than the file holds.
    let cut = dir.path().join("cut.wv");
    std::fs::write(&cut, &bytes[..bytes.len() / 2]).unwrap();
    let r = wavpack::open(File::open(&cut).unwrap()).and_then(|mut s| read_all(&mut *s));
    assert!(matches!(r, Err(MediaError::Damaged { .. })), "{r:?}");

    // A flipped byte in the middle breaks a block's checksum.
    let flipped = dir.path().join("flip.wv");
    let mut bad = bytes.clone();
    let mid = bad.len() / 2;
    bad[mid] ^= 0xFF;
    std::fs::write(&flipped, &bad).unwrap();
    let r = wavpack::open(File::open(&flipped).unwrap()).and_then(|mut s| read_all(&mut *s));
    assert!(matches!(r, Err(MediaError::Damaged { .. })), "{r:?}");
}
