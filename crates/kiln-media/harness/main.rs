//! The quality harness: holds Kiln's lossy audio to FFmpeg's.
//!
//! `cargo test -p kiln-media --features harness --test harness`. See README.md.
//!
//! With `harness = false` there is no libtest, so this file runs its own
//! checks first (the metrics, then a pass over synthetic clips it makes), then
//! the clips in `KILN_HARNESS_CLIPS`, writes `target/harness/report.md` and
//! exits non-zero on any miss.

mod metrics;
mod report;
mod video;

use kiln_media::pcm::wav;
use kiln_media::tune::{channel_scale, chosen_setting, estimate_bytes, label_of};
use kiln_media::{
    convert, probe, Block, Channels, Preset, Setting, SinkSpec, SourceInfo, Tags, Target, Tune,
};
use metrics::{cutoff_hz, decode, loudness_lufs, Pcm};
use report::{Report, Row, Side};
use std::f64::consts::PI;
use std::fs::File;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

/// The lossy targets, with the samples in one codec frame for the length check.
/// M4A is AAC in another container, with the same encoder, so it isn't run twice.
const TARGETS: [(Target, usize); 6] = [
    (Target::Mp3, 1152),
    (Target::Aac, 1024),
    (Target::Ogg, 2048),
    (Target::Opus, 960),
    (Target::Wma, 2048),
    (Target::Ac3, 1536),
];
const PRESETS: [(Preset, &str); 3] = [
    (Preset::Small, "Small"),
    (Preset::High, "High"),
    (Preset::Best, "Best"),
];

/// Where Kiln's size may sit around the requested bitrate (CBR and ABR).
const SIZE_TOLERANCE: f64 = 0.05;
/// Under this length a WMA file's size is shown but not judged: its packets
/// carry a fixed overhead of about 0.6 s of audio.
const WMA_SIZE_SECS: f64 = 30.0;
/// How far under FFmpeg's loudness Kiln may be, in LU, either way.
const LOUDNESS_TOLERANCE: f64 = 0.5;
/// How far under FFmpeg's cutoff Kiln's may be, in Hz.
const CUTOFF_TOLERANCE: f64 = 500.0;
/// ViSQOL: Kiln's score may be this far under FFmpeg's.
const VISQOL_TOLERANCE: f64 = 0.1;
/// Kiln taking longer than this many times FFmpeg's time is a warning.
const SPEED_WARN: f64 = 2.0;

const AUDIO_EXTS: [&str; 16] = [
    "wav", "wave", "flac", "aif", "aiff", "mp3", "ogg", "oga", "opus", "m4a", "wv", "mkv", "webm",
    "mp4", "ac3", "wma",
];

const VISQOL_OFF: &str = "ViSQOL: skipped (KILN_VISQOL not set)";

/// The longest `ffmpeg -version` may take, and an FFmpeg encode: the video
/// mode's own limits.
use video::{PROBE_LIMIT, WORK_LIMIT};

// ------------------------------------------------------------------ FFmpeg

/// FFmpeg's first version line, when `ffmpeg` runs from PATH.
fn ffmpeg_version() -> Option<String> {
    let out = video::run_limited(Command::new("ffmpeg").arg("-version"), PROBE_LIMIT).ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .map(str::to_string)
}

/// Whether a cutoff under FFmpeg's fails the row (R41): only where Kiln and
/// FFmpeg run the same library (MP3 with LAME, Opus with libopus) and for
/// Kiln's own AC3 5.1 encoder. Windows' encoders (AAC, WMA, AC3 in stereo)
/// and Vorbis (aoTuV against libvorbis) set their own bandwidth, so a gap
/// there is a warning.
fn cutoff_is_a_miss(target: Target, channels: u16) -> bool {
    match target {
        Target::Mp3 | Target::Opus => true,
        Target::Ac3 => channels > 2,
        _ => false,
    }
}

/// A setting as a number of kbps, for the formats that are written to a
/// bitrate. `None` for a quality (VBR) setting.
fn requested_kbps(s: &Setting) -> Option<u32> {
    match *s {
        Setting::LameCbr { kbps }
        | Setting::LameAbr { kbps }
        | Setting::VorbisAbr { kbps }
        | Setting::Kbps { kbps } => Some(kbps),
        _ => None,
    }
}

/// The FFmpeg flags that mean the same as `setting`: the V level for MP3, the
/// quality for Vorbis, and the bitrate for the rest.
fn encoder_args(t: Target, setting: &Setting) -> Vec<String> {
    let s = |v: &str| v.to_string();
    let tenths = |v: f64| format!("{}", v / 10.0);
    match (t, *setting) {
        (_, Setting::LameVbr { tenths: v }) => {
            vec![s("-c:a"), s("libmp3lame"), s("-q:a"), tenths(v as f64)]
        }
        (_, Setting::LameCbr { kbps }) => {
            vec![s("-c:a"), s("libmp3lame"), s("-b:a"), format!("{kbps}k")]
        }
        (_, Setting::LameAbr { kbps }) => vec![
            s("-c:a"),
            s("libmp3lame"),
            s("-abr"),
            s("1"),
            s("-b:a"),
            format!("{kbps}k"),
        ],
        (_, Setting::VorbisQ { tenths: v }) => {
            vec![s("-c:a"), s("libvorbis"), s("-q:a"), tenths(v as f64)]
        }
        (_, Setting::VorbisAbr { kbps }) => {
            vec![s("-c:a"), s("libvorbis"), s("-b:a"), format!("{kbps}k")]
        }
        (_, Setting::Opus { kbps, .. }) => {
            vec![s("-c:a"), s("libopus"), s("-b:a"), format!("{kbps}k")]
        }
        (Target::Aac | Target::M4a, Setting::Kbps { kbps }) => {
            vec![s("-c:a"), s("aac"), s("-b:a"), format!("{kbps}k")]
        }
        (Target::Wma, Setting::Kbps { kbps }) => {
            vec![s("-c:a"), s("wmav2"), s("-b:a"), format!("{kbps}k")]
        }
        (_, Setting::Kbps { kbps }) => {
            vec![s("-c:a"), s("ac3"), s("-b:a"), format!("{kbps}k")]
        }
        _ => Vec::new(),
    }
}

