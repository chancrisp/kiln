//! Kiln's own AC3 (Dolby Digital) encoder for surround: 3 to 6 channels at
//! 48 kHz, written as a raw elementary stream of syncframes.
//!
//! Written from ATSC A/52:2018 alone:
//!
//! - §5 (bit stream syntax and semantics): syncinfo, bsi, audblk, auxdata
//!   and errorcheck, the frame sizes of Table 5.18 and the channel order of
//!   Table 5.8 (in [`frame`] and here).
//! - §7.1 exponent coding and §8.2.7, §8.2.8 and §8.2.10 for the encoder's
//!   side of it (in [`exponents`]).
//! - §7.2 the parametric bit allocation with the tables of §7.2.3, and
//!   §8.2.12's default parameters and SNR offset search (in [`bitalloc`]
//!   and [`Encoder::frame`]).
//! - §7.3 and §8.2.13 mantissa quantization and grouping (in [`frame`]).
//! - §7.9.4 and §8.2.3 the window and forward transform (in [`mdct`]).
//! - §7.10.1 the CRCs, and the size limits of §5.5 (in [`frame`]).
//!
//! What it does: long blocks only (`blksw` 0), no coupling, no
//! rematrixing (2/0 only, and then sent off), no delta bit allocation,
//! D15/D25/D45 exponents chosen per block with reuse, dither on, and one
//! SNR offset for all channels searched so each frame holds as many
//! mantissa bits as fit; the rest of the frame is auxiliary padding.
//!
//! Delay: the transform overlaps one block, so the decoded audio starts 256
//! samples late (the first 256 decoded samples are silence). Every input
//! sample is kept: the stream has enough syncframes to carry the input plus
//! that lead, so the decoded length is the input's plus 256 to 1,791 (the
//! lead and the last syncframe's padding).
//!
//! §5.5's limits hold in every frame: syncinfo, bsi and blocks 0 and 1 fit
//! in the first 5/8, and block 5's mantissas, the auxiliary data and the
//! error check in the last 3/8. When the mantissas are few (near silence),
//! skip fields (§5.4.3.58-60) in blocks 2 to 5 take the room instead of
//! auxiliary data at the end.

mod bitalloc;
mod exponents;
mod frame;
mod mdct;
#[cfg(test)]
mod parse;
mod tables;

use crate::error::{MediaError, Result};
use crate::pipeline::{no_audio, Sink, SinkSpec};
use crate::tune::Setting;
use crate::types::{default_mask, Block};
use bitalloc::{Cost, Curve, MAX_OFFSET};
use exponents::{Coded, Strategy};
use frame::{BitWriter, BlockExps, Mantissas, Stream, LFE_END};
use mdct::{Mdct, HALF, N};
use std::fs::File;
use std::io::{BufWriter, Write};

/// The one sample rate written.
pub const RATE: u32 = 48_000;
/// Samples per channel in a syncframe: six audio blocks of 256 (§5.1).
pub const FRAME_SAMPLES: usize = 1536;
const BLOCKS: usize = 6;
/// Bits kept free in the last 3/8 for a skip field's rounding up to whole
/// bytes and its 9-bit length (§5.4.3.59).
const SKIP_SLACK: usize = 16;
/// The lowest bitrate taken, per channel (LFE included): below it there is
/// too little room for mantissas once each channel's exponents are sent.
pub const MIN_KBPS_PER_CHANNEL: u32 = 16;

/// How a stream's channels map to A/52's: the audio coding mode (Table 5.8),
/// whether there is an LFE, and for each A/52 channel (full-bandwidth ones
/// in A/52 order, then the LFE) which input channel feeds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub acmod: u32,
    pub lfe: bool,
    pub order: Vec<usize>,
}

// WAVE speaker bits.
const FL: u32 = 0x1;
const FR: u32 = 0x2;
const FC: u32 = 0x4;
const LFE: u32 = 0x8;
const BL: u32 = 0x10;
const BR: u32 = 0x20;
const BC: u32 = 0x100;
const SL: u32 = 0x200;
const SR: u32 = 0x400;

