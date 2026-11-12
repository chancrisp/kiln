//! The forward transform of A/52 §8.2.3: a 512-sample window (Table 7.33)
//! and the 512-point MDCT of §8.2.3.2 for long blocks (alpha = 0),
//!
//! ```text
//! XD[k] = -2/N * sum_{n=0}^{N-1} x[n] cos(2pi/(4N) (2n+1)(2k+1) + pi/4 (2k+1)),  0 <= k < N/2
//! ```
//!
//! computed the usual fast way: the windowed block is folded into a
//! 256-point DCT-IV, which is done with a 128-point complex FFT and a
//! twiddle before and after. The window is the Kaiser-Bessel derived window
//! with alpha = 5, which reproduces Table 7.33 (a test checks it).

use std::f64::consts::PI;

/// Samples a long transform reads.
pub const N: usize = 512;
/// Coefficients it gives (and new samples per audio block).
pub const HALF: usize = N / 2;
/// The complex FFT's length.
const FFT: usize = N / 4;

/// The window, the twiddles and the FFT's tables, made once per encoder.
pub struct Mdct {
    window: [f32; N],
    /// exp(-i pi (n + 1/4) / 256), n < 128: before the FFT.
    pre: [(f32, f32); FFT],
    /// exp(-i pi k / 256), k < 128: after it.
    post: [(f32, f32); FFT],
    /// exp(-2 pi i k / 128), k < 64: the FFT's own.
    roots: [(f32, f32); FFT / 2],
    /// Bit-reversed order of 0..128.
    order: [u8; FFT],
}

/// The modified Bessel function of the first kind, order 0 (its series).
fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term, mut k) = (1.0f64, 1.0f64, 1.0f64);
    let q = x * x / 4.0;
    loop {
        term *= q / (k * k);
        sum += term;
        if term < sum * 1e-17 {
            return sum;
        }
        k += 1.0;
    }
}

/// The 512-point Kaiser-Bessel derived window with alpha = 5.
pub fn window() -> [f32; N] {
    let alpha = 5.0f64;
    let kaiser: Vec<f64> = (0..=HALF)
        .map(|j| {
            let r = 2.0 * j as f64 / HALF as f64 - 1.0;
            bessel_i0(PI * alpha * (1.0 - r * r).max(0.0).sqrt())
        })
        .collect();
    let total: f64 = kaiser.iter().sum();
    let mut w = [0.0f32; N];
    let mut acc = 0.0f64;
    for n in 0..HALF {
        acc += kaiser[n];
        let v = (acc / total).sqrt() as f32;
        w[n] = v;
        w[N - 1 - n] = v;
    }
    w
}

impl Mdct {
    pub fn new() -> Mdct {
        let mut pre = [(0.0, 0.0); FFT];
        let mut post = [(0.0, 0.0); FFT];
        for n in 0..FFT {
            let a = -PI * (n as f64 + 0.25) / HALF as f64;
            pre[n] = (a.cos() as f32, a.sin() as f32);
            let b = -PI * n as f64 / HALF as f64;
            post[n] = (b.cos() as f32, b.sin() as f32);
        }
        let mut roots = [(0.0, 0.0); FFT / 2];
        for (k, r) in roots.iter_mut().enumerate() {
            let a = -2.0 * PI * k as f64 / FFT as f64;
            *r = (a.cos() as f32, a.sin() as f32);
        }
        let bits = FFT.trailing_zeros();
        let mut order = [0u8; FFT];
        for (i, o) in order.iter_mut().enumerate() {
            *o = ((i as u32).reverse_bits() >> (32 - bits)) as u8;
        }
        Mdct {
            window: window(),
            pre,
            post,
            roots,
            order,
        }
    }

    /// Windows `input` (the previous 256 samples, then the new 256) and
    /// transforms it into the 256 coefficients XD[k] of §8.2.3.2.
    pub fn forward(&self, input: &[f32; N], out: &mut [f32; HALF]) {
        let w = &self.window;
        let x = |n: usize| input[n] * w[n];
        // Fold into the DCT-IV input u: with the windowed block in quarters
        // a b c d, u = (-c reversed - d, a - b reversed).
        let mut u = [0.0f32; HALF];
        for n in 0..FFT {
            u[n] = -x(3 * FFT - 1 - n) - x(3 * FFT + n);
            u[FFT + n] = x(n) - x(2 * FFT - 1 - n);
        }
        // DCT-IV of u through a 128-point complex FFT.
        let mut re = [0.0f32; FFT];
        let mut im = [0.0f32; FFT];
        for n in 0..FFT {
            let (a, b) = (u[2 * n], u[HALF - 1 - 2 * n]);
            let (c, s) = self.pre[n];
            let i = self.order[n] as usize;
            re[i] = a * c - b * s;
            im[i] = a * s + b * c;
        }
        self.fft(&mut re, &mut im);
        // XD = -2/N times the MDCT.
        let scale = -2.0 / N as f32;
        for k in 0..FFT {
            let (c, s) = self.post[k];
            let yr = re[k] * c - im[k] * s;
            let yi = re[k] * s + im[k] * c;
            out[2 * k] = yr * scale;
            out[HALF - 1 - 2 * k] = -yi * scale;
        }
    }

