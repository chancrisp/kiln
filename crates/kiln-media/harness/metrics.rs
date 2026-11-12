//! What the harness measures in a decoded file: loudness (EBU R128
//! integrated, BS.1770 K-weighting and gating), the high-frequency cutoff, and
//! the small statistics the report needs. All of it is plain Rust, with no
//! outside crates: the FFT is a short radix-2 one.

use std::f64::consts::PI;
use std::path::Path;

/// Decoded audio: interleaved `f32`.
pub struct Pcm {
    pub rate: u32,
    pub channels: u16,
    /// The speaker mask, which says the channel order (WAVE order); 0 when
    /// unknown, and then the usual layout for the count is taken.
    pub mask: u32,
    pub samples: Vec<f32>,
}

impl Pcm {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1) as usize
    }

    pub fn secs(&self) -> f64 {
        self.frames() as f64 / self.rate.max(1) as f64
    }
}

/// Reads a whole file through Kiln's own readers (the same ones for what Kiln
/// wrote and what FFmpeg wrote, so a decoder can't tip the comparison).
pub fn decode(path: &Path) -> Result<Pcm, String> {
    let mut src = kiln_media::open_source(path).map_err(|e| e.to_string())?;
    let info = src.info().clone();
    let mut block = kiln_media::Block {
        channels: info.channels.count,
        rate: info.rate,
        samples: Vec::new(),
    };
    let mut samples = Vec::new();
    while src.read(&mut block).map_err(|e| e.to_string())? {
        samples.extend_from_slice(&block.samples);
    }
    Ok(Pcm {
        rate: info.rate,
        channels: info.channels.count,
        mask: info.channels.mask,
        samples,
    })
}

// ---------------------------------------------------------------- loudness

/// One biquad, direct form I.
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    x: [f64; 2],
    y: [f64; 2],
}

impl Biquad {
    fn new(b: [f64; 3], a: [f64; 2]) -> Biquad {
        Biquad {
            b,
            a,
            x: [0.0; 2],
            y: [0.0; 2],
        }
    }

    fn run(&mut self, x0: f64) -> f64 {
        let y0 = self.b[0] * x0 + self.b[1] * self.x[0] + self.b[2] * self.x[1]
            - self.a[0] * self.y[0]
            - self.a[1] * self.y[1];
        self.x = [x0, self.x[0]];
        self.y = [y0, self.y[0]];
        y0
    }
}