impl Layout {
    /// The layout of `count` channels with speaker `mask`. A mask that
    /// doesn't name `count` speakers AC3 has (L and R, and any of C, one or
    /// two surrounds and the LFE) is replaced by the default for the count:
    /// 3 is 3/0, 4 is 2/2, 5 is 3/2 and 6 is 3/2 with LFE.
    pub fn of(count: u16, mask: u32) -> Option<Layout> {
        Layout::from_mask(count, mask).or_else(|| Layout::from_mask(count, default_mask(count)))
    }

    fn from_mask(count: u16, mask: u32) -> Option<Layout> {
        if mask.count_ones() != count as u32 || mask & (FL | FR) != FL | FR {
            return None;
        }
        if mask & !(FL | FR | FC | LFE | BL | BR | BC | SL | SR) != 0 {
            return None;
        }
        let back = mask & (BL | BR);
        let side = mask & (SL | SR);
        let centre_back = mask & BC != 0;
        let surrounds: Vec<u32> = match (back, side, centre_back) {
            (0, 0, false) => vec![],
            (0, 0, true) => vec![BC],
            (b, 0, false) if b == BL | BR => vec![BL, BR],
            (0, s, false) if s == SL | SR => vec![SL, SR],
            _ => return None,
        };
        // An input channel's index is the number of speakers below its bit.
        let index = |bit: u32| (mask & (bit - 1)).count_ones() as usize;
        let centre = mask & FC != 0;
        let mut order = vec![index(FL)];
        if centre {
            order.push(index(FC));
        }
        order.push(index(FR));
        order.extend(surrounds.iter().map(|&b| index(b)));
        let lfe = mask & LFE != 0;
        if lfe {
            order.push(index(LFE));
        }
        let acmod = match (centre, surrounds.len()) {
            (false, 0) => 2,
            (true, 0) => 3,
            (false, 1) => 4,
            (true, 1) => 5,
            (false, _) => 6,
            (true, _) => 7,
        };
        Some(Layout { acmod, lfe, order })
    }

    /// Full-bandwidth channels: nfchans (Table 5.8).
    pub fn nfchans(&self) -> usize {
        self.order.len() - self.lfe as usize
    }
}

/// `chbwcod` for a full-bandwidth channel given the bitrate each one gets
/// (endmant = (chbwcod + 12) * 3 + 37 bins of 93.75 Hz, §7.1.3): the
/// bandwidth narrows as the bits per channel fall.
fn chbwcod(kbps: u32, nfchans: usize) -> u32 {
    let per = kbps as f32 / nfchans as f32;
    let hz: f32 = if per >= 80.0 {
        // Up to 23.7 kHz, the most AC3 codes.
        24_000.0
    } else if per >= 64.0 {
        20_000.0
    } else if per >= 48.0 {
        18_000.0
    } else if per >= 38.0 {
        16_000.0
    } else if per >= 30.0 {
        14_000.0
    } else if per >= 24.0 {
        12_000.0
    } else {
        9_000.0
    };
    let bins = hz / (RATE as f32 / 2.0 / 256.0);
    (((bins - 37.0) / 3.0).round() as i32 - 12).clamp(0, 60) as u32
}

/// Makes a sink that writes AC3 with Kiln's own encoder: 48 kHz, 3 to 6
/// channels in WAVE order, at any A/52 bitrate (`Setting::Kbps`) from 32 to
/// 640 kbps that gives each channel at least 16 kbps.
pub fn sink(spec: SinkSpec) -> Result<Box<dyn Sink>> {
    let Setting::Kbps { kbps } = *spec.setting else {
        return Err(MediaError::Encoder(
            "this setting isn't an AC3 setting".into(),
        ));
    };
    if spec.rate != RATE {
        return Err(MediaError::Encoder(format!(
            "AC3 is written at 48 kHz, not {} Hz",
            spec.rate
        )));
    }
    let count = spec.channels.count;
    if !(3..=6).contains(&count) {
        return Err(MediaError::Encoder(format!(
            "Kiln's AC3 encoder takes 3 to 6 channels, not {count}"
        )));
    }
    let Some(index) = tables::FRAME_SIZES.iter().position(|&(k, _)| k == kbps) else {
        return Err(MediaError::Encoder(format!("AC3 has no {kbps} kbps rate")));
    };
    let min = MIN_KBPS_PER_CHANNEL * count as u32;
    if kbps < min {
        return Err(MediaError::Encoder(format!(
            "AC3 needs at least {min} kbps for {count} channels"
        )));
    }
    let layout = Layout::of(count, spec.channels.mask)
        .ok_or_else(|| MediaError::Encoder(format!("AC3 has no layout for {count} channels")))?;
    Ok(Box::new(Ac3Sink {
        enc: Encoder::new(&layout, kbps, index),
        layout,
        out: BufWriter::with_capacity(256 * 1024, spec.out),
        pending: vec![Vec::with_capacity(FRAME_SAMPLES * 2); count as usize],
        frames_in: 0,
        frames_out: 0,
        bytes: Vec::new(),
    }))
}

