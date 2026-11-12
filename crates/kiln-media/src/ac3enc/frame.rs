//! Packing a syncframe (A/52 §5): the bit writer, syncinfo and bsi
//! (§5.3.1, §5.3.2, §5.4.1, §5.4.2), each audio block's side information
//! and mantissas (§5.3.3, §5.4.3, quantized as in §7.3 and §8.2.13), the
//! auxiliary data padding (§5.4.4) and both CRCs (§5.4.1.2, §5.4.5, §7.10.1).

use super::exponents::{Coded, Strategy};
use super::tables::SYM_LEVELS;

/// Writes bits most significant first (§5.2).
pub struct BitWriter {
    bytes: Vec<u8>,
    acc: u64,
    nacc: u32,
}

impl BitWriter {
    pub fn with_capacity(bytes: usize) -> BitWriter {
        BitWriter {
            bytes: Vec::with_capacity(bytes),
            acc: 0,
            nacc: 0,
        }
    }

    /// Writes the low `n` bits of `v` (n <= 32).
    pub fn put(&mut self, n: u32, v: u32) {
        debug_assert!(n <= 32 && (n == 32 || v >> n == 0), "{v} in {n} bits");
        if n == 0 {
            return;
        }
        self.acc = (self.acc << n) | v as u64;
        self.nacc += n;
        while self.nacc >= 8 {
            self.nacc -= 8;
            self.bytes.push((self.acc >> self.nacc) as u8);
        }
    }

    pub fn bits(&self) -> usize {
        self.bytes.len() * 8 + self.nacc as usize
    }

    /// Zero bits up to `bits` in all (none when already there or past it;
    /// the caller checks for overflow).
    pub fn pad_to(&mut self, bits: usize) {
        while self.bits() + 32 <= bits {
            self.put(32, 0);
        }
        let rest = bits.saturating_sub(self.bits());
        self.put(rest as u32, 0);
    }

    pub fn into_bytes(self) -> Vec<u8> {
        debug_assert_eq!(self.nacc, 0);
        self.bytes
    }
}

/// The CRC register of §7.10.1 (x^16 + x^15 + x^2 + 1, zero start, bits in
/// most significant first) after `bytes`, from `r`.
fn crc_update(mut r: u16, bytes: &[u8]) -> u16 {
    for &b in bytes {
        r ^= (b as u16) << 8;
        for _ in 0..8 {
            r = if r & 0x8000 != 0 {
                (r << 1) ^ 0x8005
            } else {
                r << 1
            };
        }
    }
    r
}

/// `r` times x^-1, modulo the CRC polynomial (which has a constant term, so
/// x is invertible).
fn times_inverse_x(r: u16) -> u16 {
    if r & 1 != 0 {
        (r >> 1) ^ 0xC002
    } else {
        r >> 1
    }
}

/// Fills in crc1 and crc2 of a whole syncframe of `words` 16-bit words.
///
/// crc2, the last word, is the CRC of everything after the sync word, so the
/// register runs out at zero over the whole frame. crc1, the second word,
/// comes before the data it covers, so it is that data's remainder taken
/// back by the length that follows it: crc1 * x^(8L) + data == 0 (mod the
/// polynomial), as §7.10.1 requires at the 5/8 point. crcrsv flips when
/// crc2 would read as a sync word (§5.4.5.1).
pub fn seal(frame: &mut [u8], words: usize) {
    let five_eighths = ((words >> 1) + (words >> 3)) * 2;
    frame[2] = 0;
    frame[3] = 0;
    // Remainder of the data after crc1 (bytes 4..5/8), as register * x^-16.
    let data = &frame[4..five_eighths];
    let mut c = crc_update(0, data);
    for _ in 0..(8 * data.len() + 16) {
        c = times_inverse_x(c);
    }
    frame[2..4].copy_from_slice(&c.to_be_bytes());
    let len = frame.len();
    let mut crc2 = crc_update(0, &frame[2..len - 2]);
    if crc2 == 0x0B77 {
        // crcrsv is the last bit before crc2.
        frame[len - 3] ^= 1;
        crc2 = crc_update(0, &frame[2..len - 2]);
    }
    frame[len - 2..].copy_from_slice(&crc2.to_be_bytes());
}

