//! The codec table: every source fixture converts to every target through the
//! one public `convert`, and the output reads back as the target's codec at
//! the source's length.

mod common;

use kiln_media::pcm::wav;
use kiln_media::{
    convert, open_source, probe, Block, Channels, Codec, MediaError, Preset, Setting, SinkSpec,
    Source, Tags, Target, Tune,
};
use std::cell::Cell;
use std::f64::consts::PI;
use std::fs::File;
use std::path::{Path, PathBuf};

const ALL: [Target; 11] = [
    Target::Wav,
    Target::Aiff,
    Target::Flac,
    Target::WavPack,
    Target::Mp3,
    Target::Aac,
    Target::M4a,
    Target::Ogg,
    Target::Opus,
    Target::Wma,
    Target::Ac3,
];

fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join(name)
}

/// The source fixtures: every format Kiln reads, plus a video's sound.
fn sources() -> Vec<&'static str> {
    let mut list = vec![
        "tone.wav",
        "tone.aiff",
        "tone.mp3",
        "tone_aac.m4a",
        "tone.flac",
        "tone.ogg",
        "tone_opus.ogg",
        "tone_opus.webm",
        "tone.wv",
        "av.mp4",
    ];
    // Only Media Foundation reads these two. CI without it says so in its log
    // and goes on with the rest.
    if cfg!(windows) && !common::no_mf() {
        list.extend(["tone.wma", "tone.ac3"]);
    }
    list
}

/// Whether the codec a file reads back as is the one `t` writes.
fn is_codec_of(t: Target, c: &Codec) -> bool {
    match t {
        Target::Wav | Target::Aiff => matches!(c, Codec::Pcm { .. }),
        Target::Flac => *c == Codec::Flac,
        Target::WavPack => *c == Codec::WavPack,
        Target::Mp3 => *c == Codec::Mp3,
        Target::Aac | Target::M4a => *c == Codec::Aac,
        Target::Ogg => *c == Codec::Vorbis,
        Target::Opus => *c == Codec::Opus,
        Target::Wma => *c == Codec::Wma,
        Target::Ac3 => *c == Codec::Ac3,
    }
}

