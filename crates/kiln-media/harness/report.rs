//! One row per clip x target x preset, the verdicts, and the markdown report.

use crate::metrics::median;
use std::fmt::Write;

/// What the report says about how rows are judged.
pub const RULES_NOTE: &str = "Length is judged against the source (after any rate conversion), \
within one codec frame; FFmpeg's length is shown for information. A cutoff under FFmpeg's \
by more than 500 Hz fails only for MP3 (LAME on both sides), Opus (libopus on both sides) \
and Kiln's own AC3 5.1 encoder; for AAC, WMA and AC3 stereo (Windows' encoders) and Vorbis \
(aoTuV against libvorbis) Kiln doesn't control the bandwidth, so a gap is a warning. \
WMA's size is its data packets, without the ASF header and index, and isn't judged under 30 s.";

/// Over this median estimate error (as a fraction), a target's VBR table is
/// flagged for recalibration.
pub const RECALIBRATE_OVER: f64 = 0.15;

/// What one encoder produced for one clip, target and preset.
#[derive(Default, Clone)]
pub struct Side {
    pub bytes: u64,
    pub frames: usize,
    pub lufs: Option<f64>,
    pub cutoff_hz: f64,
    pub secs: f64,
    pub visqol: Option<f64>,
}

pub struct Row {
    pub clip: String,
    pub target: &'static str,
    pub preset: &'static str,
    pub setting: String,
    /// True for a speech clip (its name has "speech"): reported apart from music.
    pub speech: bool,
    /// What the source's length comes to at the rate Kiln wrote, in frames.
    pub source_frames: Option<usize>,
    pub kiln: Side,
    /// `None` when FFmpeg couldn't make the file (a warning says why).
    pub ffmpeg: Option<Side>,
    /// How far Kiln's size is from the requested bitrate, for CBR and ABR.
    pub size_off: Option<f64>,
    /// (actual - estimate) / actual, for the VBR targets.
    pub estimate_error: Option<f64>,
    pub misses: Vec<String>,
    pub warnings: Vec<String>,
    /// Kiln's own output couldn't be made or read back: a bug, not a quality
    /// miss.
    pub error: bool,
}

impl Row {
    /// A row with nothing measured yet.
    pub fn new(clip: &str, target: &'static str, preset: &'static str) -> Row {
        Row {
            clip: clip.to_string(),
            target,
            preset,
            setting: String::new(),
            speech: clip.to_ascii_lowercase().contains("speech"),
            source_frames: None,
            kiln: Side::default(),
            ffmpeg: None,
            size_off: None,
            estimate_error: None,
            misses: Vec::new(),
            warnings: Vec::new(),
            error: false,
        }
    }

    pub fn passed(&self) -> bool {
        self.misses.is_empty()
    }
}

/// Everything a run found out.
pub struct Report {
    pub rows: Vec<Row>,
    pub notes: Vec<String>,
}

fn kb(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / 1000.0)
}

fn lufs(v: Option<f64>) -> String {
    v.map_or("n/a".into(), |v| format!("{v:.1}"))
}

fn khz(hz: f64) -> String {
    format!("{:.1}", hz / 1000.0)
}

fn percent(v: f64) -> String {
    format!("{:+.1}%", v * 100.0)
}

impl Report {
    pub fn failures(&self) -> usize {
        self.rows.iter().filter(|r| !r.passed()).count()
    }

    /// The rows with estimate errors that are speech (or not).
    fn estimate_rows(&self, speech: bool) -> impl Iterator<Item = &Row> {
        self.rows
            .iter()
            .filter(move |r| r.speech == speech && r.estimate_error.is_some())
    }

