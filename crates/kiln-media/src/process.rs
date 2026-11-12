//! Channel mixing, resampling and dither: what sits between a source's
//! blocks and a sink's blocks.

use crate::error::Result;
use crate::types::{Block, Channels};
use rubato::{
    calculate_cutoff, Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};
use std::io::{Error, ErrorKind};

/// Frames fed to the resampler at a time: one block.
const CHUNK: usize = 8192;
/// Length of the resampler's sinc filter, in input frames.
const SINC_LEN: usize = 256;
/// 1 / sqrt(2): the -3 dB used for centre and surround channels.
const MINUS_3_DB: f64 = std::f64::consts::FRAC_1_SQRT_2;
/// Seed for the dither generator, so the output is reproducible.
const DITHER_SEED: u64 = 0x9E37_79B9_7F4A_7C15;

fn invalid(what: &str) -> crate::MediaError {
    Error::new(ErrorKind::InvalidInput, what.to_string()).into()
}

/// Turns blocks of one rate and channel layout into blocks of another:
/// mix the channels, then resample, then dither.
pub struct Process {
    from_rate: u32,
    to_rate: u32,
    to_channels: u16,
    mixer: Mixer,
    resampler: Option<Resample>,
    dither: Option<Dither>,
    mixed: Vec<f32>,
}

impl Process {
    /// `dither_to_bits` is the bit depth the output will later be rounded to
    /// (16 or 24, say), or `None` when it won't be rounded.
    pub fn new(
        from_rate: u32,
        from: Channels,
        to_rate: u32,
        to: Channels,
        dither_to_bits: Option<u16>,
    ) -> Result<Process> {
        if from_rate == 0 || to_rate == 0 {
            return Err(invalid("a sample rate of zero"));
        }
        if from.count == 0 || to.count == 0 {
            return Err(invalid("a channel count of zero"));
        }
        if matches!(dither_to_bits, Some(b) if b == 0 || b > 32) {
            return Err(invalid("a dither depth outside 1 to 32 bits"));
        }
        let resampler = if from_rate == to_rate {
            None
        } else {
            Some(Resample::new(from_rate, to_rate, to.count as usize)?)
        };
        Ok(Process {
            from_rate,
            to_rate,
            to_channels: to.count,
            mixer: Mixer::new(from, to),
            resampler,
            dither: dither_to_bits.map(Dither::new),
            mixed: Vec::new(),
        })
    }

    /// Converts one block. The output may be shorter than the input while the
    /// resampler fills up (even empty); `flush` returns the rest.
    pub fn run(&mut self, input: &Block, out: &mut Block) {
        out.channels = self.to_channels;
        out.rate = self.to_rate;
        out.samples.clear();
        if self.is_identity() {
            out.samples.extend_from_slice(&input.samples);
            return;
        }
        let mixed: &[f32] = if self.mixer.is_identity() {
            &input.samples
        } else {
            self.mixer.apply(&input.samples, &mut self.mixed);
            &self.mixed
        };
        match &mut self.resampler {
            Some(r) => r.push(mixed, &mut out.samples),
            None => out.samples.extend_from_slice(mixed),
        }
        if let Some(d) = &mut self.dither {
            d.apply(&mut out.samples);
        }
    }

    /// Drains what the resampler still holds. Afterwards the `Process` starts
    /// over, as if new.
    pub fn flush(&mut self, out: &mut Block) {
        out.channels = self.to_channels;
        out.rate = self.to_rate;
        out.samples.clear();
        if let Some(r) = &mut self.resampler {
            r.finish(&mut out.samples);
        }
        if let Some(d) = &mut self.dither {
            d.apply(&mut out.samples);
            d.reset();
        }
    }

    /// True when blocks pass through untouched: same rate, the same channels
    /// in the same places, and no dither.
    pub fn is_identity(&self) -> bool {
        self.from_rate == self.to_rate && self.mixer.is_identity() && self.dither.is_none()
    }
}

/// Speaker bit positions of WAVEFORMATEXTENSIBLE.
mod spk {
    pub const FL: u32 = 0;
    pub const FR: u32 = 1;
    pub const FC: u32 = 2;
    pub const LFE: u32 = 3;
    pub const BL: u32 = 4;
    pub const BR: u32 = 5;
    pub const FLC: u32 = 6;
    pub const FRC: u32 = 7;
    pub const BC: u32 = 8;
    pub const SL: u32 = 9;
    pub const SR: u32 = 10;
    pub const TC: u32 = 11;
    pub const TFL: u32 = 12;
    pub const TFC: u32 = 13;
    pub const TFR: u32 = 14;
    pub const TBL: u32 = 15;
    pub const TBC: u32 = 16;
    pub const TBR: u32 = 17;
}

