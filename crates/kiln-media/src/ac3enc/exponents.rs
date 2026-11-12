//! Exponents: extraction (A/52 §8.2.7), the choice of strategy per block
//! (§8.2.8), and their preparation and coding (§8.2.10, decoded as in
//! §7.1.3) into a 4-bit absolute exponent and 7-bit groups of three
//! differentials (§7.1.2, Tables 7.1 to 7.4).

/// How a block's exponents are sent: `chexpstr` (A/52 Table 7.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    /// `00`: the previous block's exponents again.
    Reuse = 0,
    /// `01`: one exponent per bin.
    D15 = 1,
    /// `10`: one per pair of bins.
    D25 = 2,
    /// `11`: one per four bins.
    D45 = 3,
}

impl Strategy {
    /// Bins that share one exponent.
    fn group_size(self) -> usize {
        match self {
            Strategy::Reuse | Strategy::D15 => 1,
            Strategy::D25 => 2,
            Strategy::D45 => 4,
        }
    }
}

/// The largest exponent (A/52 §7.1.1).
pub const MAX_EXP: u8 = 24;

/// The exponent of a transform coefficient: its number of leading zeros as
/// a binary fraction, 0 to 24 (§8.2.7). |c| in [0.5, 1) is 0, [0.25, 0.5) is
/// 1, and so on; 1.0 or more (which only clipping input could give) is 0.
pub fn exponent(c: f32) -> u8 {
    let bits = c.abs().to_bits();
    // The biased binary exponent E: |c| = 1.m * 2^(E - 127).
    let e = (bits >> 23) as i32;
    if e >= 127 {
        return 0;
    }
    // 2^(E - 127) <= |c| < 2^(E - 126): the leading zeros are 126 - E.
    (126 - e).clamp(0, MAX_EXP as i32) as u8
}

/// Groups for a full-bandwidth channel with `end` bins (A/52 §7.1.3,
/// nchgrps), or for the LFE (nlfegrps = 2, which the D15 formula with
/// end = 7 also gives).
pub fn group_count(strategy: Strategy, end: usize) -> usize {
    match strategy {
        Strategy::Reuse => 0,
        Strategy::D15 => (end - 1) / 3,
        Strategy::D25 => (end - 1 + 3) / 6,
        Strategy::D45 => (end - 1 + 9) / 12,
    }
}

/// Exponents ready to pack: the 4-bit absolute exponent and the 7-bit
/// groups (`exps[ch][0]` and `exps[ch][1..]`, §5.4.3.27).
#[derive(Debug, Clone, Default)]
pub struct Coded {
    pub absolute: u8,
    pub groups: Vec<u8>,
}

/// Prepares `exps` (one per bin, `exps.len()` bins) for `strategy` and codes
/// them. Exponents only ever go down, so every coefficient still fits its
/// mantissa (§8.2.10): bins that share an exponent take their smallest, the
/// first is limited to 15, and neighbouring differentials to +-2. On return
/// `exps` holds what a decoder will rebuild from the code (§7.1.3).
pub fn code(exps: &mut [u8], strategy: Strategy) -> Coded {
    let end = exps.len();
    let size = strategy.group_size();
    let ngrps = group_count(strategy, end);
    let slots = ngrps * 3;
    // ge[0] is the absolute exponent, ge[i] the exponent of the i-th
    // differential, covering bins (i - 1) * size + 1 ..= i * size.
    let mut ge = vec![0u8; slots + 1];
    ge[0] = exps[0].min(15);
    let mut used = 0;
    for (i, g) in ge.iter_mut().enumerate().skip(1) {
        let from = (i - 1) * size + 1;
        if from >= end {
            break;
        }
        let to = (from + size).min(end);
        *g = exps[from..to].iter().copied().min().unwrap_or(MAX_EXP);
        used = i;
    }
    // Differentials of at most +-2: rising in one pass, falling in the other.
    for i in 1..=used {
        ge[i] = ge[i].min(ge[i - 1] + 2);
    }
    for i in (1..=used).rev() {
        ge[i - 1] = ge[i - 1].min(ge[i] + 2);
    }
    // Slots past the last bin repeat the last exponent (a differential of 0).
    for i in used + 1..=slots {
        ge[i] = ge[i - 1];
    }
    let mut groups = Vec::with_capacity(ngrps);
    for g in 0..ngrps {
        let m = |i: usize| (ge[i] as i32 - ge[i - 1] as i32 + 2) as u8;
        let i = 3 * g + 1;
        groups.push(25 * m(i) + 5 * m(i + 1) + m(i + 2));
    }
    exps[0] = ge[0];
    for (b, e) in exps.iter_mut().enumerate().skip(1) {
        *e = ge[(b - 1) / size + 1];
    }
    Coded {
        absolute: ge[0],
        groups,
    }
}

