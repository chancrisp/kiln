//! Running FFmpeg: every call has a time limit; what this FFmpeg can do;
//! trimming a clip; the encodes Kiln is compared with; and the CRF search
//! that matches an FFmpeg encode's size to Kiln's (R67).

use super::probe::{bytes, packets};
use super::Range;
use kiln_media::video::{Hdr, VCodec};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// The longest an ffprobe call may take.
pub const PROBE_LIMIT: Duration = Duration::from_secs(300);
/// The longest an encode, a trim or a VMAF run may take.
pub const WORK_LIMIT: Duration = Duration::from_secs(1800);

/// A CRF search wants a size this close to Kiln's when it is cheap...
const AIM: f64 = 0.05;
/// ...settles for the comparison band after this many new encodes...
const SETTLE_AFTER: usize = 5;
/// ...and gives up after this many.
const MAX_PROBES: usize = 8;
/// x264 and x265 roughly halve the size every 6 CRF; the first step's guess
/// of the slope of ln(size) against CRF.
const SLOPE: f64 = -std::f64::consts::LN_2 / 6.0;
/// The CRF range x264 and x265 take (8-bit).
const CRF_MIN: f64 = 0.0;
const CRF_MAX: f64 = 51.0;

/// Runs `cmd` with its output caught, killing it past `limit`.
pub fn run(cmd: &mut Command, limit: Duration) -> Result<Output, String> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let read = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut all = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut all);
            }
            all
        })
    };
    let out = read(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let err = read(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
            break s;
        }
        if start.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            let _ = (out.join(), err.join());
            return Err(format!("timed out after {} s", limit.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    Ok(Output {
        status,
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
    })
}

/// The last lines a tool wrote to stderr, for a reason.
pub fn tail(stderr: &[u8]) -> String {
    let err = String::from_utf8_lossy(stderr);
    let last: Vec<&str> = err.lines().rev().take(3).collect();
    last.into_iter().rev().collect::<Vec<_>>().join(" ")
}

/// FFmpeg's first version line, when `ffmpeg` runs from PATH.
pub fn version() -> Option<String> {
    let out = run(Command::new("ffmpeg").arg("-version"), PROBE_LIMIT).ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .map(str::to_string)
}

/// What the video mode needs that this FFmpeg lacks: the `libvmaf` and
/// `zscale` filters, the `libx264` and `libx265` encoders, and ffprobe.
pub fn missing_features() -> Result<Vec<&'static str>, String> {
    let list = |what: &str| -> Result<String, String> {
        let out = run(
            Command::new("ffmpeg").args(["-hide_banner", what]),
            PROBE_LIMIT,
        )?;
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let filters = list("-filters")?;
    let encoders = list("-encoders")?;
    let has = |text: &str, name: &str| text.split_whitespace().any(|w| w == name);
    let mut missing = Vec::new();
    for f in ["libvmaf", "zscale"] {
        if !has(&filters, f) {
            missing.push(f);
        }
    }
    for e in ["libx264", "libx265"] {
        if !has(&encoders, e) {
            missing.push(e);
        }
    }
    let probe = run(Command::new("ffprobe").arg("-version"), PROBE_LIMIT);
    if !probe.is_ok_and(|o| o.status.success()) {
        missing.push("ffprobe");
    }
    Ok(missing)
}

/// Runs an FFmpeg that writes `out`, with `-y` and only errors logged.
fn write(args: &[&str], input: &Path, rest: &[String], out: &Path) -> Result<f64, String> {
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-nostdin", "-y", "-hide_banner", "-loglevel", "error"])
        .args(args)
        .arg("-i")
        .arg(input)
        .args(rest)
        .arg(out);
    let start = Instant::now();
    let result = run(&mut cmd, WORK_LIMIT)?;
    let secs = start.elapsed().as_secs_f64();
    if !result.status.success() || !out.exists() {
        return Err(tail(&result.stderr));
    }
    Ok(secs)
}

/// The first `secs` seconds of `clip`, copied packet for packet into `out`.
pub fn trim(clip: &Path, out: &Path, secs: f64) -> Result<(), String> {
    let rest = ["-t", &secs.to_string(), "-map", "0", "-c", "copy"].map(str::to_string);
    write(&[], clip, &rest, out).map(|_| ())
}

/// A two-second test clip made by FFmpeg itself: `testsrc2` at 320×240 and
/// 24 fps in H.264 with a key frame every half second, and a tone in AAC.
pub fn test_clip(out: &Path) -> Result<(), String> {
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-nostdin", "-y", "-hide_banner", "-loglevel", "error"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x240:rate=24:duration=2",
        ])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=2"])
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-g", "12"])
        .args(["-c:a", "aac", "-b:a", "96k", "-shortest"])
        .arg(out);
    let result = run(&mut cmd, WORK_LIMIT)?;
    if result.status.success() && out.exists() {
        Ok(())
    } else {
        Err(tail(&result.stderr))
    }
}