    /// (target, median signed error, median absolute error, rows) for each
    /// target with estimate errors among the music (or speech) rows, in the
    /// order targets first appear.
    pub fn estimate_medians(&self, speech: bool) -> Vec<(&'static str, f64, f64, usize)> {
        let mut targets: Vec<&'static str> = Vec::new();
        for r in self.estimate_rows(speech) {
            if !targets.contains(&r.target) {
                targets.push(r.target);
            }
        }
        targets
            .into_iter()
            .filter_map(|t| {
                let errs: Vec<f64> = self
                    .estimate_rows(speech)
                    .filter(|r| r.target == t)
                    .filter_map(|r| r.estimate_error)
                    .collect();
                let abs: Vec<f64> = errs.iter().map(|e| e.abs()).collect();
                Some((t, median(&errs)?, median(&abs)?, errs.len()))
            })
            .collect()
    }

    /// The estimate-error tables for music or for speech. Only music is
    /// flagged for recalibration: speech is far below any music table.
    fn estimate_section(&self, s: &mut String, speech: bool) {
        let what = if speech { "speech" } else { "music" };
        let _ = writeln!(s, "## Median estimate error per target, {what}\n");
        let medians = self.estimate_medians(speech);
        if medians.is_empty() {
            let _ = writeln!(s, "No VBR rows for {what}, so no estimate errors.\n");
            return;
        }
        let _ = writeln!(
            s,
            "Error is (actual size - `tune::estimate_bytes`) / actual size.{}\n",
            if speech {
                " Speech is reported only; it is not calibrated to and not flagged.".to_string()
            } else {
                format!(
                    " A target is flagged when the median of its absolute errors is over {:.0}%.",
                    RECALIBRATE_OVER * 100.0
                )
            }
        );
        let _ = writeln!(
            s,
            "| Target | Rows | Median error | Median absolute error | |"
        );
        let _ = writeln!(s, "|---|---|---|---|---|");
        for (t, signed, abs, n) in medians {
            let flag = if !speech && abs > RECALIBRATE_OVER {
                "recalibrate"
            } else {
                ""
            };
            let _ = writeln!(
                s,
                "| {t} | {n} | {} | {:.1}% | {flag} |",
                percent(signed),
                abs * 100.0
            );
        }
        let _ = writeln!(s, "\n### By preset, {what}\n");
        let _ = writeln!(s, "| Target | Preset | Rows | Median error |");
        let _ = writeln!(s, "|---|---|---|---|");
        let mut seen: Vec<(&str, &str)> = Vec::new();
        for r in self.estimate_rows(speech) {
            if !seen.contains(&(r.target, r.preset)) {
                seen.push((r.target, r.preset));
            }
        }
        for (t, p) in seen {
            let errs: Vec<f64> = self
                .estimate_rows(speech)
                .filter(|r| r.target == t && r.preset == p)
                .filter_map(|r| r.estimate_error)
                .collect();
            if let Some(m) = median(&errs) {
                let _ = writeln!(s, "| {t} | {p} | {} | {} |", errs.len(), percent(m));
            }
        }
        let _ = writeln!(s);
    }

    /// Warnings counted by target and kind, for the end of the report.
    fn warning_summary(&self, s: &mut String) {
        let mut counts: Vec<(&'static str, &'static str, usize)> = Vec::new();
        for r in &self.rows {
            for w in &r.warnings {
                let kind = if w.starts_with("cutoff") {
                    "cutoff under FFmpeg's"
                } else if w.starts_with("Kiln took") {
                    "slower than FFmpeg"
                } else {
                    "FFmpeg not compared"
                };
                match counts.iter_mut().find(|c| c.0 == r.target && c.1 == kind) {
                    Some(c) => c.2 += 1,
                    None => counts.push((r.target, kind, 1)),
                }
            }
        }
        if counts.is_empty() {
            return;
        }
        let _ = writeln!(s, "## Warnings by target\n");
        let _ = writeln!(s, "| Target | Warning | Rows |");
        let _ = writeln!(s, "|---|---|---|");
        for (t, k, n) in counts {
            let _ = writeln!(s, "| {t} | {k} | {n} |");
        }
        let _ = writeln!(s);
    }

