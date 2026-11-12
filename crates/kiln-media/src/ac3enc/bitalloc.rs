//! The parametric bit allocation of A/52 §7.2.2, exactly as a decoder runs
//! it (it must match bit for bit): exponents to PSD (§7.2.2.2), PSD
//! integration (§7.2.2.3), the excitation function (§7.2.2.4), the masking
//! curve (§7.2.2.5) and the bit allocation pointers (§7.2.2.7). Delta bit
//! allocation (§7.2.2.6) isn't used. The parameters are the nominal ones of
//! §8.2.12; the encoder searches the SNR offset (§7.2.2.7, §8.2.12).

use super::tables::{
    BAPTAB, BNDSZ, BNDTAB, DBPBTAB, FASTDEC, FASTGAIN, FLOORTAB, HTH_48K, LATAB, MANT_BITS,
    MASKTAB, SLOWDEC, SLOWGAIN,
};

/// The bit allocation codes sent in block 0 (§8.2.12's defaults).
pub const SDCYCOD: u32 = 2;
pub const FDCYCOD: u32 = 1;
pub const SGAINCOD: u32 = 1;
pub const DBPBCOD: u32 = 2;
pub const FLOORCOD: u32 = 4;
pub const FGAINCOD: u32 = 4;

/// The largest combined SNR offset, `csnroffst * 16 + fsnroffst` (6 + 4 bits).
pub const MAX_OFFSET: u32 = 63 * 16 + 15;

/// One exponent set's PSD and masking curve before the SNR offset: what
/// stays the same while the encoder searches the offset.
#[derive(Clone)]
pub struct Curve {
    pub end: usize,
    psd: [i32; 256],
    mask: [i32; 50],
}

/// log-addition (§7.2.2.3).
fn logadd(a: i32, b: i32) -> i32 {
    let c = a - b;
    let address = ((c.abs() >> 1) as usize).min(255);
    if c >= 0 {
        a + LATAB[address]
    } else {
        b + LATAB[address]
    }
}

/// calc_lowcomp (§7.2.2.4).
fn calc_lowcomp(a: i32, b0: i32, b1: i32, bin: usize) -> i32 {
    if bin < 7 {
        if b0 + 256 == b1 {
            384
        } else if b0 > b1 {
            (a - 64).max(0)
        } else {
            a
        }
    } else if bin < 20 {
        if b0 + 256 == b1 {
            320
        } else if b0 > b1 {
            (a - 64).max(0)
        } else {
            a
        }
    } else {
        (a - 128).max(0)
    }
}

impl Curve {
    /// The curve of a full-bandwidth or LFE channel (start bin 0) whose
    /// decoded exponents are `exps` (one per bin, `exps.len()` = end).
    pub fn new(exps: &[u8]) -> Curve {
        let (start, end) = (0usize, exps.len());
        let sdecay = SLOWDEC[SDCYCOD as usize];
        let fdecay = FASTDEC[FDCYCOD as usize];
        let sgain = SLOWGAIN[SGAINCOD as usize];
        let dbknee = DBPBTAB[DBPBCOD as usize];
        let fgain = FASTGAIN[FGAINCOD as usize];

        // §7.2.2.2
        let mut psd = [0i32; 256];
        for bin in start..end {
            psd[bin] = 3072 - ((exps[bin] as i32) << 7);
        }

        // §7.2.2.3
        let mut bndpsd = [0i32; 50];
        let mut j = start;
        let mut k = MASKTAB[start];
        loop {
            let lastbin = (BNDTAB[k] + BNDSZ[k]).min(end);
            bndpsd[k] = psd[j];
            j += 1;
            while j < lastbin {
                bndpsd[k] = logadd(bndpsd[k], psd[j]);
                j += 1;
            }
            k += 1;
            if end <= lastbin {
                break;
            }
        }

        // §7.2.2.4, for channels starting at band 0 (full bandwidth and LFE).
        let bndstrt = MASKTAB[start];
        let bndend = MASKTAB[end - 1] + 1;
        debug_assert_eq!(bndstrt, 0);
        let lfe_last = |bin: usize| bndend == 7 && bin == 6;
        let mut excite = [0i32; 50];
        let mut lowcomp = 0;
        lowcomp = calc_lowcomp(lowcomp, bndpsd[0], bndpsd[1], 0);
        excite[0] = bndpsd[0] - fgain - lowcomp;
        lowcomp = calc_lowcomp(lowcomp, bndpsd[1], bndpsd[2], 1);
        excite[1] = bndpsd[1] - fgain - lowcomp;
        let mut begin = 7;
        let mut fastleak = 0;
        let mut slowleak = 0;
        for bin in 2..7 {
            if !lfe_last(bin) {
                lowcomp = calc_lowcomp(lowcomp, bndpsd[bin], bndpsd[bin + 1], bin);
            }
            fastleak = bndpsd[bin] - fgain;
            slowleak = bndpsd[bin] - sgain;
            excite[bin] = fastleak - lowcomp;
            if !lfe_last(bin) && bndpsd[bin] <= bndpsd[bin + 1] {
                begin = bin + 1;
                break;
            }
        }
        for bin in begin..bndend.min(22) {
            if !lfe_last(bin) {
                lowcomp = calc_lowcomp(lowcomp, bndpsd[bin], bndpsd[bin + 1], bin);
            }
            fastleak -= fdecay;
            fastleak = fastleak.max(bndpsd[bin] - fgain);
            slowleak -= sdecay;
            slowleak = slowleak.max(bndpsd[bin] - sgain);
            excite[bin] = (fastleak - lowcomp).max(slowleak);
        }
        for bin in 22..bndend {
            fastleak -= fdecay;
            fastleak = fastleak.max(bndpsd[bin] - fgain);
            slowleak -= sdecay;
            slowleak = slowleak.max(bndpsd[bin] - sgain);
            excite[bin] = fastleak.max(slowleak);
        }

        // §7.2.2.5
        let mut mask = [0i32; 50];
        for bin in bndstrt..bndend {
            if bndpsd[bin] < dbknee {
                excite[bin] += (dbknee - bndpsd[bin]) >> 2;
            }
            mask[bin] = excite[bin].max(HTH_48K[bin]);
        }
        Curve { end, psd, mask }
    }