/// Runs FFmpeg to write `out` from `input` with `encoder` flags, at the rate
/// and channel count Kiln wrote (a flag only when they differ from the input's).
/// Returns the wall time, or what FFmpeg said.
fn run_ffmpeg(
    input: &Path,
    out: &Path,
    encoder: &[String],
    rate: Option<u32>,
    channels: Option<u16>,
) -> Result<f64, String> {
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-nostdin", "-y", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(input)
        .args(["-vn", "-map_metadata", "-1"]);
    if let Some(r) = rate {
        cmd.args(["-ar", &r.to_string()]);
    }
    if let Some(c) = channels {
        // Folding down: FFmpeg only scales a fold to stay under full scale
        // when the encoder takes integers (MP3 does, WMA doesn't), so the
        // scaling is asked for. Kiln's fold is scaled the same way.
        cmd.args(["-ac", &c.to_string()])
            .args(["-af", "aresample=rematrix_maxval=1.0"]);
    }
    cmd.args(encoder).arg(out);
    let start = Instant::now();
    let result = video::run_limited(&mut cmd, WORK_LIMIT)?;
    let secs = start.elapsed().as_secs_f64();
    if !result.status.success() || !out.exists() {
        let err = String::from_utf8_lossy(&result.stderr);
        let tail: Vec<&str> = err.lines().rev().take(3).collect();
        return Err(tail.into_iter().rev().collect::<Vec<_>>().join(" "));
    }
    Ok(secs)
}

// ------------------------------------------------------------------ ViSQOL

/// ViSQOL's MOS-LQO for `degraded` against `reference`, both made 48 kHz mono
/// 16-bit WAVs by FFmpeg first (what ViSQOL's audio mode takes). `None` with
/// a reason when it can't be had.
fn visqol_score(visqol: &Path, reference: &Path, degraded: &Path) -> Result<f64, String> {
    let out = Command::new(visqol)
        .arg("--reference_file")
        .arg(reference)
        .arg("--degraded_file")
        .arg(degraded)
        .output()
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .find(|l| l.contains("MOS-LQO"))
        .and_then(|l| l.split_whitespace().last())
        .and_then(|v| v.parse::<f64>().ok())
        .ok_or_else(|| format!("no MOS-LQO in ViSQOL's output ({})", out.status))
}

/// Makes the 48 kHz mono WAV ViSQOL reads from any audio file.
fn visqol_wav(input: &Path, out: &Path) -> Result<(), String> {
    run_ffmpeg(
        input,
        out,
        &["-c:a", "pcm_s16le"].map(str::to_string),
        Some(48_000),
        Some(1),
    )
    .map(|_| ())
}

// -------------------------------------------------------------------- rows

fn side_of(pcm: &Pcm, bytes: u64, wall: f64) -> Side {
    Side {
        bytes,
        frames: pcm.frames(),
        lufs: loudness_lufs(pcm),
        cutoff_hz: cutoff_hz(pcm),
        secs: wall,
        visqol: None,
    }
}

fn safe_name(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

struct Context<'a> {
    work: &'a Path,
    visqol: Option<PathBuf>,
    /// Longest stretch of a clip to use, from its start (`KILN_HARNESS_MAX_SECS`).
    max_secs: Option<f64>,
    /// Whether FFmpeg runs. Without it only Kiln's own checks are made: the
    /// length against the source, the size against the request and the
    /// estimate's error.
    ffmpeg: bool,
}

/// What a WMA file's data packets hold, in bytes: the ASF Data Object less
/// its 50-byte header, so without the ASF header and index around it. That
/// is what the bitrate pays for; the header and index are a few KB whatever
/// the length, which on a short clip is more than the size tolerance.
/// `None` when the file doesn't start as ASF does.
fn asf_data_bytes(path: &Path) -> Option<u64> {
    use std::io::{Read, Seek, SeekFrom};
    const HEADER: [u8; 16] = [
        0x30, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11, 0xA6, 0xD9, 0x00, 0xAA, 0x00, 0x62, 0xCE,
        0x6C,
    ];
    const DATA: [u8; 16] = [
        0x36, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11, 0xA6, 0xD9, 0x00, 0xAA, 0x00, 0x62, 0xCE,
        0x6C,
    ];
    /// An ASF object's GUID and size, the size counting these 24 bytes.
    fn object(f: &mut File) -> Option<([u8; 16], u64)> {
        let mut head = [0u8; 24];
        f.read_exact(&mut head).ok()?;
        let guid: [u8; 16] = head[..16].try_into().ok()?;
        Some((guid, u64::from_le_bytes(head[16..].try_into().ok()?)))
    }
    let mut f = File::open(path).ok()?;
    let (guid, header_size) = object(&mut f)?;
    if guid != HEADER {
        return None;
    }
    f.seek(SeekFrom::Start(header_size)).ok()?;
    let (guid, data_size) = object(&mut f)?;
    (guid == DATA).then(|| data_size.saturating_sub(50))
}