/// The speaker mask Windows assumes for a channel count when none is given.
fn default_mask(count: u16) -> u32 {
    match crate::types::default_mask(count) {
        // Past eight channels the first `count` speaker bits.
        0 if count >= 32 => u32::MAX,
        0 => (1u32 << count) - 1,
        mask => mask,
    }
}

/// The speaker bit of each channel, in channel order. A mask that doesn't
/// name exactly `count` speakers is ignored in favour of the default layout.
fn speakers(ch: Channels) -> Vec<u32> {
    let mask = if ch.mask != 0 && ch.mask.count_ones() == ch.count as u32 {
        ch.mask
    } else {
        default_mask(ch.count)
    };
    (0..32)
        .filter(|b| mask & (1 << b) != 0)
        .take(ch.count as usize)
        .collect()
}

/// The stereo mix of one speaker, as (left, right), after ITU-R BS.775:
/// centre and surrounds at -3 dB, the rear centre split across both sides at
/// a further -3 dB, the LFE dropped.
fn stereo_gains(speaker: u32) -> (f64, f64) {
    use spk::*;
    let c = MINUS_3_DB;
    match speaker {
        FL => (1.0, 0.0),
        FR => (0.0, 1.0),
        FC => (c, c),
        LFE => (0.0, 0.0),
        BL | SL | FLC | TFL | TBL => (c, 0.0),
        BR | SR | FRC | TFR | TBR => (0.0, c),
        BC => (c / 2.0, c / 2.0),
        TC | TFC | TBC => (c * c, c * c),
        _ => (0.0, 0.0),
    }
}

/// Where a speaker the target lacks is folded to: the first group whose
/// speakers the target all has. A group of two splits the speaker between
/// them at -3 dB each.
fn fold_groups(speaker: u32) -> &'static [&'static [u32]] {
    use spk::*;
    match speaker {
        FC | TFC | TC => &[&[FC], &[FL, FR]],
        SL => &[&[BL], &[FL]],
        SR => &[&[BR], &[FR]],
        BL => &[&[SL], &[FL]],
        BR => &[&[SR], &[FR]],
        BC | TBC => &[&[BC], &[BL, BR], &[SL, SR], &[FL, FR]],
        FLC | TFL => &[&[FL]],
        FRC | TFR => &[&[FR]],
        TBL => &[&[BL], &[SL], &[FL]],
        TBR => &[&[BR], &[SR], &[FR]],
        _ => &[],
    }
}

/// The mix from one surround layout to another (both wider than stereo), by
/// speaker: each speaker goes to the same speaker in the target, and one the
/// target lacks is folded in after ITU-R BS.775: side and back pairs stand in
/// for each other, the back centre splits across the surrounds, the centre
/// across the fronts. A speaker folded onto one that already carries its own
/// sound goes in at -3 dB. A target speaker with no source stays silent, and
/// an LFE the target lacks is dropped. When folding makes a row add up to
/// more than one, every row is scaled down alike, so full-scale input can't
/// clip. For 7.1 to 5.1 that lowers the whole mix by about 4.6 dB, as
/// FFmpeg's default rematrix does; no clipping matters more than keeping the
/// level (ruling R29).
fn surround_matrix(from: Channels, to: Channels) -> Vec<f64> {
    let (src, dst) = (speakers(from), speakers(to));
    let (nf, nt) = (src.len(), dst.len());
    let slot = |bit: u32| dst.iter().position(|d| *d == bit);
    let direct: Vec<usize> = src.iter().filter_map(|s| slot(*s)).collect();
    let mut matrix = vec![0.0; nt * nf];
    for (i, s) in src.iter().enumerate() {
        if let Some(o) = slot(*s) {
            matrix[o * nf + i] = 1.0;
            continue;
        }
        let Some(group) = fold_groups(*s)
            .iter()
            .find(|g| g.iter().all(|b| slot(*b).is_some()))
        else {
            continue;
        };
        for b in group.iter() {
            let o = slot(*b).expect("checked above");
            let gain = if group.len() > 1 || direct.contains(&o) {
                MINUS_3_DB
            } else {
                1.0
            };
            matrix[o * nf + i] += gain;
        }
    }
    let loudest = matrix
        .chunks_exact(nf)
        .map(|row| row.iter().map(|c| c.abs()).sum::<f64>())
        .fold(0.0, f64::max);
    if loudest > 1.0 {
        matrix.iter_mut().for_each(|c| *c /= loudest);
    }
    matrix
}