/// What an FFmpeg encode is made from: the clip itself, compared in a range,
/// or Kiln's own tone-mapped q90 encode of an HDR clip from the graphics
/// card (true) or the processor (R71).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Basis {
    Source(Range),
    ToneRef(bool),
}

/// FFmpeg's encode of `input` for one codec and CRF into MP4: `libx264` or
/// `libx265 -preset medium`, 10-bit HDR10 (or HLG) when `hdr` says the
/// picture stays HDR, else 8-bit 4:2:0; frames keep their times
/// (`-fps_mode passthrough`). The sound is copied when `sound_copy`, else AAC
/// at 128 kbps. Returns the wall time.
pub fn encode(
    input: &Path,
    out: &Path,
    codec: VCodec,
    crf: f64,
    hdr: Option<Hdr>,
    sound_copy: bool,
) -> Result<f64, String> {
    let s = |v: &str| v.to_string();
    let mut rest = vec![
        s("-map"),
        s("0:v:0"),
        s("-map"),
        s("0:a:0?"),
        s("-map_metadata"),
        s("-1"),
        s("-fps_mode"),
        s("passthrough"),
    ];
    let crf = format!("{crf:.2}");
    match codec {
        VCodec::H264 => rest.extend([s("-c:v"), s("libx264")]),
        VCodec::Hevc => rest.extend([s("-c:v"), s("libx265"), s("-tag:v"), s("hvc1")]),
    }
    rest.extend([s("-preset"), s("medium"), s("-crf"), crf]);
    match (hdr, codec) {
        (Some(h), VCodec::Hevc) => {
            let transfer = if h == Hdr::Hlg {
                "arib-std-b67"
            } else {
                "smpte2084"
            };
            rest.extend([s("-pix_fmt"), s("yuv420p10le")]);
            rest.extend([s("-color_primaries"), s("bt2020")]);
            rest.extend([
                s("-color_trc"),
                s(transfer),
                s("-colorspace"),
                s("bt2020nc"),
            ]);
            rest.extend([
                s("-x265-params"),
                format!(
                    "log-level=error:hdr10=1:colorprim=bt2020:transfer={transfer}:colormatrix=bt2020nc"
                ),
            ]);
        }
        (_, VCodec::Hevc) => {
            rest.extend([
                s("-pix_fmt"),
                s("yuv420p"),
                s("-x265-params"),
                s("log-level=error"),
            ]);
        }
        (_, VCodec::H264) => rest.extend([s("-pix_fmt"), s("yuv420p")]),
    }
    if sound_copy {
        rest.extend([s("-c:a"), s("copy")]);
    } else {
        rest.extend([s("-c:a"), s("aac"), s("-b:a"), s("128k")]);
    }
    write(&[], input, &rest, out)
}

/// One FFmpeg encode the search made: its CRF, file, video-stream bytes and
/// wall time.
#[derive(Clone, Debug)]
pub struct Encoded {
    pub crf: f64,
    pub path: PathBuf,
    pub bytes: u64,
    pub secs: f64,
}

/// The encodes made for one clip, shared by its rows, and the VMAF of the
/// ones chosen.
#[derive(Default)]
pub struct FfCache {
    encodes: HashMap<(VCodec, Basis), Vec<Encoded>>,
    pub vmaf: HashMap<PathBuf, Result<f64, String>>,
}

/// What [`matched`] found: the encode, and how many encodes the search made.
pub struct Matched {
    pub enc: Encoded,
    pub probes: usize,
}

/// The next CRF to try for a size of e^`target_ln`, from the (CRF,
/// ln size) points known so far, sorted by CRF; `seed` when there are none.
/// Between a point over the target and one under it, interpolates
/// log-linearly; past every point, steps from the nearest along the slope
/// of the two nearest (or [`SLOPE`]). `None` when the step would leave
/// 0–51 from a point already at the bound.
pub fn next_crf(points: &[(f64, f64)], target_ln: f64, seed: f64) -> Option<f64> {
    let round = |c: f64| (c.clamp(CRF_MIN, CRF_MAX) * 100.0).round() / 100.0;
    if points.is_empty() {
        return Some(round(seed));
    }
    // Over the target: a higher CRF is wanted; under it, a lower one.
    let over = points
        .iter()
        .filter(|p| p.1 > target_ln)
        .max_by(|a, b| a.0.total_cmp(&b.0));
    let under = points
        .iter()
        .filter(|p| p.1 < target_ln)
        .min_by(|a, b| a.0.total_cmp(&b.0));
    if let (Some(o), Some(u)) = (over, under) {
        if o.0 < u.0 && u.1 < o.1 {
            let c = o.0 + (target_ln - o.1) * (u.0 - o.0) / (u.1 - o.1);
            return Some(round(c));
        }
        return Some(round((o.0 + u.0) / 2.0));
    }
    let nearest = points
        .iter()
        .min_by(|a, b| (a.1 - target_ln).abs().total_cmp(&(b.1 - target_ln).abs()))?;
    let mut by_crf: Vec<&(f64, f64)> = points.iter().collect();
    by_crf.sort_by(|a, b| (a.0 - nearest.0).abs().total_cmp(&(b.0 - nearest.0).abs()));
    let slope = match by_crf.get(1) {
        Some(other) if (other.0 - nearest.0).abs() > 0.01 => {
            let s = (other.1 - nearest.1) / (other.0 - nearest.0);
            if s < -0.01 {
                s
            } else {
                SLOPE
            }
        }
        _ => SLOPE,
    };
    let c = round(nearest.0 + (target_ln - nearest.1) / slope);
    ((c - nearest.0).abs() >= 0.005).then_some(c)
}

