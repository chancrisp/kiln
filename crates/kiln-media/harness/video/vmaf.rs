//! Comparing pictures: what a file is compared with (its reference), the
//! filter chains that make both sides comparable, VMAF, and the check that
//! an output's first frame is the source's first.
//!
//! Colour (R73): a source that describes no colour is labelled the way
//! players read it (R68: BT.601 under 720 lines, else BT.709); every tagged
//! file keeps its own tags, and `scale` converts it into the reference's
//! matrix. So a wrongly tagged output scores what a player would show.

use super::ffmpeg::{run, tail, WORK_LIMIT};
use super::probe::Tags;
use super::Clip;
use kiln_media::video::Hdr;
use std::path::{Path, PathBuf};
use std::process::Command;

/// HDR to SDR by Hable's curve: linear light, BT.709 primaries, then BT.709
/// limited range. `{tin}` is the source's transfer. Only the tone-curve note
/// uses it (R71); no row is judged against it.
const TONEMAP: &str = "zscale=tin={tin}:min=bt2020nc:pin=bt2020:rin=tv:t=linear:npl=100,\
format=gbrpf32le,zscale=p=bt709,tonemap=tonemap=hable:desat=0,\
zscale=t=bt709:m=bt709:r=tv,format=yuv420p";

/// VMAF's model. Every row uses the default; the tone-curve note uses the
/// one that doesn't reward a picture with more contrast than its reference,
/// which the default scores over 100.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Model {
    Default,
    NoEnhancement,
}

/// What a picture is compared with: a file, the filters that come before
/// its scaling (a label for an untagged source, Hable's curve), the colour
/// it is compared in, its bit depth and the size both sides are scaled to.
#[derive(Clone, Debug)]
pub struct Reference {
    pub path: PathBuf,
    pub pre: Vec<String>,
    pub tags: Tags,
    pub ten_bit: bool,
    pub size: (u32, u32),
}

impl Reference {
    /// The clip itself: 10-bit for an output kept HDR, else 8-bit.
    pub fn source(clip: &Clip, hdr: bool) -> Reference {
        let tags = clip.untagged_fix.clone().unwrap_or(clip.facts.tags.clone());
        Reference {
            path: clip.path.clone(),
            pre: clip
                .untagged_fix
                .as_ref()
                .map(Tags::setparams)
                .into_iter()
                .collect(),
            tags: tags.or_tv(),
            ten_bit: hdr,
            size: clip.upright,
        }
    }

    /// The clip tone-mapped by Hable's curve, for the tone-curve note.
    pub fn hable(clip: &Clip) -> Reference {
        let tin = if clip.info.hdr == Hdr::Hlg {
            "arib-std-b67"
        } else {
            "smpte2084"
        };
        Reference {
            path: clip.path.clone(),
            pre: vec![TONEMAP.replace("{tin}", tin)],
            tags: Tags::bt709(),
            ten_bit: false,
            size: clip.upright,
        }
    }

    /// Another file, compared in its own tags: Kiln's tone-mapped q90
    /// encode of an HDR clip (R71).
    pub fn file(path: &Path, tags: Tags, size: (u32, u32)) -> Reference {
        Reference {
            path: path.to_path_buf(),
            pre: Vec::new(),
            tags: tags.or_tv(),
            ten_bit: false,
            size,
        }
    }

    /// Scaling from `from`'s matrix and range into the reference's, the
    /// pixel format, the reference's labels and square pixels: the end of
    /// both sides' chains. The input's matrix is named, not left to the
    /// frames: a stream whose container says BT.709 but whose frames don't
    /// would otherwise be read as BT.601.
    fn tail_filters(&self, from: &Tags) -> Vec<String> {
        let (w, h) = self.size;
        let range = |t: &Tags| if t.range == "pc" { "pc" } else { "tv" };
        vec![
            format!(
                "scale={w}:{h}:flags=bicubic:in_color_matrix={}:in_range={}:\
                 out_color_matrix={}:out_range=tv",
                from.matrix(),
                range(from),
                self.tags.matrix()
            ),
            format!(
                "format={}",
                if self.ten_bit {
                    "yuv420p10le"
                } else {
                    "yuv420p"
                }
            ),
            self.tags.setparams(),
            "setsar=1".into(),
        ]
    }

    /// The chain the reference itself goes through.
    pub fn chain(&self) -> String {
        let mut f = self.pre.clone();
        f.extend(self.tail_filters(&self.tags));
        f.join(",")
    }

    /// The chain a compared file goes through: labelled by `fix` only when
    /// it says nothing of its colour, then converted into the reference's.
    pub fn chain_for(&self, tags: &Tags, fix: Option<&Tags>) -> String {
        let mut f = Vec::new();
        let mut from = tags.clone().or_tv();
        if tags.untagged() {
            if let Some(fix) = fix {
                f.push(fix.setparams());
                from = fix.clone();
            }
        }
        f.extend(self.tail_filters(&from));
        f.join(",")
    }
}

/// The VMAF score in FFmpeg's log: the number after "VMAF score:".
pub fn vmaf_of(log: &str) -> Option<f64> {
    log.lines()
        .rev()
        .find_map(|l| l.split("VMAF score:").nth(1))
        .and_then(|v| v.trim().parse().ok())
}