/// A channel-mixing matrix, rows = output channels.
struct Mixer {
    from: usize,
    to: usize,
    /// `to` rows of `from` coefficients. Empty when the mix is a pass-through.
    matrix: Vec<f64>,
}

impl Mixer {
    fn new(from: Channels, to: Channels) -> Mixer {
        let (nf, nt) = (from.count as usize, to.count as usize);
        // The same count of surround channels in other places (6.0 to 5.1,
        // say) is mapped speaker by speaker; otherwise an equal count passes.
        if nf == nt && (nf <= 2 || speakers(from) == speakers(to)) {
            return Mixer {
                from: nf,
                to: nt,
                matrix: Vec::new(),
            };
        }
        if nf > 2 && nt > 2 {
            return Mixer {
                from: nf,
                to: nt,
                matrix: surround_matrix(from, to),
            };
        }
        // Step one: everything goes to stereo (2 x nf).
        let mut stereo = vec![0.0; 2 * nf];
        match nf {
            1 => {
                stereo[0] = 1.0;
                stereo[1] = 1.0;
            }
            2 => {
                stereo[0] = 1.0;
                stereo[3] = 1.0;
            }
            _ => {
                for (i, s) in speakers(from).into_iter().enumerate() {
                    let (l, r) = stereo_gains(s);
                    stereo[i] = l;
                    stereo[nf + i] = r;
                }
                // One factor for both rows, so full-scale input can't clip.
                let row = |r: usize| stereo[r * nf..(r + 1) * nf].iter().map(|c| c.abs()).sum();
                let loudest = f64::max(row(0), row(1));
                if loudest > 0.0 {
                    stereo.iter_mut().for_each(|c| *c /= loudest);
                }
            }
        }
        // Step two: stereo to the target (nt x 2).
        let mut target = vec![0.0; nt * 2];
        match nt {
            1 => {
                target[0] = 0.5;
                target[1] = 0.5;
            }
            2 => {
                target[0] = 1.0;
                target[3] = 1.0;
            }
            _ => {
                let layout = speakers(to);
                let at = |bit: u32, fallback: usize| {
                    layout.iter().position(|s| *s == bit).unwrap_or(fallback)
                };
                target[at(spk::FL, 0) * 2] = 1.0;
                target[at(spk::FR, 1) * 2 + 1] = 1.0;
            }
        }
        let mut matrix = vec![0.0; nt * nf];
        for o in 0..nt {
            for i in 0..nf {
                matrix[o * nf + i] = target[o * 2] * stereo[i] + target[o * 2 + 1] * stereo[nf + i];
            }
        }
        Mixer {
            from: nf,
            to: nt,
            matrix,
        }
    }

    fn is_identity(&self) -> bool {
        self.matrix.is_empty()
    }

    /// Mixes interleaved `input` into `out`. Sums in `f64` so a row that adds
    /// up to exactly 1.0 stays at or under full scale after rounding to `f32`.
    fn apply(&self, input: &[f32], out: &mut Vec<f32>) {
        out.clear();
        out.reserve(input.len() / self.from * self.to);
        for frame in input.chunks_exact(self.from) {
            for row in self.matrix.chunks_exact(self.from) {
                let sum: f64 = row.iter().zip(frame).map(|(c, s)| c * *s as f64).sum();
                out.push(sum as f32);
            }
        }
    }
}

/// A `SincFixedIn` fed in 8,192-frame chunks, buffering so any block length
/// goes in, with its tail drained on `finish`.
///
/// rubato's windowed sinc is centred, so it adds no filter delay, but its
/// first output frame sits `ratio - 1` output frames late. `skip` (when
/// upsampling) or `lead_in` (when downsampling) cancels that to within half an
/// input frame, so the output lines up with the input in time.
struct Resample {
    inner: SincFixedIn<f32>,
    channels: usize,
    ratio: f64,
    /// Output frames to drop from the start.
    skip_at_start: usize,
    /// Zero frames fed before the real input.
    lead_in: usize,
    skip: usize,
    /// Input not yet fed to the resampler, one vector per channel.
    pending: Vec<Vec<f32>>,
    out_buf: Vec<Vec<f32>>,
    /// Input frames received, and output frames handed back, since the start.
    fed: u64,
    emitted: u64,
}