/// The first `secs` seconds of `clip` as a float WAV in `out`, with the rate
/// and layout the clip has. The clip itself isn't touched.
fn trim_to_wav(clip: &Path, out: &Path, secs: f64) -> Result<(), String> {
    let mut src = kiln_media::open_source(clip).map_err(|e| e.to_string())?;
    let info = src.info().clone();
    let keep = (secs * info.rate as f64).round() as usize;
    let count = info.channels.count as usize;
    let tags = Tags::default();
    let setting = Setting::Pcm {
        bits: 32,
        float: true,
    };
    let mut sink = wav::sink(SinkSpec {
        rate: info.rate,
        channels: info.channels,
        tags: &tags,
        setting: &setting,
        source_bits: None,
        out: File::create(out).map_err(|e| e.to_string())?,
    })
    .map_err(|e| e.to_string())?;
    let mut block = Block {
        channels: info.channels.count,
        rate: info.rate,
        samples: Vec::new(),
    };
    let mut written = 0;
    while written < keep && src.read(&mut block).map_err(|e| e.to_string())? {
        let take = block.frames().min(keep - written);
        block.samples.truncate(take * count);
        sink.write(&block).map_err(|e| e.to_string())?;
        written += take;
    }
    sink.finish().map_err(|e| e.to_string())
}

/// Reads a clip's facts and, over the cap, makes the copy of its first seconds
/// that is worked from: its safe name, the file to use and what it holds.
fn prepare(
    ctx: &Context,
    clip: &Path,
    report: &mut Report,
) -> Option<(String, PathBuf, SourceInfo)> {
    let name = clip
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("clip")
        .to_string();
    let info = match probe(clip) {
        Ok(i) => i,
        Err(e) => {
            report
                .notes
                .push(format!("{name}: skipped, can't read it ({e})"));
            return None;
        }
    };
    let stem = safe_name(&name);
    // Over the cap: work from a copy of the first seconds.
    let trimmed = ctx.work.join(format!("{stem}.trimmed.wav"));
    match ctx.max_secs {
        Some(max) if info.duration_secs().is_none_or(|d| d > max) => {
            let made = trim_to_wav(clip, &trimmed, max)
                .and_then(|_| probe(&trimmed).map_err(|e| e.to_string()));
            match made {
                Ok(i) => {
                    report.notes.push(format!("{name}: first {max} s only"));
                    Some((stem, trimmed, i))
                }
                Err(e) => {
                    report
                        .notes
                        .push(format!("{name}: skipped, can't trim it ({e})"));
                    None
                }
            }
        }
        _ => Some((stem, clip.to_path_buf(), info)),
    }
}