/// Decodes the whole file and returns its length in seconds.
fn decoded_secs(path: &Path) -> f64 {
    let mut src = open_source(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let rate = src.info().rate;
    let frames = count_frames(&mut *src);
    frames as f64 / rate as f64
}

fn count_frames(src: &mut dyn Source) -> u64 {
    let mut block = Block {
        channels: 0,
        rate: 0,
        samples: Vec::new(),
    };
    let mut frames = 0u64;
    while src.read(&mut block).unwrap() {
        frames += block.frames() as u64;
    }
    frames
}

fn high(input: &Path, output: &Path, t: Target) -> kiln_media::Result<()> {
    convert(
        input,
        output,
        t,
        Preset::High,
        &Tune::default(),
        &mut |_| {},
        &|| false,
    )
}

/// What `convert` must say for a target this platform can't write, if any.
fn expected_failure(t: Target) -> Option<MediaError> {
    kiln_media::can_run(t).err()
}

#[test]
fn every_source_converts_to_every_target() {
    let dir = tempfile::tempdir().unwrap();
    let failures: Vec<String> = std::thread::scope(|s| {
        let jobs: Vec<_> = sources()
            .into_iter()
            .map(|name| {
                let dir = dir.path().to_path_buf();
                s.spawn(move || {
                    let mut failed = Vec::new();
                    let input = data(name);
                    let want = decoded_secs(&input);
                    for t in ALL {
                        let stem = name.replace('.', "_");
                        let out = dir.join(format!("{stem}.{}", t.ext()));
                        let r = high(&input, &out, t);
                        if let Some(e) = expected_failure(t) {
                            if !matches!((&r, &e), (Err(a), b) if a.to_string() == b.to_string()) {
                                failed.push(format!("{name} -> {t:?}: {r:?}, wanted {e}"));
                            }
                            continue;
                        }
                        if let Err(e) = r {
                            failed.push(format!("{name} -> {t:?}: {e}"));
                            continue;
                        }
                        match probe(&out) {
                            Ok(info) if is_codec_of(t, &info.codec) => {}
                            other => {
                                failed.push(format!("{name} -> {t:?}: probes as {other:?}"));
                                continue;
                            }
                        }
                        let got = decoded_secs(&out);
                        if (got - want).abs() > 0.1 {
                            failed.push(format!("{name} -> {t:?}: {got:.3} s, source {want:.3} s"));
                        }
                    }
                    failed
                })
            })
            .collect();
        jobs.into_iter()
            .flat_map(|j| j.join().expect("a conversion thread panicked"))
            .collect()
    });
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn aac_priming_is_trimmed_from_an_mp4() {
    // FFmpeg's M4A carries 1,024 frames of priming in its edit list, and the
    // tone is exactly 3 s at 44.1 kHz.
    for (name, secs) in [("tone_aac.m4a", 3u64), ("av.mp4", 1)] {
        let info = probe(&data(name)).unwrap();
        assert_eq!(info.codec, Codec::Aac);
        let want = secs * 44_100;
        assert_eq!(info.frames, Some(want), "{name}");
        let mut src = open_source(&data(name)).unwrap();
        let got = count_frames(&mut *src);
        assert!(
            got.abs_diff(want) <= 1024,
            "{name}: {got} frames, {want} wanted"
        );
    }
}

#[test]
fn aac_priming_trim_keeps_the_audio_in_place() {
    // The tone starts at full level on the first frame: with the priming
    // left in, the first 1,024 frames would be near silence.
    let mut src = open_source(&data("tone_aac.m4a")).unwrap();
    let mut block = Block {
        channels: 0,
        rate: 0,
        samples: Vec::new(),
    };
    assert!(src.read(&mut block).unwrap());
    let first: Vec<f32> = block
        .samples
        .iter()
        .step_by(2)
        .take(1024)
        .copied()
        .collect();
    let peak = first.iter().fold(0f32, |m, s| m.max(s.abs()));
    assert!(peak > 0.05, "the first 1,024 frames peak at {peak}");
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
            let noise = ((state >> 40) as f64 / (1u64 << 24) as f64 - 0.5) * 0.1;
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

/// Writes a 16-bit WAV of `secs` seconds of music-like sound.
fn write_wav(path: &Path, secs: usize, channels: Channels, rate: u32) {
    let tags = Tags::default();
    let setting = Setting::Pcm {
        bits: 16,
        float: false,
    };
    let mut sink = wav::sink(SinkSpec {
        rate,
        channels,
        tags: &tags,
        setting: &setting,
        source_bits: None,
        out: File::create(path).unwrap(),
    })
    .unwrap();
    let n = channels.count as usize;
    let all = music(secs * rate as usize, n, rate);
    for chunk in all.chunks(8192 * n) {
        sink.write(&Block {
            channels: n as u16,
            rate,
            samples: chunk.to_vec(),
        })
        .unwrap();
    }
    sink.finish().unwrap();
}

const STEREO: Channels = Channels {
    count: 2,
    mask: 0x3,
};

#[test]
fn cancel_removes_the_partial_file() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("long.wav");
    write_wav(&input, 20, STEREO, 44_100);
    for t in [
        Target::Wav,
        Target::Flac,
        Target::Mp3,
        Target::M4a,
        Target::Wma,
    ] {
        if expected_failure(t).is_some() {
            continue;
        }
        let out = dir.path().join(format!("cancelled.{}", t.ext()));
        let reads = Cell::new(0u32);
        let r = convert(
            &input,
            &out,
            t,
            Preset::High,
            &Tune::default(),
            &mut |_| {},
            &|| {
                reads.set(reads.get() + 1);
                reads.get() > 5
            },
        );
        assert!(matches!(r, Err(MediaError::Cancelled)), "{t:?}: {r:?}");
        assert!(!out.exists(), "{t:?}: the partial file is still there");
    }
    // Nothing else was left behind either (no M4A frames file).
    let left: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(left, vec![std::ffi::OsString::from("long.wav")]);
}

#[test]
fn an_error_removes_the_partial_file_and_an_existing_output_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    // A setting the target can't take fails once the output is open.
    let out = dir.path().join("bad.flac");
    let tune = Tune {
        setting: Some(Setting::Kbps { kbps: 128 }),
        ..Tune::default()
    };
    let r = convert(
        &data("tone.wav"),
        &out,
        Target::Flac,
        Preset::High,
        &tune,
        &mut |_| {},
        &|| false,
    );
    assert!(r.is_err());
    assert!(!out.exists(), "the failed output is still there");

    // An output that already exists is neither overwritten nor removed.
    let existing = dir.path().join("existing.wav");
    std::fs::write(&existing, b"keep me").unwrap();
    let r = high(&data("tone.wav"), &existing, Target::Wav);
    assert!(r.is_err());
    assert_eq!(std::fs::read(&existing).unwrap(), b"keep me");
}

#[test]
fn fit_under_result_is_under() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("minute.wav");
    write_wav(&input, 60, STEREO, 44_100);
    const MB: u64 = 1_000_000;
    for t in [Target::Opus, Target::Mp3] {
        let out = dir.path().join(format!("fit.{}", t.ext()));
        let tune = Tune {
            fit_bytes: Some(MB),
            ..Tune::default()
        };
        convert(&input, &out, t, Preset::High, &tune, &mut |_| {}, &|| false)
            .unwrap_or_else(|e| panic!("{t:?}: {e}"));
        let size = std::fs::metadata(&out).unwrap().len();
        assert!(size <= MB, "{t:?}: {size} bytes");
        // And it used the room: not far under.
        assert!(size >= MB * 7 / 10, "{t:?}: only {size} bytes");
    }
    // Below the format's floor, the answer is TooSmall and no file is made.
    let out = dir.path().join("tiny.mp3");
    let tune = Tune {
        fit_bytes: Some(10_000),
        ..Tune::default()
    };
    let r = convert(
        &input,
        &out,
        Target::Mp3,
        Preset::High,
        &tune,
        &mut |_| {},
        &|| false,
    );
    assert!(matches!(r, Err(MediaError::TooSmall { .. })), "{r:?}");
    assert!(!out.exists());
}