struct Ac3Sink {
    enc: Encoder,
    layout: Layout,
    out: BufWriter<File>,
    /// Input not yet encoded, per A/52 channel, clamped to [-1, 1].
    pending: Vec<Vec<f32>>,
    /// Sample frames taken in, and syncframes written.
    frames_in: u64,
    frames_out: u64,
    bytes: Vec<u8>,
}

impl Ac3Sink {
    fn encode_ready(&mut self) -> Result<()> {
        while self.pending[0].len() >= FRAME_SAMPLES {
            self.enc.frame(&self.pending, &mut self.bytes)?;
            self.out.write_all(&self.bytes)?;
            self.frames_out += 1;
            for p in self.pending.iter_mut() {
                p.drain(..FRAME_SAMPLES);
            }
        }
        Ok(())
    }
}

impl Sink for Ac3Sink {
    fn write(&mut self, b: &Block) -> Result<()> {
        let count = self.layout.order.len();
        if b.rate != RATE || b.channels as usize != count {
            return Err(MediaError::Encoder(format!(
                "a block of {} channels at {} Hz came to a {count}-channel {RATE} Hz AC3 encoder",
                b.channels, b.rate
            )));
        }
        for (a52, &input) in self.layout.order.iter().enumerate() {
            let p = &mut self.pending[a52];
            p.extend(b.samples.chunks_exact(count).map(|f| {
                let v = f[input];
                if v.is_nan() {
                    0.0
                } else {
                    v.clamp(-1.0, 1.0)
                }
            }));
        }
        self.frames_in += b.frames() as u64;
        self.encode_ready()
    }

    fn finish(mut self: Box<Self>) -> Result<()> {
        if self.frames_in == 0 {
            return Err(no_audio("AC3"));
        }
        // Enough syncframes to carry every input sample through the
        // 256-sample transform delay.
        let total = (self.frames_in + HALF as u64).div_ceil(FRAME_SAMPLES as u64);
        let missing = (total - self.frames_out) as usize * FRAME_SAMPLES;
        for p in self.pending.iter_mut() {
            p.resize(missing, 0.0);
        }
        self.encode_ready()?;
        self.out.flush()?;
        Ok(())
    }
}

/// One channel's exponents for a run of blocks that share them.
struct ExpSet {
    strategy: Strategy,
    coded: Coded,
    /// The exponents a decoder rebuilds, one per bin.
    exps: Vec<u8>,
    curve: Curve,
    baps: Vec<u8>,
}

/// The encoder proper: transform state and a syncframe at a time.
struct Encoder {
    stream: Stream,
    lfe: bool,
    mdct: Mdct,
    pow2: [f32; 25],
    /// The previous block's samples, per A/52 channel.
    prev: Vec<[f32; HALF]>,
    /// The frame's coefficients: [channel][block][bin].
    coefs: Vec<[[f32; HALF]; BLOCKS]>,
    /// The frame's exponent sets per channel, and which set each block uses.
    sets: Vec<Vec<ExpSet>>,
    uses: Vec<[usize; BLOCKS]>,
    /// The exponent strategy floor and change threshold for this bitrate.
    coarsest: Strategy,
    threshold: f32,
    mantissas: Mantissas,
    /// Bits of the last frame's syncinfo, bsi and blocks 0 and 1 (§5.5).
    early_bits: usize,
}