    pub fn markdown(&self, title: &str) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "# {title}\n");
        for n in &self.notes {
            let _ = writeln!(s, "- {n}");
        }
        if self.rows.is_empty() {
            return s;
        }
        let _ = writeln!(s, "- {RULES_NOTE}");
        let _ = writeln!(s);
        let _ = writeln!(
            s,
            "| Clip | Target | Preset | Setting | Kiln KB | FFmpeg KB | Size vs request | Estimate error | \
             Frames source / Kiln / FFmpeg | LUFS Kiln / FFmpeg | Cutoff kHz Kiln / FFmpeg | ViSQOL Kiln / FFmpeg | \
             Seconds Kiln / FFmpeg | Result |"
        );
        let _ = writeln!(s, "|{}", "---|".repeat(14));
        for r in &self.rows {
            let ff = r.ffmpeg.as_ref();
            let both =
                |k: String, f: Option<String>| format!("{k} / {}", f.unwrap_or("n/a".into()));
            let result = if r.passed() {
                if r.warnings.is_empty() {
                    "pass".to_string()
                } else {
                    format!("pass, warn: {}", r.warnings.join("; "))
                }
            } else {
                format!("FAIL: {}", r.misses.join("; "))
            };
            let frames = format!(
                "{} / {} / {}",
                r.source_frames.map_or("n/a".into(), |f| f.to_string()),
                r.kiln.frames,
                ff.map_or("n/a".into(), |f| f.frames.to_string())
            );
            let _ = writeln!(
                s,
                "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
                r.clip,
                r.target,
                r.preset,
                r.setting,
                kb(r.kiln.bytes),
                ff.map_or("n/a".into(), |f| kb(f.bytes)),
                r.size_off.map_or("n/a".into(), percent),
                r.estimate_error.map_or("n/a".into(), percent),
                frames,
                both(lufs(r.kiln.lufs), ff.map(|f| lufs(f.lufs))),
                both(khz(r.kiln.cutoff_hz), ff.map(|f| khz(f.cutoff_hz))),
                both(
                    r.kiln.visqol.map_or("n/a".into(), |v| format!("{v:.2}")),
                    ff.map(|f| f.visqol.map_or("n/a".into(), |v| format!("{v:.2}")))
                ),
                both(
                    format!("{:.2}", r.kiln.secs),
                    ff.map(|f| format!("{:.2}", f.secs))
                ),
                result,
            );
        }
        let _ = writeln!(
            s,
            "\n{} rows, {} failed.\n",
            self.rows.len(),
            self.failures()
        );
        self.warning_summary(&mut s);
        self.estimate_section(&mut s, false);
        self.estimate_section(&mut s, true);
        s
    }
}

// -------------------------------------------------------------------- video

/// What the video report says about how rows are judged.
pub const VIDEO_RULES_NOTE: &str = "Each Kiln encode is paired with FFmpeg's libx264 or libx265 \
(-preset medium, into MP4) at the fractional CRF, searched from the preset's (23/20/17 for x264, \
28/24/20 for x265), whose video stream leaves Kiln's within ±15% of it (Kiln's size against \
FFmpeg's, as the verdict reads it, R83), aiming for ±5% (R67); the row shows the CRF found and how many encodes the search made. Sizes are the video stream's bytes \
(ffprobe's packet sizes), not the file's. A row passes when Kiln's VMAF is at least FFmpeg's minus \
2 at a video size within ±15% of FFmpeg's, or Kiln is better on both. Length is the span of the \
decoded frames' times, within one frame of the source's, and the first decoded frame must be the \
source's first (the nearest of its first four, compared small and gray). VMAF reads only the \
luma, so the colour is checked apart: the mean U and V over ten frames within 12 levels of the \
source's (through Hable's curve for a tone-mapped output). VMAF is FFmpeg's libvmaf \
with the default model, both sides scaled to the source's upright size and pixel format and synced \
by their times from the first frame (-fps_mode passthrough); FFmpeg turns rotated inputs upright. \
Colour (R73): an untagged source is labelled as players read it (R68: BT.601 under 720 lines, else \
BT.709); every tagged file keeps its tags and is converted into the reference's matrix by scale, \
with the input's matrix named from its stream's tags, \
and Kiln's tags are checked against R40/R68. HDR kept as HDR is compared in PQ at 10 bits and is \
indicative; HDR tone-mapped to SDR is compared with Kiln's own q90 tone-mapped encode, which \
FFmpeg encodes from (R71), and the tone curve against Hable's is a note. Anything that can't be \
measured is a miss (R72). Speed is wall time (FFmpeg's final encode only); only a warning past 2x \
FFmpeg's.";

