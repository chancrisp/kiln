//! Kiln's own AC3 surround encoder: the stream is checked against the A/52
//! syntax here (sync word, frame size, both CRCs), and decoded by Windows'
//! Dolby Digital decoder (`mf::source`) on Windows.

mod common;

use kiln_media::{ac3enc, Block, Channels, MediaError, Setting, Sink, SinkSpec, Tags};
use std::f64::consts::PI;
use std::fs::File;
use std::path::{Path, PathBuf};

const RATE: u32 = 48_000;
const BLOCK: usize = 8192;

/// 5.1 in WAVE order: FL FR FC LFE BL BR.
const MASK_51: u32 = 0x3F;

fn tmp() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let inner = dir.path().join("t\u{f6}ne");
    std::fs::create_dir(&inner).unwrap();
    (dir, inner)
}

fn encode(path: &Path, samples: &[f32], count: u16, mask: u32, kbps: u32) {
    let tags = Tags::default();
    let setting = Setting::Kbps { kbps };
    let spec = SinkSpec {
        rate: RATE,
        channels: Channels { count, mask },
        tags: &tags,
        setting: &setting,
        source_bits: None,
        out: File::create(path).unwrap(),
    };
    let mut sink: Box<dyn Sink> = ac3enc::sink(spec).unwrap();
    for c in samples.chunks(BLOCK * count as usize) {
        sink.write(&Block {
            channels: count,
            rate: RATE,
            samples: c.to_vec(),
        })
        .unwrap();
    }
    sink.finish().unwrap();
}

/// The CRC register of A/52 §7.10.1 (x^16 + x^15 + x^2 + 1, zero start,
/// bits shifted in most significant first) after `bytes`.
fn crc_register(bytes: &[u8]) -> u16 {
    let mut r: u16 = 0;
    for &b in bytes {
        for i in (0..8).rev() {
            let bit = (b >> i) & 1;
            let top = (r >> 15) as u8 & 1;
            r <<= 1;
            if top ^ bit == 1 {
                r ^= 0x8005;
            }
        }
    }
    r
}

/// Words per syncframe at 48 kHz, by frmsizecod / 2 (A/52 Table 5.18).
const WORDS_48K: [usize; 19] = [
    64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 448, 512, 640, 768, 896, 1024, 1152, 1280,
];

/// What a syncframe's header says.
#[derive(Debug, PartialEq, Eq)]
struct Header {
    kbps: u32,
    acmod: u8,
    lfeon: bool,
}

/// Reads the bits of `bytes` most significant first.
struct Bits<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Bits<'_> {
    fn get(&mut self, n: usize) -> u32 {
        let mut v = 0u32;
        for _ in 0..n {
            let bit = (self.bytes[self.at / 8] >> (7 - self.at % 8)) & 1;
            v = (v << 1) | bit as u32;
            self.at += 1;
        }
        v
    }
}

/// Checks every syncframe of an AC3 stream: sync word, 48 kHz, the frame
/// size its code gives, crc1 over the first 5/8 and crc2 over the whole
/// frame (A/52 §5.4.1, §7.10.1). Returns each frame's header.
fn check_frames(bytes: &[u8]) -> Vec<Header> {
    let mut at = 0usize;
    let mut out = Vec::new();
    while at < bytes.len() {
        let f = &bytes[at..];
        assert!(f.len() >= 8, "a short frame at byte {at}");
        assert_eq!(&f[..2], &[0x0B, 0x77], "no sync word at byte {at}");
        let fscod = f[4] >> 6;
        let frmsizecod = (f[4] & 0x3F) as usize;
        assert_eq!(fscod, 0, "not 48 kHz at byte {at}");
        assert!(frmsizecod < 38, "frmsizecod {frmsizecod}");
        let words = WORDS_48K[frmsizecod / 2];
        let size = words * 2;
        assert!(f.len() >= size, "the last frame is cut short");
        let frame = &f[..size];
        let five_eighths = ((words >> 1) + (words >> 3)) * 2;
        assert_eq!(
            crc_register(&frame[2..five_eighths]),
            0,
            "crc1 at byte {at}"
        );
        assert_eq!(crc_register(&frame[2..]), 0, "crc2 at byte {at}");
        let mut b = Bits {
            bytes: frame,
            at: 40,
        };
        let bsid = b.get(5);
        assert_eq!(bsid, 8, "bsid");
        let _bsmod = b.get(3);
        let acmod = b.get(3) as u8;
        if acmod & 1 != 0 && acmod != 1 {
            b.get(2);
        }
        if acmod & 4 != 0 {
            b.get(2);
        }
        if acmod == 2 {
            b.get(2);
        }
        let lfeon = b.get(1) == 1;
        let kbps = [
            32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 448, 512, 576, 640,
        ][frmsizecod / 2];
        out.push(Header { kbps, acmod, lfeon });
        at += size;
    }
    out
}