/// One clip through every lossy target at every preset. What Kiln can't write
/// on this PC goes into `report.notes` and no row.
fn run_clip(ctx: &Context, clip: &Path, report: &mut Report) {
    let Some((stem, clip, info)) = prepare(ctx, clip, report) else {
        return;
    };
    let clip = clip.as_path();
    // ViSQOL's reference: the clip itself, made a WAV by FFmpeg.
    let reference = ctx.visqol.as_ref().filter(|_| ctx.ffmpeg).and_then(|_| {
        let wav = ctx.work.join(format!("{stem}.ref.wav"));
        match visqol_wav(clip, &wav) {
            Ok(()) => Some(wav),
            Err(e) => {
                report
                    .notes
                    .push(format!("ViSQOL failed for {stem}'s reference: {e}"));
                None
            }
        }
    });
    for (target, codec_frame) in TARGETS {
        if let Err(e) = kiln_media::can_run(target) {
            let note = format!("{}: skipped on this PC ({e})", target.name());
            if !report.notes.contains(&note) {
                report.notes.push(note);
            }
            continue;
        }
        for (preset, preset_name) in PRESETS {
            let row = run_row(
                ctx,
                clip,
                &stem,
                &info,
                target,
                codec_frame,
                preset,
                preset_name,
                reference.as_deref(),
                &mut report.notes,
            );
            report.rows.push(row);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_row(
    ctx: &Context,
    clip: &Path,
    stem: &str,
    info: &SourceInfo,
    target: Target,
    codec_frame: usize,
    preset: Preset,
    preset_name: &'static str,
    visqol_reference: Option<&Path>,
    notes: &mut Vec<String>,
) -> Row {
    let mut row = Row::new(stem, target.name(), preset_name);
    let fail = |mut row: Row, why: String| {
        row.misses.push(why);
        row.error = true;
        row
    };

    // Kiln.
    let ext = target.ext();
    let kiln_out = ctx.work.join(format!("{stem}.{preset_name}.kiln.{ext}"));
    let start = Instant::now();
    let result = convert(
        clip,
        &kiln_out,
        target,
        preset,
        &Tune::default(),
        &mut |_| {},
        &|| false,
    );
    let kiln_wall = start.elapsed().as_secs_f64();
    if let Err(e) = result {
        return fail(row, format!("Kiln failed: {e}"));
    }
    let kiln_pcm = match decode(&kiln_out) {
        Ok(p) => p,
        Err(e) => return fail(row, format!("Kiln's file can't be read back: {e}")),
    };
    let kiln_bytes = std::fs::metadata(&kiln_out).map_or(0, |m| m.len());
    row.kiln = side_of(&kiln_pcm, kiln_bytes, kiln_wall);

    // What Kiln wrote is what the setting is worked out for: its rate and channels.
    let mut written = info.clone();
    let written_channels = Channels {
        count: kiln_pcm.channels,
        mask: 0,
    };
    written.frames = info
        .frames
        .map(|f| (f as f64 * kiln_pcm.rate as f64 / info.rate.max(1) as f64).round() as u64);
    written.rate = kiln_pcm.rate;
    written.channels = written_channels;
    let setting = match chosen_setting(target, preset, &Tune::default(), &written) {
        Ok(s) => s,
        Err(e) => return fail(row, format!("no setting for the preset: {e}")),
    };
    row.setting = label_of(target, &setting, &written);

    // Size against the request (CBR and ABR), and against the estimate (VBR).
    match requested_kbps(&setting) {
        Some(kbps) if row.kiln.secs > 0.0 && kiln_pcm.secs() > 0.0 => {
            // WMA's bitrate pays for its data packets, not for the ASF
            // header and index around them.
            let paid = match target {
                Target::Wma => asf_data_bytes(&kiln_out).unwrap_or_else(|| {
                    row.warnings
                        .push("no ASF data object, so the whole file is measured".into());
                    kiln_bytes
                }),
                _ => kiln_bytes,
            };
            let actual = paid as f64 * 8.0 / 1000.0 / kiln_pcm.secs();
            let off = actual / kbps as f64 - 1.0;
            row.size_off = Some(off);
            // Even without the header, WMA's packets hold about 0.6 s more
            // than the length, whatever the length: past 5% on a short clip.
            let short_wma = target == Target::Wma && kiln_pcm.secs() < WMA_SIZE_SECS;
            if short_wma && off.abs() > SIZE_TOLERANCE {
                row.warnings.push(format!(
                    "size is {:+.1}% from {kbps} kbps, not judged under {WMA_SIZE_SECS:.0} s for WMA",
                    off * 100.0
                ));
            } else if off.abs() > SIZE_TOLERANCE {
                row.misses.push(format!(
                    "size is {:+.1}% from {kbps} kbps (limit {:.0}%)",
                    off * 100.0,
                    SIZE_TOLERANCE * 100.0
                ));
            }
        }
        Some(_) => {}
        None => {
            if let Some(est) = estimate_bytes(target, &setting, &written) {
                if kiln_bytes > 0 {
                    row.estimate_error = Some((kiln_bytes as f64 - est as f64) / kiln_bytes as f64);
                }
            }
        }
    }

    // Length is judged against the source, after any rate conversion (R40):
    // FFmpeg's own length is only shown.
    row.source_frames = info
        .frames
        .map(|f| (f as f64 * kiln_pcm.rate as f64 / info.rate.max(1) as f64).round() as usize);
    if let Some(expected) = row.source_frames {
        let diff = kiln_pcm.frames().abs_diff(expected);
        if diff > codec_frame {
            row.misses.push(format!(
                "length is {diff} frames from the source's (one codec frame is {codec_frame})"
            ));
        }
    }

    // FFmpeg, at the same rate and channels, for the same setting. Without
    // it, the checks above are the row's verdict.
    if !ctx.ffmpeg {
        return row;
    }
    let ff_out = ctx.work.join(format!("{stem}.{preset_name}.ffmpeg.{ext}"));
    let rate = (kiln_pcm.rate != info.rate).then_some(kiln_pcm.rate);
    let channels = (kiln_pcm.channels != info.channels.count).then_some(kiln_pcm.channels);
    let encoder = encoder_args(target, &setting);
    let ff_wall = match run_ffmpeg(clip, &ff_out, &encoder, rate, channels) {
        Ok(w) => w,
        Err(e) => {
            row.warnings
                .push(format!("FFmpeg couldn't write it, so not compared ({e})"));
            return row;
        }
    };
    let ff_pcm = match decode(&ff_out) {
        Ok(p) => p,
        Err(e) => {
            row.warnings.push(format!(
                "FFmpeg's file can't be read back, so not compared ({e})"
            ));
            return row;
        }
    };
    let ff_bytes = std::fs::metadata(&ff_out).map_or(0, |m| m.len());
    let mut ff = side_of(&ff_pcm, ff_bytes, ff_wall);

    // ViSQOL, when it is set up.
    if let (Some(v), Some(reference)) = (&ctx.visqol, visqol_reference) {
        let mut score = |file: &Path, tag: &str| -> Option<f64> {
            let wav = ctx
                .work
                .join(format!("{stem}.{preset_name}.{tag}.visqol.wav"));
            match visqol_wav(file, &wav).and_then(|_| visqol_score(v, reference, &wav)) {
                Ok(s) => Some(s),
                Err(e) => {
                    notes.push(format!(
                        "ViSQOL failed for {stem} {} {preset_name} ({tag}): {e}",
                        target.name()
                    ));
                    None
                }
            }
        };
        row.kiln.visqol = score(&kiln_out, "kiln");
        ff.visqol = score(&ff_out, "ffmpeg");
    }

    // The checks against FFmpeg.
    if let (Some(k), Some(f)) = (row.kiln.lufs, ff.lufs) {
        if (k - f).abs() > LOUDNESS_TOLERANCE {
            row.misses
                .push(format!("loudness differs by {:+.2} LU", k - f));
        }
    }
    if row.kiln.cutoff_hz < ff.cutoff_hz - CUTOFF_TOLERANCE {
        let said = format!(
            "cutoff {:.1} kHz is under FFmpeg's {:.1} kHz",
            row.kiln.cutoff_hz / 1000.0,
            ff.cutoff_hz / 1000.0
        );
        if cutoff_is_a_miss(target, kiln_pcm.channels) {
            row.misses.push(said);
        } else {
            row.warnings.push(said);
        }
    }
    if let (Some(k), Some(f)) = (row.kiln.visqol, ff.visqol) {
        if k < f - VISQOL_TOLERANCE {
            row.misses
                .push(format!("ViSQOL {k:.2} is under FFmpeg's {f:.2}"));
        }
    }
    if row.kiln.secs > SPEED_WARN * ff.secs {
        row.warnings.push(format!(
            "Kiln took {:.1}x FFmpeg's time",
            row.kiln.secs / ff.secs.max(1e-9)
        ));
    }
    row.ffmpeg = Some(ff);
    row
}

/// Every clip in `dir`, by name.
fn clips_in(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut list: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| AUDIO_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        })
        .collect();
    list.sort();
    Ok(list)
}

/// What a panic said, as far as it can be read.
fn panic_text(panic: &(dyn std::any::Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "no message".to_string())
}

fn run_folder(
    dir: &Path,
    visqol: Option<PathBuf>,
    max_secs: Option<f64>,
    ffmpeg: bool,
) -> Result<Report, String> {
    let clips = clips_in(dir).map_err(|e| format!("can't read {}: {e}", dir.display()))?;
    let mut report = Report {
        rows: Vec::new(),
        notes: Vec::new(),
    };
    if clips.is_empty() {
        report.notes.push(format!("no clips in {}", dir.display()));
        return Ok(report);
    }
    let work = tempfile::tempdir().map_err(|e| e.to_string())?;
    let ctx = Context {
        work: work.path(),
        visqol,
        max_secs,
        ffmpeg,
    };
    for clip in &clips {
        eprintln!("harness: {}", clip.display());
        guarded(clip, &mut report, |report| run_clip(&ctx, clip, report));
    }
    Ok(report)
}

/// `run` for `clip`, with a panic made a failed row naming the clip, so the
/// clips after it still run and the run fails.
fn guarded(clip: &Path, report: &mut Report, run: impl FnOnce(&mut Report)) {
    let ran = catch_unwind(AssertUnwindSafe(|| run(report)));
    if let Err(panic) = ran {
        let name = clip.file_stem().and_then(|s| s.to_str()).unwrap_or("clip");
        let mut row = Row::new(name, "every target", "every preset");
        row.misses
            .push(format!("the harness panicked: {}", panic_text(&*panic)));
        row.error = true;
        report.rows.push(row);
    }
}

// ------------------------------------------------------------- calibration

/// `KILN_HARNESS_CALIBRATE`: converts every music clip (not a speech one) at
/// every V level of MP3 (V0 to V9) and every whole quality step of Vorbis (q-1
/// to q10) with Kiln alone, and gives the median measured kbps per setting,
/// divided by the channel scale `tune` uses. These are the numbers for the
/// kbps tables in `tune.rs`.
fn calibrate(dir: &Path, max_secs: Option<f64>) -> Result<String, String> {
    use std::fmt::Write;
    let clips = clips_in(dir).map_err(|e| format!("can't read {}: {e}", dir.display()))?;
    let work = tempfile::tempdir().map_err(|e| e.to_string())?;
    let ctx = Context {
        work: work.path(),
        visqol: None,
        max_secs,
        ffmpeg: false,
    };
    let mut notes = Report {
        rows: Vec::new(),
        notes: Vec::new(),
    };
    let mut prepared = Vec::new();
    for clip in &clips {
        let name = clip.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if name.to_ascii_lowercase().contains("speech") {
            notes.notes.push(format!("{name}: speech, left out"));
            continue;
        }
        eprintln!("calibrate: {}", clip.display());
        prepared.extend(prepare(&ctx, clip, &mut notes));
    }
    let mut out = String::new();
    let _ = writeln!(out, "# Calibration: median kbps per setting\n");
    for n in &notes.notes {
        let _ = writeln!(out, "- {n}");
    }
    let _ = writeln!(
        out,
        "- {} clips. Each value is the file's bytes over its length, divided by the channel scale.\n",
        prepared.len()
    );
    let _ = writeln!(
        out,
        "| Target | Setting | Median kbps | Lowest | Highest | Clips |"
    );
    let _ = writeln!(out, "|---|---|---|---|---|---|");
    let mut lists: Vec<(Target, Vec<(Setting, f64)>)> = Vec::new();
    let mp3: Vec<Setting> = (0..=9u8)
        .map(|v| Setting::LameVbr { tenths: v * 10 })
        .collect();
    let ogg: Vec<Setting> = (-1..=10i16)
        .map(|q| Setting::VorbisQ { tenths: q * 10 })
        .collect();
    for (target, settings) in [(Target::Mp3, mp3), (Target::Ogg, ogg)] {
        let mut medians = Vec::new();
        for setting in settings {
            let mut values = Vec::new();
            for (stem, clip, info) in &prepared {
                let file = work.path().join(format!("{stem}.cal.{}", target.ext()));
                let tune = Tune {
                    setting: Some(setting),
                    ..Tune::default()
                };
                let made = convert(
                    clip,
                    &file,
                    target,
                    Preset::High,
                    &tune,
                    &mut |_| {},
                    &|| false,
                );
                let kbps = made.map_err(|e| e.to_string()).and_then(|_| {
                    let bytes = std::fs::metadata(&file).map_err(|e| e.to_string())?.len();
                    let channels = probe(&file).map_err(|e| e.to_string())?.channels.count;
                    let secs = info.duration_secs().ok_or("no length")?;
                    Ok::<f64, String>(bytes as f64 * 8.0 / 1000.0 / secs / channel_scale(channels))
                });
                let _ = std::fs::remove_file(&file);
                match kbps {
                    Ok(k) => values.push(k),
                    Err(e) => notes
                        .notes
                        .push(format!("{stem}: {} failed ({e})", target.name())),
                }
            }
            let m = metrics::median(&values).ok_or("nothing measured")?;
            let lo = values.iter().copied().fold(f64::MAX, f64::min);
            let hi = values.iter().copied().fold(0.0, f64::max);
            let label = label_of(target, &setting, &kbps_info());
            let _ = writeln!(
                out,
                "| {} | {label} | {m:.1} | {lo:.1} | {hi:.1} | {} |",
                target.name(),
                values.len()
            );
            medians.push((setting, m));
        }
        lists.push((target, medians));
    }
    let _ = writeln!(out);
    for (target, medians) in lists {
        let nums: Vec<String> = medians
            .iter()
            .map(|(_, m)| format!("{}", m.round() as u32))
            .collect();
        let _ = writeln!(out, "{}: [{}]", target.name(), nums.join(", "));
    }
    Ok(out)
}

/// A stereo source of a minute, only so a label can be drawn.
fn kbps_info() -> SourceInfo {
    SourceInfo {
        codec: kiln_media::Codec::Flac,
        rate: 44_100,
        channels: Channels {
            count: 2,
            mask: 0x3,
        },
        bits: Some(16),
        frames: Some(44_100 * 60),
        kbps: None,
        tags: Tags::default(),
    }
}

// -------------------------------------------------------- synthetic clips

/// A few seconds of music-like audio, the same on every run: an arpeggio of
/// notes with harmonics and decays, a bass line, and noise hi-hats. In stereo,
/// or as 5.1 (melody on the front pair, a quieter copy in the centre, the bass
/// in the LFE and a delayed wash in the surrounds).
fn synthetic(rate: u32, channels: u16, secs: usize) -> Vec<f32> {
    let n = rate as usize * secs;
    let noise = metrics::white_noise(n, 11);
    let notes = [220.0, 261.63, 329.63, 392.0, 329.63, 261.63, 440.0, 392.0];
    let beat = rate as usize / 4;
    let mut melody = vec![0.0f64; n];
    let mut bass = vec![0.0f64; n];
    let mut hats = vec![0.0f64; n];
    for i in 0..n {
        let b = i / beat;
        let t = (i % beat) as f64 / rate as f64;
        let f = notes[b % notes.len()];
        let env = (-t * 7.0).exp();
        melody[i] = (1..=8)
            .map(|h| (2.0 * PI * f * h as f64 * i as f64 / rate as f64).sin() / h as f64)
            .sum::<f64>()
            * env
            * 0.25;
        bass[i] = (2.0 * PI * f / 4.0 * i as f64 / rate as f64).sin() * 0.3;
        let hat_env = (-((i % (beat / 2)) as f64) / (rate as f64 * 0.03)).exp();
        let prev = if i > 0 { noise[i - 1] } else { 0.0 };
        hats[i] = (noise[i] - prev) as f64 * hat_env * 0.12;
    }
    let delay = rate as usize / 40;
    let at = |v: &[f64], i: usize, d: usize| if i >= d { v[i - d] } else { 0.0 };
    let mut out = Vec::with_capacity(n * channels as usize);
    for i in 0..n {
        let left = melody[i] * 0.8 + bass[i] + hats[i];
        let right = melody[i] * 0.6 + bass[i] + hats[i] * 0.7;
        if channels == 2 {
            out.extend([left as f32, right as f32]);
        } else {
            out.extend([
                left as f32,
                right as f32,
                (melody[i] * 0.4) as f32,
                bass[i] as f32,
                (at(&melody, i, delay) * 0.3) as f32,
                (at(&melody, i, delay * 2) * 0.3) as f32,
            ]);
        }
    }
    out
}

fn write_wav(path: &Path, rate: u32, channels: u16, samples: &[f32]) {
    let mask = if channels == 2 { 0x3 } else { 0x3F };
    let tags = Tags::default();
    let setting = Setting::Pcm {
        bits: 16,
        float: false,
    };
    let mut sink = wav::sink(SinkSpec {
        rate,
        channels: Channels {
            count: channels,
            mask,
        },
        tags: &tags,
        setting: &setting,
        source_bits: Some(16),
        out: File::create(path).expect("create the synthetic clip"),
    })
    .expect("a WAV sink");
    for chunk in samples.chunks(8192 * channels as usize) {
        sink.write(&Block {
            channels,
            rate,
            samples: chunk.to_vec(),
        })
        .expect("write the synthetic clip");
    }
    sink.finish().expect("finish the synthetic clip");
}

// ------------------------------------------------------------------ checks

/// Where the report goes: `target/harness`, beside the profile folders.
fn report_dir() -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR"))
        .parent()
        .map_or_else(|| PathBuf::from("target"), Path::to_path_buf)
        .join("harness")
}