impl Encoder {
    fn new(layout: &Layout, kbps: u32, index: usize) -> Encoder {
        let nfchans = layout.nfchans();
        let chbwcod = chbwcod(kbps, nfchans);
        let channels = layout.order.len();
        let per = kbps as f32 / channels as f32;
        let coarsest = if per >= 40.0 {
            Strategy::D15
        } else if per >= 24.0 {
            Strategy::D25
        } else {
            Strategy::D45
        };
        Encoder {
            stream: Stream {
                frmsizecod: 2 * index as u32,
                words: tables::FRAME_SIZES[index].1,
                acmod: layout.acmod,
                lfe: layout.lfe,
                nfchans,
                chbwcod,
                end: ((chbwcod + 12) * 3 + 37) as usize,
            },
            lfe: layout.lfe,
            mdct: Mdct::new(),
            pow2: frame::powers_of_two(),
            prev: vec![[0.0; HALF]; channels],
            coefs: vec![[[0.0; HALF]; BLOCKS]; channels],
            sets: (0..channels).map(|_| Vec::new()).collect(),
            uses: vec![[0; BLOCKS]; channels],
            coarsest,
            threshold: 2.0,
            mantissas: Mantissas::new(),
            early_bits: 0,
        }
    }

    /// Bins coded for A/52 channel `ch`.
    fn end(&self, ch: usize) -> usize {
        if self.lfe && ch == self.stream.nfchans {
            LFE_END
        } else {
            self.stream.end
        }
    }