/// Something that reads like music to an encoder, in every channel.
/// Deterministic.
fn music(frames: usize, channels: usize) -> Vec<f32> {
    let mut state = 0x1234_5678_9ABC_DEF0u64;
    let mut out = Vec::with_capacity(frames * channels);
    for i in 0..frames {
        let t = i as f64 / RATE as f64;
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

/// Pink noise (Voss-McCartney, 16 rows), about -20 dBFS RMS. Deterministic.
fn pink(frames: usize, seed: u64) -> Vec<f32> {
    let mut state = seed;
    let mut next = move || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (state >> 40) as f64 / (1u64 << 24) as f64 - 0.5
    };
    let mut rows = [0.0f64; 16];
    for r in rows.iter_mut() {
        *r = next();
    }
    let mut sum: f64 = rows.iter().sum();
    let mut out = Vec::with_capacity(frames);
    for i in 1..=frames {
        // The row whose turn it is: the number of trailing zeros of i.
        let r = (i.trailing_zeros() as usize).min(15);
        sum -= rows[r];
        rows[r] = next();
        sum += rows[r];
        out.push(((sum + next()) * 0.085) as f32);
    }
    out
}

/// `x` low-passed by four one-pole filters at `hz`.
fn low_pass(x: &[f32], hz: f64) -> Vec<f32> {
    let a = (-2.0 * PI * hz / RATE as f64).exp();
    let mut s = [0.0f64; 4];
    x.iter()
        .map(|&v| {
            let mut y = v as f64;
            for st in s.iter_mut() {
                *st = (1.0 - a) * y + a * *st;
                y = *st;
            }
            y as f32
        })
        .collect()
}

fn interleave(chans: &[Vec<f32>]) -> Vec<f32> {
    let n = chans[0].len();
    let mut out = Vec::with_capacity(n * chans.len());
    for i in 0..n {
        for c in chans {
            out.push(c[i]);
        }
    }
    out
}

fn channel(samples: &[f32], count: usize, ch: usize) -> Vec<f32> {
    samples.chunks(count).map(|f| f[ch]).collect()
}

fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|&v| v as f64 * v as f64).sum::<f64>() / x.len().max(1) as f64).sqrt()
}

/// FFmpeg's verdict on `path` when FFmpeg is on PATH: decoded with no
/// complaint (CRCs checked), and what it said. `None` without FFmpeg.
fn ffmpeg_decodes(path: &Path) -> Option<(bool, String)> {
    let out = std::process::Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-v",
            "error",
            "-err_detect",
            "crccheck+bitstream",
            "-i",
        ])
        .arg(path)
        .args(["-f", "null", "-"])
        .output()
        .ok()?;
    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Some((out.status.success() && err.is_empty(), err))
}

#[test]
fn frames_are_valid_ac3() {
    let (_d, dir) = tmp();
    let path = dir.join("out.ac3");
    encode(&path, &music(RATE as usize, 6), 6, MASK_51, 448);
    let bytes = std::fs::read(&path).unwrap();
    // 448 kbps is 1,792 bytes per 1,536-sample syncframe.
    let size = 448 * 1000 / 8 * 1536 / 48_000;
    assert_eq!(size, 1792);
    assert_eq!(bytes.len() % size, 0, "{} bytes", bytes.len());
    let headers = check_frames(&bytes);
    // One second plus the 256-sample transform delay.
    assert_eq!(headers.len(), (48_000 + 256usize).div_ceil(1536));
    for h in &headers {
        assert_eq!(
            *h,
            Header {
                kbps: 448,
                acmod: 7,
                lfeon: true
            }
        );
    }
}

#[test]
fn every_input_sample_is_kept() {
    // Input that ends in the last 256 samples of a syncframe still gets a
    // syncframe for its end, after the 256-sample lead (R27).
    let (_d, dir) = tmp();
    for frames in [1536 * 3 - 100, 1536 * 3, 1536 * 3 - 256, 1] {
        let path = dir.join(format!("n{frames}.ac3"));
        encode(&path, &music(frames, 6), 6, MASK_51, 448);
        let headers = check_frames(&std::fs::read(&path).unwrap());
        let decoded = headers.len() * 1536;
        assert_eq!(headers.len(), (frames + 256).div_ceil(1536), "{frames}");
        assert!(decoded >= frames + 256 && decoded - frames <= 1536 + 256);
    }
}