impl Resample {
    fn new(from_rate: u32, to_rate: u32, channels: usize) -> Result<Resample> {
        let ratio = to_rate as f64 / from_rate as f64;
        let params = SincInterpolationParameters {
            sinc_len: SINC_LEN,
            f_cutoff: calculate_cutoff(SINC_LEN, WindowFunction::BlackmanHarris2),
            interpolation: SincInterpolationType::Cubic,
            oversampling_factor: 256,
            window: WindowFunction::BlackmanHarris2,
        };
        let inner = SincFixedIn::<f32>::new(ratio, 1.0, params, CHUNK, channels)
            .map_err(|e| invalid(&format!("a resampling ratio rubato refused: {e}")))?;
        let out_buf = inner.output_buffer_allocate(true);
        let skip_at_start = (ratio.round() as usize).saturating_sub(1);
        let lead_in = if ratio < 1.0 {
            (1.0 / ratio - 1.0).round() as usize
        } else {
            0
        };
        let mut r = Resample {
            inner,
            channels,
            ratio,
            skip_at_start,
            lead_in,
            skip: 0,
            pending: vec![Vec::new(); channels],
            out_buf,
            fed: 0,
            emitted: 0,
        };
        r.start_over();
        Ok(r)
    }

    /// Forgets all audio and gets ready for a new stream.
    fn start_over(&mut self) {
        self.inner.reset();
        for p in &mut self.pending {
            p.clear();
            p.resize(self.lead_in, 0.0);
        }
        self.skip = self.skip_at_start;
        self.fed = 0;
        self.emitted = 0;
    }

    /// Takes interleaved frames and appends whatever output is ready.
    fn push(&mut self, interleaved: &[f32], out: &mut Vec<f32>) {
        for frame in interleaved.chunks_exact(self.channels) {
            for (p, s) in self.pending.iter_mut().zip(frame) {
                p.push(*s);
            }
        }
        self.fed += (interleaved.len() / self.channels) as u64;
        while self.pending[0].len() >= CHUNK {
            self.chunk(out, u64::MAX);
        }
    }

    /// Runs one chunk (zero-padded when less than a chunk is pending) and
    /// appends its output, trimmed at the start and capped so the total never
    /// passes `limit` frames.
    fn chunk(&mut self, out: &mut Vec<f32>, limit: u64) {
        for p in &mut self.pending {
            p.resize(CHUNK.max(p.len()), 0.0);
        }
        let n = {
            let input: Vec<&[f32]> = self.pending.iter().map(|p| &p[..CHUNK]).collect();
            let (_, n) = self
                .inner
                .process_into_buffer(&input, &mut self.out_buf, None)
                .expect("the buffers are sized by the resampler itself");
            n
        };
        for p in &mut self.pending {
            p.drain(..CHUNK);
        }
        let start = self.skip.min(n);
        self.skip -= start;
        let room = limit.saturating_sub(self.emitted);
        let end = n.min(start.saturating_add(room.min(n as u64) as usize));
        for i in start..end {
            for ch in &self.out_buf {
                out.push(ch[i]);
            }
        }
        self.emitted += (end - start) as u64;
    }

    /// Pushes the remaining input and enough silence through to drain the
    /// filter's reach past the last input frame, so the total is
    /// round(frames_in * ratio), then starts over.
    fn finish(&mut self, out: &mut Vec<f32>) {
        let target = (self.fed as f64 * self.ratio).round() as u64;
        let mut rounds = 0;
        while self.emitted < target {
            self.chunk(out, target);
            rounds += 1;
            debug_assert!(rounds < 64, "the resampler isn't draining");
            if rounds >= 64 {
                break;
            }
        }
        self.start_over();
    }
}

/// Triangular dither of plus or minus one least significant bit at the target
/// depth, from a fixed-seed xorshift generator.
struct Dither {
    lsb: f32,
    state: u64,
}

impl Dither {
    fn new(bits: u16) -> Dither {
        Dither {
            lsb: (1.0 / 2f64.powi(bits as i32 - 1)) as f32,
            state: DITHER_SEED,
        }
    }

    fn reset(&mut self) {
        self.state = DITHER_SEED;
    }

