//! Ogg Vorbis and Opus, written and read back.

use kiln_media::ogg::{opus_open, opus_sink, vorbis_sink};
use kiln_media::{
    sym, Block, Channels, Codec, MediaError, Picture, Process, Setting, Sink, SinkSpec, Source,
    Tags,
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
            3 => 0x7,
            4 => 0x33,
            5 => 0x37,
            6 => 0x3F,
            7 => 0x70F,
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

fn opus_setting() -> Setting {
    Setting::Opus {
        kbps: 96,
        speech: false,
    }
}

/// Writes `samples` through a sink of the given kind and returns the file size.
fn write_with(mut sink: Box<dyn Sink>, samples: &[f32], channels: u16, rate: u32) {
    for b in blocks(samples, channels, rate) {
        sink.write(&b).unwrap();
    }
    sink.finish().unwrap();
}

fn write_opus(path: &Path, samples: &[f32], channels: u16, tags: &Tags, setting: &Setting) -> u64 {
    let s = opus_sink(spec(path, 48000, channels_of(channels), tags, setting)).unwrap();
    write_with(s, samples, channels, 48000);
    std::fs::metadata(path).unwrap().len()
}

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
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

fn energy(samples: &[f32], channels: usize, channel: usize) -> f64 {
    samples
        .chunks(channels)
        .map(|f| (f[channel] as f64).powi(2))
        .sum()
}

// ---------------------------------------------------------------------------
// The five from the plan

#[test]
fn opus_length_matches_the_source() {
    // 44.1 kHz stereo, 10.000 s, through the same Process a conversion uses.
    let samples = music(441_000, 2, 44100);
    let dir = tmp();
    let path = dir.path().join("a.opus");
    let tags = Tags::default();
    let setting = opus_setting();
    let mut sink = opus_sink(spec(&path, 48000, channels_of(2), &tags, &setting)).unwrap();
    let mut process = Process::new(44100, channels_of(2), 48000, channels_of(2), None).unwrap();
    let mut fed = 0usize;
    let mut converted = empty_block();
    for b in blocks(&samples, 2, 44100) {
        process.run(&b, &mut converted);
        if converted.frames() > 0 {
            fed += converted.frames();
            sink.write(&converted).unwrap();
        }
    }
    process.flush(&mut converted);
    if converted.frames() > 0 {
        fed += converted.frames();
        sink.write(&converted).unwrap();
    }
    sink.finish().unwrap();

    let mut back = opus_open(File::open(&path).unwrap()).unwrap();
    assert_eq!(back.info().codec, Codec::Opus);
    assert_eq!(back.info().rate, 48000);
    assert_eq!(back.info().channels.count, 2);
    // The Opus stage is exact: what went into the sink comes out.
    assert_eq!(back.info().frames, Some(fed as u64));
    let got = read_all(&mut *back).unwrap();
    assert_eq!(got.len() / 2, fed);
    // And Process gives exactly 10.000 s at 48 kHz.
    assert_eq!(fed, 480_000, "Process output for 441,000 frames");
}

#[test]
fn opus_51_uses_family_1() {
    let samples = music(48_000, 6, 48000);
    let dir = tmp();
    let path = dir.path().join("a.opus");
    let setting = Setting::Opus {
        kbps: 256,
        speech: false,
    };
    write_opus(&path, &samples, 6, &Tags::default(), &setting);

    let bytes = std::fs::read(&path).unwrap();
    let head = find(&bytes, b"OpusHead").expect("an OpusHead");
    let h = &bytes[head..];
    assert_eq!(h[8], 1, "version");
    assert_eq!(h[9], 6, "channels");
    assert_eq!(h[18], 1, "channel mapping family");
    assert_eq!(h[19], 4, "streams");
    assert_eq!(h[20], 2, "coupled streams");
    assert_eq!(&h[21..27], &[0, 4, 1, 2, 3, 5], "mapping table");

    let mut back = opus_open(File::open(&path).unwrap()).unwrap();
    assert_eq!(back.info().channels.count, 6);
    assert_eq!(back.info().channels.mask, 0x3F);
    assert_eq!(back.info().frames, Some(48_000));
    let got = read_all(&mut *back).unwrap();
    assert_eq!(got.len(), 48_000 * 6);
}

/// A 100 Hz tone in one channel and silence in the rest.
fn one_channel(count: usize, ch: usize, frames: usize, rate: u32) -> Vec<f32> {
    let mut samples = vec![0.0f32; frames * count];
    for (i, frame) in samples.chunks_mut(count).enumerate() {
        frame[ch] = (0.5 * (2.0 * PI * 100.0 * i as f64 / rate as f64).sin()) as f32;
    }
    samples
}

/// Checks that the energy of `got` sits in channel `ch`.
fn assert_in_channel(got: &[f32], count: usize, ch: usize, what: &str) {
    let e: Vec<f64> = (0..count).map(|c| energy(got, count, c)).collect();
    let total: f64 = e.iter().sum();
    assert!(
        e[ch] > 0.8 * total,
        "{what}: channel {ch} holds {:.0}% of the energy: {e:?}",
        100.0 * e[ch] / total
    );
}

#[test]
fn opus_puts_each_channel_where_it_was() {
    // A tone in one channel only, in Kiln's (WAVE) order: it must come back in
    // the same channel, whatever order the file keeps them in.
    let dir = tmp();
    for count in [3usize, 4, 5, 6, 7, 8] {
        let setting = Setting::Opus {
            kbps: 64 * count as u32,
            speech: false,
        };
        for ch in 0..count {
            let samples = one_channel(count, ch, 24_000, 48000);
            let path = dir.path().join(format!("c{count}_{ch}.opus"));
            write_opus(&path, &samples, count as u16, &Tags::default(), &setting);
            let mut back = opus_open(File::open(&path).unwrap()).unwrap();
            assert_eq!(back.info().channels, channels_of(count as u16));
            let got = read_all(&mut *back).unwrap();
            assert_in_channel(&got, count, ch, &format!("{count}-channel Opus"));
        }
    }
}

#[test]
fn vorbis_puts_each_channel_where_it_was() {
    // The same through the Vorbis sink, read back by Symphonia.
    let dir = tmp();
    let tags = Tags::default();
    let setting = Setting::VorbisQ { tenths: 50 };
    for count in [3usize, 4, 5, 6, 7, 8] {
        for ch in 0..count {
            let samples = one_channel(count, ch, 24_000, 44100);
            let path = dir.path().join(format!("c{count}_{ch}.ogg"));
            let channels = channels_of(count as u16);
            let s = vorbis_sink(spec(&path, 44100, channels, &tags, &setting)).unwrap();
            write_with(s, &samples, count as u16, 44100);
            let mut back = sym::open(File::open(&path).unwrap(), "ogg").unwrap();
            assert_eq!(back.info().channels, channels, "{count} channels");
            let got = read_all(&mut *back).unwrap();
            assert_in_channel(&got, count, ch, &format!("{count}-channel Vorbis"));
        }
    }
}

#[test]
fn side_speakers_land_where_back_ones_do() {
    // 5.1 with side speakers (0x60F) goes into the file as Vorbis 5.1 and
    // each channel comes back at the same place.
    let dir = tmp();
    let layout = Channels {
        count: 6,
        mask: 0x60F,
    };
    for ch in 0..6usize {
        let samples = one_channel(6, ch, 24_000, 48000);
        let setting = Setting::Opus {
            kbps: 256,
            speech: false,
        };
        let path = dir.path().join(format!("s{ch}.opus"));
        let s = opus_sink(spec(&path, 48000, layout, &Tags::default(), &setting)).unwrap();
        write_with(s, &samples, 6, 48000);
        let got = read_all(&mut *opus_open(File::open(&path).unwrap()).unwrap()).unwrap();
        assert_in_channel(&got, 6, ch, "side 5.1");
    }
}

#[test]
fn opus_51_uses_the_standard_mapping_table() {
    let dir = tmp();
    let path = dir.path().join("a.opus");
    let setting = Setting::Opus {
        kbps: 256,
        speech: false,
    };
    write_opus(&path, &music(4800, 6, 48000), 6, &Tags::default(), &setting);
    let bytes = std::fs::read(&path).unwrap();
    let head = find(&bytes, b"OpusHead").unwrap();
    assert_eq!(&bytes[head + 21..head + 27], &[0, 4, 1, 2, 3, 5]);
}

#[test]
fn opus_bitrate_is_near_the_request() {
    let samples = music(30 * 48_000, 2, 48000);
    let dir = tmp();
    for kbps in [96u32, 64] {
        let path = dir.path().join(format!("b{kbps}.opus"));
        let setting = Setting::Opus {
            kbps,
            speech: false,
        };
        let size = write_opus(&path, &samples, 2, &Tags::default(), &setting) as f64;
        let want = kbps as f64 * 30.0 * 1000.0 / 8.0;
        assert!(
            (size - want).abs() <= want * 0.10,
            "{kbps} kbps: {size} bytes against {want}"
        );
    }
}

#[test]
fn vorbis_q5_decodes_with_symphonia() {
    let frames = 5 * 44_100 + 123;
    let samples = music(frames, 2, 44100);
    let dir = tmp();
    let path = dir.path().join("a.ogg");
    let tags = Tags::default();
    let setting = Setting::VorbisQ { tenths: 50 };
    let s = vorbis_sink(spec(&path, 44100, channels_of(2), &tags, &setting)).unwrap();
    write_with(s, &samples, 2, 44100);

    let mut back = sym::open(File::open(&path).unwrap(), "ogg").unwrap();
    assert_eq!(back.info().codec, Codec::Vorbis);
    assert_eq!(back.info().rate, 44100);
    assert_eq!(back.info().channels.count, 2);
    let declared = back.info().frames.expect("a length");
    assert!(
        declared.abs_diff(frames as u64) <= 2048,
        "declared {declared}"
    );
    let got = read_all(&mut *back).unwrap();
    assert!(
        (got.len() as u64 / 2).abs_diff(frames as u64) <= 2048,
        "decoded {}",
        got.len() / 2
    );
    // Quality 5 is about 160 kbps for stereo music; a tone mix costs less.
    let size = std::fs::metadata(&path).unwrap().len();
    let kbps = size as f64 * 8.0 / (frames as f64 / 44100.0) / 1000.0;
    assert!((20.0..260.0).contains(&kbps), "{kbps} kbps");
}

#[test]
fn ogg_keeps_tags_and_cover() {
    let samples = music(48_000, 2, 48000);
    let tags = full_tags();
    let dir = tmp();

    // Vorbis, read back by Symphonia.
    let path = dir.path().join("a.ogg");
    let setting = Setting::VorbisQ { tenths: 40 };
    let s = vorbis_sink(spec(&path, 48000, channels_of(2), &tags, &setting)).unwrap();
    write_with(s, &samples, 2, 48000);
    let back = sym::open(File::open(&path).unwrap(), "ogg").unwrap();
    assert_eq!(back.info().tags, tags);

    // Opus, read back by opus_open.
    let path = dir.path().join("a.opus");
    write_opus(&path, &samples, 2, &tags, &opus_setting());
    let back = opus_open(File::open(&path).unwrap()).unwrap();
    assert_eq!(back.info().tags, tags);

    // The tags packet names the vendor, and the cover is a comment line.
    let bytes = std::fs::read(&path).unwrap();
    let at = find(&bytes, b"OpusTags").expect("OpusTags");
    assert_eq!(&bytes[at + 8..at + 12], &[4, 0, 0, 0]);
    assert_eq!(&bytes[at + 12..at + 16], b"Kiln");
    assert!(find(&bytes, b"METADATA_BLOCK_PICTURE=").is_some());
}

// ---------------------------------------------------------------------------
// Lengths, alignment, odd input

#[test]
fn opus_lengths_are_exact_at_odd_sizes() {
    let dir = tmp();
    for frames in [
        0usize,
        1,
        311,
        312,
        313,
        959,
        960,
        961,
        1920,
        5000,
        48_000 + 7,
    ] {
        let samples = music(frames, 2, 48000);
        let path = dir.path().join(format!("n{frames}.opus"));
        write_opus(&path, &samples, 2, &Tags::default(), &opus_setting());
        let mut back = opus_open(File::open(&path).unwrap()).unwrap();
        assert_eq!(back.info().frames, Some(frames as u64), "{frames}");
        let got = read_all(&mut *back).unwrap();
        assert_eq!(got.len(), frames * 2, "{frames}");
    }
}

#[test]
fn opus_output_lines_up_with_the_input() {
    // Pre-skip is right when the decoded sound sits on the input, not a few
    // hundred samples late.
    let frames = 48_000usize;
    let samples = music(frames, 1, 48000);
    let dir = tmp();
    let path = dir.path().join("a.opus");
    let setting = Setting::Opus {
        kbps: 128,
        speech: false,
    };
    write_opus(&path, &samples, 1, &Tags::default(), &setting);
    let got = read_all(&mut *opus_open(File::open(&path).unwrap()).unwrap()).unwrap();
    assert_eq!(got.len(), frames);
    let window = 4000..40_000usize;
    let corr = |lag: i64| -> f64 {
        window
            .clone()
            .map(|i| samples[i] as f64 * got[(i as i64 + lag) as usize] as f64)
            .sum()
    };
    let best = (-400i64..=400)
        .max_by(|a, b| corr(*a).partial_cmp(&corr(*b)).unwrap())
        .unwrap();
    assert!(best.abs() <= 2, "the decoded sound is {best} samples off");
}

#[test]
fn samples_past_full_scale_clamp() {
    let dir = tmp();
    let mut loud = music(20_000, 2, 48000);
    for (i, s) in loud.iter_mut().enumerate() {
        *s *= 40.0;
        if i % 997 == 0 {
            *s = f32::NAN;
        }
        if i % 1999 == 0 {
            *s = f32::INFINITY;
        }
    }
    let path = dir.path().join("a.opus");
    write_opus(&path, &loud, 2, &Tags::default(), &opus_setting());
    let got = read_all(&mut *opus_open(File::open(&path).unwrap()).unwrap()).unwrap();
    assert_eq!(got.len(), loud.len());
    assert!(got.iter().all(|s| s.is_finite() && s.abs() < 2.0));

    let path = dir.path().join("a.ogg");
    let tags = Tags::default();
    let setting = Setting::VorbisQ { tenths: 50 };
    let s = vorbis_sink(spec(&path, 44100, channels_of(2), &tags, &setting)).unwrap();
    write_with(s, &loud, 2, 44100);
    let got = read_all(&mut *sym::open(File::open(&path).unwrap(), "ogg").unwrap()).unwrap();
    assert!(got.iter().all(|s| s.is_finite()));
}

#[test]
fn vorbis_abr_follows_the_request() {
    let frames = 20 * 44_100;
    let samples = music(frames, 2, 44100);
    let dir = tmp();
    let path = dir.path().join("a.ogg");
    let tags = Tags::default();
    let setting = Setting::VorbisAbr { kbps: 128 };
    let s = vorbis_sink(spec(&path, 44100, channels_of(2), &tags, &setting)).unwrap();
    write_with(s, &samples, 2, 44100);
    let size = std::fs::metadata(&path).unwrap().len() as f64;
    let want = 128.0 * 20.0 * 1000.0 / 8.0;
    assert!(
        (size - want).abs() <= want * 0.15,
        "{size} bytes against {want}"
    );
    let got = read_all(&mut *sym::open(File::open(&path).unwrap(), "ogg").unwrap()).unwrap();
    assert!((got.len() as u64 / 2).abs_diff(frames as u64) <= 2048);
}

#[test]
fn vorbis_takes_one_to_eight_channels() {
    let dir = tmp();
    let tags = Tags::default();
    let setting = Setting::VorbisQ { tenths: 30 };
    for n in [1u16, 2, 3, 4, 5, 6, 7, 8] {
        let samples = music(20_000, n as usize, 44100);
        let path = dir.path().join(format!("c{n}.ogg"));
        let s = vorbis_sink(spec(&path, 44100, channels_of(n), &tags, &setting)).unwrap();
        write_with(s, &samples, n, 44100);
        let back = sym::open(File::open(&path).unwrap(), "ogg").unwrap();
        assert_eq!(back.info().channels.count, n);
    }
    let samples = music(1000, 9, 44100);
    let path = dir.path().join("c9.ogg");
    let r = vorbis_sink(spec(
        &path,
        44100,
        Channels { count: 9, mask: 0 },
        &tags,
        &setting,
    ));
    assert!(matches!(r, Err(MediaError::Encoder(_))));
    drop(samples);
}

#[test]
fn opus_takes_one_to_eight_channels() {
    let dir = tmp();
    for n in [1u16, 2, 3, 4, 5, 6, 7, 8] {
        let samples = music(10_000, n as usize, 48000);
        let path = dir.path().join(format!("c{n}.opus"));
        let setting = Setting::Opus {
            kbps: 64 * n as u32,
            speech: false,
        };
        write_opus(&path, &samples, n, &Tags::default(), &setting);
        let mut back = opus_open(File::open(&path).unwrap()).unwrap();
        assert_eq!(back.info().channels.count, n);
        assert_eq!(back.info().channels.mask, channels_of(n).mask);
        let got = read_all(&mut *back).unwrap();
        assert_eq!(got.len(), 10_000 * n as usize, "{n} channels");
        let bytes = std::fs::read(&path).unwrap();
        let head = find(&bytes, b"OpusHead").unwrap();
        assert_eq!(bytes[head + 18], if n <= 2 { 0 } else { 1 }, "{n} channels");
    }
}

#[test]
fn opus_speech_setting_is_accepted() {
    let dir = tmp();
    let path = dir.path().join("a.opus");
    let setting = Setting::Opus {
        kbps: 24,
        speech: true,
    };
    let samples = music(48_000, 1, 48000);
    let size = write_opus(&path, &samples, 1, &Tags::default(), &setting);
    assert!(size > 1000);
    assert_eq!(
        read_all(&mut *opus_open(File::open(&path).unwrap()).unwrap())
            .unwrap()
            .len(),
        48_000
    );
}

#[test]
fn opus_refuses_other_rates_and_other_settings() {
    let dir = tmp();
    let path = dir.path().join("a.opus");
    let tags = Tags::default();
    let setting = opus_setting();
    let r = opus_sink(spec(&path, 44100, channels_of(2), &tags, &setting));
    assert!(matches!(r, Err(MediaError::Encoder(_))));
    let wrong = Setting::VorbisQ { tenths: 50 };
    let r = opus_sink(spec(&path, 48000, channels_of(2), &tags, &wrong));
    assert!(matches!(r, Err(MediaError::Encoder(_))));
    let r = vorbis_sink(spec(&path, 44100, channels_of(2), &tags, &setting));
    assert!(matches!(r, Err(MediaError::Encoder(_))));
    let r = opus_sink(spec(
        &path,
        48000,
        Channels { count: 0, mask: 0 },
        &tags,
        &setting,
    ));
    assert!(matches!(r, Err(MediaError::Encoder(_))));
}

#[test]
fn a_block_of_the_wrong_width_is_refused() {
    let dir = tmp();
    let tags = Tags::default();
    let setting = opus_setting();
    let path = dir.path().join("a.opus");
    let mut s = opus_sink(spec(&path, 48000, channels_of(2), &tags, &setting)).unwrap();
    let b = Block {
        channels: 1,
        rate: 48000,
        samples: vec![0.0; 100],
    };
    assert!(s.write(&b).is_err());
    let setting = Setting::VorbisQ { tenths: 50 };
    let path = dir.path().join("a.ogg");
    let mut s = vorbis_sink(spec(&path, 48000, channels_of(2), &tags, &setting)).unwrap();
    assert!(s.write(&b).is_err());
}

#[test]
fn an_unfinished_sink_can_be_dropped() {
    let dir = tmp();
    let tags = Tags::default();
    let setting = opus_setting();
    let mut s = opus_sink(spec(
        &dir.path().join("a.opus"),
        48000,
        channels_of(2),
        &tags,
        &setting,
    ))
    .unwrap();
    s.write(&Block {
        channels: 2,
        rate: 48000,
        samples: music(5000, 2, 48000),
    })
    .unwrap();
    drop(s);
    let setting = Setting::VorbisQ { tenths: 50 };
    let mut s = vorbis_sink(spec(
        &dir.path().join("a.ogg"),
        48000,
        channels_of(2),
        &tags,
        &setting,
    ))
    .unwrap();
    s.write(&Block {
        channels: 2,
        rate: 48000,
        samples: music(5000, 2, 48000),
    })
    .unwrap();
    drop(s);
}

#[test]
fn an_empty_conversion_still_makes_a_file() {
    let dir = tmp();
    let tags = Tags::default();
    let setting = Setting::VorbisQ { tenths: 50 };
    let path = dir.path().join("a.ogg");
    let s = vorbis_sink(spec(&path, 44100, channels_of(2), &tags, &setting)).unwrap();
    s.finish().unwrap();
    assert!(std::fs::metadata(&path).unwrap().len() > 100);
    let mut back = sym::open(File::open(&path).unwrap(), "ogg").unwrap();
    assert!(!back.read(&mut empty_block()).unwrap());
}

// ---------------------------------------------------------------------------
// Reading Opus

#[test]
fn opus_open_reads_a_file_ffmpeg_wrote() {
    let path = data("tone_opus.ogg");
    let mut back = opus_open(File::open(&path).unwrap()).unwrap();
    let info = back.info().clone();
    assert_eq!(info.codec, Codec::Opus);
    assert_eq!(info.rate, 48000);
    assert_eq!(info.channels.count, 2);
    assert_eq!(info.bits, None);
    let frames = info.frames.expect("a length");
    // The clip is one second of a 440 Hz sine.
    assert!(frames.abs_diff(48_000) <= 1, "{frames} frames");
    assert!(info.kbps.is_some());
    let got = read_all(&mut *back).unwrap();
    assert_eq!(got.len() as u64, frames * 2);
    let at = back.position();
    assert_eq!(at.frames, frames);
    assert!(at.bytes > 0 && at.bytes <= at.total_bytes);
    assert_eq!(at.total_bytes, std::fs::metadata(&path).unwrap().len());
    // A 440 Hz sine at full-scale-ish level: loud, and mostly that frequency.
    let mid = &got[2 * 10_000..2 * 10_000 + 2 * 4800];
    let rms = (energy(mid, 2, 0) / 4800.0).sqrt();
    assert!(rms > 0.05, "rms {rms}");
    let zero_crossings = mid
        .chunks(2)
        .map(|f| f[0])
        .collect::<Vec<_>>()
        .windows(2)
        .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
        .count();
    // 4800 frames is 0.1 s: about 44 cycles.
    assert!((43..=45).contains(&zero_crossings), "{zero_crossings}");
}

/// `parts`, end to end in one file in `dir`: a chained Ogg file.
fn chain(dir: &Path, name: &str, parts: &[&Path]) -> PathBuf {
    let bytes: Vec<u8> = parts
        .iter()
        .flat_map(|p| std::fs::read(p).unwrap())
        .collect();
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn opus_frames(path: &Path) -> usize {
    let mut src = opus_open(File::open(path).unwrap()).unwrap();
    let ch = src.info().channels.count as usize;
    read_all(&mut *src).unwrap().len() / ch
}

#[test]
fn a_chained_ogg_opus_decodes_whole() {
    // The same stream twice: each part keeps its own pre-skip and end, so
    // the whole is the sum of both, sample for sample.
    let dir = tmp();
    let one = data("tone_opus.ogg");
    let twice = chain(dir.path(), "twice.opus", &[&one, &one]);
    let alone = opus_frames(&one);
    assert_eq!(opus_frames(&twice), 2 * alone);
    let part = read_all(&mut *opus_open(File::open(&one).unwrap()).unwrap()).unwrap();
    let both = read_all(&mut *opus_open(File::open(&twice).unwrap()).unwrap()).unwrap();
    assert_eq!(&both[..part.len()], &part[..]);
    assert_eq!(&both[part.len()..], &part[..]);
    // And through convert, as the app and the command line run it.
    let wav = dir.path().join("twice.wav");
    kiln_media::convert(
        &twice,
        &wav,
        kiln_media::Target::Wav,
        kiln_media::Preset::High,
        &kiln_media::Tune::default(),
        &mut |_| {},
        &|| false,
    )
    .unwrap();
    let mut back = kiln_media::open_source(&wav).unwrap();
    assert_eq!(read_all(&mut *back).unwrap().len(), both.len());
}

#[test]
fn a_chained_ogg_opus_whose_channels_change_says_so() {
    // A mono Opus Kiln wrote, then the stereo fixture, and the other way
    // round: one pipeline can't carry both, so neither converts in part.
    let dir = tmp();
    let mono = dir.path().join("mono.opus");
    kiln_media::convert(
        &data("tone.wav"),
        &mono,
        kiln_media::Target::Opus,
        kiln_media::Preset::High,
        &kiln_media::Tune::default(),
        &mut |_| {},
        &|| false,
    )
    .unwrap();
    let stereo = data("tone_opus.ogg");
    for (name, parts) in [("ms.opus", [&mono, &stereo]), ("sm.opus", [&stereo, &mono])] {
        let path = chain(dir.path(), name, &[parts[0].as_path(), parts[1].as_path()]);
        let mut src = opus_open(File::open(&path).unwrap()).unwrap();
        match read_all(&mut *src) {
            Err(MediaError::Unsupported { codec }) => assert_eq!(codec, "a chained stream"),
            other => panic!("{name}: {:?}", other.map(|s| s.len())),
        }
        let out = dir.path().join(format!("{name}.wav"));
        let r = kiln_media::convert(
            &path,
            &out,
            kiln_media::Target::Wav,
            kiln_media::Preset::High,
            &kiln_media::Tune::default(),
            &mut |_| {},
            &|| false,
        );
        assert!(
            matches!(r, Err(MediaError::Unsupported { .. })),
            "{name}: {r:?}"
        );
        assert!(!out.exists(), "{name}: a partial file was left");
    }
}

#[test]
fn opus_open_rejects_what_is_not_opus() {
    let dir = tmp();
    let junk = dir.path().join("junk.opus");
    std::fs::write(&junk, vec![0x55u8; 5000]).unwrap();
    let r = opus_open(File::open(&junk).unwrap());
    assert!(matches!(r, Err(MediaError::Damaged { .. })), "junk");

    let empty = dir.path().join("empty.opus");
    std::fs::write(&empty, b"").unwrap();
    let r = opus_open(File::open(&empty).unwrap());
    assert!(matches!(r, Err(MediaError::Damaged { .. })), "empty");

    // An Ogg Vorbis file is Ogg, but not Opus.
    let r = opus_open(File::open(data("tone.ogg")).unwrap());
    assert!(matches!(r, Err(MediaError::Damaged { .. })), "vorbis");
}

#[test]
fn a_damaged_page_is_reported_and_a_cut_tail_still_plays() {
    let dir = tmp();
    let samples = music(3 * 48_000, 2, 48000);
    let path = dir.path().join("a.opus");
    write_opus(&path, &samples, 2, &Tags::default(), &opus_setting());
    let bytes = std::fs::read(&path).unwrap();

    // A flipped bit in the middle breaks a page's checksum.
    let mut bad = bytes.clone();
    let mid = bad.len() / 2;
    bad[mid] ^= 0x10;
    let bad_path = dir.path().join("bad.opus");
    std::fs::write(&bad_path, &bad).unwrap();
    let r = opus_open(File::open(&bad_path).unwrap()).and_then(|mut s| read_all(&mut *s));
    assert!(matches!(r, Err(MediaError::Damaged { .. })), "{r:?}");

    // A file cut short plays up to the cut.
    let cut = &bytes[..bytes.len() - 700];
    let cut_path = dir.path().join("cut.opus");
    std::fs::write(&cut_path, cut).unwrap();
    let mut s = opus_open(File::open(&cut_path).unwrap()).unwrap();
    let got = read_all(&mut *s).unwrap();
    assert!(got.len() / 2 > 48_000 && got.len() / 2 < 3 * 48_000);
}

#[test]
fn big_covers_span_pages() {
    // A cover far bigger than one Ogg page, and than a packet of audio.
    let mut tags = full_tags();
    tags.picture = Some(Picture {
        mime: "image/jpeg".into(),
        data: (0..700_000u32).map(|i| (i * 31 % 251) as u8).collect(),
    });
    let dir = tmp();
    let path = dir.path().join("a.opus");
    write_opus(&path, &music(48_000, 2, 48000), 2, &tags, &opus_setting());
    let back = opus_open(File::open(&path).unwrap()).unwrap();
    assert_eq!(back.info().tags, tags);
    assert_eq!(back.info().frames, Some(48_000));
}

/// Writes one file of each kind into the folder named by `KILN_OGG_OUT`, for
/// looking at with other tools (ffprobe, a player). Run by hand:
/// `KILN_OGG_OUT=... cargo test -p kiln-media --test ogg -- --ignored`.
#[test]
#[ignore]
fn write_samples_for_other_tools() {
    let Ok(dir) = std::env::var("KILN_OGG_OUT") else {
        return;
    };
    let dir = PathBuf::from(dir);
    let tags = full_tags();
    let samples = music(3 * 48_000, 2, 48000);
    write_opus(
        &dir.join("stereo.opus"),
        &samples,
        2,
        &tags,
        &opus_setting(),
    );
    let six = music(48_000, 6, 48000);
    let setting = Setting::Opus {
        kbps: 256,
        speech: false,
    };
    write_opus(&dir.join("surround.opus"), &six, 6, &tags, &setting);
    for ch in [2usize, 3, 4] {
        let mut one = vec![0.0f32; 24_000 * 6];
        for (i, frame) in one.chunks_mut(6).enumerate() {
            frame[ch] = (0.5 * (2.0 * PI * 100.0 * i as f64 / 48000.0).sin()) as f32;
        }
        write_opus(
            &dir.join(format!("only{ch}.opus")),
            &one,
            6,
            &Tags::default(),
            &setting,
        );
    }
    let setting = Setting::VorbisQ { tenths: 50 };
    let s = vorbis_sink(spec(
        &dir.join("stereo.ogg"),
        48000,
        channels_of(2),
        &tags,
        &setting,
    ))
    .unwrap();
    write_with(s, &samples, 2, 48000);
    // One tone in one channel, for 5.1 and 6.1, to check the channel order.
    let vq = Setting::VorbisQ { tenths: 50 };
    let opus = Setting::Opus {
        kbps: 384,
        speech: false,
    };
    for count in [6usize, 7] {
        for ch in 0..count {
            let one = one_channel(count, ch, 24_000, 48000);
            let layout = channels_of(count as u16);
            let path = dir.join(format!("v{count}_{ch}.ogg"));
            let s = vorbis_sink(spec(&path, 48000, layout, &Tags::default(), &vq)).unwrap();
            write_with(s, &one, count as u16, 48000);
            let path = dir.join(format!("o{count}_{ch}.opus"));
            write_opus(&path, &one, count as u16, &Tags::default(), &opus);
        }
    }
}
