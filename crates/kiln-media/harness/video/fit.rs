//! The size estimate's calibration: every Kiln encode's measured size, one
//! table per encoder (R70) fitted to the encodes' own errors (not to the
//! medians of their bits per pixel), and the estimate's error per encoder
//! and codec.

use super::{codec_name, CODECS, ESTIMATE_TARGET};
use crate::metrics::median;
use kiln_media::video::VCodec;
use std::fmt::Write;

/// One Kiln encode, for the calibration: the setting, which encoder wrote
/// it, and what the estimate works from.
#[derive(Clone, Debug)]
pub struct Sample {
    pub clip: String,
    pub codec: VCodec,
    pub quality: u8,
    /// The graphics card wrote it (`JobReport.hardware`), whatever was asked.
    pub hardware: bool,
    /// The video stream's bits over the pixels the estimate counts: the
    /// written width × height × frame rate × length.
    pub bpp: f64,
    /// The pixels a second the estimate counts, and its seconds.
    pub pixels_per_sec: f64,
    pub secs: f64,
    pub sound_kbps: f64,
    pub file_bytes: u64,
    /// The source's own bits per pixel (`tune::source_bpp`), when known.
    pub source_bpp: Option<f64>,
    /// (actual − `tune::estimate_bytes`) / actual, with `tune.rs` as it is.
    pub error_now: f64,
}

impl Sample {
    /// The estimate's error with `model` instead of `tune.rs`'s table.
    pub fn error_with(&self, model: &Model) -> f64 {
        let bpp = model.bpp(self.codec, self.quality) * model.content(self.source_bpp);
        let kbps = bpp * self.pixels_per_sec / 1000.0;
        let est = (kbps + self.sound_kbps) * 1000.0 / 8.0 * self.secs;
        (self.file_bytes as f64 - est) / self.file_bytes as f64
    }
}

/// A bits-per-pixel table of `tune.rs`'s shape: H.264 a + b·(q/100)², HEVC
/// `hevc` times that, scaled for a source of known bits per pixel by
/// (source / `reference`) to the power `k` (R81).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Model {
    pub a: f64,
    pub b: f64,
    pub hevc: f64,
    pub k: f64,
    pub reference: f64,
}

impl Model {
    /// The table's bits per pixel, before the content term.
    pub fn bpp(&self, codec: VCodec, q: u8) -> f64 {
        let x = q as f64 / 100.0;
        let h264 = self.a + self.b * x * x;
        match codec {
            VCodec::H264 => h264,
            VCodec::Hevc => h264 * self.hevc,
        }
    }

    /// The content term for a source of `source` bits per pixel: 1 when
    /// that is unknown.
    pub fn content(&self, source: Option<f64>) -> f64 {
        source.map_or(1.0, |s| (s / self.reference).powf(self.k))
    }

    /// The constants as `tune.rs` holds them: a to 4 decimals, b to 3, the
    /// HEVC factor and k to 2, the reference to 3 significant figures.
    pub fn rounded(&self) -> Model {
        // Adding 0.0 turns a rounded -0.0 into 0.0.
        let r = |v: f64, d: i32| (v * 10f64.powi(d)).round() / 10f64.powi(d) + 0.0;
        Model {
            a: r(self.a, 4),
            b: r(self.b, 3),
            hevc: r(self.hevc, 2),
            k: r(self.k, 2),
            reference: significant(self.reference, 3),
        }
    }
}

/// `v` to `figures` significant figures.
fn significant(v: f64, figures: i32) -> f64 {
    if v <= 0.0 || !v.is_finite() {
        return v;
    }
    let d = figures - 1 - v.log10().floor() as i32;
    (v * 10f64.powi(d)).round() / 10f64.powi(d)
}

/// A sample's error as a straight line in the table: error = base −
/// per_bpp·bpp, with x = (q/100)². `per_bpp` holds the content term.
#[derive(Clone, Copy)]
struct Line {
    x: f64,
    base: f64,
    per_bpp: f64,
}