#[test]
fn every_bitrate_and_mode_encodes() {
    let (_d, dir) = tmp();
    // (WAVE mask, acmod, lfeon)
    let modes: [(u32, u8, bool); 12] = [
        (0x7, 3, false),   // 3/0
        (0xF, 3, true),    // 3/0 + LFE
        (0x103, 4, false), // 2/1
        (0x10B, 4, true),  // 2/1 + LFE
        (0x107, 5, false), // 3/1
        (0x10F, 5, true),  // 3/1 + LFE
        (0x33, 6, false),  // 2/2
        (0x3B, 6, true),   // 2/2 + LFE
        (0x37, 7, false),  // 3/2
        (0x3F, 7, true),   // 3/2 + LFE
        (0x60F, 7, true),  // 3/2 + LFE, side surrounds
        (0xB, 2, true),    // 2/0 + LFE
    ];
    for (mask, acmod, lfeon) in modes {
        let count = mask.count_ones() as u16;
        let samples = music(RATE as usize / 2, count as usize);
        for kbps in [192, 384, 640] {
            let path = dir.join(format!("m{mask:x}_{kbps}.ac3"));
            encode(&path, &samples, count, mask, kbps);
            let bytes = std::fs::read(&path).unwrap();
            let headers = check_frames(&bytes);
            assert_eq!(headers.len(), (24_000 + 256usize).div_ceil(1536));
            for h in headers {
                assert_eq!(h, Header { kbps, acmod, lfeon }, "mask {mask:#x}");
            }
            // Windows' decoder maps the layouts other than 5.1 to its own
            // speakers, so FFmpeg (when it is there) checks these.
            if let Some((ok, err)) = ffmpeg_decodes(&path) {
                assert!(ok, "mask {mask:#x} at {kbps}: ffmpeg said {err}");
            }
        }
    }
}

#[test]
fn a_bare_count_takes_the_default_layout() {
    let (_d, dir) = tmp();
    // 3 → 3/0, 4 → 2/2, 5 → 3/2, 6 → 3/2 + LFE.
    for (count, acmod, lfeon) in [
        (3u16, 3u8, false),
        (4, 6, false),
        (5, 7, false),
        (6, 7, true),
    ] {
        let path = dir.join(format!("c{count}.ac3"));
        encode(&path, &music(4000, count as usize), count, 0, 384);
        let headers = check_frames(&std::fs::read(&path).unwrap());
        assert_eq!(
            headers[0],
            Header {
                kbps: 384,
                acmod,
                lfeon
            },
            "{count} ch"
        );
    }
}

#[test]
fn rates_too_low_and_bad_inputs_are_encoder_errors() {
    let (_d, dir) = tmp();
    let tags = Tags::default();
    let try_sink = |count: u16, mask: u32, kbps: u32, rate: u32| {
        let setting = Setting::Kbps { kbps };
        ac3enc::sink(SinkSpec {
            rate,
            channels: Channels { count, mask },
            tags: &tags,
            setting: &setting,
            source_bits: None,
            out: File::create(dir.join("x.ac3")).unwrap(),
        })
        .err()
    };
    // At least 16 kbps a channel: 5.1 takes 96 but not 80.
    assert!(try_sink(6, MASK_51, 96, RATE).is_none());
    assert!(matches!(
        try_sink(6, MASK_51, 80, RATE),
        Some(MediaError::Encoder(_))
    ));
    assert!(try_sink(3, 0x7, 48, RATE).is_none());
    assert!(matches!(
        try_sink(3, 0x7, 40, RATE),
        Some(MediaError::Encoder(_))
    ));
    // Not an A/52 bitrate.
    assert!(matches!(
        try_sink(6, MASK_51, 450, RATE),
        Some(MediaError::Encoder(_))
    ));
    // Only 48 kHz, only 3 to 6 channels.
    assert!(matches!(
        try_sink(6, MASK_51, 448, 44_100),
        Some(MediaError::Encoder(_))
    ));
    assert!(matches!(
        try_sink(2, 0x3, 192, RATE),
        Some(MediaError::Encoder(_))
    ));
    assert!(matches!(
        try_sink(7, 0x70F, 448, RATE),
        Some(MediaError::Encoder(_))
    ));
    // Nothing written is an error, as for the other lossy sinks.
    let setting = Setting::Kbps { kbps: 448 };
    let sink = ac3enc::sink(SinkSpec {
        rate: RATE,
        channels: Channels {
            count: 6,
            mask: MASK_51,
        },
        tags: &tags,
        setting: &setting,
        source_bits: None,
        out: File::create(dir.join("e.ac3")).unwrap(),
    })
    .unwrap();
    match sink.finish() {
        Err(MediaError::Encoder(why)) => assert_eq!(why, "AC3 can't be written with no audio."),
        r => panic!("expected the no-audio error, got {r:?}"),
    }
}