/// What every syncframe of a stream shares.
pub struct Stream {
    /// `frmsizecod` (the even code of the pair; both mean the same at 48 kHz).
    pub frmsizecod: u32,
    pub words: usize,
    pub acmod: u32,
    pub lfe: bool,
    /// Full-bandwidth channels.
    pub nfchans: usize,
    /// `chbwcod`, the same for every full-bandwidth channel.
    pub chbwcod: u32,
    /// Bins per full-bandwidth channel: endmant (§7.1.3).
    pub end: usize,
}

/// Bins of the LFE channel (lfeendmant, §7.1.3).
pub const LFE_END: usize = 7;

impl Stream {
    /// syncinfo (crc1 left at zero for `seal`) and bsi.
    pub fn header(&self, w: &mut BitWriter) {
        // syncinfo, §5.3.1
        w.put(16, 0x0B77);
        w.put(16, 0); // crc1
        w.put(2, 0); // fscod: 48 kHz (Table 5.6)
        w.put(6, self.frmsizecod);
        // bsi, §5.3.2
        w.put(5, 8); // bsid
        w.put(3, 0); // bsmod: complete main
        w.put(3, self.acmod);
        if self.acmod & 1 != 0 && self.acmod != 1 {
            w.put(2, 0); // cmixlev: -3 dB (Table 5.9)
        }
        if self.acmod & 4 != 0 {
            w.put(2, 0); // surmixlev: -3 dB (Table 5.10)
        }
        if self.acmod == 2 {
            w.put(2, 0); // dsurmod: not indicated
        }
        w.put(1, self.lfe as u32);
        w.put(5, 31); // dialnorm: -31 dB, no change in level
        w.put(1, 0); // compre
        w.put(1, 0); // langcode
        w.put(1, 0); // audprodie
        w.put(1, 0); // copyrightb
        w.put(1, 1); // origbs
        w.put(1, 0); // timecod1e
        w.put(1, 0); // timecod2e
        w.put(1, 0); // addbsie
    }
}

/// One channel's exponents in one block: new ones (with their strategy) or
/// the previous block's again.
pub struct BlockExps<'a> {
    pub strategy: Strategy,
    /// Present when `strategy` isn't `Reuse`.
    pub coded: Option<&'a Coded>,
}

/// Side information of audio block `blk` (§5.3.3) up to the mantissas.
/// `fbw` has a channel's exponents for each full-bandwidth channel and
/// `lfe` the LFE's; `offset` is `csnroffst * 16 + fsnroffst`. `skip` is a
/// skip field of that many null bytes (§5.4.3.58-60), or none.
pub fn block_side(
    s: &Stream,
    w: &mut BitWriter,
    blk: usize,
    fbw: &[BlockExps],
    lfe: Option<&BlockExps>,
    offset: u32,
    skip: Option<u32>,
) {
    use super::bitalloc::{DBPBCOD, FDCYCOD, FGAINCOD, FLOORCOD, SDCYCOD, SGAINCOD};
    let first = blk == 0;
    for _ in 0..s.nfchans {
        w.put(1, 0); // blksw: long blocks only
    }
    for _ in 0..s.nfchans {
        w.put(1, 1); // dithflag
    }
    w.put(1, 0); // dynrnge (0 dB in block 0, then kept)
                 // Coupling strategy: sent (and off) in block 0, reused after.
    if first {
        w.put(1, 1); // cplstre
        w.put(1, 0); // cplinu
    } else {
        w.put(1, 0);
    }
    if s.acmod == 2 {
        // Rematrixing (2/0 only): sent in block 0 with every band off.
        if first {
            w.put(1, 1); // rematstr
            w.put(4, 0); // rematflg[0..4]: no coupling, so four bands
        } else {
            w.put(1, 0);
        }
    }
    for e in fbw {
        w.put(2, e.strategy as u32); // chexpstr
    }
    if let Some(e) = lfe {
        w.put(1, (e.strategy != Strategy::Reuse) as u32); // lfeexpstr
    }
    for e in fbw {
        if e.strategy != Strategy::Reuse {
            w.put(6, s.chbwcod);
        }
    }
    for e in fbw {
        if let Some(c) = e.coded {
            w.put(4, c.absolute as u32);
            for &g in &c.groups {
                w.put(7, g as u32);
            }
            w.put(2, 0); // gainrng (§7.9.5: 0 when not measured)
        }
    }
    if let Some(c) = lfe.and_then(|e| e.coded) {
        w.put(4, c.absolute as u32);
        for &g in &c.groups {
            w.put(7, g as u32);
        }
    }
    // Bit allocation parameters and SNR offsets: block 0 only.
    if first {
        w.put(1, 1); // baie
        w.put(2, SDCYCOD);
        w.put(2, FDCYCOD);
        w.put(2, SGAINCOD);
        w.put(2, DBPBCOD);
        w.put(3, FLOORCOD);
        w.put(1, 1); // snroffste
        w.put(6, offset >> 4); // csnroffst
        for _ in 0..s.nfchans {
            w.put(4, offset & 15); // fsnroffst
            w.put(3, FGAINCOD);
        }
        if lfe.is_some() {
            w.put(4, offset & 15); // lfefsnroffst
            w.put(3, FGAINCOD);
        }
    } else {
        w.put(1, 0); // baie
        w.put(1, 0); // snroffste
    }
    w.put(1, 0); // deltbaie
    match skip {
        Some(bytes) => {
            debug_assert!(bytes <= MAX_SKIP_BYTES);
            w.put(1, 1); // skiple
            w.put(9, bytes); // skipl
            for _ in 0..bytes {
                w.put(8, 0); // skipfld
            }
        }
        None => w.put(1, 0), // skiple
    }
}

