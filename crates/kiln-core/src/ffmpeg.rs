//! Audio, video, subtitles, animations and the image types the image
//! backend can't handle (JPEG XL, and writing AVIF), through an external
//! FFmpeg process. It also reads HEIC and AVIF for the image backend when
//! Windows can't.

use std::collections::{BTreeSet, HashMap};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};

use crate::converter::{CancelToken, Context, Converter, Options, Progress, Request};
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
/// (images and animations) at `cap`, keeping the aspect ratio. Video has
/// both sides rounded down to even, which most video encoders need (R60);
/// `-2` keeps an image's computed side even.
fn scale_filter(cap: u32, short_side: bool) -> String {
    let (landscape_w, landscape_h, portrait_w, portrait_h) = if short_side {
        (
            "trunc(iw*oh/ih/2)*2".to_owned(),
            format!("trunc(min({cap},ih)/2)*2"),
            format!("trunc(min({cap},iw)/2)*2"),
            "trunc(ih*ow/iw/2)*2".to_owned(),
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

/// The streams FFmpeg writes for a target of kind `to`: the file's first
/// moving picture and first sound track, and no subtitles or data. They are
/// named, not left to FFmpeg's choice of the best (the one flagged default,
/// else the larger picture and the sound with more channels), so that what
/// Kiln says of a file's tracks is true. `0:V:0?`, not `0:v:0?`: a cover
/// picture attached to the file is never taken as its video. A file need
/// have neither.
fn media_streams(to: Kind) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["-map_metadata", "0", "-sn", "-dn"]
        .map(OsString::from)
        .into();
    if to == Kind::Audio {
        args.extend(["-vn", "-map", "0:a:0?"].map(OsString::from));
    } else {
        args.extend(["-map", "0:V:0?", "-map", "0:a:0?"].map(OsString::from));
    }
    args
}

/// Audio to audio, video to video, video to audio, and animations to
/// video.
fn media(req: &Request<'_>, ctx: &mut Context<'_>) -> Result<()> {
    let mut args = media_streams(req.to.kind);
    if req.to.kind != Kind::Audio {
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

/// JPEG XL to PNG, and PNG to AVIF or JPEG XL, keeping
/// transparency.
fn still(req: &Request<'_>, ctx: &mut Context<'_>) -> Result<()> {
    let q = &req.options.quality;
    let cap = q.size.resolve(None);
    let scale = cap.map(|c| scale_filter(c, false));
    let mut args: Vec<OsString> = vec!["-map_metadata".into(), "-1".into()];
    match req.to.ext.as_str() {
        "png" => {
            args.extend(png_args(req.options, ctx, req.input, &req.from.ext, scale)?);
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

/// The arguments that write one still picture of `input`, an `from` file,
/// as a PNG, scaled by the filter `scale` if one is given. An AVIF with
/// transparency carries it as a second picture, which is merged back in.
/// Whether it has one is read from the file's own boxes: the tiles of a
/// grid are extra pictures too, and FFmpeg puts a grid together itself.
fn png_args(
    opts: &Options,
    ctx: &mut Context<'_>,
    input: &Path,
    from: &str,
    scale: Option<String>,
) -> Result<Vec<OsString>> {
    let mut args: Vec<OsString> = Vec::new();
    let alpha = from == "avif" && crate::heif::extras(input).alpha && {
        let pictures = probe_lines(opts, ctx, input, Some("v"), "stream=index")?;
        pictures.len() == 2
    };
    if alpha {
        let scale = scale.map(|s| format!(",{s}")).unwrap_or_default();
        args.push("-filter_complex".into());
        args.push(format!("[0:v:0][0:v:1]alphamerge{scale}[out]").into());
        args.extend(["-map", "[out]"].map(OsString::from));
    } else if let Some(s) = scale {
        args.extend(["-vf".into(), OsString::from(s)]);
    }
    args.extend(["-frames:v", "1", "-update", "1"].map(OsString::from));
    Ok(args)
}

/// The HEIC, AVIF or JPG at `input` (of type `from`) as a full-size PNG at
/// `output`, a new path in the job's folder, for the image backend to read
/// when Windows or Kiln's own reader can't. FFmpeg turns a HEIC or AVIF
/// upright, as it does video; a JPG is left as stored, and fails when
/// damaged. A missing FFmpeg is `Error::EngineMissing`.
pub(crate) fn decode_to_png(
    opts: &Options,
    cancel: &CancelToken,
    input: &Path,
    from: &str,
    output: &Path,
) -> Result<()> {
    let mut ctx = Context {
        progress: &mut |_| {},
        cancel,
    };
    let args = png_args(opts, &mut ctx, input, from, None)?;
    // A JPG is left as stored: its caller turns it as its Exif asks, as
    // Kiln's own reader does, whatever this FFmpeg would do with it. One
    // that is damaged fails, rather than having what can't be read filled
    // in and saved as if it were whole.
    let pre_input: Vec<OsString> = if from == "jpg" {
        ["-noautorotate", "-err_detect", "explode"]
            .map(OsString::from)
            .to_vec()
    } else {
        vec![]
    };
    let job = Encode {
        inputs: &[input],
        pre_input: &pre_input,
        args: &args,
        output,
        duration: None,
        span: (0.0, 1.0),
    };
    job.run(opts, &mut ctx).map(|_| ())
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

        // The stand-in encoders made real for this FFmpeg (R11).
        let args: Vec<OsString> = if self.args.iter().any(|a| a == H264 || a == HEVC) {
            let found = encoders(opts, ctx)?;
            let strings: Vec<String> = self
                .args
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            os(&expand_encoders(&strings, &found))
        } else {
            self.args.to_vec()
        };

        let mut cmd = base(opts);
        cmd.args(["-progress", "pipe:1"]);
        for input in self.inputs {
            cmd.args(self.pre_input)
                // Local files only: no network, even if a playlist asks.
                .args(["-protocol_whitelist", "file"])
                .arg("-i")
                .arg(file_url(input));
        }
        cmd.args(&args)
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
    let mut cmd = Command::new(ffmpeg_program(opts));
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

/// The FFmpeg program a job starts (R10).
fn ffmpeg_program(opts: &Options) -> PathBuf {
    program_in(opts.engines.ffmpeg.as_deref(), bundled_dir().as_deref())
}

/// FFprobe, from the same place as [`ffmpeg_program`].
fn ffprobe_program(opts: &Options) -> PathBuf {
    ffprobe_in(opts.engines.ffmpeg.as_deref(), bundled_dir().as_deref())
}

fn base(opts: &Options) -> Command {
    let mut cmd = Command::new(ffmpeg_program(opts));
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
    let mut cmd = Command::new(ffprobe_program(opts));
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
    // The sound and subtitle tracks are counted in the one run that finds
    // the sound.
    let streams = match probe_lines(opts, ctx, input, None, "stream=index,codec_type") {
        Ok(lines) => lines,
        Err(Error::Cancelled) => return Err(Error::Cancelled),
        Err(_) => vec![],
    };
    (info.sound_tracks, info.subtitle_tracks) = count_tracks(&streams);
    info.has_audio = info.sound_tracks > 0;
    Ok(info)
}

/// How many sound and subtitle tracks FFprobe's `stream=index,codec_type`
/// lines list, an index and a kind for each. A transport stream lists each
/// stream again under its program, so an index counts once.
fn count_tracks(lines: &[String]) -> (u16, u16) {
    let mut seen = std::collections::HashSet::new();
    let (mut sound, mut subtitles) = (0u16, 0u16);
    for pair in lines.chunks_exact(2) {
        if !seen.insert(pair[0].as_str()) {
            continue;
        }
        match pair[1].as_str() {
            "audio" => sound = sound.saturating_add(1),
            "subtitle" => subtitles = subtitles.saturating_add(1),
            _ => {}
        }
    }
    (sound, subtitles)
}

/// What FFprobe reports for one video stream of a still image.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StillStream {
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Such as "yuva420p"; empty when FFprobe gives none.
    pub pix_fmt: String,
}

/// Size and pixel format of every video stream, in one FFprobe run.
pub(crate) fn probe_still(
    opts: &Options,
    input: &Path,
    ctx: &mut Context<'_>,
) -> Result<Vec<StillStream>> {
    let mut cmd = Command::new(ffprobe_program(opts));
    cmd.args(["-v", "error", "-protocol_whitelist", "file"])
        .args(["-select_streams", "v"])
        .args(["-show_entries", "stream=width,height,pix_fmt"])
        .args(["-of", "compact=p=0"])
        .arg(file_url(input));
    let mut streams = vec![];
    process::run("FFprobe", cmd, ctx.cancel, |l| {
        if let Some(s) = still_stream(l) {
            streams.push(s);
        }
    })?;
    Ok(streams)
}

/// One line of `probe_still`'s output, such as
/// "width=64|height=48|pix_fmt=yuv420p".
fn still_stream(line: &str) -> Option<StillStream> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let mut s = StillStream::default();
    for pair in line.split('|') {
        match pair.split_once('=') {
            Some(("width", v)) => s.width = v.parse().ok(),
            Some(("height", v)) => s.height = v.parse().ok(),
            Some(("pix_fmt", v)) => s.pix_fmt = v.to_owned(),
            _ => {}
        }
    }
    Some(s)
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

// ---------------------------------------------------------------------------
// Which FFmpeg (R10)

/// The FFmpeg folder inside the program's folder.
const BUNDLED: [&str; 2] = ["ffmpeg", "bin"];

/// The folder of the FFmpeg that comes with Kiln, `ffmpeg\bin` beside the
/// program, when `ffmpeg.exe` is in it. Looked for once a session.
pub fn bundled_dir() -> Option<PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let exe = std::env::current_exe().ok()?;
        bundled_dir_in(exe.parent()?)
    })
    .clone()
}

/// [`bundled_dir`] for a program in `exe_dir`.
pub fn bundled_dir_in(exe_dir: &Path) -> Option<PathBuf> {
    let dir = BUNDLED.iter().fold(exe_dir.to_path_buf(), |d, p| d.join(p));
    dir.join(exe("ffmpeg")).is_file().then_some(dir)
}

/// `name` as a program's file name: `ffmpeg.exe` on Windows.
fn exe(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

/// The FFmpeg a job would use, if there is one: the path chosen in
/// Settings, else the copy in `bundled` (from [`bundled_dir`]), else the
/// first on `path_var`. A chosen path that is gone is not replaced by
/// another copy.
pub fn find(
    configured: Option<&Path>,
    bundled: Option<&Path>,
    path_var: &OsStr,
) -> Option<PathBuf> {
    match (configured, bundled) {
        (Some(p), _) => p.is_file().then(|| p.to_path_buf()),
        (None, Some(dir)) if dir.join(exe("ffmpeg")).is_file() => Some(dir.join(exe("ffmpeg"))),
        _ => process::find_on_path(path_var, "ffmpeg"),
    }
}

/// The program a job starts for FFmpeg: the chosen path, else the bundled
/// copy, else `ffmpeg` from PATH.
pub fn program_in(configured: Option<&Path>, bundled: Option<&Path>) -> PathBuf {
    match (configured, bundled) {
        (Some(p), _) => p.to_path_buf(),
        (None, Some(dir)) => dir.join(exe("ffmpeg")),
        (None, None) => PathBuf::from("ffmpeg"),
    }
}

/// FFprobe beside the FFmpeg [`program_in`] gives.
pub fn ffprobe_in(configured: Option<&Path>, bundled: Option<&Path>) -> PathBuf {
    match (configured, bundled) {
        (Some(p), _) => process::sibling(Some(p), "ffprobe"),
        (None, Some(dir)) => dir.join(exe("ffprobe")),
        (None, None) => PathBuf::from("ffprobe"),
    }
}

/// Whether the FFmpeg `opts` would use is there (R10).
pub(crate) fn installed(opts: &Options) -> bool {
    find(
        opts.engines.ffmpeg.as_deref(),
        bundled_dir().as_deref(),
        &process::current_path(),
    )
    .is_some()
}

// ---------------------------------------------------------------------------
// H.264 and HEVC without x264 or x265 (R11)

/// formats.toml's stand-in for the H.264 encoder.
pub(crate) const H264: &str = "@h264";
/// The stand-in for the HEVC encoder, which the video backend's fallback
/// asks for.
pub(crate) const HEVC: &str = "@hevc";

/// The encoders one FFmpeg has, by name, as `-encoders` lists them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Encoders(BTreeSet<String>);

impl Encoders {
    /// Reads FFmpeg's `-hide_banner -encoders` output: after the legend,
    /// each line is six capability letters (or dots) and the name.
    fn parse(text: &str) -> Encoders {
        let names = text
            .lines()
            .filter_map(|line| {
                let mut words = line.split_whitespace();
                let caps = words.next()?;
                let name = words.next()?;
                let is_caps = caps.len() == 6
                    && caps.starts_with(['V', 'A', 'S'])
                    && caps.chars().all(|c| c == '.' || c.is_ascii_uppercase());
                (is_caps && name != "=").then(|| name.to_owned())
            })
            .collect();
        Encoders(names)
    }

    fn has(&self, name: &str) -> bool {
        self.0.contains(name)
    }
}

/// Takes `flag` and its value out of `args`, returning the value.
fn take(args: &mut Vec<String>, flag: &str) -> Option<String> {
    let i = args.iter().position(|a| a == flag)?;
    let value = args.get(i + 1).cloned();
    args.drain(i..(i + 2).min(args.len()));
    value
}

/// `args` with `@h264` and `@hevc` made real for an FFmpeg with
/// `encoders` (R11):
/// - libx264 and libx265 where the FFmpeg has them (a person's own full
///   build), with formats.toml's CRF and preset;
/// - otherwise Windows' own encoders through FFmpeg, `h264_mf` and
///   `hevc_mf`, with the CRF as their quality ([`quality::mf_quality`];
///   HEVC's CRF is libx265's, 4 above libx264's) and no preset. A baseline
///   profile is given by number, the only way they take one. `hevc_mf`
///   asks for the graphics card's encoder, as Windows' HEVC encoder fails
///   through FFmpeg without it.
fn expand_encoders(args: &[String], encoders: &Encoders) -> Vec<String> {
    let mut out = args.to_vec();
    let Some(at) = out.windows(2).position(|w| w[0] == "-c:v") else {
        return out;
    };
    let hevc = match out[at + 1].as_str() {
        H264 => false,
        HEVC => true,
        _ => return out,
    };
    let x = if hevc { "libx265" } else { "libx264" };
    if encoders.has(x) {
        out[at + 1] = x.to_owned();
        return out;
    }
    let crf = take(&mut out, "-crf").and_then(|v| v.parse::<u32>().ok());
    take(&mut out, "-preset");
    let profile = take(&mut out, "-profile:v");
    let mut own: Vec<String> = Vec::new();
    if hevc {
        own.extend(["-hw_encoding", "1"].map(String::from));
    } else if profile.as_deref() == Some("baseline") {
        own.extend(["-profile:v", "66"].map(String::from));
    }
    if let Some(crf) = crf {
        let crf = if hevc { crf.saturating_sub(4) } else { crf };
        own.extend(["-rate_control", "quality", "-quality"].map(String::from));
        own.push(quality::mf_quality(crf).to_string());
    }
    let at = out
        .iter()
        .position(|a| a == H264 || a == HEVC)
        .unwrap_or(at + 1);
    out[at] = if hevc { "hevc_mf" } else { "h264_mf" }.to_owned();
    out.splice(at + 1..at + 1, own);
    out
}

/// Each FFmpeg's encoders, by the program's path, asked once a session.
fn encoder_lists() -> &'static Mutex<HashMap<PathBuf, Arc<Encoders>>> {
    static LISTS: OnceLock<Mutex<HashMap<PathBuf, Arc<Encoders>>>> = OnceLock::new();
    LISTS.get_or_init(Default::default)
}

/// The encoders of the FFmpeg `opts` names, asked once per FFmpeg per
/// session, on the job's own thread. An FFmpeg that can't be asked has an
/// empty list (and is asked again next time): the job then fails on
/// FFmpeg's own error, or as missing.
fn encoders(opts: &Options, ctx: &mut Context<'_>) -> Result<Arc<Encoders>> {
    let program = ffmpeg_program(opts);
    let key = if program.parent().is_some_and(|d| !d.as_os_str().is_empty()) {
        program.clone()
    } else {
        process::find_on_path(&process::current_path(), "ffmpeg").unwrap_or(program.clone())
    };
    if let Some(found) = encoder_lists()
        .lock()
        .ok()
        .and_then(|m| m.get(&key).cloned())
    {
        return Ok(found);
    }
    let mut cmd = Command::new(&program);
    cmd.args(["-hide_banner", "-nostdin", "-encoders"]);
    let mut text = String::new();
    let listed = process::run(ENGINE, cmd, ctx.cancel, |l| {
        text.push_str(l);
        text.push('\n');
    });
    match listed {
        Ok(_) => {
            let found = Arc::new(Encoders::parse(&text));
            if let Ok(mut m) = encoder_lists().lock() {
                m.insert(key, found.clone());
            }
            Ok(found)
        }
        Err(Error::Cancelled) => Err(Error::Cancelled),
        Err(_) => Ok(Arc::default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    fn arg<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
        args.windows(2)
            .find(|w| w[0] == flag)
            .map(|w| w[1].as_str())
    }

    /// What `-hide_banner -encoders` prints, cut down.
    const LISTED: &str = " Encoders:
 V..... = Video
 A..... = Audio
 ------
 V....D libx264              libx264 H.264 / AVC / MPEG-4 AVC / MPEG-4 part 10 (codec h264)
 V....D h264_mf              H264 via MediaFoundation (codec h264)
 V....D libx265              libx265 H.265 / HEVC (codec hevc)
 V....D hevc_mf              HEVC via MediaFoundation (codec hevc)
 A....D aac                  AAC (Advanced Audio Coding)
";

    fn lgpl() -> Encoders {
        Encoders::parse(&LISTED.replace("libx26", "nothere"))
    }

    #[test]
    fn encoders_are_read_from_ffmpegs_list() {
        let e = Encoders::parse(LISTED);
        for name in ["libx264", "h264_mf", "libx265", "hevc_mf", "aac"] {
            assert!(e.has(name), "{name}");
        }
        // The legend and the rule are not encoders.
        assert!(!e.has("="));
        assert!(!e.has("Video"));
        assert!(!e.has("------"));
        assert!(!lgpl().has("libx264"));
        assert_eq!(Encoders::parse(""), Encoders::default());
    }

    /// formats.toml's MP4 arguments, tuned for `preset`.
    fn mp4(preset: Preset) -> Vec<String> {
        let reg = crate::registry::Registry::builtin();
        let mp4 = reg.format("mp4").unwrap();
        let q = quality::Quality {
            preset,
            ..quality::Quality::default()
        };
        quality::tune(&mp4.ffmpeg, &q)
    }

    #[test]
    fn h264_is_libx264_where_ffmpeg_has_it() {
        let full = Encoders::parse(LISTED);
        let tuned = mp4(Preset::High);
        let out = expand_encoders(&tuned, &full);
        assert_eq!(arg(&out, "-c:v"), Some("libx264"));
        assert_eq!(arg(&out, "-crf"), Some("20"));
        assert_eq!(arg(&out, "-preset"), Some("medium"));
        // Only the encoder's name changes.
        let back: Vec<String> = out
            .iter()
            .map(|a| {
                if a == "libx264" {
                    H264.to_owned()
                } else {
                    a.clone()
                }
            })
            .collect();
        assert_eq!(back, tuned);
    }

    #[test]
    fn h264_is_windows_encoder_through_ffmpeg_otherwise() {
        for (preset, q) in [
            (Preset::Small, "41"),
            (Preset::High, "60"),
            (Preset::Best, "70"),
        ] {
            let out = expand_encoders(&mp4(preset), &lgpl());
            assert_eq!(arg(&out, "-c:v"), Some("h264_mf"), "{preset:?}");
            let at = out.iter().position(|a| a == "h264_mf").unwrap();
            assert_eq!(
                out[at + 1..at + 5],
                strings(&["-rate_control", "quality", "-quality", q]),
                "{preset:?}"
            );
            // libx264's own options go.
            assert_eq!(arg(&out, "-crf"), None, "{preset:?}");
            assert_eq!(arg(&out, "-preset"), None, "{preset:?}");
            // The rest stays.
            assert_eq!(arg(&out, "-pix_fmt"), Some("yuv420p"));
            assert_eq!(arg(&out, "-c:a"), Some("aac"));
            assert_eq!(arg(&out, "-movflags"), Some("+faststart"));
        }
        // 3GP's baseline profile by number, the only way h264_mf takes it.
        let reg = crate::registry::Registry::builtin();
        let three = expand_encoders(&reg.format("3gp").unwrap().ffmpeg, &lgpl());
        assert_eq!(arg(&three, "-profile:v"), Some("66"));
        assert_eq!(arg(&three, "-quality"), Some("50"));
        // A bitrate asked for (Fit under) is h264_mf's own rate control.
        let kbps = strings(&[
            "-c:v", H264, "-pix_fmt", "yuv420p", "-preset", "medium", "-b:v", "500k",
        ]);
        assert_eq!(
            expand_encoders(&kbps, &lgpl()),
            strings(&["-c:v", "h264_mf", "-pix_fmt", "yuv420p", "-b:v", "500k"])
        );
        // A copied picture has nothing to expand.
        let copy = strings(&["-c:v", "copy", "-c:a", "aac"]);
        assert_eq!(expand_encoders(&copy, &lgpl()), copy);
    }

    #[test]
    fn hevc_is_libx265_or_windows_encoder() {
        // The fallback's CRF for HEVC is libx265's, 4 above libx264's.
        let args = strings(&[
            "-c:v", HEVC, "-pix_fmt", "yuv420p", "-crf", "24", "-preset", "medium", "-tag:v",
            "hvc1",
        ]);
        let full = expand_encoders(&args, &Encoders::parse(LISTED));
        assert_eq!(arg(&full, "-c:v"), Some("libx265"));
        assert_eq!(arg(&full, "-crf"), Some("24"));
        let mf = expand_encoders(&args, &lgpl());
        assert_eq!(
            mf,
            strings(&[
                "-c:v",
                "hevc_mf",
                "-hw_encoding",
                "1",
                "-rate_control",
                "quality",
                "-quality",
                "60",
                "-pix_fmt",
                "yuv420p",
                "-tag:v",
                "hvc1",
            ])
        );
    }

    #[test]
    fn no_format_names_x264_or_x265() {
        // The FFmpeg Kiln ships has neither (GPL); formats.toml names the
        // stand-in, made real per FFmpeg.
        let text = include_str!("../formats.toml");
        assert!(!text.contains("libx264"), "formats.toml names libx264");
        assert!(!text.contains("libx265"), "formats.toml names libx265");
        let reg = crate::registry::Registry::builtin();
        let h264: BTreeSet<&str> = reg
            .extensions()
            .filter_map(|e| reg.format(e))
            .filter(|f| arg(&f.ffmpeg, "-c:v") == Some(H264))
            .map(|f| f.ext.as_str())
            .collect();
        assert_eq!(
            h264.into_iter().collect::<Vec<_>>(),
            ["3gp", "flv", "m4v", "mkv", "mov", "mp4", "ts"]
        );
    }

    #[test]
    fn the_windows_h264_arguments_run() {
        // With the FFmpeg on PATH, if it has h264_mf: the arguments Kiln
        // gives it make an H.264 file.
        let path = crate::process::current_path();
        let Some(ffmpeg) = crate::process::find_on_path(&path, "ffmpeg") else {
            eprintln!("the_windows_h264_arguments_run: skipped, no FFmpeg on PATH");
            return;
        };
        let listed = Command::new(&ffmpeg)
            .args(["-hide_banner", "-encoders"])
            .output()
            .unwrap();
        let encoders = Encoders::parse(&String::from_utf8_lossy(&listed.stdout));
        if !encoders.has("h264_mf") {
            eprintln!("the_windows_h264_arguments_run: skipped, this FFmpeg has no h264_mf");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let reg = crate::registry::Registry::builtin();
        for ext in ["mp4", "3gp"] {
            let args = expand_encoders(&reg.format(ext).unwrap().ffmpeg, &lgpl());
            let out = dir.path().join(format!("out.{ext}"));
            let made = Command::new(&ffmpeg)
                .args(["-v", "error", "-nostdin", "-f", "lavfi", "-i"])
                .arg("testsrc=size=160x120:rate=10")
                .args(["-f", "lavfi", "-i", "sine=frequency=440"])
                .args(["-t", "1"])
                .args(&args)
                .arg(&out)
                .output()
                .unwrap();
            assert!(
                made.status.success(),
                "{ext}: {}",
                String::from_utf8_lossy(&made.stderr)
            );
            assert!(std::fs::metadata(&out).unwrap().len() > 0, "{ext}");
        }
    }

    #[test]
    fn still_streams_are_read_by_key() {
        assert_eq!(
            still_stream("width=64|height=48|pix_fmt=yuva420p"),
            Some(StillStream {
                width: Some(64),
                height: Some(48),
                pix_fmt: "yuva420p".to_owned(),
            })
        );
        assert_eq!(
            still_stream("pix_fmt=gray|width=N/A"),
            Some(StillStream {
                width: None,
                height: None,
                pix_fmt: "gray".to_owned(),
            })
        );
        assert_eq!(still_stream("  "), None);
    }

    #[test]
    fn mpeg2_gets_an_allowed_frame_rate() {
        assert_eq!(mpeg2_rate(25.0), None);
        assert_eq!(mpeg2_rate(29.97003), None);
        assert_eq!(mpeg2_rate(10.0).as_deref(), Some("fps=24000/1001"));
        assert_eq!(mpeg2_rate(26.0).as_deref(), Some("fps=30000/1001"));
        assert_eq!(mpeg2_rate(120.0).as_deref(), Some("fps=60/1"));
    }

    #[test]
    fn video_scaling_rounds_both_sides_down_to_even() {
        // 61 lines asked of 160x120: 60 high, and 80 wide from that.
        let f = scale_filter(61, true);
        assert!(f.contains("trunc(min(61,ih)/2)*2"), "{f}");
        assert!(f.contains("trunc(iw*oh/ih/2)*2"), "{f}");
    }

    #[test]
    fn scaling_caps_the_right_side() {
        assert_eq!(
            scale_filter(720, true),
            "scale=w='if(gte(iw,ih),trunc(iw*oh/ih/2)*2,trunc(min(720,iw)/2)*2)':\
             h='if(gte(iw,ih),trunc(min(720,ih)/2)*2,trunc(ih*ow/iw/2)*2)':flags=lanczos"
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

    #[test]
    fn tracks_are_counted_once_each() {
        let lines = |s: &str| s.split_whitespace().map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(
            count_tracks(&lines("0 video 1 audio 2 audio 3 subtitle")),
            (2, 1)
        );
        assert_eq!(count_tracks(&lines("0 video")), (0, 0));
        assert_eq!(count_tracks(&[]), (0, 0));
        // A transport stream's streams come twice.
        assert_eq!(
            count_tracks(&lines("0 video 1 audio 0 video 1 audio")),
            (1, 0)
        );
        // A line left over from a cut-short answer is no stream.
        assert_eq!(count_tracks(&lines("0 video 1")), (0, 0));
    }

    #[test]
    fn media_streams_are_the_first_picture_and_sound() {
        let has = |args: &[OsString], want: &[&str]| {
            args.windows(want.len())
                .any(|w| w.iter().zip(want).all(|(a, b)| a == b))
        };
        // Video takes the first moving picture and the first sound,
        // whichever FFmpeg would have found best, and neither need be
        // there. `V`, not `v`: a cover picture attached to the file is never
        // taken as the video.
        let video = media_streams(Kind::Video);
        assert!(
            has(&video, &["-map", "0:V:0?", "-map", "0:a:0?"]),
            "{video:?}"
        );
        assert!(!has(&video, &["0:v:0?"]), "{video:?}");
        assert!(has(&video, &["-sn", "-dn"]), "{video:?}");
        assert!(!has(&video, &["-vn"]), "{video:?}");
        // Sound takes the first sound and no picture.
        let audio = media_streams(Kind::Audio);
        assert!(has(&audio, &["-map", "0:a:0?"]), "{audio:?}");
        assert!(has(&audio, &["-vn"]), "{audio:?}");
        assert!(!has(&audio, &["0:V:0?"]), "{audio:?}");
    }
}