#[test]
fn out_of_range_samples_are_clamped() {
    let (_d, dir) = tmp();
    let path = dir.join("loud.ac3");
    let mut s = music(RATE as usize / 2, 6);
    for (i, v) in s.iter_mut().enumerate() {
        *v *= 8.0;
        if i % 997 == 0 {
            *v = f32::NAN;
        }
    }
    encode(&path, &s, 6, MASK_51, 448);
    check_frames(&std::fs::read(&path).unwrap());
}

#[cfg(windows)]
mod decoded {
    use super::*;
    use kiln_media::mf::source;
    use kiln_media::Source;

    fn decode(path: &Path) -> (Box<dyn Source>, Vec<f32>) {
        let mut m = source::open(File::open(path).unwrap()).unwrap();
        let mut out = Vec::new();
        let mut b = Block {
            channels: 0,
            rate: 0,
            samples: Vec::new(),
        };
        while m.read(&mut b).unwrap() {
            out.extend_from_slice(&b.samples);
        }
        (m, out)
    }

    /// The lag (0..max) at which `got` best lines up with `want`.
    fn best_lag(want: &[f32], got: &[f32], max: usize) -> usize {
        let len = want.len().min(got.len()).saturating_sub(max);
        let c = |lag: usize| -> f64 {
            let (mut dot, mut energy) = (0.0f64, 0.0f64);
            for n in 0..len {
                let g = got[n + lag] as f64;
                dot += want[n] as f64 * g;
                energy += g * g;
            }
            dot / energy.sqrt().max(1e-12)
        };
        (0..max)
            .max_by(|&a, &b| c(a).partial_cmp(&c(b)).unwrap())
            .unwrap()
    }

    fn snr_db(want: &[f32], got: &[f32], lag: usize) -> f64 {
        let (mut s, mut e) = (0.0f64, 0.0f64);
        for (n, &w) in want.iter().enumerate() {
            let g = got.get(n + lag).copied().unwrap_or(0.0) as f64;
            s += w as f64 * w as f64;
            e += (w as f64 - g) * (w as f64 - g);
        }
        10.0 * (s / e.max(1e-30)).log10()
    }

    #[test]
    fn ffmpeg_and_windows_decode_it() {
        if common::no_mf() {
            return;
        }
        let (_d, dir) = tmp();
        let path = dir.join("out51.ac3");
        let frames = RATE as usize * 3;
        let src = music(frames, 6);
        encode(&path, &src, 6, MASK_51, 448);

        let (m, got) = decode(&path);
        assert_eq!(m.info().rate, RATE);
        assert_eq!(m.info().channels.count, 6);
        let mask = m.info().channels.mask;
        assert!(mask == 0x3F || mask == 0x60F, "mask {mask:#x}");
        let n = got.len() / 6;
        // Every input sample, plus the 256-sample lead and the last
        // syncframe's padding (R27).
        assert!(
            n >= frames + 256 && n - frames <= 1536 + 256,
            "{n} vs {frames}"
        );
        // The full-band channels keep their level (the LFE carries only
        // its low band, so it is left out).
        for c in [0usize, 1, 2, 4, 5] {
            let want = rms(&channel(&src, 6, c));
            let have = rms(&channel(&got, 6, c));
            let db = 20.0 * (have / want).log10();
            assert!(db.abs() <= 1.0, "channel {c}: {db:.2} dB");
        }
        let all = 20.0 * (rms(&got) * (n as f64 / frames as f64).sqrt() / rms(&src)).log10();
        assert!(all.abs() <= 1.0, "overall {all:.2} dB");

        // FFmpeg, when it is on PATH, decodes it with no complaint.
        if let Some((ok, err)) = ffmpeg_decodes(&path) {
            assert!(ok, "ffmpeg said: {err}");
        }
    }