/// FFmpeg's encode of `input` whose video stream leaves Kiln's, `target`
/// bytes, within ±15% of it, aiming for ±5% (R67), measured as the verdict
/// measures it (R83): fractional CRF, from `seed` (the CRF the
/// preset's table gives), each next CRF interpolated log-linearly from the
/// encodes already made for this codec and basis, which every row of the
/// clip shares. Settles for ±15% after 5 new encodes, gives up after 8, and
/// says why when 0–51 can't reach the band.
#[allow(clippy::too_many_arguments)]
pub fn matched(
    cache: &mut FfCache,
    work: &Path,
    stem: &str,
    codec: VCodec,
    basis: Basis,
    input: &Path,
    hdr: Option<Hdr>,
    sound_copy: bool,
    target: u64,
    seed: f64,
) -> Result<Matched, String> {
    let target_ln = (target.max(1) as f64).ln();
    // Kiln's size against this encode's, as the verdict reads it (R83).
    let off = |e: &Encoded| super::size_off(target, e.bytes);
    let mut made = 0;
    loop {
        let known = cache.encodes.entry((codec, basis)).or_default();
        let best = known
            .iter()
            .min_by(|a, b| off(a).abs().total_cmp(&off(b).abs()))
            .cloned();
        if let Some(b) = &best {
            let o = off(b).abs();
            if o <= AIM || (made >= SETTLE_AFTER && o <= super::SIZE_BAND) {
                return Ok(Matched {
                    enc: b.clone(),
                    probes: made,
                });
            }
        }
        let mut points: Vec<(f64, f64)> = known
            .iter()
            .map(|e| (e.crf, (e.bytes.max(1) as f64).ln()))
            .collect();
        points.sort_by(|a, b| a.0.total_cmp(&b.0));
        let next = (made < MAX_PROBES)
            .then(|| next_crf(&points, target_ln, seed))
            .flatten()
            .filter(|c| !points.iter().any(|p| (p.0 - c).abs() < 0.005));
        let Some(crf) = next else {
            let Some(b) = best else {
                return Err("no CRF to try".into());
            };
            if off(&b).abs() <= super::SIZE_BAND {
                return Ok(Matched {
                    enc: b,
                    probes: made,
                });
            }
            let at_bound = |c: f64| c <= CRF_MIN + 0.005 || c >= CRF_MAX - 0.005;
            return Err(if at_bound(b.crf) {
                format!(
                    "no CRF in 0–51 brings FFmpeg within ±15% of Kiln's size: at CRF {:.2} \
                     Kiln's is {:+.1}% from FFmpeg's",
                    b.crf,
                    off(&b) * 100.0
                )
            } else {
                format!(
                    "no CRF within ±15% of Kiln's size after {made} encodes (closest: CRF {:.2}, \
                     Kiln's {:+.1}% from FFmpeg's)",
                    b.crf,
                    off(&b) * 100.0
                )
            });
        };
        let out = work.join(format!(
            "{stem}.{}.{}.crf{crf:.2}.ffmpeg.mp4",
            match codec {
                VCodec::H264 => "H264",
                VCodec::Hevc => "HEVC",
            },
            match basis {
                Basis::Source(r) => format!("{r:?}"),
                Basis::ToneRef(true) => "ToneRefCard".to_string(),
                Basis::ToneRef(false) => "ToneRefCpu".to_string(),
            }
        ));
        let secs = encode(input, &out, codec, crf, hdr, sound_copy)
            .map_err(|e| format!("FFmpeg failed at CRF {crf:.2}: {e}"))?;
        let size = bytes(&packets(&out, "v:0", false)?);
        made += 1;
        cache
            .encodes
            .entry((codec, basis))
            .or_default()
            .push(Encoded {
                crf,
                path: out,
                bytes: size,
                secs,
            });
    }
}