/// The two K-weighting stages of BS.1770 for any sample rate: a high shelf
/// (the head's acoustics), then a high-pass.
fn k_weighting(rate: f64) -> [Biquad; 2] {
    let (f0, gain_db, q) = (
        1_681.974_450_955_533,
        3.999_843_853_973_347,
        0.707_175_236_955_419_6,
    );
    let k = (PI * f0 / rate).tan();
    let vh = 10f64.powf(gain_db / 20.0);
    let vb = vh.powf(0.499_666_774_154_541_6);
    let a0 = 1.0 + k / q + k * k;
    let shelf = Biquad::new(
        [
            (vh + vb * k / q + k * k) / a0,
            2.0 * (k * k - vh) / a0,
            (vh - vb * k / q + k * k) / a0,
        ],
        [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
    );
    let (f0, q) = (38.135_470_876_024_44, 0.500_327_037_323_877_3);
    let k = (PI * f0 / rate).tan();
    let a0 = 1.0 + k / q + k * k;
    let high_pass = Biquad::new(
        [1.0, -2.0, 1.0],
        [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
    );
    [shelf, high_pass]
}

/// BS.1770's weight for each channel, by the speaker it feeds: +1.5 dB for
/// the surrounds (back and side, left and right), none for the LFE, and 1
/// for the rest. Channels are in WAVE order, the mask's bits from the lowest.
/// A mask that doesn't match the count gives way to the usual layout for
/// the count; with none, every channel weighs 1.
fn channel_weights(channels: u16, mask: u32) -> Vec<f64> {
    const SURROUND: f64 = 1.412_537_544_622_754_4;
    const LFE: u32 = 0x8;
    // Back left and right, side left and right.
    const SURROUNDS: u32 = 0x10 | 0x20 | 0x200 | 0x400;
    let fits = |m: u32| m.count_ones() == u32::from(channels);
    let mask = if fits(mask) {
        mask
    } else {
        kiln_media::types::default_mask(channels)
    };
    if !fits(mask) {
        return vec![1.0; channels as usize];
    }
    (0..32)
        .map(|b| 1u32 << b)
        .filter(|bit| mask & bit != 0)
        .map(|bit| match bit {
            LFE => 0.0,
            b if b & SURROUNDS != 0 => SURROUND,
            _ => 1.0,
        })
        .collect()
}

/// Integrated loudness in LUFS (EBU R128 / BS.1770-4): 400 ms blocks every
/// 100 ms, an absolute gate at -70 LUFS and a relative gate 10 LU under what
/// passes it. `None` when the file is shorter than one block or silent.
pub fn loudness_lufs(pcm: &Pcm) -> Option<f64> {
    let ch = pcm.channels as usize;
    let n = pcm.frames();
    let rate = pcm.rate as f64;
    let block = (0.4 * rate).round() as usize;
    let step = (0.1 * rate).round() as usize;
    if ch == 0 || block == 0 || step == 0 || n < block {
        return None;
    }
    // Weighted energy per frame, summed over channels, then as a running sum
    // so any block's mean square is two lookups.
    let weights = channel_weights(pcm.channels, pcm.mask);
    let mut energy = vec![0.0f64; n];
    for (c, &w) in weights.iter().enumerate() {
        if w == 0.0 {
            continue;
        }
        let mut stages = k_weighting(rate);
        for (i, e) in energy.iter_mut().enumerate() {
            let mut v = pcm.samples[i * ch + c] as f64;
            for s in stages.iter_mut() {
                v = s.run(v);
            }
            *e += w * v * v;
        }
    }
    let mut sums = Vec::with_capacity(n + 1);
    sums.push(0.0f64);
    let mut running = 0.0;
    for e in &energy {
        running += e;
        sums.push(running);
    }
    let lufs = |z: f64| -0.691 + 10.0 * z.log10();
    let blocks: Vec<f64> = (0..=(n - block) / step)
        .map(|i| (sums[i * step + block] - sums[i * step]) / block as f64)
        .collect();
    let above = |gate: f64, list: &[f64]| -> Vec<f64> {
        list.iter()
            .copied()
            .filter(|&z| z > 0.0 && lufs(z) > gate)
            .collect()
    };
    let mean = |list: &[f64]| list.iter().sum::<f64>() / list.len() as f64;
    let first = above(-70.0, &blocks);
    if first.is_empty() {
        return None;
    }
    let relative = lufs(mean(&first)) - 10.0;
    let second = above(relative, &first);
    if second.is_empty() {
        return None;
    }
    Some(lufs(mean(&second)))
}

// ------------------------------------------------------------------ cutoff

const FFT_SIZE: usize = 4096;

/// In-place radix-2 FFT. `re.len()` is a power of two.
fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let angle = -2.0 * PI / len as f64;
        let (wr, wi) = (angle.cos(), angle.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0, 0.0);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * cr - im[b] * ci;
                let ti = re[b] * ci + im[b] * cr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let next = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = next;
            }
        }
        len <<= 1;
    }
}

