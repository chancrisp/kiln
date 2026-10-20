//! Audio, video, subtitles, animations and the image types the image
//! backend can't handle (AVIF, JPEG XL, HEIC), through an external FFmpeg
//! process.

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use crate::converter::{Context, Converter, Options, Progress, Request};
use crate::error::{Error, Result};
use crate::process;
use crate::quality::{self, Preset};
use crate::raster;
use crate::registry::Kind;

pub struct FfmpegConverter;

const ENGINE: &str = "FFmpeg";

/// FFmpeg treats "name:" prefixes as protocols. Saying "file:" explicitly
/// means a file called "http:x.mp4" or "concat:a|b" is just a file.
pub(crate) fn file_url(path: &Path) -> OsString {
    let mut s = OsString::from("file:");
    s.push(path.as_os_str());
    s
}

impl Converter for FfmpegConverter {
    fn id(&self) -> &'static str {
        "ffmpeg"
    }

    fn convert(&self, req: &Request<'_>, ctx: &mut Context<'_>) -> Result<()> {
        match (req.from.kind, req.to.kind) {
            (_, Kind::Subtitle) => subtitles(req, ctx),
            (Kind::Image, Kind::Image) => still(req, ctx),
            (Kind::Video, Kind::Image) => animation(req, ctx),
            _ => media(req, ctx),
        }
    }
}