    /// The bit allocation pointers (§7.2.2.7) for the combined SNR offset
    /// `offset` = `csnroffst * 16 + fsnroffst`. An offset of 0 is §7.2.2.1.1's
    /// special case (every SNR offset zero): no bits at all.
    pub fn baps(&self, offset: u32, bap: &mut [u8]) {
        let end = self.end;
        if offset == 0 {
            bap[..end].fill(0);
            return;
        }
        // snroffset = (((csnroffst - 15) << 4) + fsnroffst) << 2
        let snroffset = (offset as i32 - 240) << 2;
        let floor = FLOORTAB[FLOORCOD as usize];
        let mut i = 0usize;
        let mut j = MASKTAB[0];
        loop {
            let lastbin = (BNDTAB[j] + BNDSZ[j]).min(end);
            let mut m = self.mask[j] - snroffset - floor;
            if m < 0 {
                m = 0;
            }
            m &= 0x1fe0;
            m += floor;
            while i < lastbin {
                let address = ((self.psd[i] - m) >> 5).clamp(0, 63);
                bap[i] = BAPTAB[address as usize];
                i += 1;
            }
            j += 1;
            if end <= lastbin {
                break;
            }
        }
    }
}

/// What a block's mantissas cost: the grouped quantizers counted by
/// mantissa, the rest in bits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cost {
    /// Mantissas at bap 1 (3 to a 5-bit group), 2 (3 to 7 bits), 4 (2 to 7 bits).
    pub n1: u32,
    pub n2: u32,
    pub n4: u32,
    pub bits: u32,
}

impl Cost {
    pub fn of(baps: &[u8]) -> Cost {
        let mut c = Cost::default();
        for &b in baps {
            match b {
                1 => c.n1 += 1,
                2 => c.n2 += 1,
                4 => c.n4 += 1,
                _ => c.bits += MANT_BITS[b as usize],
            }
        }
        c
    }

    pub fn add(&mut self, o: &Cost) {
        self.n1 += o.n1;
        self.n2 += o.n2;
        self.n4 += o.n4;
        self.bits += o.bits;
    }

    /// Bits for one block's mantissas, groups shared across its channels
    /// and the last ones padded (§7.3.5).
    pub fn block_bits(&self) -> u32 {
        self.n1.div_ceil(3) * 5 + self.n2.div_ceil(3) * 7 + self.n4.div_ceil(2) * 7 + self.bits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masktab_is_the_band_table() {
        // Table 7.13 says the same as Table 7.12.
        for (bin, &band) in MASKTAB.iter().enumerate().take(253) {
            assert!(
                BNDTAB[band] <= bin && bin < BNDTAB[band] + BNDSZ[band],
                "bin {bin}"
            );
        }
        assert_eq!(&MASKTAB[253..], &[0, 0, 0]);
    }

    #[test]
    fn more_offset_costs_more_bits() {
        let mut s = 5u64;
        let exps: Vec<u8> = (0..253)
            .map(|i| {
                s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
                ((i / 24) as u8 + ((s >> 62) as u8)).min(24)
            })
            .collect();
        let curve = Curve::new(&exps);
        let mut bap = [0u8; 256];
        // Never fewer bits, but for a group's padding: a lone mantissa moving
        // out of a part-filled group can save up to its 7 bits.
        let mut most = 0;
        for offset in 0..=MAX_OFFSET {
            curve.baps(offset, &mut bap);
            let bits = Cost::of(&bap[..253]).block_bits();
            assert!(bits + 7 >= most, "offset {offset}: {bits} after {most}");
            most = most.max(bits);
        }
        assert!(most > 253 * 10);
    }

    #[test]
    fn the_lfe_curve_takes_seven_bins() {
        let curve = Curve::new(&[3, 4, 5, 6, 8, 10, 12]);
        let mut bap = [0u8; 7];
        curve.baps(MAX_OFFSET, &mut bap);
        assert!(bap.iter().all(|&b| b > 0), "{bap:?}");
    }
}