/// The long-term average power spectrum: Hann-windowed 4,096-point frames
/// with half overlap, power summed over every channel but the LFE.
fn average_spectrum(pcm: &Pcm) -> Vec<f64> {
    let ch = pcm.channels as usize;
    let n = pcm.frames();
    let window: Vec<f64> = (0..FFT_SIZE)
        .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f64 / FFT_SIZE as f64).cos())
        .collect();
    let mut power = vec![0.0f64; FFT_SIZE / 2 + 1];
    let (mut re, mut im) = (vec![0.0; FFT_SIZE], vec![0.0; FFT_SIZE]);
    let weights = channel_weights(pcm.channels, pcm.mask);
    let frames = if n <= FFT_SIZE {
        1
    } else {
        1 + (n - FFT_SIZE) / (FFT_SIZE / 2)
    };
    for (c, &w) in weights.iter().enumerate() {
        if w == 0.0 {
            continue;
        }
        for f in 0..frames {
            let at = f * FFT_SIZE / 2;
            for i in 0..FFT_SIZE {
                let s = if at + i < n {
                    pcm.samples[(at + i) * ch + c] as f64
                } else {
                    0.0
                };
                re[i] = s * window[i];
                im[i] = 0.0;
            }
            fft(&mut re, &mut im);
            for (p, (r, m)) in power.iter_mut().zip(re.iter().zip(&im)) {
                *p += r * r + m * m;
            }
        }
    }
    power
}

/// The highest frequency (Hz) whose long-term average spectrum is within 30 dB
/// of the average between 1 and 4 kHz. Each bin is judged on the mean of the
/// nine around it, which keeps one noisy bin from setting the answer. 0 when
/// the file has nothing in 1 to 4 kHz.
pub fn cutoff_hz(pcm: &Pcm) -> f64 {
    let power = average_spectrum(pcm);
    let hz = pcm.rate as f64 / FFT_SIZE as f64;
    let lo = (1_000.0 / hz).ceil() as usize;
    let hi = ((4_000.0 / hz).floor() as usize).min(power.len() - 1);
    if hi <= lo {
        return 0.0;
    }
    let reference = power[lo..=hi].iter().sum::<f64>() / (hi - lo + 1) as f64;
    if reference <= 0.0 {
        return 0.0;
    }
    let floor = reference * 1e-3;
    let smooth = |i: usize| {
        let from = i.saturating_sub(4);
        let to = (i + 4).min(power.len() - 1);
        power[from..=to].iter().sum::<f64>() / (to - from + 1) as f64
    };
    (1..power.len())
        .rev()
        .find(|&i| smooth(i) >= floor)
        .map_or(0.0, |i| i as f64 * hz)
}

// -------------------------------------------------------------- statistics

/// The median of a list; `None` when it is empty.
pub fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    let mid = v.len() / 2;
    Some(if v.len() % 2 == 1 {
        v[mid]
    } else {
        (v[mid - 1] + v[mid]) / 2.0
    })
}

// ------------------------------------------------------------------- tests

/// Deterministic white noise in -1..1 (a 64-bit LCG), the same on every run.
pub fn white_noise(len: usize, seed: u64) -> Vec<f32> {
    let mut state = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
    (0..len)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((state >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) as f32
        })
        .collect()
}

/// A windowed-sinc low-pass (Blackman, 511 taps) applied to `x`.
pub fn lowpass(x: &[f32], rate: f64, cutoff: f64) -> Vec<f32> {
    const TAPS: usize = 511;
    let mid = (TAPS / 2) as f64;
    let fc = cutoff / rate;
    let mut h: Vec<f64> = (0..TAPS)
        .map(|i| {
            let t = i as f64 - mid;
            let sinc = if t == 0.0 {
                2.0 * fc
            } else {
                (2.0 * PI * fc * t).sin() / (PI * t)
            };
            let w = 0.42 - 0.5 * (2.0 * PI * i as f64 / (TAPS - 1) as f64).cos()
                + 0.08 * (4.0 * PI * i as f64 / (TAPS - 1) as f64).cos();
            sinc * w
        })
        .collect();
    let sum: f64 = h.iter().sum();
    h.iter_mut().for_each(|v| *v /= sum);
    (0..x.len())
        .map(|n| {
            h.iter()
                .enumerate()
                .filter(|&(k, _)| n >= k)
                .map(|(k, c)| c * x[n - k] as f64)
                .sum::<f64>() as f32
        })
        .collect()
}