fn os(args: &[String]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

/// Run FFmpeg once on the request's input with `args` before the output.
fn encode_one(req: &Request<'_>, ctx: &mut Context<'_>, args: &[OsString]) -> Result<()> {
    let duration = probe_duration(req.options, req.input, ctx)?;
    let job = Encode {
        inputs: &[req.input],
        pre_input: &[],
        args,
        output: req.output,
        duration,
        span: (0.0, 1.0),
    };
    job.run(req.options, ctx).map(|_| ())
}

/// Frame rates MPEG-2 allows, as (numerator, denominator).
const MPEG2_RATES: [(u32, u32); 8] = [
    (24000, 1001),
    (24, 1),
    (25, 1),
    (30000, 1001),
    (30, 1),
    (50, 1),
    (60000, 1001),
    (60, 1),
];

/// The `fps` filter value for MPEG-2: the lowest allowed rate at or above
/// `fps`, so no motion is dropped.
fn mpeg2_rate(fps: f64) -> Option<String> {
    let ok = |(n, d): (u32, u32)| (n as f64 / d as f64 - fps).abs() < 0.01;
    if MPEG2_RATES.into_iter().any(ok) {
        return None;
    }
    let (n, d) = MPEG2_RATES
        .into_iter()
        .find(|&(n, d)| n as f64 / d as f64 >= fps)
        .unwrap_or((60, 1));
    Some(format!("fps={n}/{d}"))
}

/// A scale filter that caps the short side (video) or the long side
/// (images and animations) at `cap`, keeping the aspect ratio. `-2`
/// keeps the computed side even, which most video encoders need.
fn scale_filter(cap: u32, short_side: bool) -> String {
    let (landscape_w, landscape_h, portrait_w, portrait_h) = if short_side {
        (
            "-2".to_owned(),
            format!("min({cap},ih)"),
            format!("min({cap},iw)"),
            "-2".to_owned(),
        )
    } else {
        (
            format!("min({cap},iw)"),
            "-2".to_owned(),
            "-2".to_owned(),
            format!("min({cap},ih)"),
        )
    };
    format!(
        "scale=w='if(gte(iw,ih),{landscape_w},{portrait_w})':h='if(gte(iw,ih),{landscape_h},{portrait_h})':flags=lanczos"
    )
}

/// The picture filters a video or animation target needs: frame rate,
/// size, and even dimensions for encoders that require them.
fn picture_filters(
    req: &Request<'_>,
    info: &quality::MediaInfo,
    short_side: bool,
    even: bool,
) -> Vec<String> {
    let q = &req.options.quality;
    let mut filters = Vec::new();
    let src_fps = info.fps.unwrap_or(0.0);
    let cap_fps = q.fps.resolve(quality::auto_fps(req.to));
    let mut fps = src_fps;
    if let Some(cap) = cap_fps {
        if src_fps == 0.0 || src_fps > cap as f64 + 0.01 {
            filters.push(format!("fps={cap}"));
            fps = cap as f64;
        }
    }
    if req.to.ext == "mpg" {
        if let Some(rate) = mpeg2_rate(if fps > 0.0 { fps } else { 25.0 }) {
            filters.retain(|f| !f.starts_with("fps="));
            filters.push(rate);
        }
    }
    let cap = q.size.resolve(quality::auto_size(req.to));
    let (w, h) = (info.width.unwrap_or(0), info.height.unwrap_or(0));
    let side = if short_side { w.min(h) } else { w.max(h) };
    match cap {
        Some(cap) if side > cap || side == 0 => filters.push(scale_filter(cap, short_side)),
        _ if even && (w % 2 == 1 || h % 2 == 1) => {
            filters.push("scale=trunc(iw/2)*2:trunc(ih/2)*2".to_owned())
        }
        _ => {}
    }
    filters
}

/// Audio to audio, video to video, video to audio, and animations to
/// video.
fn media(req: &Request<'_>, ctx: &mut Context<'_>) -> Result<()> {
    let mut args: Vec<OsString> = vec!["-map_metadata".into(), "0".into()];
    args.extend(["-sn", "-dn"].map(OsString::from));
    if req.to.kind == Kind::Audio {
        args.push("-vn".into());
    } else {
        let info = probe_media(req.options, req.input, ctx)?;
        let filters = picture_filters(req, &info, true, true);
        if !filters.is_empty() {
            args.push("-vf".into());
            args.push(filters.join(",").into());
        }
    }
    args.extend(os(&quality::tune(&req.to.ffmpeg, &req.options.quality)));
    encode_one(req, ctx, &args)
}

/// Video to an animated GIF, WebP or PNG, silent and looping.
fn animation(req: &Request<'_>, ctx: &mut Context<'_>) -> Result<()> {
    let info = probe_media(req.options, req.input, ctx)?;
    let mut filters = picture_filters(req, &info, false, false);
    let mut args: Vec<OsString> = vec!["-an".into(), "-sn".into(), "-dn".into()];
    if req.to.ext == "gif" {
        // One palette made for the whole clip, then each frame mapped to
        // it: far cleaner than FFmpeg's default web palette.
        let (colors, stats, dither) = match req.options.quality.preset {
            Preset::Small => (128, "diff", "bayer:bayer_scale=3"),
            Preset::High => (256, "diff", "sierra2_4a"),
            Preset::Best => (256, "full", "floyd_steinberg"),
        };
        filters.push(format!(
            "split[a][b];[a]palettegen=max_colors={colors}:stats_mode={stats}[p];[b][p]paletteuse=dither={dither}:diff_mode=rectangle"
        ));
        args.extend(["-loop", "0"].map(OsString::from));
    } else {
        args.extend(os(&quality::tune(&req.to.ffmpeg, &req.options.quality)));
    }
    let mut all: Vec<OsString> = Vec::new();
    if !filters.is_empty() {
        all.push("-vf".into());
        all.push(filters.join(",").into());
    }
    all.extend(args);
    encode_one(req, ctx, &all)
}

/// AVIF, JPEG XL and HEIC to PNG, and PNG to AVIF or JPEG XL, keeping
/// transparency.
fn still(req: &Request<'_>, ctx: &mut Context<'_>) -> Result<()> {
    let q = &req.options.quality;
    let cap = q.size.resolve(None);
    let scale = cap.map(|c| scale_filter(c, false));
    let mut args: Vec<OsString> = vec!["-map_metadata".into(), "-1".into()];
    match req.to.ext.as_str() {
        "png" => {
            // An AVIF with transparency carries it as a second picture.
            let pictures = probe_lines(req.options, ctx, req.input, Some("v"), "stream=index")?;
            if req.from.ext == "avif" && pictures.len() >= 2 {
                let scale = scale.map(|s| format!(",{s}")).unwrap_or_default();
                args.push("-filter_complex".into());
                args.push(format!("[0:v:0][0:v:1]alphamerge{scale}[out]").into());
                args.extend(["-map", "[out]"].map(OsString::from));
            } else if let Some(s) = scale {
                args.extend(["-vf".into(), OsString::from(s)]);
            }
            args.extend(["-frames:v", "1", "-update", "1"].map(OsString::from));
        }
        "avif" | "jxl" => {
            let transparent = raster::decode(req.input, &req.from.ext)
                .map(|img| has_transparency(&img))
                .unwrap_or(false);
            let pre = scale.map(|s| format!("{s},")).unwrap_or_default();
            if req.to.ext == "avif" && transparent {
                // libaom takes colour and alpha as two pictures; the
                // colour must be full-resolution 4:4:4 for both to match.
                args.push("-filter_complex".into());
                args.push(format!(
                    "[0:v]{pre}format=yuva444p,split[c][a];[c]format=yuv444p[c2];[a]alphaextract,format=gray[al]"
                ).into());
                args.extend(["-map", "[c2]", "-map", "[al]"].map(OsString::from));
            } else {
                if !pre.is_empty() {
                    args.extend(["-vf".into(), OsString::from(pre.trim_end_matches(','))]);
                }
                if req.to.ext == "avif" {
                    args.extend(["-pix_fmt", "yuv444p"].map(OsString::from));
                }
            }
            args.extend(["-frames:v", "1"].map(OsString::from));
            args.extend(os(&quality::tune(&req.to.ffmpeg, q)));
        }
        _ => {
            return Err(Error::Registry(format!(
                "FFmpeg can't convert {} to {}.",
                req.from.code(),
                req.to.code()
            )))
        }
    }
    encode_one(req, ctx, &args)
}

fn has_transparency(img: &image::DynamicImage) -> bool {
    img.color().has_alpha() && img.to_rgba8().pixels().any(|p| p.0[3] != 255)
}

/// Subtitles between SRT, WebVTT and ASS, or the first subtitle track of a
/// video.
fn subtitles(req: &Request<'_>, ctx: &mut Context<'_>) -> Result<()> {
    let mut args: Vec<OsString> = Vec::new();
    if req.from.kind == Kind::Video {
        let tracks = probe_lines(req.options, ctx, req.input, Some("s"), "stream=codec_name")?;
        let Some(first) = tracks.first() else {
            return Err(Error::NothingToDo(
                "The video has no subtitle track.".to_owned(),
            ));
        };
        if matches!(
            first.as_str(),
            "hdmv_pgs_subtitle" | "dvd_subtitle" | "dvb_subtitle" | "xsub"
        ) {
            return Err(Error::NothingToDo(format!(
                "The video's subtitles are pictures, not text, so they can't become {}.",
                req.to.code()
            )));
        }
        args.extend(["-map", "0:s:0"].map(OsString::from));
    }
    args.extend(os(&req.to.ffmpeg));
    encode_one(req, ctx, &args)
}

/// One FFmpeg run that writes a file.
pub(crate) struct Encode<'a> {
    pub inputs: &'a [&'a Path],
    /// Arguments placed before every `-i`, such as `-ss`.
    pub pre_input: &'a [OsString],
    /// Arguments between the inputs and the output.
    pub args: &'a [OsString],
    pub output: &'a Path,
    /// Total seconds of media being written, for progress.
    pub duration: Option<f64>,
    /// The part of the job's progress this run covers.
    pub span: (f32, f32),
}