fn write_report(name: &str, text: &str) {
    let dir = report_dir();
    if std::fs::create_dir_all(&dir).is_ok() && std::fs::write(dir.join(name), text).is_ok() {
        eprintln!("harness: wrote {}", dir.join(name).display());
    } else {
        eprintln!("harness: could not write {}", dir.join(name).display());
    }
}

/// Two synthetic clips (stereo and 5.1) through the whole pipeline: every
/// lossy target at every preset, both encoders, every metric, the report. It
/// proves the harness works end to end; a quality miss on synthetic audio is
/// printed but doesn't fail it, since the real verdict is the maintainer's clips.
fn synthetic_clips_go_through_the_pipeline() {
    let Some(version) = ffmpeg_version() else {
        println!("    skipped: FFmpeg not found");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    write_wav(
        &dir.path().join("synthetic-stereo.wav"),
        44_100,
        2,
        &synthetic(44_100, 2, 12),
    );
    write_wav(
        &dir.path().join("synthetic-51.wav"),
        48_000,
        6,
        &synthetic(48_000, 6, 12),
    );
    let mut report = run_folder(dir.path(), None, None, true).expect("the synthetic folder runs");
    report.notes.insert(0, version);
    report.notes.insert(1, VISQOL_OFF.to_string());
    let text = report.markdown("Synthetic clips (self-check, not a verdict)");
    println!("{text}");
    write_report("synthetic-report.md", &text);

    let broken: Vec<String> = report
        .rows
        .iter()
        .filter(|r| r.error)
        .map(|r| {
            format!(
                "{} {} {}: {}",
                r.clip,
                r.target,
                r.preset,
                r.misses.join("; ")
            )
        })
        .collect();
    assert!(broken.is_empty(), "Kiln failed on: {broken:#?}");
    assert!(!report.rows.is_empty(), "no rows were made");
    for r in &report.rows {
        assert!(
            r.kiln.bytes > 0 && r.kiln.frames > 0,
            "{} {}",
            r.target,
            r.preset
        );
        assert!(
            r.kiln.cutoff_hz > 0.0,
            "{} {} has no cutoff",
            r.target,
            r.preset
        );
        assert!(
            r.kiln.lufs.is_some(),
            "{} {} has no loudness",
            r.target,
            r.preset
        );
    }
    assert!(
        report.rows.iter().any(|r| r.ffmpeg.is_some()),
        "FFmpeg made no file at all"
    );
}

fn a_panicking_clip_is_a_failed_row() {
    let mut report = Report {
        rows: Vec::new(),
        notes: Vec::new(),
    };
    guarded(Path::new("clips/odd one.flac"), &mut report, |_| {
        panic!("a test panic")
    });
    guarded(Path::new("clips/fine.flac"), &mut report, |r| {
        r.rows.push(Row::new("fine", "MP3", "High"))
    });
    assert_eq!(report.rows.len(), 2);
    let failed = &report.rows[0];
    assert_eq!(failed.clip, "odd one");
    assert!(failed.error && !failed.passed());
    assert!(
        failed.misses[0].contains("a test panic"),
        "{:?}",
        failed.misses
    );
    assert!(report.rows[1].passed());
    assert_eq!(report.failures(), 1);
}

fn the_asf_data_object_is_measured_without_the_header() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.wma");
    let mut bytes = Vec::new();
    // A Header Object of 40 bytes (24 of them its own head), then a Data
    // Object of 1,050, whose first 50 are its header, then an index.
    bytes.extend_from_slice(&[
        0x30, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11, 0xA6, 0xD9, 0x00, 0xAA, 0x00, 0x62, 0xCE,
        0x6C,
    ]);
    bytes.extend_from_slice(&40u64.to_le_bytes());
    bytes.extend_from_slice(&[0; 16]);
    bytes.extend_from_slice(&[
        0x36, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11, 0xA6, 0xD9, 0x00, 0xAA, 0x00, 0x62, 0xCE,
        0x6C,
    ]);
    bytes.extend_from_slice(&1_050u64.to_le_bytes());
    bytes.extend_from_slice(&[0; 1_026 + 200]);
    std::fs::write(&file, &bytes).unwrap();
    assert_eq!(asf_data_bytes(&file), Some(1_000));
    std::fs::write(&file, b"RIFF....WAVE").unwrap();
    assert_eq!(asf_data_bytes(&file), None);
}

