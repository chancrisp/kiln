//! A test-only reader of the syncframes this encoder writes, following the
//! A/52 syntax (§5.3) the way a decoder does: bsi, then each audio block's
//! side information, the exponents decoded as in §7.1.3, the bit
//! allocation run as in §7.2.2 to size the mantissas (§7.3.5's grouping),
//! and the skip fields (§5.4.3.58-60). It records where each part falls, so
//! tests can check §5.5's limits on the stream itself.

use super::bitalloc::Curve;
use super::exponents::{group_count, Strategy};

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

/// Where a syncframe's parts fall, in bits from its start.
#[derive(Debug)]
pub struct Layout {
    pub frame_bits: usize,
    /// The 5/8 point (§7.10.1, Table 7.34).
    pub five_eighths: usize,
    /// The end of block 1.
    pub block1_end: usize,
    /// Where block 5's mantissas start.
    pub block5_mantissas: usize,
    /// The end of block 5 (auxbits start here).
    pub blocks_end: usize,
    pub acmod: u32,
    pub lfe: bool,
    /// Bytes skipped per block.
    pub skipped: [u32; 6],
}

/// The words in a 48 kHz syncframe by frmsizecod / 2 (Table 5.18).
const WORDS: [usize; 19] = [
    64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 448, 512, 640, 768, 896, 1024, 1152, 1280,
];

/// Decodes a channel's exponents (§7.1.3).
fn exponents(b: &mut Bits, strategy: u32, end: usize) -> Vec<u8> {
    let strategy = match strategy {
        1 => Strategy::D15,
        2 => Strategy::D25,
        _ => Strategy::D45,
    };
    let size = match strategy {
        Strategy::D25 => 2,
        Strategy::D45 => 4,
        _ => 1,
    };
    let absolute = b.get(4) as i32;
    let mut exp = vec![absolute];
    let mut prev = absolute;
    for _ in 0..group_count(strategy, end) {
        let g = b.get(7) as i32;
        assert!(g <= 124, "a grouped exponent of {g}");
        for d in [g / 25, (g % 25) / 5, g % 5] {
            prev += d - 2;
            assert!((0..=24).contains(&prev), "an exponent of {prev}");
            for _ in 0..size {
                exp.push(prev);
            }
        }
    }
    assert!(exp.len() >= end);
    exp.truncate(end);
    exp.into_iter().map(|e| e as u8).collect()
}