impl Line {
    fn of(s: &Sample, content: f64) -> Line {
        let file = s.file_bytes.max(1) as f64;
        let q = s.quality as f64 / 100.0;
        Line {
            x: q * q,
            base: 1.0 - s.sound_kbps * 125.0 * s.secs / file,
            per_bpp: content * s.pixels_per_sec * s.secs / 8.0 / file,
        }
    }
}

/// The median absolute error of `lines` with H.264 bits per pixel a + b·x,
/// times `factor`.
fn median_abs(lines: &[Line], a: f64, b: f64, factor: f64, scratch: &mut Vec<f64>) -> f64 {
    scratch.clear();
    scratch.extend(
        lines
            .iter()
            .map(|l| (l.base - factor * (a + b * l.x) * l.per_bpp).abs()),
    );
    let n = scratch.len();
    if n == 0 {
        return 0.0;
    }
    let mid = n / 2;
    scratch.select_nth_unstable_by(mid, f64::total_cmp);
    let upper = scratch[mid];
    if n % 2 == 1 {
        return upper;
    }
    let lower = scratch[..mid].iter().copied().fold(f64::MIN, f64::max);
    (lower + upper) / 2.0
}

/// Fits a [`Model`] to the samples' own errors (I6): the table and content
/// term whose larger of the two codecs' median absolute errors is least
/// (then their sum). The reference is the median of the sources' own bits
/// per pixel: it and the table's scale trade off, so it is set, not
/// searched. k is searched from 0 to 1; for each k, a, b and the HEVC factor
/// coarsely, then the rounding grid around the best. `None` with fewer than
/// two H.264 qualities or no HEVC sample. Without `content`, k is 0: the
/// table alone, to show what the content term buys.
pub fn fit(samples: &[&Sample], content: bool) -> Option<Model> {
    let mut qualities: Vec<u8> = samples
        .iter()
        .filter(|s| s.codec == VCodec::H264)
        .map(|s| s.quality)
        .collect();
    qualities.sort_unstable();
    qualities.dedup();
    if qualities.len() < 2 || !samples.iter().any(|s| s.codec == VCodec::Hevc) {
        return None;
    }
    let sources: Vec<f64> = samples.iter().filter_map(|s| s.source_bpp).collect();
    let reference = median(&sources).map_or(0.1, |m| significant(m, 3));
    let mut best: Option<Best> = None;
    let mut scratch = Vec::new();
    let ks = if content { 10 } else { 0 };
    for ki in 0..=ks {
        let grid = (0..=60)
            .flat_map(|ai| (1..=300).map(move |bi| (ai as f64 * 0.0005, bi as f64 * 0.002)));
        let factors: Vec<f64> = (30..=130).map(|i| i as f64 * 0.01).collect();
        let k = ki as f64 * 0.1;
        search(
            samples,
            k,
            reference,
            grid,
            &factors,
            &mut best,
            &mut scratch,
        );
    }
    let coarse = best.as_ref()?.model.rounded();
    let factors: Vec<f64> = (-2..=2).map(|i| coarse.hevc + i as f64 * 0.01).collect();
    let ks = if content { 5 } else { 0 };
    for ki in -ks..=ks {
        let k = coarse.k + ki as f64 * 0.01;
        if k < 0.0 {
            continue;
        }
        let grid = (-10..=10).flat_map(|ai| {
            (-5..=5).map(move |bi| (coarse.a + ai as f64 * 0.0001, coarse.b + bi as f64 * 0.001))
        });
        search(
            samples,
            k,
            reference,
            grid,
            &factors,
            &mut best,
            &mut scratch,
        );
    }
    best.map(|b| b.model.rounded())
}

/// The best model found so far: the worse codec's median absolute error,
/// the two codecs' sum, and the model.
struct Best {
    worse: f64,
    sum: f64,
    model: Model,
}