/// The most null bytes one block's skip field holds (9-bit skipl).
pub const MAX_SKIP_BYTES: u32 = 511;

/// Skip fields for blocks 2 to 5 that move block 5's mantissas at least
/// `bits` later in the frame (§5.4.3.58-60): each costs its 9-bit skipl
/// and its bytes on top of the `skiple` bit already counted. Blocks 0 and 1
/// never skip, so the 5/8 limit is untouched. `None` when four fields
/// can't hold that much.
pub fn skips_for(mut bits: usize) -> Option<[Option<u32>; 6]> {
    let mut skips = [None; 6];
    for skip in skips.iter_mut().skip(2) {
        if bits == 0 {
            break;
        }
        let bytes = (bits.saturating_sub(9).div_ceil(8) as u32).min(MAX_SKIP_BYTES);
        *skip = Some(bytes);
        bits = bits.saturating_sub(9 + 8 * bytes as usize);
    }
    (bits == 0).then_some(skips)
}

/// 2^e for e = 0..=24.
pub fn powers_of_two() -> [f32; 25] {
    let mut p = [1.0f32; 25];
    for e in 1..25 {
        p[e] = p[e - 1] * 2.0;
    }
    p
}

/// A pending group of 3- , 5- or 11-level codes (§7.3.5).
struct Group {
    /// Where its codeword goes in the block's list.
    at: usize,
    codes: [u32; 3],
    n: usize,
}

/// One block's mantissas in bit stream order, grouped (§7.3.5): a group's
/// codeword stands where its first mantissa is, groups run on across
/// channels, and the last ones of the block are padded with zero codes.
pub struct Mantissas {
    words: Vec<(u32, u32)>,
    groups: [Option<Group>; 3],
}

/// The grouped quantizers: index, mantissas per group, codeword bits, the
/// code of a zero mantissa.
fn grouping(bap: u8) -> Option<(usize, usize, u32, u32)> {
    match bap {
        1 => Some((0, 3, 5, 1)),
        2 => Some((1, 3, 7, 2)),
        4 => Some((2, 2, 7, 5)),
        _ => None,
    }
}

impl Mantissas {
    pub fn new() -> Mantissas {
        Mantissas {
            words: Vec::with_capacity(1600),
            groups: [None, None, None],
        }
    }

    pub fn clear(&mut self) {
        self.words.clear();
        self.groups = [None, None, None];
    }

    /// Quantizes and adds one channel's mantissas: `coefs` scaled by 2^exp
    /// (§8.2.11) and quantized by `baps` (§8.2.13, Tables 7.18 to 7.23).
    pub fn add(&mut self, coefs: &[f32], exps: &[u8], baps: &[u8], pow2: &[f32; 25]) {
        for ((&c, &e), &bap) in coefs.iter().zip(exps).zip(baps) {
            if bap == 0 {
                continue;
            }
            let m = c * pow2[e as usize];
            if bap <= 5 {
                let levels = SYM_LEVELS[bap as usize];
                let l = levels as f32;
                let code = ((m * l + l) * 0.5).floor().clamp(0.0, (levels - 1) as f32) as u32;
                match grouping(bap) {
                    Some(g) => self.grouped(g, code),
                    None => self.words.push((code, if bap == 3 { 3 } else { 4 })),
                }
            } else {
                let q = super::tables::MANT_BITS[bap as usize];
                let scale = (1u32 << (q - 1)) as f32;
                let max = (1i32 << (q - 1)) - 1;
                let code = (m * scale).round().clamp(-(max as f32) - 1.0, max as f32) as i32;
                self.words.push(((code as u32) & ((1u32 << q) - 1), q));
            }
        }
    }