/// `secs` of a 1 kHz sine at `lufs` in each of the channels `on`, out of
/// `channels`, the rest silent. In both channels of a stereo pair a full
/// scale sine reads 0 LUFS (the K-weighting is 0 dB there and the -0.691
/// offset cancels), so the amplitude is `lufs` dB under full scale.
fn sine(rate: u32, channels: u16, on: &[usize], lufs: f64, secs: f64) -> Vec<f32> {
    let amp = 10f64.powf(lufs / 20.0);
    let n = (rate as f64 * secs).round() as usize;
    let mut samples = vec![0.0f32; n * channels as usize];
    for i in 0..n {
        let v = (amp * (2.0 * PI * 1_000.0 * i as f64 / rate as f64).sin()) as f32;
        for &c in on {
            samples[i * channels as usize + c] = v;
        }
    }
    samples
}

fn stereo(rate: u32, samples: Vec<f32>) -> Pcm {
    Pcm {
        rate,
        channels: 2,
        mask: 0x3,
        samples,
    }
}

pub fn loudness_of_a_minus_23_lufs_sine_is_minus_23() {
    // At 48 kHz and at 44.1 kHz: the K-weighting is worked out for the rate.
    for rate in [48_000u32, 44_100] {
        let pcm = stereo(rate, sine(rate, 2, &[0, 1], -23.0, 10.0));
        let got = loudness_lufs(&pcm).expect("a loudness");
        assert!(
            (got + 23.0).abs() <= 0.1,
            "{got} LUFS at {rate} Hz, wanted -23.0 +- 0.1"
        );
    }
}

pub fn the_relative_gate_drops_quiet_passages() {
    // A -20 LUFS tone, then 10 s at -40 LUFS: with no relative gate the mean
    // would be about -23, but the gate (10 LU under) leaves only the tone.
    let rate = 48_000u32;
    let tone = sine(rate, 2, &[0, 1], -20.0, 10.0);
    let mut both = tone.clone();
    both.extend(sine(rate, 2, &[0, 1], -40.0, 10.0));
    let alone = loudness_lufs(&stereo(rate, tone)).unwrap();
    let got = loudness_lufs(&stereo(rate, both)).unwrap();
    assert!((got - alone).abs() <= 0.5, "{got} against {alone}");
    // On its own the quiet part passes the absolute gate.
    let quiet = loudness_lufs(&stereo(rate, sine(rate, 2, &[0, 1], -40.0, 10.0))).unwrap();
    assert!((quiet + 40.0).abs() <= 0.1, "{quiet}");
}

pub fn surround_channels_weigh_more_and_the_lfe_nothing() {
    let rate = 48_000u32;
    let on = |channels: u16, mask: u32, c: usize| {
        loudness_lufs(&Pcm {
            rate,
            channels,
            mask,
            samples: sine(rate, channels, &[c], -20.0, 5.0),
        })
    };
    // 5.1 (FL FR FC LFE BL BR): the LFE alone reads as silence does.
    assert_eq!(on(6, 0x3F, 3), None);
    // The same tone on Ls reads 1.5 dB over it on L.
    let left = on(6, 0x3F, 0).unwrap();
    let ls = on(6, 0x3F, 4).unwrap();
    assert!((ls - left - 1.5).abs() < 0.01, "{ls} against {left}");
    // With no mask, six channels are taken as 5.1.
    assert_eq!(on(6, 0, 4), Some(ls));
    // 7.1 (FL FR FC LFE BL BR SL SR): the back and the side pairs are both
    // surrounds.
    for c in [4, 5, 6, 7] {
        let got = on(8, 0x63F, c).unwrap();
        assert!((got - ls).abs() < 0.01, "7.1 channel {c}: {got}");
    }
    assert_eq!(on(8, 0x63F, 3), None);
    // 6.1 (FL FR FC LFE BC SL SR): the back centre weighs as a front does.
    let bc = on(7, 0x70F, 4).unwrap();
    assert!((bc - left).abs() < 0.01, "{bc} against {left}");
    let sl = on(7, 0x70F, 5).unwrap();
    assert!((sl - ls).abs() < 0.01, "{sl} against {ls}");
}