/// Tries every (a, b) of `grid` and every HEVC factor of `factors` with the
/// content term at `k` and `reference`, keeping the best in `best`.
fn search(
    samples: &[&Sample],
    k: f64,
    reference: f64,
    grid: impl Iterator<Item = (f64, f64)>,
    factors: &[f64],
    best: &mut Option<Best>,
    scratch: &mut Vec<f64>,
) {
    let model = Model {
        a: 0.0,
        b: 0.0,
        hevc: 1.0,
        k,
        reference,
    };
    let lines = |codec: VCodec| -> Vec<Line> {
        samples
            .iter()
            .filter(|s| s.codec == codec)
            .map(|s| Line::of(s, model.content(s.source_bpp)))
            .collect()
    };
    let (h264, hevc) = (lines(VCodec::H264), lines(VCodec::Hevc));
    for (a, b) in grid {
        if a < 0.0 || b <= 0.0 {
            continue;
        }
        let h = median_abs(&h264, a, b, 1.0, scratch);
        if best.as_ref().is_some_and(|x| h > x.worse) {
            continue;
        }
        for &f in factors {
            let e = median_abs(&hevc, a, b, f, scratch);
            let worse = h.max(e);
            let better = best
                .as_ref()
                .is_none_or(|x| worse < x.worse || (worse == x.worse && h + e < x.sum));
            if better {
                *best = Some(Best {
                    worse,
                    sum: h + e,
                    model: Model {
                        a,
                        b,
                        hevc: f,
                        ..model
                    },
                });
            }
        }
    }
}