/// VMAF of `distorted` (whose tags are `tags`) against `reference`, both
/// scaled to the reference's size and pixel format and synced by their
/// times from the first frame (`setpts=PTS-STARTPTS`, `-fps_mode
/// passthrough`). FFmpeg turns rotated inputs upright as it reads them.
pub fn vmaf(
    distorted: &Path,
    tags: &Tags,
    fix: Option<&Tags>,
    reference: &Reference,
    model: Model,
) -> Result<f64, String> {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let model = match model {
        Model::Default => "",
        Model::NoEnhancement => ":model=version=vmaf_v0.6.1neg",
    };
    let graph = format!(
        "[0:v]setpts=PTS-STARTPTS,{}[d];[1:v]setpts=PTS-STARTPTS,{}[r];\
         [d][r]libvmaf=n_threads={threads}{model}",
        reference.chain_for(tags, fix),
        reference.chain()
    );
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-nostdin", "-hide_banner", "-i"])
        .arg(distorted)
        .arg("-i")
        .arg(&reference.path)
        .args([
            "-lavfi",
            &graph,
            "-fps_mode",
            "passthrough",
            "-f",
            "null",
            "-",
        ]);
    let out = run(&mut cmd, WORK_LIMIT)?;
    let log = String::from_utf8_lossy(&out.stderr);
    match vmaf_of(&log) {
        Some(v) if out.status.success() => Ok(v),
        _ => Err(format!("libvmaf gave no score ({})", tail(&out.stderr))),
    }
}

/// The first `count` decoded frames of `file` through `chain`, made small
/// gray pictures for comparing.
fn small_frames(file: &Path, chain: &str, count: usize) -> Result<Vec<Vec<u8>>, String> {
    const W: usize = 256;
    const H: usize = 144;
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-nostdin", "-v", "error", "-i"]).arg(file).args([
        "-vf",
        &format!("{chain},scale={W}:{H},format=gray"),
        "-frames:v",
        &count.to_string(),
        "-fps_mode",
        "passthrough",
        "-f",
        "rawvideo",
        "-",
    ]);
    let out = run(&mut cmd, WORK_LIMIT)?;
    if !out.status.success() {
        return Err(tail(&out.stderr));
    }
    Ok(out.stdout.chunks_exact(W * H).map(<[u8]>::to_vec).collect())
}

/// The mean U and V of `file` through `chain`, in 8-bit levels, over ten
/// frames 24 frames apart from the first (a second apart at 24 fps, less
/// at higher rates), each scaled to 128 × 72 in 4:4:4.
fn chroma_means(file: &Path, chain: &str) -> Result<(f64, f64), String> {
    const W: usize = 128;
    const H: usize = 72;
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-nostdin", "-v", "error", "-i"]).arg(file).args([
        "-vf",
        &format!("{chain},select=not(mod(n\\,24)),scale={W}:{H},format=yuv444p"),
        "-frames:v",
        "10",
        "-fps_mode",
        "passthrough",
        "-f",
        "rawvideo",
        "-",
    ]);
    let out = run(&mut cmd, WORK_LIMIT)?;
    if !out.status.success() || out.stdout.len() < 3 * W * H {
        return Err(tail(&out.stderr));
    }
    let (mut u, mut v, mut n) = (0.0, 0.0, 0.0);
    for frame in out.stdout.chunks_exact(3 * W * H) {
        u += frame[W * H..2 * W * H]
            .iter()
            .map(|&x| x as f64)
            .sum::<f64>();
        v += frame[2 * W * H..].iter().map(|&x| x as f64).sum::<f64>();
        n += (W * H) as f64;
    }
    Ok((u / n, v / n))
}

/// VMAF reads the luma alone, so the colour is checked apart: the mean U
/// and V of `distorted` against `reference`'s, in 8-bit levels. A picture
/// whose colour is lost (U and V of 0, a green picture) is far off; a
/// different tone curve is a few levels off. (U, V) differences.
pub fn chroma_offset(
    distorted: &Path,
    tags: &Tags,
    fix: Option<&Tags>,
    reference: &Reference,
) -> Result<(f64, f64), String> {
    let (du, dv) = chroma_means(distorted, &reference.chain_for(tags, fix))?;
    let (ru, rv) = chroma_means(&reference.path, &reference.chain())?;
    Ok((du - ru, dv - rv))
}

/// Which of the reference's first four frames `distorted`'s first frame is,
/// by the least mean squared difference: 0 when it starts where the
/// reference does.
pub fn first_frame_offset(
    distorted: &Path,
    tags: &Tags,
    fix: Option<&Tags>,
    reference: &Reference,
) -> Result<usize, String> {
    let first = small_frames(distorted, &reference.chain_for(tags, fix), 1)?
        .into_iter()
        .next()
        .ok_or("no first frame")?;
    let source = small_frames(&reference.path, &reference.chain(), 4)?;
    nearest(&first, &source).ok_or_else(|| "the reference has no frames".into())
}

/// The index of the picture in `candidates` nearest `frame`.
pub fn nearest(frame: &[u8], candidates: &[Vec<u8>]) -> Option<usize> {
    let mse = |c: &[u8]| -> f64 {
        frame
            .iter()
            .zip(c)
            .map(|(&a, &b)| (a as f64 - b as f64).powi(2))
            .sum::<f64>()
            / frame.len().max(1) as f64
    };
    candidates
        .iter()
        .enumerate()
        .min_by(|a, b| mse(a.1).total_cmp(&mse(b.1)))
        .map(|(i, _)| i)
}