/// What one encoder produced for one clip, codec and preset.
#[derive(Default, Clone, Debug)]
pub struct VSide {
    /// The video stream's bytes.
    pub video_bytes: u64,
    pub file_bytes: u64,
    /// Wall time.
    pub secs: f64,
    pub vmaf: Option<f64>,
    /// Seconds, from the video packets' times.
    pub length: Option<f64>,
    pub frames: usize,
}

pub struct VideoRow {
    pub clip: String,
    pub codec: &'static str,
    pub preset: &'static str,
    pub encoder: &'static str,
    pub setting: String,
    /// The graphics card wrote Kiln's side.
    pub hardware: bool,
    pub source_length: Option<f64>,
    pub source_frames: usize,
    pub kiln: VSide,
    pub ffmpeg: Option<VSide>,
    /// Kiln's video stream against FFmpeg's.
    pub size_off: Option<f64>,
    /// (actual file - estimate) / actual file.
    pub estimate_error: Option<f64>,
    pub misses: Vec<String>,
    pub warnings: Vec<String>,
    /// Kiln's own output couldn't be made or read back.
    pub error: bool,
}

impl VideoRow {
    pub fn new(
        clip: &str,
        codec: &'static str,
        preset: &'static str,
        encoder: &'static str,
    ) -> VideoRow {
        VideoRow {
            clip: clip.to_string(),
            codec,
            preset,
            encoder,
            setting: String::new(),
            hardware: false,
            source_length: None,
            source_frames: 0,
            kiln: VSide::default(),
            ffmpeg: None,
            size_off: None,
            estimate_error: None,
            misses: Vec::new(),
            warnings: Vec::new(),
            error: false,
        }
    }

    pub fn passed(&self) -> bool {
        self.misses.is_empty()
    }
}

/// A copy: every video packet's payload against the source's. Or, with
/// `encoded`, the MKV encode a clip gets where MKV can't copy it (R74).
#[derive(Default, Clone, Debug)]
pub struct CopyRow {
    pub clip: String,
    pub route: String,
    /// An encode checked for length and first frame, not a copy.
    pub encoded: bool,
    pub source_packets: usize,
    /// The output's video packets (frames, for an encode).
    pub packets: usize,
    /// Hidden lead-in packets dropped from the front (R69).
    pub dropped: usize,
    /// Packets whose MD5 differs, plus any missing or extra.
    pub mismatched: usize,
    /// The sound's packets, source and out, when the sound was copied, and
    /// how many differ.
    pub sound_packets: (usize, usize),
    pub sound_mismatched: usize,
    pub source_length: Option<f64>,
    pub length: Option<f64>,
    pub secs: f64,
    pub video_copied: bool,
    /// The source has sound; `sound_copied` says what became of it.
    pub has_sound: bool,
    pub sound_copied: bool,
    pub misses: Vec<String>,
}

/// Everything the video mode found out.
#[derive(Default)]
pub struct VideoReport {
    pub rows: Vec<VideoRow>,
    pub copies: Vec<CopyRow>,
    pub notes: Vec<String>,
    /// The calibration section, already written.
    pub calibration: String,
    /// Misses that belong to no row: the estimate's error, missing FFmpeg
    /// features, calibration encodes that failed.
    pub misses: Vec<String>,
    /// Kiln encodes the calibration measured.
    pub encodes: usize,
}

fn secs3(v: Option<f64>) -> String {
    v.map_or("n/a".into(), |v| format!("{v:.3}"))
}

fn vmaf2(v: Option<f64>) -> String {
    v.map_or("n/a".into(), |v| format!("{v:.2}"))
}