/// The calibration section: the median measured bits per pixel per encoder,
/// codec and quality; a table fitted per encoder; and the estimate's error
/// per encoder and codec, with `tune.rs` as it is and with the fit. Errors
/// over the target with `tune.rs` as it is go to `misses`.
pub fn calibration(samples: &[Sample], misses: &mut Vec<String>) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "## Calibration\n");
    if samples.is_empty() {
        let _ = writeln!(s, "No Kiln encodes, so nothing to calibrate.\n");
        return s;
    }
    let groups: [(&str, bool); 2] = [("Graphics card", true), ("Processor", false)];
    let _ = writeln!(
        s,
        "Every Kiln encode counts: the preset rows and the encodes at quality 20 and 90 made \
         for this table alone. An encode counts for the encoder that wrote it \
         (`JobReport.hardware`). Bits per pixel are the video stream's bits over the written \
         width × height × frame rate × length, the pixels `tune::estimate_bytes` counts.\n"
    );
    let _ = writeln!(
        s,
        "| Encoder | Codec | Quality | Encodes | Median bpp | Lowest | Highest |"
    );
    let _ = writeln!(s, "|---|---|---|---|---|---|---|");
    let mut fits = Vec::new();
    for (name, hw) in groups {
        let mine: Vec<&Sample> = samples.iter().filter(|x| x.hardware == hw).collect();
        for codec in CODECS {
            let mut qualities: Vec<u8> = mine
                .iter()
                .filter(|x| x.codec == codec)
                .map(|x| x.quality)
                .collect();
            qualities.sort_unstable();
            qualities.dedup();
            for q in qualities {
                let v: Vec<f64> = mine
                    .iter()
                    .filter(|x| x.codec == codec && x.quality == q)
                    .map(|x| x.bpp)
                    .collect();
                let Some(m) = median(&v) else { continue };
                let lo = v.iter().copied().fold(f64::MAX, f64::min);
                let hi = v.iter().copied().fold(0.0, f64::max);
                let _ = writeln!(
                    s,
                    "| {name} | {} | {q} | {} | {m:.4} | {lo:.4} | {hi:.4} |",
                    codec_name(codec),
                    v.len()
                );
            }
        }
        fits.push((name, hw, fit(&mine, true), fit(&mine, false)));
    }
    let _ = writeln!(s, "\n### Fitted tables, one per encoder (R70, R81)\n");
    let _ = writeln!(
        s,
        "In `tune.rs`'s shape, H.264 bpp = a + b·(q/100)² and HEVC = factor × H.264's, times \
         (the source's own bpp / reference)^k for a source whose bitrate is known, fitted so that \
         the larger of the two codecs' median absolute errors over the encodes themselves is \
         least. The reference is the median of the sources' bpp; k is searched from 0 to 1. These \
         go in `CARD` and `PROCESSOR` in `video/tune.rs`. The table alone (k = 0) is fitted too, \
         to show what the content term buys.\n"
    );
    let _ = writeln!(
        s,
        "| Encoder | a | b | HEVC factor | k | Reference bpp | Table alone: a, b, factor |"
    );
    let _ = writeln!(s, "|---|---|---|---|---|---|---|");
    for (name, _, m, plain) in &fits {
        let plain = plain.map_or("n/a".into(), |p| format!("{}, {}, {}", p.a, p.b, p.hevc));
        match m {
            Some(m) => {
                let _ = writeln!(
                    s,
                    "| {name} | {} | {} | {} | {} | {} | {plain} |",
                    m.a, m.b, m.hevc, m.k, m.reference
                );
            }
            None => {
                let _ = writeln!(s, "| {name} | n/a | n/a | n/a | n/a | n/a | {plain} |");
            }
        }
    }
    let _ = writeln!(s, "\n### Estimate error per encoder and codec\n");
    let _ = writeln!(
        s,
        "Error is (actual file − `tune::estimate_bytes`) / actual file, sound included. The target \
         is a median absolute error of at most {:.0}% for each encoder and codec, with \
         `tune.rs` as it is.\n",
        ESTIMATE_TARGET * 100.0
    );
    let _ = writeln!(
        s,
        "| Encoder | Codec | Encodes | Median error, tune.rs | Median absolute, tune.rs | \
         Median absolute, the fit | Median absolute, the table alone | |"
    );
    let _ = writeln!(s, "|---|---|---|---|---|---|---|---|");
    for (name, hw, model, plain) in &fits {
        for codec in CODECS {
            let rows: Vec<&Sample> = samples
                .iter()
                .filter(|x| x.hardware == *hw && x.codec == codec)
                .collect();
            if rows.is_empty() {
                continue;
            }
            let now: Vec<f64> = rows.iter().map(|x| x.error_now).collect();
            let now_abs: Vec<f64> = now.iter().map(|e| e.abs()).collect();
            let with = |m: &Option<Model>| {
                m.map(|m| {
                    let v: Vec<f64> = rows.iter().map(|x| x.error_with(&m).abs()).collect();
                    median(&v).unwrap_or(0.0)
                })
            };
            let (fitted, alone) = (with(model), with(plain));
            let pct = |f: Option<f64>| f.map_or("n/a".into(), |f| format!("{:.1}%", f * 100.0));
            let signed = median(&now).unwrap_or(0.0);
            let abs = median(&now_abs).unwrap_or(0.0);
            let over = abs > ESTIMATE_TARGET;
            if over {
                misses.push(format!(
                    "estimate: {name} {} median absolute error {:.1}% is over {:.0}% (the fit \
                     with the content term gives {}, the table alone {})",
                    codec_name(codec),
                    abs * 100.0,
                    ESTIMATE_TARGET * 100.0,
                    pct(fitted),
                    pct(alone)
                ));
            }
            let _ = writeln!(
                s,
                "| {name} | {} | {} | {:+.1}% | {:.1}% | {} | {} | {} |",
                codec_name(codec),
                rows.len(),
                signed * 100.0,
                abs * 100.0,
                pct(fitted),
                pct(alone),
                if over { "MISS" } else { "" }
            );
        }
    }
    let _ = writeln!(s, "\n### Per encode, tune.rs as it is\n");
    let _ = writeln!(
        s,
        "| Clip | Encoder | Codec | Quality | bpp | Source bpp | Error |"
    );
    let _ = writeln!(s, "|---|---|---|---|---|---|---|");
    for x in samples {
        let _ = writeln!(
            s,
            "| {} | {} | {} | {} | {:.4} | {} | {:+.1}% |",
            x.clip,
            if x.hardware {
                "Graphics card"
            } else {
                "Processor"
            },
            codec_name(x.codec),
            x.quality,
            x.bpp,
            x.source_bpp.map_or("n/a".into(), |b| format!("{b:.4}")),
            x.error_now * 100.0
        );
    }
    let _ = writeln!(s);
    s
}