    /// Uniform in [0, 1).
    fn next(&mut self) -> f32 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        (self.state >> 40) as f32 / (1u32 << 24) as f32
    }

    /// The sum of two uniforms minus one makes a triangle over (-1, 1) LSB.
    /// It goes in before the integer rounding, which adds up to half an LSB.
    fn apply(&mut self, samples: &mut [f32]) {
        for s in samples {
            let tpdf = self.next() - self.next();
            *s += tpdf * self.lsb;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    const STEREO: Channels = Channels {
        count: 2,
        mask: 0x3,
    };
    const MONO: Channels = Channels {
        count: 1,
        mask: 0x4,
    };
    const SURROUND_51: Channels = Channels {
        count: 6,
        mask: 0x3F,
    };
    const SURROUND_71: Channels = Channels {
        count: 8,
        mask: 0x63F,
    };

    fn block(channels: u16, rate: u32, samples: Vec<f32>) -> Block {
        Block {
            channels,
            rate,
            samples,
        }
    }

    fn empty() -> Block {
        block(0, 0, Vec::new())
    }

    /// A mono sine at `amp`, `frames` long.
    fn sine(freq: f64, rate: u32, frames: usize, amp: f64) -> Vec<f32> {
        (0..frames)
            .map(|n| (amp * (2.0 * PI * freq * n as f64 / rate as f64).sin()) as f32)
            .collect()
    }

    /// Runs `samples` through `p` in blocks of 8,192 frames, then flushes.
    fn convert(p: &mut Process, channels: u16, rate: u32, samples: &[f32]) -> Vec<f32> {
        let mut all = Vec::new();
        let mut out = empty();
        for chunk in samples.chunks(8192 * channels as usize) {
            p.run(&block(channels, rate, chunk.to_vec()), &mut out);
            all.extend_from_slice(&out.samples);
        }
        p.flush(&mut out);
        all.extend_from_slice(&out.samples);
        all
    }

    /// The frequency of the strongest bin of a naive DFT over `window` samples
    /// taken from the middle of `x` (Hann windowed).
    fn peak_hz(x: &[f32], rate: u32, window: usize) -> f64 {
        let start = (x.len() - window) / 2;
        let w: Vec<f64> = (0..window)
            .map(|n| {
                let h = 0.5 - 0.5 * (2.0 * PI * n as f64 / window as f64).cos();
                x[start + n] as f64 * h
            })
            .collect();
        let mut best = (0usize, 0.0f64);
        for k in 1..window / 2 {
            let (mut re, mut im) = (0.0, 0.0);
            for (n, v) in w.iter().enumerate() {
                let a = 2.0 * PI * (k * n % window) as f64 / window as f64;
                re += v * a.cos();
                im -= v * a.sin();
            }
            let m = re * re + im * im;
            if m > best.1 {
                best = (k, m);
            }
        }
        best.0 as f64 * rate as f64 / window as f64
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
    }

    #[test]
    fn identity_is_bit_exact() {
        let mut p = Process::new(44100, STEREO, 44100, STEREO, None).unwrap();
        assert!(p.is_identity());
        let samples: Vec<f32> = (0..2000)
            .map(|i| ((i as f32) * 0.37).sin() * 0.999 + f32::EPSILON * i as f32)
            .collect();
        let input = block(2, 44100, samples.clone());
        let mut out = empty();
        p.run(&input, &mut out);
        assert_eq!(out.samples, samples);
        assert_eq!((out.channels, out.rate), (2, 44100));
        p.flush(&mut out);
        assert!(out.samples.is_empty());
    }

    #[test]
    fn surround_51_downmix_does_not_clip() {
        let mut p = Process::new(48000, SURROUND_51, 48000, STEREO, None).unwrap();
        assert!(!p.is_identity());
        let input = block(6, 48000, vec![1.0; 6 * 1000]);
        let mut out = empty();
        p.run(&input, &mut out);
        assert_eq!(out.channels, 2);
        assert_eq!(out.samples.len(), 2000);
        assert!(
            out.samples.iter().all(|s| *s <= 1.0),
            "{:?}",
            &out.samples[..4]
        );
        // The loudest row is scaled to exactly full scale, not merely below it.
        assert!(out.samples.iter().all(|s| *s > 0.99));
    }

    #[test]
    fn eight_k_mono_to_48k_stereo_keeps_a_1k_tone() {
        let frames = 20_000;
        let tone = sine(1000.0, 8000, frames, 0.5);
        let mut p = Process::new(8000, MONO, 48000, STEREO, None).unwrap();
        let out = convert(&mut p, 1, 8000, &tone);
        assert_eq!(out.len() % 2, 0);
        let out_frames = out.len() / 2;
        assert!(
            out_frames.abs_diff(frames * 6) <= 1,
            "{out_frames} vs {}",
            frames * 6
        );
        let left: Vec<f32> = out.chunks(2).map(|f| f[0]).collect();
        let right: Vec<f32> = out.chunks(2).map(|f| f[1]).collect();
        assert_eq!(left, right, "mono to stereo copies");
        let hz = peak_hz(&left, 48000, 8192);
        assert!((hz - 1000.0).abs() <= 10.0, "peak at {hz} Hz");
    }

    #[test]
    fn output_lines_up_with_input_in_time() {
        // A resampled sine must match the sine computed directly at the new
        // rate, up or down: no lag is left in front of the audio.
        for (from, to) in [
            (8000, 48000),
            (44100, 48000),
            (48000, 44100),
            (96000, 48000),
        ] {
            let tone = sine(300.0, from, 20_000, 0.5);
            let mut p = Process::new(from, MONO, to, MONO, None).unwrap();
            let out = convert(&mut p, 1, from, &tone);
            let want = sine(300.0, to, out.len(), 0.5);
            let worst = (500..out.len() - 500)
                .map(|i| (out[i] - want[i]).abs())
                .fold(0.0f32, f32::max);
            assert!(worst < 0.01, "{from}->{to}: worst error {worst}");
        }
    }

    #[test]
    fn rate_192k_to_48k_removes_content_above_24k() {
        // Faded in and out, so the tone's own start and end don't add
        // in-band splatter of their own.
        let mut tone = sine(30_000.0, 192_000, 192_000, 1.0);
        let len = tone.len() as f64;
        for (n, s) in tone.iter_mut().enumerate() {
            *s *= (0.5 - 0.5 * (2.0 * PI * n as f64 / len).cos()) as f32;
        }
        let mut p = Process::new(192_000, MONO, 48_000, MONO, None).unwrap();
        let out = convert(&mut p, 1, 192_000, &tone);
        assert!(out.len().abs_diff(48_000) <= 1);
        let db = 20.0 * rms(&out).log10();
        assert!(db < -90.0, "{db} dBFS");
    }

    #[test]
    fn seven_one_to_stereo() {
        let mut p = Process::new(48000, SURROUND_71, 48000, STEREO, None).unwrap();
        let input = block(8, 48000, vec![0.25; 8 * 100]);
        let mut out = empty();
        p.run(&input, &mut out);
        assert_eq!(out.channels, 2);
        assert_eq!(out.samples.len(), 200);
        // An unlabelled source gets the default layout for its channel count.
        let unlabelled = Channels { count: 8, mask: 0 };
        let mut q = Process::new(48000, unlabelled, 48000, STEREO, None).unwrap();
        let mut out2 = empty();
        q.run(&input, &mut out2);
        assert_eq!(out2.samples, out.samples);
    }

    #[test]
    fn stereo_to_mono_averages_and_other_counts_go_through_stereo() {
        let mut p = Process::new(44100, STEREO, 44100, MONO, None).unwrap();
        let mut out = empty();
        p.run(&block(2, 44100, vec![1.0, 0.0, 0.5, -0.5]), &mut out);
        assert_eq!(out.channels, 1);
        assert_eq!(out.samples, vec![0.5, 0.0]);

        // Mono to 5.1: the copy lands on front left and front right.
        let mut q = Process::new(44100, MONO, 44100, SURROUND_51, None).unwrap();
        q.run(&block(1, 44100, vec![0.5]), &mut out);
        assert_eq!(out.channels, 6);
        assert_eq!(out.samples, vec![0.5, 0.5, 0.0, 0.0, 0.0, 0.0]);

        // 5.1 to mono: the stereo downmix, averaged.
        let mut r = Process::new(44100, SURROUND_51, 44100, MONO, None).unwrap();
        r.run(&block(6, 44100, vec![1.0; 6]), &mut out);
        assert_eq!(out.samples.len(), 1);
        assert!((out.samples[0] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn dither_to_16_is_reproducible_and_small() {
        let tone = sine(440.0, 44100, 20_000, 0.5);
        let run = || {
            let mut p = Process::new(44100, MONO, 44100, MONO, Some(16)).unwrap();
            assert!(!p.is_identity());
            convert(&mut p, 1, 44100, &tone)
        };
        let (a, b) = (run(), run());
        assert_eq!(a, b);
        assert_eq!(a.len(), tone.len());
        let lsb = 1.0 / 32768.0;
        let worst = a
            .iter()
            .zip(&tone)
            .map(|(o, i)| (o - i).abs())
            .fold(0.0f32, f32::max);
        assert!(worst <= 2.0 * lsb, "worst {worst}");
        assert!(worst > 0.1 * lsb, "dither did nothing");
        let mean: f64 = a
            .iter()
            .zip(&tone)
            .map(|(o, i)| (*o - *i) as f64)
            .sum::<f64>()
            / a.len() as f64;
        assert!(mean.abs() < 0.05 * lsb as f64, "mean {mean}");
    }

    #[test]
    fn flush_drains_the_resampler_tail() {
        for (from, to, frames) in [
            (44100u32, 48000u32, 100_001usize),
            (48000, 44100, 50_000),
            (8000, 48000, 8192),
            (96000, 16000, 30_000),
            (44100, 48000, 5),
        ] {
            let mut p = Process::new(from, MONO, to, MONO, None).unwrap();
            let out = convert(&mut p, 1, from, &sine(500.0, from, frames, 0.5));
            let want = (frames as f64 * to as f64 / from as f64).round() as usize;
            assert!(
                out.len().abs_diff(want) <= 1,
                "{from}->{to}: {} vs {want}",
                out.len()
            );
        }
    }

    #[test]
    fn a_process_can_be_used_again_after_flush() {
        let tone = sine(1000.0, 44100, 10_000, 0.5);
        let mut p = Process::new(44100, MONO, 48000, MONO, Some(16)).unwrap();
        let first = convert(&mut p, 1, 44100, &tone);
        let second = convert(&mut p, 1, 44100, &tone);
        assert_eq!(first, second);
    }

    #[test]
    fn block_lengths_do_not_change_the_output() {
        let tone = sine(700.0, 44100, 30_000, 0.5);
        let mut p = Process::new(44100, MONO, 48000, MONO, None).unwrap();
        let whole = convert(&mut p, 1, 44100, &tone);

        let mut q = Process::new(44100, MONO, 48000, MONO, None).unwrap();
        let mut all = Vec::new();
        let mut out = empty();
        let mut rest = &tone[..];
        for len in [1usize, 1000, 7777, 8192, 3, 9999].iter().cycle() {
            let take = (*len).min(rest.len());
            q.run(&block(1, 44100, rest[..take].to_vec()), &mut out);
            all.extend_from_slice(&out.samples);
            rest = &rest[take..];
            if rest.is_empty() {
                break;
            }
        }
        q.flush(&mut out);
        all.extend_from_slice(&out.samples);
        assert_eq!(all, whole);
    }

    #[test]
    fn nothing_in_nothing_out() {
        let mut p = Process::new(44100, STEREO, 48000, STEREO, None).unwrap();
        let mut out = empty();
        p.flush(&mut out);
        assert!(out.samples.is_empty());
    }

    /// Runs one frame through a channel map and returns it.
    fn map_frame(from: Channels, to: Channels, frame: &[f32]) -> Vec<f32> {
        let mut p = Process::new(48000, from, 48000, to, None).unwrap();
        let mut out = empty();
        p.run(&block(from.count, 48000, frame.to_vec()), &mut out);
        assert_eq!(out.channels, to.count);
        out.samples
    }

    #[test]
    fn three_to_five_channels_go_to_51_with_each_speaker_in_place() {
        // 3.0: FL FR FC. The LFE and surrounds stay silent.
        let three = Channels {
            count: 3,
            mask: 0x7,
        };
        assert_eq!(
            map_frame(three, SURROUND_51, &[0.1, 0.2, 0.3]),
            vec![0.1, 0.2, 0.3, 0.0, 0.0, 0.0]
        );
        // 4.0 quad: FL FR BL BR.
        let quad = Channels {
            count: 4,
            mask: 0x33,
        };
        assert_eq!(
            map_frame(quad, SURROUND_51, &[0.1, 0.2, 0.3, 0.4]),
            vec![0.1, 0.2, 0.0, 0.0, 0.3, 0.4]
        );
        // 5.0 with side speakers: FL FR FC SL SR, the sides on the surrounds.
        let five_side = Channels {
            count: 5,
            mask: 0x607,
        };
        assert_eq!(
            map_frame(five_side, SURROUND_51, &[0.1, 0.2, 0.3, 0.4, 0.5]),
            vec![0.1, 0.2, 0.3, 0.0, 0.4, 0.5]
        );
        // An unlabelled 5 channels is the default 5.0 (FL FR FC BL BR).
        let five = Channels { count: 5, mask: 0 };
        assert_eq!(
            map_frame(five, SURROUND_51, &[0.1, 0.2, 0.3, 0.4, 0.5]),
            vec![0.1, 0.2, 0.3, 0.0, 0.4, 0.5]
        );
    }

    #[test]
    fn seven_one_folds_to_51_without_clipping() {
        // FL FR FC LFE BL BR SL SR: the sides fold into the surrounds at -3 dB.
        let one = |i: usize| {
            let mut f = vec![0.0f32; 8];
            f[i] = 1.0;
            map_frame(SURROUND_71, SURROUND_51, &f)
        };
        let fl = one(0);
        let g = fl[0];
        assert!(g > 0.5 && g <= 1.0, "{fl:?}");
        assert_eq!(fl, vec![g, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(one(3), vec![0.0, 0.0, 0.0, g, 0.0, 0.0]);
        assert_eq!(one(4), vec![0.0, 0.0, 0.0, 0.0, g, 0.0]);
        let sl = one(6);
        assert!(
            (sl[4] as f64 - g as f64 * MINUS_3_DB).abs() < 1e-6,
            "{sl:?}"
        );
        assert_eq!(sl.iter().filter(|s| **s != 0.0).count(), 1);
        let sr = one(7);
        assert!(
            (sr[5] as f64 - g as f64 * MINUS_3_DB).abs() < 1e-6,
            "{sr:?}"
        );
        // Full scale everywhere stays at or under full scale.
        let loud = map_frame(SURROUND_71, SURROUND_51, &[1.0; 8]);
        assert!(loud.iter().all(|s| *s <= 1.0), "{loud:?}");
        assert!(loud.iter().any(|s| *s > 0.99), "{loud:?}");
        // 6.1 (FL FR FC LFE BC SL SR): the back centre splits across both surrounds.
        let six_one = Channels {
            count: 7,
            mask: 0x70F,
        };
        let bc = map_frame(six_one, SURROUND_51, &[0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        assert!(bc[4] > 0.0 && bc[4] == bc[5], "{bc:?}");
        assert!(bc[..4].iter().all(|s| *s == 0.0), "{bc:?}");
    }

    #[test]
    fn six_channels_that_are_not_51_are_mapped_by_mask() {
        // 6.0 (FL FR FC BL BR BC) to 5.1: the back centre splits over the
        // surrounds at -3 dB, and the LFE stays silent.
        let six = Channels {
            count: 6,
            mask: 0x137,
        };
        let mut p = Process::new(48000, six, 48000, SURROUND_51, None).unwrap();
        assert!(!p.is_identity());
        let mut out = empty();
        p.run(
            &block(6, 48000, vec![0.0, 0.0, 0.0, 0.0, 0.0, 1.0]),
            &mut out,
        );
        let bc = out.samples.clone();
        assert_eq!(bc[3], 0.0, "{bc:?}");
        assert!(bc[4] > 0.0 && bc[4] == bc[5], "{bc:?}");
        assert!(bc[..4].iter().all(|s| *s == 0.0), "{bc:?}");
        p.run(
            &block(6, 48000, vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
            &mut out,
        );
        let bl = out.samples.clone();
        assert!(
            (bc[4] as f64 / bl[4] as f64 - MINUS_3_DB).abs() < 1e-6,
            "{bc:?} {bl:?}"
        );
        // The same layout under another label is still a pass-through.
        let side = Channels {
            count: 6,
            mask: 0x60F,
        };
        let unlabelled = Channels { count: 6, mask: 0 };
        assert!(Process::new(48000, unlabelled, 48000, SURROUND_51, None)
            .unwrap()
            .is_identity());
        assert!(!Process::new(48000, side, 48000, SURROUND_51, None)
            .unwrap()
            .is_identity());
    }

    #[test]
    fn bad_arguments_are_errors() {
        assert!(Process::new(0, STEREO, 48000, STEREO, None).is_err());
        assert!(Process::new(48000, STEREO, 0, STEREO, None).is_err());
        let none = Channels { count: 0, mask: 0 };
        assert!(Process::new(48000, none, 48000, STEREO, None).is_err());
        assert!(Process::new(48000, STEREO, 48000, none, None).is_err());
        assert!(Process::new(48000, STEREO, 48000, STEREO, Some(0)).is_err());
    }
}