type Check = (&'static str, fn());

/// Runs the checks, printing each as libtest would. False when one panicked.
fn run_checks() -> bool {
    let checks: [Check; 16] = [
        (
            "loudness_of_a_minus_23_lufs_sine_is_minus_23",
            metrics::loudness_of_a_minus_23_lufs_sine_is_minus_23,
        ),
        (
            "the_relative_gate_drops_quiet_passages",
            metrics::the_relative_gate_drops_quiet_passages,
        ),
        (
            "surround_channels_weigh_more_and_the_lfe_nothing",
            metrics::surround_channels_weigh_more_and_the_lfe_nothing,
        ),
        (
            "cutoff_of_a_lowpassed_noise",
            metrics::cutoff_of_a_lowpassed_noise,
        ),
        (
            "the_cutoff_is_where_the_spectrum_falls_30_db",
            metrics::the_cutoff_is_where_the_spectrum_falls_30_db,
        ),
        (
            "a_panicking_clip_is_a_failed_row",
            a_panicking_clip_is_a_failed_row,
        ),
        (
            "the_asf_data_object_is_measured_without_the_header",
            the_asf_data_object_is_measured_without_the_header,
        ),
        (
            "loudness_gates_out_silence_and_short_files",
            metrics::loudness_gates_out_silence_and_short_files,
        ),
        (
            "the_fft_finds_a_tone_and_the_median_takes_the_middle",
            metrics::the_fft_finds_a_tone_and_the_median_takes_the_middle,
        ),
        (
            "the_bpp_fit_finds_a_table_with_no_error",
            video::the_bpp_fit_finds_a_table_with_no_error,
        ),
        (
            "vmaf_spans_and_packets_are_read_right",
            video::vmaf_spans_and_packets_are_read_right,
        ),
        (
            "the_verdict_is_read_right",
            video::the_verdict_is_read_right,
        ),
        (
            "the_crf_search_steps_toward_the_size",
            video::the_crf_search_steps_toward_the_size,
        ),
        (
            "only_the_rows_asked_for_are_run",
            video::only_the_rows_asked_for_are_run,
        ),
        (
            "a_tiny_clip_goes_through_the_video_rows",
            video::a_tiny_clip_goes_through_the_video_rows,
        ),
        (
            "synthetic_clips_go_through_the_pipeline",
            synthetic_clips_go_through_the_pipeline,
        ),
    ];
    let mut ok = true;
    for (name, check) in checks {
        println!("check {name} ...");
        match catch_unwind(AssertUnwindSafe(check)) {
            Ok(()) => println!("check {name} ... ok"),
            Err(_) => {
                println!("check {name} ... FAILED");
                ok = false;
            }
        }
    }
    ok
}

/// The run on the maintainer's clips. True unless a row missed.
fn run_clips() -> bool {
    let mut notes = Vec::new();
    let version = ffmpeg_version();
    let ffmpeg = version.is_some();
    notes.push(version.unwrap_or_else(|| {
        "FFmpeg not found: only Kiln's own checks run (the length against the source, \
         the size against the request, the estimate's error), and nothing is compared"
            .to_string()
    }));
    let visqol = std::env::var_os("KILN_VISQOL").map(PathBuf::from);
    notes.push(match &visqol {
        Some(p) => format!("ViSQOL: {}", p.display()),
        None => VISQOL_OFF.to_string(),
    });
    let max_secs = std::env::var("KILN_HARNESS_MAX_SECS")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|&s| s > 0.0);
    let mut report = Report {
        rows: Vec::new(),
        notes,
    };
    let mut ok = true;
    if let (Some(dir), true) = (
        std::env::var_os("KILN_HARNESS_CLIPS"),
        std::env::var_os("KILN_HARNESS_CALIBRATE").is_some(),
    ) {
        return match calibrate(Path::new(&dir), max_secs) {
            Ok(text) => {
                println!("{text}");
                write_report("calibration.md", &text);
                true
            }
            Err(e) => {
                println!("calibration failed: {e}");
                false
            }
        };
    }
    match std::env::var_os("KILN_HARNESS_CLIPS") {
        None => report
            .notes
            .push("no clips (KILN_HARNESS_CLIPS is not set)".to_string()),
        Some(dir) => match run_folder(Path::new(&dir), visqol, max_secs, ffmpeg) {
            Ok(r) => {
                report.rows = r.rows;
                report.notes.extend(r.notes);
                ok = report.failures() == 0;
            }
            Err(e) => {
                report.notes.push(e);
                ok = false;
            }
        },
    }
    let text = report.markdown("Kiln against FFmpeg");
    println!("{text}");
    write_report("report.md", &text);
    ok
}