fn secs1(v: Option<f64>) -> String {
    v.map_or("n/a".into(), |v| format!("{v:.1}"))
}

impl VideoReport {
    pub fn failures(&self) -> usize {
        self.rows.iter().filter(|r| !r.passed()).count()
            + self.copies.iter().filter(|r| !r.misses.is_empty()).count()
            + self.misses.len()
    }

    /// The summary: rows and passes per codec and encoder, and the copies.
    pub fn summary(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(
            s,
            "| Codec | Encoder | Rows | Pass | Miss | Median VMAF Kiln / FFmpeg | \
             Median size vs FFmpeg | Median seconds Kiln / FFmpeg |"
        );
        let _ = writeln!(s, "|---|---|---|---|---|---|---|---|");
        let mut keys: Vec<(&str, &str)> = Vec::new();
        for r in &self.rows {
            if !keys.contains(&(r.codec, r.encoder)) {
                keys.push((r.codec, r.encoder));
            }
        }
        for (codec, encoder) in keys {
            let rows: Vec<&VideoRow> = self
                .rows
                .iter()
                .filter(|r| r.codec == codec && r.encoder == encoder)
                .collect();
            let pass = rows.iter().filter(|r| r.passed()).count();
            let m = |f: &dyn Fn(&VideoRow) -> Option<f64>| -> Option<f64> {
                median(&rows.iter().filter_map(|r| f(r)).collect::<Vec<_>>())
            };
            let _ = writeln!(
                s,
                "| {codec} | {encoder} | {} | {pass} | {} | {} / {} | {} | {} / {} |",
                rows.len(),
                rows.len() - pass,
                vmaf2(m(&|r| r.kiln.vmaf)),
                vmaf2(m(&|r| r.ffmpeg.as_ref().and_then(|f| f.vmaf))),
                m(&|r| r.size_off).map_or("n/a".into(), percent),
                secs1(m(&|r| Some(r.kiln.secs))),
                secs1(m(&|r| r.ffmpeg.as_ref().map(|f| f.secs))),
            );
        }
        let copies_ok = self.copies.iter().filter(|r| r.misses.is_empty()).count();
        let _ = writeln!(
            s,
            "| Copy | Original | {} | {copies_ok} | {} | | | |",
            self.copies.len(),
            self.copies.len() - copies_ok
        );
        s
    }