impl Encode<'_> {
    pub fn run(&self, opts: &Options, ctx: &mut Context<'_>) -> Result<String> {
        let (lo, hi) = self.span;
        let total_us = self.duration.map(|s| s * 1_000_000.0);
        (ctx.progress)(match total_us {
            Some(_) => Progress::Fraction(lo),
            None => Progress::Indeterminate,
        });

        let mut cmd = base(opts);
        cmd.args(["-progress", "pipe:1"]);
        for input in self.inputs {
            cmd.args(self.pre_input)
                // Local files only: no network, even if a playlist asks.
                .args(["-protocol_whitelist", "file"])
                .arg("-i")
                .arg(file_url(input));
        }
        cmd.args(self.args)
            // Never overwrite. Outputs are fresh paths, so this only
            // guards against mistakes.
            .arg("-n")
            .arg(file_url(self.output));

        let progress = &mut *ctx.progress;
        process::run(ENGINE, cmd, ctx.cancel, |line| {
            let Some(total) = total_us else { return };
            if let Some(v) = line.strip_prefix("out_time_us=") {
                if let Ok(done) = v.trim().parse::<f64>() {
                    let f = (done / total).clamp(0.0, 1.0) as f32;
                    progress(Progress::Fraction(lo + (hi - lo) * f));
                }
            }
        })
    }
}