    #[test]
    fn each_channel_lands_in_place() {
        if common::no_mf() {
            return;
        }
        let (_d, dir) = tmp();
        let frames = RATE as usize;
        for c in 0..6usize {
            let hz = if c == 3 { 60.0 } else { 1000.0 };
            let mut chans = vec![vec![0.0f32; frames]; 6];
            for (i, v) in chans[c].iter_mut().enumerate() {
                *v = (0.5 * (2.0 * PI * hz * i as f64 / RATE as f64).sin()) as f32;
            }
            let path = dir.join(format!("ch{c}.ac3"));
            encode(&path, &interleave(&chans), 6, MASK_51, 448);
            let (_m, got) = decode(&path);
            let energy: Vec<f64> = (0..6)
                .map(|o| {
                    channel(&got, 6, o)
                        .iter()
                        .map(|&v| v as f64 * v as f64)
                        .sum()
                })
                .collect();
            let total: f64 = energy.iter().sum();
            let share = energy[c] / total;
            assert!(
                share >= 0.8,
                "channel {c}: {share:.3} of the energy, {energy:?}"
            );
        }
    }

    #[test]
    fn quality_is_sane() {
        if common::no_mf() {
            return;
        }
        // 3/2 + LFE at 448 kbps. A floor, not a target.
        let (_d, dir) = tmp();
        let frames = RATE as usize * 4;

        // Pink noise in every full-band channel. Its waveform SNR is bounded
        // by the bits there are: 448 kbps leaves about 1.45 mantissa bits a
        // coefficient, and the rate-distortion bound for a Gaussian source
        // with a 1/f spectrum over the 253 coded bins at that rate is 12.3 dB
        // (reverse water-filling). The floor sits just under the bound. The
        // LFE's seven bins get the same SNR offset; it holds about 20 dB.
        let mut chans: Vec<Vec<f32>> = (0..6).map(|c| pink(frames, 0x9E37 + c as u64)).collect();
        // The LFE carries only its band: pink noise under about 120 Hz.
        chans[3] = low_pass(&chans[3], 80.0).iter().map(|v| v * 4.0).collect();
        let path = dir.join("pink.ac3");
        encode(&path, &interleave(&chans), 6, MASK_51, 448);
        let (_m, got) = decode(&path);
        let lag = best_lag(&chans[0], &channel(&got, 6, 0), 4096);
        let mut report = Vec::new();
        for (c, want) in chans.iter().enumerate() {
            let snr = snr_db(want, &channel(&got, 6, c), lag);
            report.push(format!("{c}: {snr:.1} dB"));
            let floor = if c == 3 { 15.0 } else { 10.0 };
            assert!(
                snr >= floor,
                "pink, channel {c}: SNR {snr:.1} dB (lag {lag})"
            );
        }
        eprintln!(
            "448 kbps 5.1 pink noise SNR (lag {lag}): {}",
            report.join(", ")
        );

        // Music-like tones with a little noise: at least 20 dB in each
        // full-band channel (the LFE gets the same full-band signal, so it is
        // left out).
        let src = music(frames, 6);
        let path = dir.join("music.ac3");
        encode(&path, &src, 6, MASK_51, 448);
        let (_m, got) = decode(&path);
        let lag = best_lag(&channel(&src, 6, 0), &channel(&got, 6, 0), 4096);
        let mut report = Vec::new();
        for c in [0usize, 1, 2, 4, 5] {
            let snr = snr_db(&channel(&src, 6, c), &channel(&got, 6, c), lag);
            report.push(format!("{c}: {snr:.1} dB"));
            assert!(
                snr >= 20.0,
                "music, channel {c}: SNR {snr:.1} dB (lag {lag})"
            );
        }
        eprintln!("448 kbps 5.1 music SNR (lag {lag}): {}", report.join(", "));
    }
}

/// Speed: run with `cargo test --release -p kiln-media --test ac3enc -- --ignored`.
#[test]
#[ignore = "a release-build speed check"]
fn speed_51_448() {
    let (_d, dir) = tmp();
    let secs = 30usize;
    let src = music(RATE as usize * secs, 6);
    let path = dir.join("speed.ac3");
    let t = std::time::Instant::now();
    encode(&path, &src, 6, MASK_51, 448);
    let took = t.elapsed().as_secs_f64();
    let x = secs as f64 / took;
    eprintln!("5.1 at 448 kbps: {secs} s of audio in {took:.3} s, {x:.0}x realtime");
    assert!(x >= 20.0, "{x:.1}x realtime");
}