    /// Encodes the first 1,536 samples of each channel of `input` into one
    /// syncframe in `out`.
    fn frame(&mut self, input: &[Vec<f32>], out: &mut Vec<u8>) -> Result<()> {
        let channels = input.len();
        // Transform (§8.2.3) and exponents (§8.2.7, §8.2.8, §8.2.10).
        for (ch, samples) in input.iter().enumerate() {
            for blk in 0..BLOCKS {
                let mut block = [0.0f32; N];
                block[..HALF].copy_from_slice(&self.prev[ch]);
                block[HALF..].copy_from_slice(&samples[blk * HALF..(blk + 1) * HALF]);
                self.prev[ch].copy_from_slice(&block[HALF..]);
                self.mdct.forward(&block, &mut self.coefs[ch][blk]);
            }
            self.plan_exponents(ch);
        }

        // Everything but the mantissas has a fixed size; the SNR offset
        // decides the mantissas' (§8.2.12).
        let frame_bits = self.stream.words * 16;
        let five_eighths = ((self.stream.words >> 1) + (self.stream.words >> 3)) * 16;
        let mut w = BitWriter::with_capacity(self.stream.words * 2);
        self.stream.header(&mut w);
        let header = w.bits();
        let mut side = [0usize; BLOCKS];
        for (blk, s) in side.iter_mut().enumerate() {
            let before = w.bits();
            self.block_side(&mut w, blk, 0, None);
            *s = w.bits() - before;
        }
        // auxdatae, crcrsv and crc2 end the frame.
        let fixed = header + side.iter().sum::<usize>() + 18;

        // §5.5: blocks 0 and 1 end within the first 5/8; block 5's
        // mantissas and the 18 closing bits fit the last 3/8 (with room for
        // a skip field's rounding, should block 5 need moving up to it).
        let three_eighths = frame_bits - five_eighths;
        let fits = |enc: &mut Encoder, offset: u32| -> bool {
            let mant = enc.mantissa_bits(offset);
            let total = fixed + mant.iter().sum::<u32>() as usize;
            let early = header + side[0] + side[1] + (mant[0] + mant[1]) as usize;
            total <= frame_bits
                && early <= five_eighths
                && mant[5] as usize + 18 + SKIP_SLACK <= three_eighths
        };
        // The largest offset that fits (bits only grow with the offset).
        let (mut lo, mut hi) = (0u32, MAX_OFFSET);
        if fits(self, hi) {
            lo = hi;
        } else {
            while hi - lo > 1 {
                let mid = (lo + hi) / 2;
                if fits(self, mid) {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
        }
        let offset = lo;
        let mant = self.mantissa_bits(offset);

        // Where block 5's mantissas would start; before the 5/8 point, skip
        // fields in blocks 2 to 5 move them there.
        let block5 = header
            + side[..5].iter().sum::<usize>()
            + mant[..5].iter().sum::<u32>() as usize
            + side[5];
        let skips = frame::skips_for(five_eighths.saturating_sub(block5)).ok_or_else(|| {
            MediaError::Encoder("an AC3 frame has more room than its skip fields hold".into())
        })?;

        // Pack it.
        let mut w = BitWriter::with_capacity(self.stream.words * 2);
        self.stream.header(&mut w);
        let mut block5_mantissas = 0;
        for (blk, &skip) in skips.iter().enumerate() {
            self.block_side(&mut w, blk, offset, skip);
            if blk == 5 {
                block5_mantissas = w.bits();
            }
            self.mantissas.clear();
            for ch in 0..channels {
                let set = &self.sets[ch][self.uses[ch][blk]];
                let end = set.exps.len();
                self.mantissas.add(
                    &self.coefs[ch][blk][..end],
                    &set.exps,
                    &set.baps,
                    &self.pow2,
                );
            }
            self.mantissas.write(&mut w);
            if blk == 1 {
                self.early_bits = w.bits();
            }
        }
        // A real check, not a debug one: a frame that breaks the size or
        // §5.5 is an error, never written.
        if w.bits() + 18 > frame_bits
            || self.early_bits > five_eighths
            || block5_mantissas < five_eighths
        {
            return Err(MediaError::Encoder(format!(
                "an AC3 frame came out wrong: {} of {frame_bits} bits, block 1 ends at {}, \
                 block 5's mantissas start at {block5_mantissas}, 5/8 is {five_eighths}",
                w.bits() + 18,
                self.early_bits
            )));
        }
        w.pad_to(frame_bits - 18);
        w.put(1, 0); // auxdatae
        w.put(1, 0); // crcrsv
        w.put(16, 0); // crc2
        *out = w.into_bytes();
        frame::seal(out, self.stream.words);
        Ok(())
    }

    /// Chooses channel `ch`'s exponent strategy per block and codes each set.
    fn plan_exponents(&mut self, ch: usize) {
        let end = self.end(ch);
        let raw: Vec<Vec<u8>> = self.coefs[ch]
            .iter()
            .map(|c| c[..end].iter().map(|&v| exponents::exponent(v)).collect())
            .collect();
        let views: Vec<&[u8]> = raw.iter().map(|r| r.as_slice()).collect();
        let lfe = self.lfe && ch == self.stream.nfchans;
        let strategies = if lfe {
            // The LFE has D15 or reuse only (Table 7.5).
            exponents::strategies(&views, self.threshold, Strategy::D15)
                .into_iter()
                .map(|s| {
                    if s == Strategy::Reuse {
                        s
                    } else {
                        Strategy::D15
                    }
                })
                .collect()
        } else {
            exponents::strategies(&views, self.threshold, self.coarsest)
        };
        let sets = &mut self.sets[ch];
        sets.clear();
        for blk in 0..BLOCKS {
            if strategies[blk] != Strategy::Reuse {
                // The set's exponents serve every block of its run: take the
                // smallest of each bin so every coefficient fits.
                let run_end = (blk + 1..BLOCKS)
                    .find(|&b| strategies[b] != Strategy::Reuse)
                    .unwrap_or(BLOCKS);
                let mut exps = raw[blk].clone();
                for r in &raw[blk + 1..run_end] {
                    for (e, &x) in exps.iter_mut().zip(r) {
                        *e = (*e).min(x);
                    }
                }
                let coded = exponents::code(&mut exps, strategies[blk]);
                let curve = Curve::new(&exps);
                sets.push(ExpSet {
                    strategy: strategies[blk],
                    coded,
                    exps,
                    curve,
                    baps: vec![0; end],
                });
            }
            self.uses[ch][blk] = sets.len() - 1;
        }
    }

    /// Block `blk`'s side information.
    fn block_side(&self, w: &mut BitWriter, blk: usize, offset: u32, skip: Option<u32>) {
        let exps = |ch: usize| {
            let set = &self.sets[ch][self.uses[ch][blk]];
            let new = blk == 0 || self.uses[ch][blk] != self.uses[ch][blk - 1];
            if new {
                BlockExps {
                    strategy: set.strategy,
                    coded: Some(&set.coded),
                }
            } else {
                BlockExps {
                    strategy: Strategy::Reuse,
                    coded: None,
                }
            }
        };
        let nf = self.stream.nfchans;
        let fbw: Vec<BlockExps> = (0..nf).map(exps).collect();
        let lfe = self.lfe.then(|| exps(nf));
        frame::block_side(&self.stream, w, blk, &fbw, lfe.as_ref(), offset, skip);
    }

    fn set_baps(&mut self, offset: u32) {
        for sets in self.sets.iter_mut() {
            for set in sets.iter_mut() {
                set.curve.baps(offset, &mut set.baps);
            }
        }
    }

    /// Each block's mantissa bits at `offset`.
    fn mantissa_bits(&mut self, offset: u32) -> [u32; BLOCKS] {
        self.set_baps(offset);
        let mut out = [0u32; BLOCKS];
        let costs: Vec<Vec<Cost>> = self
            .sets
            .iter()
            .map(|sets| sets.iter().map(|s| Cost::of(&s.baps)).collect())
            .collect();
        for (blk, o) in out.iter_mut().enumerate() {
            let mut c = Cost::default();
            for (ch, cs) in costs.iter().enumerate() {
                c.add(&cs[self.uses[ch][blk]]);
            }
            *o = c.block_bits();
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_follow_the_mask() {
        let l = Layout::of(6, 0x3F).unwrap();
        // FL FR FC LFE BL BR -> L C R SL SR, LFE.
        assert_eq!((l.acmod, l.lfe), (7, true));
        assert_eq!(l.order, [0, 2, 1, 4, 5, 3]);
        let l = Layout::of(6, 0x60F).unwrap();
        assert_eq!(l.order, [0, 2, 1, 4, 5, 3]);
        let l = Layout::of(4, 0x107).unwrap();
        assert_eq!(
            (l.acmod, l.lfe, l.order.clone()),
            (5, false, vec![0, 2, 1, 3])
        );
        let l = Layout::of(3, 0xB).unwrap();
        assert_eq!((l.acmod, l.lfe, l.order.clone()), (2, true, vec![0, 1, 2]));
        let l = Layout::of(5, 0x3B).unwrap();
        assert_eq!(
            (l.acmod, l.lfe, l.order.clone()),
            (6, true, vec![0, 1, 3, 4, 2])
        );
        // No mask, or one AC3 can't carry: the default for the count.
        assert_eq!(Layout::of(3, 0).unwrap().acmod, 3);
        assert_eq!(Layout::of(4, 0).unwrap().acmod, 6);
        assert_eq!(Layout::of(5, 0).unwrap().acmod, 7);
        assert_eq!(
            Layout::of(6, 0xC0 | 0xF).unwrap(),
            Layout::of(6, 0x3F).unwrap()
        );
    }

    #[test]
    fn bandwidth_codes_stay_in_range() {
        for &(kbps, _) in tables::FRAME_SIZES.iter() {
            for nf in 2..=5 {
                assert!(chbwcod(kbps, nf) <= 60);
            }
        }
        assert_eq!(chbwcod(640, 5), 60);
    }

    /// Encodes `frames` syncframes of `signal(channel, sample)` with
    /// `layout` at `kbps` and reads each one back with the test parser.
    fn encode_and_parse(
        layout: &Layout,
        kbps: u32,
        frames: usize,
        signal: impl Fn(usize, usize) -> f32,
    ) -> Vec<parse::Layout> {
        let index = tables::FRAME_SIZES
            .iter()
            .position(|&(k, _)| k == kbps)
            .unwrap();
        let mut enc = Encoder::new(layout, kbps, index);
        let channels = layout.order.len();
        let mut out = Vec::new();
        (0..frames)
            .map(|f| {
                let input: Vec<Vec<f32>> = (0..channels)
                    .map(|ch| {
                        (0..FRAME_SAMPLES)
                            .map(|i| signal(ch, f * FRAME_SAMPLES + i))
                            .collect()
                    })
                    .collect();
                enc.frame(&input, &mut out).unwrap();
                let parsed = parse::parse(&out);
                assert_eq!((parsed.acmod, parsed.lfe), (layout.acmod, layout.lfe));
                parsed
            })
            .collect()
    }

    /// §5.5: syncinfo, bsi and blocks 0 and 1 within the first 5/8; block
    /// 5's mantissas, auxdata and errorcheck within the last 3/8.
    fn assert_five_and_three_eighths(p: &parse::Layout, what: &str) {
        assert!(
            p.block1_end <= p.five_eighths,
            "{what}: block 1 ends at {} past the 5/8 point {}",
            p.block1_end,
            p.five_eighths
        );
        let three_eighths = p.frame_bits - p.five_eighths;
        assert!(
            p.frame_bits - p.block5_mantissas <= three_eighths,
            "{what}: block 5's mantissas start at {}, so the end takes {} bits of a {}-bit 3/8",
            p.block5_mantissas,
            p.frame_bits - p.block5_mantissas,
            three_eighths
        );
        assert!(p.blocks_end + 18 <= p.frame_bits, "{what}: overflow");
    }

    #[test]
    fn silence_keeps_the_three_eighths_rule() {
        let tone = |ch: usize, i: usize| {
            // A quiet tone in the centre, in one block of every other frame.
            let in_frame = i % FRAME_SAMPLES;
            if ch == 1 && (i / FRAME_SAMPLES) % 2 == 1 && (4 * HALF..5 * HALF).contains(&in_frame) {
                0.01 * (i as f32 * 0.13).sin()
            } else {
                0.0
            }
        };
        let late = |_: usize, i: usize| {
            // Silence, then loud noise in block 5 only.
            let in_frame = i % FRAME_SAMPLES;
            if in_frame >= 5 * HALF {
                (((i * 7919) % 1000) as f32 / 1000.0 - 0.5) * 1.8
            } else {
                0.0
            }
        };
        for (mask, count) in [(0x3Fu32, 6u16), (0x7, 3)] {
            let layout = Layout::of(count, mask).unwrap();
            for kbps in [448u32, 640] {
                for p in encode_and_parse(&layout, kbps, 4, |_, _| 0.0) {
                    assert_five_and_three_eighths(&p, &format!("silence {mask:#x} {kbps}"));
                    // The room went to skip fields in blocks 2 to 5.
                    assert_eq!(&p.skipped[..2], &[0, 0]);
                    assert!(p.skipped.iter().sum::<u32>() > 0, "{:?}", p.skipped);
                }
                for p in encode_and_parse(&layout, kbps, 4, tone) {
                    assert_five_and_three_eighths(&p, &format!("tone {mask:#x} {kbps}"));
                }
                for p in encode_and_parse(&layout, kbps, 3, late) {
                    assert_five_and_three_eighths(&p, &format!("late {mask:#x} {kbps}"));
                }
            }
        }
    }

    #[test]
    fn every_layout_parses_at_every_rate() {
        // Each syncframe reads back to the end of block 5 by the syntax,
        // with legal exponents and bandwidth codes, for every layout.
        let music = |ch: usize, i: usize| {
            let t = i as f32 / RATE as f32;
            0.3 * (t * (220.0 + 110.0 * ch as f32) * std::f32::consts::TAU).sin()
                + 0.02 * (((i * 7919 + ch * 31) % 1000) as f32 / 1000.0 - 0.5)
        };
        for mask in [
            0x7u32, 0xF, 0x103, 0x10B, 0x107, 0x10F, 0x33, 0x3B, 0x37, 0x3F, 0x60F, 0xB,
        ] {
            let count = mask.count_ones() as u16;
            let layout = Layout::of(count, mask).unwrap();
            for &(kbps, _) in tables::FRAME_SIZES.iter() {
                if kbps < MIN_KBPS_PER_CHANNEL * count as u32 {
                    continue;
                }
                for p in encode_and_parse(&layout, kbps, 2, music) {
                    assert_five_and_three_eighths(&p, &format!("{mask:#x} at {kbps}"));
                }
            }
        }
    }

    #[test]
    fn blocks_0_and_1_fit_the_first_five_eighths() {
        // A loud first two blocks, quiet after: §5.5's limit still holds.
        let layout = Layout::of(6, 0x3F).unwrap();
        for &(kbps, index) in &[(96u32, 6usize), (448, 15), (640, 18)] {
            let mut enc = Encoder::new(&layout, kbps, index);
            let mut s = 1u64;
            let input: Vec<Vec<f32>> = (0..6)
                .map(|_| {
                    (0..FRAME_SAMPLES)
                        .map(|i| {
                            s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
                            let n = (s >> 40) as f32 / (1u64 << 24) as f32 - 0.5;
                            if i < 2 * HALF {
                                n * 1.9
                            } else {
                                n * 0.001
                            }
                        })
                        .collect()
                })
                .collect();
            let mut out = Vec::new();
            enc.frame(&input, &mut out).unwrap();
            let words = enc.stream.words;
            assert_eq!(out.len(), words * 2, "{kbps}");
            assert!(
                enc.early_bits <= ((words >> 1) + (words >> 3)) * 16,
                "{kbps} kbps: {} bits before block 2",
                enc.early_bits
            );
        }
    }
}