/// An analysis pass that reads `input` through `filter` and writes
/// nothing. Returns FFmpeg's log, where filters such as cropdetect and
/// silencedetect report what they found.
pub(crate) fn analyse(
    opts: &Options,
    ctx: &mut Context<'_>,
    input: &Path,
    filter_flag: &str,
    filter: &str,
    max_seconds: Option<f64>,
) -> Result<String> {
    analyse_with(opts, ctx, input, &[], filter_flag, filter, max_seconds)
}

/// `analyse` with `input_args` before the input, such as `-threads 1`.
pub(crate) fn analyse_with(
    opts: &Options,
    ctx: &mut Context<'_>,
    input: &Path,
    input_args: &[&str],
    filter_flag: &str,
    filter: &str,
    max_seconds: Option<f64>,
) -> Result<String> {
    let mut cmd = Command::new(process::program(opts.engines.ffmpeg.as_deref(), "ffmpeg"));
    cmd.args(["-hide_banner", "-nostdin", "-nostats", "-loglevel", "info"])
        .args(input_args)
        .args(["-protocol_whitelist", "file", "-i"])
        .arg(file_url(input));
    if let Some(t) = max_seconds {
        cmd.arg("-t").arg(format!("{t}"));
    }
    cmd.args([filter_flag, filter, "-f", "null", "-"]);
    process::run(ENGINE, cmd, ctx.cancel, |_| {})
}

fn base(opts: &Options) -> Command {
    let mut cmd = Command::new(process::program(opts.engines.ffmpeg.as_deref(), "ffmpeg"));
    cmd.args(["-hide_banner", "-nostdin", "-nostats", "-loglevel", "error"]);
    cmd
}

fn probe(
    opts: &Options,
    ctx: &mut Context<'_>,
    input: &Path,
    entries: &str,
) -> Result<Vec<String>> {
    probe_lines(opts, ctx, input, None, entries)
}

/// ffprobe's values for `entries`, one per line, optionally for one kind
/// of stream ("v", "a" or "s").
fn probe_lines(
    opts: &Options,
    ctx: &mut Context<'_>,
    input: &Path,
    select: Option<&str>,
    entries: &str,
) -> Result<Vec<String>> {
    let mut cmd = Command::new(process::sibling(opts.engines.ffmpeg.as_deref(), "ffprobe"));
    cmd.args(["-v", "error", "-protocol_whitelist", "file"]);
    if let Some(s) = select {
        cmd.args(["-select_streams", s]);
    }
    cmd.args(["-show_entries", entries])
        .args(["-of", "default=noprint_wrappers=1:nokey=1"])
        .arg(file_url(input));
    let mut lines = vec![];
    process::run("FFprobe", cmd, ctx.cancel, |l| {
        lines.push(l.trim().to_owned())
    })?;
    Ok(lines)
}