/// The video mode on the clips in `KILN_HARNESS_VIDEO_CLIPS`, written to
/// `target/harness/video-report.md`; with `KILN_HARNESS_VIDEO_CALIBRATE=1`,
/// Kiln alone at every calibration quality, written to
/// `target/harness/video-calibration.md`. True unless something missed;
/// true with a note when the variable isn't set.
fn run_video_clips() -> bool {
    let Some(dir) = std::env::var_os("KILN_HARNESS_VIDEO_CLIPS") else {
        println!("video: no clips (KILN_HARNESS_VIDEO_CLIPS is not set)");
        return true;
    };
    let max_secs = std::env::var("KILN_HARNESS_MAX_SECS")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|&s| s > 0.0);
    let set = |name: &str| std::env::var(name).is_ok_and(|v| v == "1");
    // Files made are kept beside the report only when asked with 1.
    let keep = set("KILN_HARNESS_VIDEO_KEEP").then(|| report_dir().join("video-work"));
    let calibrate = set("KILN_HARNESS_VIDEO_CALIBRATE");
    let only = match video::only_from(&std::env::var("KILN_HARNESS_VIDEO_ONLY").unwrap_or_default())
    {
        Ok(o) => o,
        Err(e) => {
            println!("video: {e}");
            return false;
        }
    };
    let report = match ffmpeg_version() {
        // R72: clips that can't be measured are a miss.
        None => {
            let mut r = report::VideoReport::default();
            r.misses.push(
                "FFmpeg not found: the video mode needs FFmpeg (libvmaf, zscale, libx264, \
                 libx265) and ffprobe, so nothing was measured"
                    .into(),
            );
            r
        }
        Some(version) => match video::run_folder(Path::new(&dir), max_secs, keep, calibrate, &only)
        {
            Ok(mut r) => {
                r.notes.insert(0, version);
                r
            }
            Err(e) => {
                let mut r = report::VideoReport::default();
                r.notes.push(e);
                r.misses.push("the video clips couldn't be run".into());
                r
            }
        },
    };
    let (title, file) = if calibrate {
        (
            "Kiln's video sizes, for the estimate",
            "video-calibration.md",
        )
    } else {
        ("Kiln's video against FFmpeg's", "video-report.md")
    };
    let text = report.markdown(title);
    println!("{text}");
    write_report(file, &text);
    report.failures() == 0
}

fn main() {
    let checks = run_checks();
    let clips = run_clips();
    let videos = run_video_clips();
    if !(checks && clips && videos) {
        std::process::exit(1);
    }
}