    /// An in-place radix-2 FFT of bit-reverse-ordered input.
    fn fft(&self, re: &mut [f32; FFT], im: &mut [f32; FFT]) {
        let mut size = 2;
        while size <= FFT {
            let half = size / 2;
            let step = FFT / size;
            for start in (0..FFT).step_by(size) {
                for j in 0..half {
                    let (c, s) = self.roots[j * step];
                    let (a, b) = (start + j, start + j + half);
                    let tr = re[b] * c - im[b] * s;
                    let ti = re[b] * s + im[b] * c;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            size *= 2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ac3enc::tables::WINDOW_TABLE;

    fn noise(n: usize, seed: u64) -> Vec<f32> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s = s
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                ((s >> 40) as f64 / (1u64 << 24) as f64 - 0.5) as f32
            })
            .collect()
    }

    #[test]
    fn the_window_is_table_7_33() {
        let w = window();
        for (n, &t) in WINDOW_TABLE.iter().enumerate() {
            assert!((w[n] - t).abs() <= 6e-6, "w[{n}] = {} vs {t}", w[n]);
            assert_eq!(w[N - 1 - n], w[n]);
        }
    }

    #[test]
    fn the_fast_transform_is_the_formula() {
        // §8.2.3.2 straight from its definition, alpha = 0.
        let m = Mdct::new();
        let input: [f32; N] = noise(N, 7).try_into().unwrap();
        let mut fast = [0.0f32; HALF];
        m.forward(&input, &mut fast);
        let w = window();
        for (k, &f) in fast.iter().enumerate() {
            let mut sum = 0.0f64;
            for (n, &v) in input.iter().enumerate() {
                let x = v as f64 * w[n] as f64;
                let arg = 2.0 * PI / (4.0 * N as f64) * (2 * n + 1) as f64 * (2 * k + 1) as f64
                    + PI / 4.0 * (2 * k + 1) as f64;
                sum += x * arg.cos();
            }
            let want = -2.0 / N as f64 * sum;
            assert!((f as f64 - want).abs() < 2e-6, "X[{k}] {f} vs {want}");
        }
    }

    /// The inverse transform of A/52 §7.9.4.1, steps 1 to 5, as written.
    fn imdct(xk: &[f32; HALF], w: &[f32; N]) -> [f64; N] {
        let n = N as f64;
        let xc = |k: usize| -(2.0 * PI * (8 * k + 1) as f64 / (8.0 * n)).cos();
        let xs = |k: usize| -(2.0 * PI * (8 * k + 1) as f64 / (8.0 * n)).sin();
        let mut z = vec![(0.0f64, 0.0f64); N / 4];
        for (k, zk) in z.iter_mut().enumerate() {
            let (a, b) = (xk[N / 2 - 2 * k - 1] as f64, xk[2 * k] as f64);
            *zk = (a * xc(k) - b * xs(k), b * xc(k) + a * xs(k));
        }
        let mut y = vec![(0.0f64, 0.0f64); N / 4];
        for (m, ym) in y.iter_mut().enumerate() {
            let (mut r, mut i) = (0.0, 0.0);
            for (k, &(zr, zi)) in z.iter().enumerate() {
                let a = 8.0 * PI * k as f64 * m as f64 / n;
                r += zr * a.cos() - zi * a.sin();
                i += zr * a.sin() + zi * a.cos();
            }
            *ym = (r * xc(m) - i * xs(m), i * xc(m) + r * xs(m));
        }
        let (yr, yi): (Vec<f64>, Vec<f64>) = y.into_iter().unzip();
        let w = |i: usize| w[i] as f64;
        let mut x = [0.0f64; N];
        for k in 0..N / 8 {
            x[2 * k] = -yi[N / 8 + k] * w(2 * k);
            x[2 * k + 1] = yr[N / 8 - k - 1] * w(2 * k + 1);
            x[N / 4 + 2 * k] = -yr[k] * w(N / 4 + 2 * k);
            x[N / 4 + 2 * k + 1] = yi[N / 4 - k - 1] * w(N / 4 + 2 * k + 1);
            x[N / 2 + 2 * k] = -yr[N / 8 + k] * w(N / 2 - 2 * k - 1);
            x[N / 2 + 2 * k + 1] = yi[N / 8 - k - 1] * w(N / 2 - 2 * k - 2);
            x[3 * N / 4 + 2 * k] = yi[k] * w(N / 4 - 2 * k - 1);
            x[3 * N / 4 + 2 * k + 1] = -yr[N / 4 - k - 1] * w(N / 4 - 2 * k - 2);
        }
        x
    }

    #[test]
    fn the_standard_decoder_gets_the_samples_back_256_late() {
        // Transform overlapping blocks the way the encoder does, invert them
        // with §7.9.4.1 and overlap-add (step 6): the input comes back,
        // delayed by one block of 256 samples.
        let m = Mdct::new();
        let w = window();
        let signal = noise(HALF * 8, 3);
        let mut prev = [0.0f32; HALF];
        let mut delay = [0.0f64; HALF];
        let mut pcm = Vec::new();
        for block in signal.chunks(HALF) {
            let mut input = [0.0f32; N];
            input[..HALF].copy_from_slice(&prev);
            input[HALF..].copy_from_slice(block);
            prev.copy_from_slice(block);
            let mut xk = [0.0f32; HALF];
            m.forward(&input, &mut xk);
            let x = imdct(&xk, &w);
            for n in 0..HALF {
                pcm.push(2.0 * (x[n] + delay[n]));
                delay[n] = x[HALF + n];
            }
        }
        for (i, &want) in signal[..signal.len() - HALF].iter().enumerate() {
            let got = pcm[i + HALF];
            assert!(
                (got - want as f64).abs() < 1e-5,
                "sample {i}: {got} vs {want}"
            );
        }
    }
}