    fn grouped(&mut self, (i, per, bits, _zero): (usize, usize, u32, u32), code: u32) {
        match &mut self.groups[i] {
            Some(g) => {
                g.codes[g.n] = code;
                g.n += 1;
            }
            None => {
                self.words.push((0, bits));
                self.groups[i] = Some(Group {
                    at: self.words.len() - 1,
                    codes: [code, 0, 0],
                    n: 1,
                });
            }
        }
        if self.groups[i].as_ref().is_some_and(|g| g.n == per) {
            self.close(i);
        }
    }

    fn close(&mut self, i: usize) {
        let Some(mut g) = self.groups[i].take() else {
            return;
        };
        let (per, zero) = match i {
            0 => (3, 1),
            1 => (3, 2),
            _ => (2, 5),
        };
        for c in g.codes.iter_mut().take(per).skip(g.n) {
            *c = zero;
        }
        let value = match i {
            0 => 9 * g.codes[0] + 3 * g.codes[1] + g.codes[2],
            1 => 25 * g.codes[0] + 5 * g.codes[1] + g.codes[2],
            _ => 11 * g.codes[0] + g.codes[1],
        };
        self.words[g.at].0 = value;
    }

    /// Pads the open groups and writes the block's mantissas.
    pub fn write(&mut self, w: &mut BitWriter) {
        for i in 0..3 {
            self.close(i);
        }
        for &(v, n) in &self.words {
            w.put(n, v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn register(bytes: &[u8]) -> u16 {
        crc_update(0, bytes)
    }

    #[test]
    fn both_crcs_check_at_zero() {
        for words in [64usize, 896, 1280] {
            let mut frame: Vec<u8> = (0..words * 2).map(|i| (i * 7 + 3) as u8).collect();
            frame[0] = 0x0B;
            frame[1] = 0x77;
            seal(&mut frame, words);
            let five_eighths = ((words >> 1) + (words >> 3)) * 2;
            assert_eq!(register(&frame[2..five_eighths]), 0, "crc1, {words} words");
            assert_eq!(register(&frame[2..]), 0, "crc2, {words} words");
        }
    }

    #[test]
    fn the_writer_packs_msb_first() {
        let mut w = BitWriter::with_capacity(4);
        w.put(16, 0x0B77);
        w.put(3, 0b101);
        w.put(5, 0b00011);
        w.pad_to(32);
        assert_eq!(w.into_bytes(), [0x0B, 0x77, 0xA3, 0x00]);
    }

    #[test]
    fn mantissas_group_across_channels() {
        let pow2 = powers_of_two();
        let mut m = Mantissas::new();
        // Two 3-level mantissas in one channel and one in the next make one
        // group; the 11-level one is padded.
        m.add(&[0.5, -0.6], &[0, 0], &[1, 1], &pow2);
        m.add(&[0.0, 0.9], &[0, 0], &[1, 4], &pow2);
        let mut w = BitWriter::with_capacity(4);
        m.write(&mut w);
        // 3-level codes 2, 0, 1 -> 9*2 + 0 + 1 = 19 in 5 bits; 11-level code
        // of 0.9 is 10 (10/11), padded with 5 -> 11*10 + 5 = 115.
        assert_eq!(w.bits(), 12);
        w.pad_to(16);
        assert_eq!(w.into_bytes(), [(19 << 3) | (115 >> 4), (115 & 15) << 4]);
    }

    #[test]
    fn asymmetric_mantissas_are_twos_complement() {
        let pow2 = powers_of_two();
        let mut m = Mantissas::new();
        // bap 6 is 5 bits: -0.5 is -8/16 -> 0b11000; 2^1 * 0.3 = 0.6 -> 10.
        m.add(&[-0.5, 0.3], &[0, 1], &[6, 6], &pow2);
        let mut w = BitWriter::with_capacity(2);
        m.write(&mut w);
        w.pad_to(16);
        assert_eq!(w.into_bytes(), [0b1100_0010, 0b1000_0000]);
    }
}