/// Media duration in seconds via ffprobe, if it can be found and the file
/// has one. Only cancellation is an error here: without a duration a job
/// still runs, it just reports indeterminate progress.
pub(crate) fn probe_duration(
    opts: &Options,
    input: &Path,
    ctx: &mut Context<'_>,
) -> Result<Option<f64>> {
    match probe(opts, ctx, input, "format=duration") {
        Ok(lines) => Ok(lines
            .iter()
            .filter_map(|l| l.parse::<f64>().ok())
            .find(|s| s.is_finite() && *s > 0.0)),
        Err(Error::Cancelled) => Err(Error::Cancelled),
        Err(_) => Ok(None),
    }
}

/// Size, frame rate, duration and audio of a media file. Missing values
/// stay `None`; only cancellation is an error.
pub(crate) fn probe_media(
    opts: &Options,
    input: &Path,
    ctx: &mut Context<'_>,
) -> Result<quality::MediaInfo> {
    let mut info = quality::MediaInfo {
        duration: probe_duration(opts, input, ctx)?,
        ..Default::default()
    };
    let video = match probe_lines(
        opts,
        ctx,
        input,
        Some("v:0"),
        "stream=width,height,avg_frame_rate,r_frame_rate",
    ) {
        Ok(lines) => lines,
        Err(Error::Cancelled) => return Err(Error::Cancelled),
        Err(_) => vec![],
    };
    let rate = |s: &str| -> Option<f64> {
        let (n, d) = s.split_once('/')?;
        let (n, d): (f64, f64) = (n.parse().ok()?, d.parse().ok()?);
        (d > 0.0 && n > 0.0).then_some(n / d)
    };
    if let [w, h, avg, r, ..] = video.as_slice() {
        info.width = w.parse().ok();
        info.height = h.parse().ok();
        info.fps = rate(avg).or_else(|| rate(r)).filter(|f| *f < 1000.0);
    }
    info.has_audio = match probe_lines(opts, ctx, input, Some("a"), "stream=index") {
        Ok(lines) => !lines.is_empty(),
        Err(Error::Cancelled) => return Err(Error::Cancelled),
        Err(_) => false,
    };
    Ok(info)
}

/// Whether the file has an audio stream.
pub(crate) fn has_audio(opts: &Options, input: &Path, ctx: &mut Context<'_>) -> Result<bool> {
    let lines = probe(opts, ctx, input, "stream=codec_type")?;
    Ok(lines.iter().any(|l| l == "audio"))
}

/// Width and height of the first video stream.
pub(crate) fn probe_size(
    opts: &Options,
    input: &Path,
    ctx: &mut Context<'_>,
) -> Result<Option<(u32, u32)>> {
    let lines = probe(opts, ctx, input, "stream=width,height")?;
    let nums: Vec<u32> = lines.iter().filter_map(|l| l.parse().ok()).collect();
    Ok(match nums.as_slice() {
        [w, h, ..] => Some((*w, *h)),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mpeg2_gets_an_allowed_frame_rate() {
        assert_eq!(mpeg2_rate(25.0), None);
        assert_eq!(mpeg2_rate(29.97003), None);
        assert_eq!(mpeg2_rate(10.0).as_deref(), Some("fps=24000/1001"));
        assert_eq!(mpeg2_rate(26.0).as_deref(), Some("fps=30000/1001"));
        assert_eq!(mpeg2_rate(120.0).as_deref(), Some("fps=60/1"));
    }

    #[test]
    fn scaling_caps_the_right_side() {
        assert_eq!(
            scale_filter(720, true),
            "scale=w='if(gte(iw,ih),-2,min(720,iw))':h='if(gte(iw,ih),min(720,ih),-2)':flags=lanczos"
        );
        assert_eq!(
            scale_filter(480, false),
            "scale=w='if(gte(iw,ih),min(480,iw),-2)':h='if(gte(iw,ih),-2,min(480,ih))':flags=lanczos"
        );
    }

    #[test]
    fn file_urls_are_explicit() {
        assert_eq!(
            file_url(Path::new("/a/b.mp4")),
            OsString::from("file:/a/b.mp4")
        );
        assert_eq!(
            file_url(Path::new("/a/http:evil.mp4")),
            OsString::from("file:/a/http:evil.mp4")
        );
    }
}