pub fn cutoff_of_a_lowpassed_noise() {
    let rate = 44_100u32;
    let noise = white_noise(rate as usize * 4, 7);
    let filtered = lowpass(&noise, rate as f64, 16_000.0);
    let pcm = Pcm {
        rate,
        channels: 1,
        mask: 0,
        samples: filtered,
    };
    let got = cutoff_hz(&pcm);
    assert!(
        (got - 16_000.0).abs() <= 300.0,
        "cutoff {got} Hz, wanted 16000 +- 300"
    );
}

pub fn the_cutoff_is_where_the_spectrum_falls_30_db() {
    // Noise up to 12 kHz, with a shelf of other noise over it: 25 dB down is
    // within 30 dB, so the cutoff is past 12 kHz; 35 dB down isn't, so the
    // cutoff is at 12 kHz.
    let rate = 44_100u32;
    let n = rate as usize * 2;
    let low = lowpass(&white_noise(n, 3), rate as f64, 12_000.0);
    let shelf = white_noise(n, 5);
    let at = |db: f64| {
        let g = 10f64.powf(-db / 20.0) as f32;
        cutoff_hz(&Pcm {
            rate,
            channels: 1,
            mask: 0,
            samples: low.iter().zip(&shelf).map(|(a, b)| a + g * b).collect(),
        })
    };
    let shallow = at(25.0);
    assert!(shallow > 20_000.0, "25 dB down: cutoff {shallow} Hz");
    let deep = at(35.0);
    assert!(
        (11_500.0..13_000.0).contains(&deep),
        "35 dB down: cutoff {deep} Hz"
    );
}

pub fn loudness_gates_out_silence_and_short_files() {
    let quiet = stereo(48_000, vec![0.0; 48_000 * 2 * 3]);
    assert_eq!(loudness_lufs(&quiet), None);
    let short = stereo(48_000, vec![0.1; 1000]);
    assert_eq!(loudness_lufs(&short), None);
    // A loud sine and as long again of silence reads as the sine alone: the
    // gate drops the silent blocks (the few that straddle the join count, and
    // pull the reading down a little).
    let rate = 48_000u32;
    let mut samples = Vec::new();
    for i in 0..rate as usize * 20 {
        let v = if i < rate as usize * 10 {
            (0.1 * (2.0 * PI * 1_000.0 * i as f64 / rate as f64).sin()) as f32
        } else {
            0.0
        };
        samples.extend_from_slice(&[v, v]);
    }
    let both = loudness_lufs(&stereo(rate, samples.clone())).unwrap();
    let alone = loudness_lufs(&stereo(rate, samples[..rate as usize * 10 * 2].to_vec())).unwrap();
    assert!((both - alone).abs() < 0.1, "{both} against {alone}");
}

pub fn the_fft_finds_a_tone_and_the_median_takes_the_middle() {
    let mut re: Vec<f64> = (0..64)
        .map(|i| (2.0 * PI * 5.0 * i as f64 / 64.0).cos())
        .collect();
    let mut im = vec![0.0; 64];
    fft(&mut re, &mut im);
    let mags: Vec<f64> = re.iter().zip(&im).map(|(r, i)| r.hypot(*i)).collect();
    assert!((mags[5] - 32.0).abs() < 1e-9 && mags[6] < 1e-9);
    assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
    assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]), Some(2.5));
    assert_eq!(median(&[]), None);
}