// ------------------------------------------------------------------- checks

/// Samples a model gives exactly, at five qualities for both codecs, each
/// from a source of its own bits per pixel (one unknown).
fn exact(truth: &Model) -> Vec<Sample> {
    let mut out = Vec::new();
    for (i, codec) in CODECS.into_iter().enumerate() {
        for (j, q) in [20u8, 45, 65, 80, 90].into_iter().enumerate() {
            let pixels_per_sec = 1920.0 * 1080.0 * 24.0 * (1.0 + 0.1 * (i + j) as f64);
            let secs = 10.0;
            let sound_kbps = 128.0;
            let source_bpp = if i == 1 && j == 2 {
                None
            } else {
                Some(0.05 * (j + 1) as f64)
            };
            let bpp = truth.bpp(codec, q) * truth.content(source_bpp);
            let video = bpp * pixels_per_sec * secs / 8.0;
            let file_bytes = (video + sound_kbps * 125.0 * secs).round() as u64;
            out.push(Sample {
                clip: format!("c{j}"),
                codec,
                quality: q,
                hardware: true,
                bpp,
                pixels_per_sec,
                secs,
                sound_kbps,
                file_bytes,
                source_bpp,
                error_now: 0.0,
            });
        }
    }
    out
}

pub fn the_bpp_fit_finds_a_table_with_no_error() {
    // The sources' bpp run 0.05 to 0.25, so the median, the reference, is
    // 0.15.
    let truth = Model {
        a: 0.004,
        b: 0.15,
        hevc: 0.75,
        k: 0.5,
        reference: 0.15,
    };
    let samples = exact(&truth);
    let refs: Vec<&Sample> = samples.iter().collect();
    let worst = |m: &Model| {
        CODECS
            .into_iter()
            .map(|codec| {
                let errs: Vec<f64> = samples
                    .iter()
                    .filter(|s| s.codec == codec)
                    .map(|s| s.error_with(m).abs())
                    .collect();
                median(&errs).unwrap()
            })
            .fold(0.0, f64::max)
    };
    let got = fit(&refs, true).expect("a fit");
    assert!(worst(&got) < 0.005, "{} with {got:?}", worst(&got));
    assert_eq!(got.reference, 0.15);
    assert!((got.k - 0.5).abs() <= 0.02, "{got:?}");
    // The table alone can't follow the sources.
    let alone = fit(&refs, false).expect("a fit");
    assert_eq!(alone.k, 0.0);
    assert!(worst(&alone) > 0.05, "{} with {alone:?}", worst(&alone));
    assert_eq!(
        Model {
            a: 0.012345,
            b: 0.12345,
            hevc: 0.6789,
            k: 0.456,
            reference: 0.012345,
        }
        .rounded(),
        Model {
            a: 0.0123,
            b: 0.123,
            hevc: 0.68,
            k: 0.46,
            reference: 0.0123,
        }
    );
    let h264: Vec<&Sample> = samples.iter().filter(|s| s.codec == VCodec::H264).collect();
    assert!(fit(&h264, true).is_none(), "no HEVC sample, no factor");
    // The median of an even count is the mean of the middle two.
    let lines = [0.1, 0.3, 0.2, 0.4].map(|base| Line {
        x: 0.0,
        base,
        per_bpp: 0.0,
    });
    assert!((median_abs(&lines, 0.0, 0.0, 1.0, &mut Vec::new()) - 0.25).abs() < 1e-12);
}