/// Reads one syncframe and says where its parts fall. Panics on anything
/// this encoder should never write or that breaks the syntax.
pub fn parse(frame: &[u8]) -> Layout {
    let mut b = Bits {
        bytes: frame,
        at: 0,
    };
    assert_eq!(b.get(16), 0x0B77);
    b.get(16); // crc1
    assert_eq!(b.get(2), 0, "fscod");
    let frmsizecod = b.get(6) as usize;
    let words = WORDS[frmsizecod / 2];
    assert_eq!(frame.len(), words * 2);
    let frame_bits = words * 16;
    let five_eighths = ((words >> 1) + (words >> 3)) * 16;

    // bsi (§5.3.2)
    assert_eq!(b.get(5), 8, "bsid");
    b.get(3);
    let acmod = b.get(3);
    assert!(acmod >= 2, "acmod {acmod}");
    if acmod & 1 != 0 && acmod != 1 {
        b.get(2);
    }
    if acmod & 4 != 0 {
        b.get(2);
    }
    if acmod == 2 {
        b.get(2);
    }
    let lfe = b.get(1) == 1;
    b.get(5); // dialnorm
    if b.get(1) == 1 {
        b.get(8);
    }
    if b.get(1) == 1 {
        b.get(8);
    }
    if b.get(1) == 1 {
        b.get(7);
    }
    b.get(2); // copyrightb, origbs
    if b.get(1) == 1 {
        b.get(14);
    }
    if b.get(1) == 1 {
        b.get(14);
    }
    if b.get(1) == 1 {
        let n = b.get(6) as usize + 1;
        b.get(8 * n);
    }

    let nfchans = [2usize, 1, 2, 3, 3, 4, 4, 5][acmod as usize];
    let channels = nfchans + lfe as usize;
    let mut exps: Vec<Vec<u8>> = vec![Vec::new(); channels];
    let mut offsets = vec![0u32; channels];
    let mut skipped = [0u32; 6];
    let (mut block1_end, mut block5_mantissas) = (0, 0);
    for (blk, skip) in skipped.iter_mut().enumerate() {
        for _ in 0..nfchans {
            assert_eq!(b.get(1), 0, "blksw");
        }
        b.get(nfchans); // dithflag
        if b.get(1) == 1 {
            b.get(8); // dynrng
        }
        let cplstre = b.get(1);
        assert!(blk > 0 || cplstre == 1, "cplstre in block 0");
        if cplstre == 1 {
            assert_eq!(b.get(1), 0, "cplinu");
        }
        if acmod == 2 {
            let rematstr = b.get(1);
            assert!(blk > 0 || rematstr == 1, "rematstr in block 0");
            if rematstr == 1 {
                b.get(4);
            }
        }
        let chexpstr: Vec<u32> = (0..nfchans).map(|_| b.get(2)).collect();
        let lfeexpstr = if lfe { b.get(1) } else { 0 };
        assert!(
            blk > 0 || chexpstr.iter().all(|&s| s != 0),
            "reuse in block 0"
        );
        assert!(blk > 0 || !lfe || lfeexpstr == 1, "LFE reuse in block 0");
        let mut ends = vec![0usize; channels];
        for (ch, &s) in chexpstr.iter().enumerate() {
            if s != 0 {
                let bw = b.get(6) as usize;
                assert!(bw <= 60, "chbwcod {bw}");
                ends[ch] = (bw + 12) * 3 + 37;
            } else {
                ends[ch] = exps[ch].len();
            }
        }
        for (ch, &s) in chexpstr.iter().enumerate() {
            if s != 0 {
                exps[ch] = exponents(&mut b, s, ends[ch]);
                b.get(2); // gainrng
            }
        }
        if lfe && lfeexpstr == 1 {
            exps[nfchans] = exponents(&mut b, 1, 7);
        }
        let baie = b.get(1);
        assert!(blk > 0 || baie == 1);
        if baie == 1 {
            // sdcycod 2, fdcycod 1, sgaincod 1, dbpbcod 2, floorcod 4.
            let codes = [b.get(2), b.get(2), b.get(2), b.get(2), b.get(3)];
            assert_eq!(codes, [2, 1, 1, 2, 4], "bit allocation codes");
        }
        let snroffste = b.get(1);
        assert!(blk > 0 || snroffste == 1);
        if snroffste == 1 {
            let csnr = b.get(6);
            for o in offsets.iter_mut() {
                let fine = b.get(4);
                assert_eq!(b.get(3), 4, "fgaincod");
                *o = csnr * 16 + fine;
            }
        }
        assert_eq!(b.get(1), 0, "deltbaie");
        if b.get(1) == 1 {
            *skip = b.get(9);
            b.get(8 * *skip as usize);
        }
        if blk == 5 {
            block5_mantissas = b.at;
        }
        // Mantissas: sized by the bit allocation, grouped per block.
        let mut open = [0usize; 3];
        for ch in 0..channels {
            assert!(!exps[ch].is_empty(), "channel {ch} has no exponents");
            let mut baps = vec![0u8; exps[ch].len()];
            Curve::new(&exps[ch]).baps(offsets[ch], &mut baps);
            for &bap in &baps {
                let (i, per, bits) = match bap {
                    0 => continue,
                    1 => (0, 3, 5),
                    2 => (1, 3, 7),
                    4 => (2, 2, 7),
                    3 => (3, 0, 3),
                    5 => (3, 0, 4),
                    _ => (3, 0, super::tables::MANT_BITS[bap as usize] as usize),
                };
                if i == 3 {
                    b.get(bits);
                } else {
                    if open[i] == 0 {
                        b.get(bits);
                    }
                    open[i] = (open[i] + 1) % per;
                }
            }
        }
        if blk == 1 {
            block1_end = b.at;
        }
        assert!(b.at + 18 <= frame_bits, "block {blk} runs past the frame");
    }
    Layout {
        frame_bits,
        five_eighths,
        block1_end,
        block5_mantissas,
        blocks_end: b.at,
        acmod,
        lfe,
        skipped,
    }
}