fn every_target_converts_in(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    let input = dir.join("Tóne ünïcode 🎵.wav");
    std::fs::copy(data("tone.wav"), &input).unwrap();
    for t in ALL {
        let out = dir.join(format!("Ausgabe 🎵 ü.{}", t.ext()));
        let r = high(&input, &out, t);
        if let Some(e) = expected_failure(t) {
            assert_eq!(r.unwrap_err().to_string(), e.to_string());
            continue;
        }
        r.unwrap_or_else(|e| panic!("{t:?} in {}: {e}", dir.display()));
        let info = probe(&out).unwrap_or_else(|e| panic!("{t:?}: {e}"));
        assert!(is_codec_of(t, &info.codec), "{t:?}: {:?}", info.codec);
        // No frames file was left beside the output.
        let names: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("kiln-mdat"))
            .collect();
        assert!(names.is_empty(), "{names:?}");
    }
}

#[test]
fn unicode_and_long_paths_work() {
    let dir = tempfile::tempdir().unwrap();
    every_target_converts_in(&dir.path().join("Música").join("Ünïcode 🎵"));

    #[cfg(windows)]
    {
        // Past MAX_PATH, through a verbatim path.
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let root = root.to_string_lossy().into_owned();
        assert!(root.starts_with(r"\\?\"), "{root}");
        let mut long = PathBuf::from(&root);
        let mut n = 0;
        while long.as_os_str().len() < 300 {
            long.push(format!("ein sehr langer Ordnername Nummer {n} 🎵"));
            n += 1;
        }
        assert!(long.as_os_str().len() >= 300);
        every_target_converts_in(&long);
    }
}

#[test]
fn surround_goes_where_each_target_can_take_it() {
    // A 4.0 source (FL FR BL BR): AAC and AC3 get 5.1 with the speakers in
    // place, MP3 and WMA get stereo, Opus, Vorbis and FLAC keep four.
    let dir = tempfile::tempdir().unwrap();
    let quad = Channels {
        count: 4,
        mask: 0x33,
    };
    let input = dir.path().join("quad.wav");
    write_wav(&input, 1, quad, 48_000);
    for (t, want) in [
        (Target::Aac, 6u16),
        (Target::M4a, 6),
        (Target::Ac3, 6),
        (Target::Mp3, 2),
        (Target::Wma, 2),
        (Target::Opus, 4),
        (Target::Ogg, 4),
        (Target::Flac, 4),
    ] {
        if expected_failure(t).is_some() {
            continue;
        }
        let out = dir.path().join(format!("quad.{}", t.ext()));
        high(&input, &out, t).unwrap_or_else(|e| panic!("{t:?}: {e}"));
        let info = probe(&out).unwrap();
        assert_eq!(info.channels.count, want, "{t:?}");
    }
}

#[test]
fn a_batch_bitrate_converts_files_whose_list_lacks_it() {
    // The options sheet works a batch's setting out for a stereo 44.1 kHz
    // stand-in and hands it to every file: 128 kbps for WMA, 160 for AAC.
    // A mono 48 kHz file and a 5.1 file each take the nearest they list.
    let dir = tempfile::tempdir().unwrap();
    let mono = dir.path().join("mono.wav");
    write_wav(
        &mono,
        1,
        Channels {
            count: 1,
            mask: 0x4,
        },
        48_000,
    );
    let film = dir.path().join("film.wav");
    write_wav(
        &film,
        1,
        Channels {
            count: 6,
            mask: 0x3F,
        },
        48_000,
    );
    for (t, kbps, input) in [
        (Target::Wma, 128, &mono),
        (Target::Aac, 160, &film),
        (Target::M4a, 160, &film),
        (Target::Ac3, 256, &mono),
    ] {
        if expected_failure(t).is_some() {
            continue;
        }
        let tune = Tune {
            setting: Some(Setting::Kbps { kbps }),
            ..Tune::default()
        };
        let out = dir.path().join(format!("batch.{}", t.ext()));
        convert(input, &out, t, Preset::High, &tune, &mut |_| {}, &|| false)
            .unwrap_or_else(|e| panic!("{t:?} at {kbps} kbps: {e}"));
        assert!(is_codec_of(t, &probe(&out).unwrap().codec), "{t:?}");
    }
}

/// Windows' own byte stream over a file cached its writes and, under load,
/// could read a block back before writing it: the WMA came out with its
/// audio replaced by garbage. Many at once makes that load.
#[cfg(windows)]
#[test]
fn wma_written_many_at_once_reads_back_whole() {
    if expected_failure(Target::Wma).is_some() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let want = decoded_secs(&data("tone.wav"));
    let bad: Vec<String> = std::thread::scope(|s| {
        let jobs: Vec<_> = (0..12)
            .map(|n| {
                let dir = dir.path().to_path_buf();
                s.spawn(move || {
                    let mut bad = Vec::new();
                    for round in 0..3 {
                        let out = dir.join(format!("w{n}_{round}.wma"));
                        high(&data("tone.wav"), &out, Target::Wma).unwrap();
                        let got = decoded_secs(&out);
                        let size = std::fs::metadata(&out).unwrap().len();
                        // 1 s at 48 kbps or so: nowhere near a padded 64 KiB.
                        if (got - want).abs() > 0.1 || size >= 65_536 {
                            bad.push(format!("{}: {got:.3} s, {size} bytes", out.display()));
                        }
                    }
                    bad
                })
            })
            .collect();
        jobs.into_iter().flat_map(|j| j.join().unwrap()).collect()
    });
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn what_symphonia_cannot_decode_goes_to_the_next_reader() {
    // Opus and WavPack have Kiln's own readers.
    assert_eq!(probe(&data("tone_opus.ogg")).unwrap().codec, Codec::Opus);
    assert_eq!(probe(&data("tone.wv")).unwrap().codec, Codec::WavPack);
    // AC3 in a container goes to Media Foundation.
    for name in ["ac3.mkv", "ac3.mp4"] {
        match probe(&data(name)) {
            Ok(info) => assert_eq!(info.codec, Codec::Ac3, "{name}"),
            Err(e) => {
                let mf = kiln_media::mf_codecs().ac3_dec;
                assert!(
                    !mf && matches!(e, MediaError::NeedsMediaFoundation),
                    "{name}: {e:?}"
                );
            }
        }
    }
    // What nothing reads is named.
    assert!(matches!(
        probe(&data("dts.mkv")),
        Err(MediaError::Unsupported { codec }) if codec == "DTS"
    ));
}

/// Media Foundation reads from work-queue threads too: many readers at once
/// must each get their whole file.
#[cfg(windows)]
#[test]
fn media_foundation_reads_many_at_once() {
    if !kiln_media::mf_codecs().wma_dec || !kiln_media::mf_codecs().ac3_dec {
        return;
    }
    let want: Vec<(&str, u64)> = ["tone.wma", "tone.ac3", "tone51.ac3"]
        .into_iter()
        .map(|n| {
            let mut s = open_source(&data(n)).unwrap();
            (n, count_frames(&mut *s))
        })
        .collect();
    std::thread::scope(|s| {
        for _ in 0..8 {
            let want = &want;
            s.spawn(move || {
                for _ in 0..3 {
                    for (name, frames) in want {
                        let mut src = open_source(&data(name)).unwrap();
                        assert_eq!(count_frames(&mut *src), *frames, "{name}");
                    }
                }
            });
        }
    });
}

/// The strength of `hz` in one channel of interleaved samples (Goertzel).
fn tone_power(samples: &[f32], channels: usize, ch: usize, rate: u32, hz: f64) -> f64 {
    let w = 2.0 * PI * hz / rate as f64;
    let coeff = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for frame in samples.chunks_exact(channels) {
        let s0 = frame[ch] as f64 + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    s1 * s1 + s2 * s2 - coeff * s1 * s2
}

fn decode_all(path: &Path) -> (u16, Vec<f32>) {
    let mut src = open_source(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let channels = src.info().channels.count;
    let mut block = Block {
        channels: 0,
        rate: 0,
        samples: Vec::new(),
    };
    let mut all = Vec::new();
    while src.read(&mut block).unwrap() {
        all.extend_from_slice(&block.samples);
    }
    (channels, all)
}

#[test]
fn opus_in_webm_reads_and_converts_at_its_length() {
    // One second at 48 kHz, made by FFmpeg's libopus: 312 frames of pre-skip
    // and end padding cut off by the decoder.
    let webm = data("tone_opus.webm");
    let info = probe(&webm).unwrap();
    assert_eq!(info.codec, Codec::Opus);
    assert_eq!((info.rate, info.channels.count), (48_000, 2));
    let (_, samples) = decode_all(&webm);
    let frames = samples.len() as u64 / 2;
    assert!(frames.abs_diff(48_000) <= 960, "{frames} frames");
    // The tone is there from the start: the pre-skip is gone.
    let head = samples[..960 * 2].iter().fold(0f32, |m, s| m.max(s.abs()));
    assert!(head > 0.05, "the first 960 frames peak at {head}");

    let dir = tempfile::tempdir().unwrap();
    for t in [Target::Mp3, Target::Flac] {
        let out = dir.path().join(format!("webm.{}", t.ext()));
        high(&webm, &out, t).unwrap_or_else(|e| panic!("{t:?}: {e}"));
        let mut src = open_source(&out).unwrap();
        let got = count_frames(&mut *src);
        let slack = if t == Target::Flac { 0 } else { 1152 };
        assert!(
            got.abs_diff(frames) <= slack,
            "{t:?}: {got} frames, {frames} read"
        );
    }
}

#[test]
fn opus_51_in_mkv_keeps_each_channel_in_place() {
    // FL 440, FR 660, FC 880, LFE 55, BL 1100, BR 1320 Hz, one per channel.
    const TONES: [f64; 6] = [440.0, 660.0, 880.0, 55.0, 1100.0, 1320.0];
    let check = |path: &Path| {
        let (channels, samples) = decode_all(path);
        assert_eq!(channels, 6, "{}", path.display());
        for (ch, want) in TONES.iter().enumerate() {
            let loudest = TONES
                .iter()
                .copied()
                .max_by(|a, b| {
                    let pa = tone_power(&samples, 6, ch, 48_000, *a);
                    let pb = tone_power(&samples, 6, ch, 48_000, *b);
                    pa.total_cmp(&pb)
                })
                .unwrap();
            assert_eq!(loudest, *want, "{}: channel {ch}", path.display());
        }
    };
    let mkv = data("tone51_opus.mkv");
    let info = probe(&mkv).unwrap();
    assert_eq!(info.codec, Codec::Opus);
    assert_eq!(info.channels.mask, 0x3F);
    check(&mkv);
    let dir = tempfile::tempdir().unwrap();
    let flac = dir.path().join("opus51.flac");
    high(&mkv, &flac, Target::Flac).unwrap();
    check(&flac);
}