/// Picks each block's strategy for one channel (§8.2.8): a block gets new
/// exponents when its own differ from the ones in use by more than
/// `threshold` on average; a set of exponents used for one block is D45,
/// for two or three D25, for four to six D15. `coarsest` raises that floor
/// (for low bitrates). Block 0 always has new exponents.
pub fn strategies(blocks: &[&[u8]], threshold: f32, coarsest: Strategy) -> Vec<Strategy> {
    let mut starts = vec![0usize];
    let mut reference = 0usize;
    for b in 1..blocks.len() {
        let (a, r) = (blocks[b], blocks[reference]);
        let diff: u32 = a
            .iter()
            .zip(r)
            .map(|(&x, &y)| (x as i32 - y as i32).unsigned_abs())
            .sum();
        if diff as f32 > threshold * a.len() as f32 {
            starts.push(b);
            reference = b;
        }
    }
    let mut out = vec![Strategy::Reuse; blocks.len()];
    for (i, &s) in starts.iter().enumerate() {
        let run = starts.get(i + 1).copied().unwrap_or(blocks.len()) - s;
        let by_run = match run {
            1 => Strategy::D45,
            2 | 3 => Strategy::D25,
            _ => Strategy::D15,
        };
        out[s] = if by_run as u8 >= coarsest as u8 {
            by_run
        } else {
            coarsest
        };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decodes exponents as A/52 §7.1.3's pseudo code does.
    fn decode(coded: &Coded, strategy: Strategy, end: usize) -> Vec<u8> {
        let size = strategy.group_size();
        let mut dexp = Vec::new();
        for &g in &coded.groups {
            let g = g as i32;
            dexp.push(g / 25 - 2);
            dexp.push((g % 25) / 5 - 2);
            dexp.push((g % 25) % 5 - 2);
        }
        let mut prev = coded.absolute as i32;
        let mut exp = vec![coded.absolute as i32];
        for d in dexp {
            prev += d;
            assert!((0..=24).contains(&prev), "exponent {prev} out of range");
            for _ in 0..size {
                exp.push(prev);
            }
        }
        exp.truncate(end);
        exp.into_iter().map(|e| e as u8).collect()
    }

    #[test]
    fn exponents_count_leading_zeros() {
        assert_eq!(exponent(0.75), 0);
        assert_eq!(exponent(-0.5), 0);
        assert_eq!(exponent(0.4999), 1);
        assert_eq!(exponent(0.25), 1);
        assert_eq!(exponent(0.1), 3);
        assert_eq!(exponent(1.5), 0);
        assert_eq!(exponent(1e-9), 24);
        assert_eq!(exponent(0.0), 24);
    }

    #[test]
    fn coded_exponents_decode_to_what_the_encoder_keeps() {
        let mut s = 99u64;
        for strategy in [Strategy::D15, Strategy::D25, Strategy::D45] {
            for end in [7usize, 73, 76, 100, 211, 253] {
                let raw: Vec<u8> = (0..end)
                    .map(|_| {
                        s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
                        ((s >> 33) % 25) as u8
                    })
                    .collect();
                let mut exps = raw.clone();
                let coded = code(&mut exps, strategy);
                assert_eq!(coded.groups.len(), group_count(strategy, end));
                assert!(coded.groups.iter().all(|&g| g <= 124));
                assert_eq!(decode(&coded, strategy, end), exps, "{strategy:?} {end}");
                // Never above the raw exponent, so no mantissa overflows.
                assert!(exps.iter().zip(&raw).all(|(e, r)| e <= r));
                assert!(exps[0] <= 15);
            }
        }
    }

    #[test]
    fn strategies_follow_the_runs() {
        let quiet = vec![10u8; 50];
        let loud = vec![2u8; 50];
        let same: Vec<&[u8]> = vec![&quiet; 6];
        assert_eq!(
            strategies(&same, 0.5, Strategy::D15),
            [
                Strategy::D15,
                Strategy::Reuse,
                Strategy::Reuse,
                Strategy::Reuse,
                Strategy::Reuse,
                Strategy::Reuse
            ]
        );
        let changing: Vec<&[u8]> = vec![&quiet, &quiet, &loud, &quiet, &quiet, &quiet];
        assert_eq!(
            strategies(&changing, 0.5, Strategy::D15),
            [
                Strategy::D25,
                Strategy::Reuse,
                Strategy::D45,
                Strategy::D25,
                Strategy::Reuse,
                Strategy::Reuse
            ]
        );
        assert_eq!(strategies(&same, 0.5, Strategy::D45)[0], Strategy::D45);
    }
}