    pub fn markdown(&self, title: &str) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "# {title}\n");
        for n in &self.notes {
            let _ = writeln!(s, "- {n}");
        }
        if self.rows.is_empty() && self.copies.is_empty() {
            let _ = writeln!(s, "\n## Misses\n");
            for m in &self.misses {
                let _ = writeln!(s, "- {m}");
            }
            if self.misses.is_empty() {
                let _ = writeln!(s, "None.");
            }
            let _ = writeln!(s);
            s.push_str(&self.calibration);
            return s;
        }
        let _ = writeln!(s, "- {VIDEO_RULES_NOTE}");
        let _ = writeln!(s, "\n## Summary\n");
        s.push_str(&self.summary());
        let _ = writeln!(
            s,
            "\n{} encode rows and {} copy rows; {} failed, and {} misses outside the rows. The \
             calibration measured {} Kiln encodes.\n",
            self.rows.len(),
            self.copies.len(),
            self.failures() - self.misses.len(),
            self.misses.len(),
            self.encodes
        );
        let _ = writeln!(s, "## Encodes\n");
        let _ = writeln!(
            s,
            "| Clip | Codec | Preset | Encoder | Setting | Video KB Kiln / FFmpeg | Size vs FFmpeg | \
             VMAF Kiln / FFmpeg | Length s source / Kiln / FFmpeg | Frames source / Kiln | \
             Seconds Kiln / FFmpeg | Estimate error | Result |"
        );
        let _ = writeln!(s, "|{}", "---|".repeat(13));
        for r in &self.rows {
            let ff = r.ffmpeg.as_ref();
            let mut result = if r.passed() {
                "pass".to_string()
            } else {
                format!("FAIL: {}", r.misses.join("; "))
            };
            let extra: Vec<String> = r.warnings.clone();
            if !extra.is_empty() {
                let _ = write!(result, "; warn: {}", extra.join("; "));
            }
            let _ = writeln!(
                s,
                "| {} | {} | {} | {} | {} | {} / {} | {} | {} / {} | {} / {} / {} | {} / {} | {:.1} / {} | {} | {} |",
                r.clip,
                r.codec,
                r.preset,
                r.encoder,
                r.setting,
                kb(r.kiln.video_bytes),
                ff.map_or("n/a".into(), |f| kb(f.video_bytes)),
                r.size_off.map_or("n/a".into(), percent),
                vmaf2(r.kiln.vmaf),
                vmaf2(ff.and_then(|f| f.vmaf)),
                secs3(r.source_length),
                secs3(r.kiln.length),
                secs3(ff.and_then(|f| f.length)),
                r.source_frames,
                r.kiln.frames,
                r.kiln.secs,
                secs1(ff.map(|f| f.secs)),
                r.estimate_error.map_or("n/a".into(), percent),
                result,
            );
        }
        let _ = writeln!(s, "\n## Copies\n");
        if self.copies.is_empty() {
            let _ = writeln!(s, "No copy rows.\n");
        } else {
            let _ = writeln!(
                s,
                "Original, which must copy the picture. Each packet's payload, the picture's and \
                 the copied sound's, is hashed (ffprobe -show_data_hash MD5) on both sides: the \
                 copy must be the source's packets from some packet on, every one after it, \
                 dropping only hidden lead-in (R69). An MKV encode row is where MKV can't copy \
                 the clip (R74): it is checked for length and first frame.\n"
            );
            let _ = writeln!(
                s,
                "| Clip | Route | Video packets source / out | Lead-in dropped | Video payloads \
                 differing | Sound packets source / out (differing) | Length s source / out | \
                 Sound | Seconds | Result |"
            );
            let _ = writeln!(s, "|{}", "---|".repeat(10));
            for r in &self.copies {
                let result = if r.misses.is_empty() {
                    "pass".to_string()
                } else {
                    format!("FAIL: {}", r.misses.join("; "))
                };
                let (sound_in, sound_out) = r.sound_packets;
                let _ = writeln!(
                    s,
                    "| {} | {} | {} / {} | {} | {} | {} | {} / {} | {} | {:.2} | {result} |",
                    r.clip,
                    r.route,
                    r.source_packets,
                    if r.encoded {
                        format!("{} frames", r.packets)
                    } else {
                        r.packets.to_string()
                    },
                    if r.encoded {
                        "n/a".to_string()
                    } else {
                        r.dropped.to_string()
                    },
                    if r.encoded {
                        "n/a".to_string()
                    } else {
                        r.mismatched.to_string()
                    },
                    if r.sound_copied && !r.encoded {
                        format!("{sound_in} / {sound_out} ({})", r.sound_mismatched)
                    } else {
                        "n/a".into()
                    },
                    secs3(r.source_length),
                    secs3(r.length),
                    match (r.has_sound, r.sound_copied) {
                        (false, _) => "none",
                        (true, true) => "copied",
                        (true, false) => "encoded",
                    },
                    r.secs,
                );
            }
            let _ = writeln!(s);
        }
        let _ = writeln!(s, "## Misses\n");
        let mut any = false;
        for r in &self.rows {
            for m in &r.misses {
                any = true;
                let _ = writeln!(
                    s,
                    "- {} {} {} {}: {m}",
                    r.clip, r.codec, r.preset, r.encoder
                );
            }
        }
        for r in &self.copies {
            for m in &r.misses {
                any = true;
                let _ = writeln!(s, "- {} copy {}: {m}", r.clip, r.route);
            }
        }
        for m in &self.misses {
            any = true;
            let _ = writeln!(s, "- {m}");
        }
        if !any {
            let _ = writeln!(s, "None.");
        }
        let _ = writeln!(s);
        s.push_str(&self.calibration);
        s
    }
}
