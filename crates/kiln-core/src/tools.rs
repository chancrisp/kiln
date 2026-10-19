//! File tools: compress, crop, trim, split and merge. Each one writes new
//! files next to its inputs and never touches the originals.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use image::{DynamicImage, GenericImageView, Rgba};

use crate::converter::{CancelToken, Context, Options, Progress};
use crate::error::{Error, Result};
use crate::ffmpeg::{self, Encode};
use crate::job::{checked_input, ensure_written, out_dir_for, place, stem_of, Kiln, WorkDir};
use crate::registry::{Format, Kind};
use crate::{pdf, process, raster};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tool {
    Compress,
    Crop,
    Trim,
    Split,
    Merge,
}

impl Tool {
    pub const ALL: [Tool; 5] = [
        Tool::Compress,
        Tool::Crop,
        Tool::Trim,
        Tool::Split,
        Tool::Merge,
    ];

    /// The verb shown in menus.
    pub fn label(self) -> &'static str {
        match self {
            Tool::Compress => "Compress",
            Tool::Crop => "Crop",
            Tool::Trim => "Trim",
            Tool::Split => "Split",
            Tool::Merge => "Merge",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Tool::Compress => "compress",
            Tool::Crop => "crop",
            Tool::Trim => "trim",
            Tool::Split => "split",
            Tool::Merge => "merge",
        }
    }

    pub fn parse(s: &str) -> Option<Tool> {
        Tool::ALL
            .into_iter()
            .find(|t| t.id().eq_ignore_ascii_case(s))
    }

    /// The word added to output names: "photo compressed.jpg".
    fn suffix(self) -> &'static str {
        match self {
            Tool::Compress => "compressed",
            Tool::Crop => "cropped",
            Tool::Trim => "trimmed",
            Tool::Split => "part",
            Tool::Merge => "merged",
        }
    }

    /// Whether this tool can work on one file of this format.
    fn handles(self, f: &Format) -> bool {
        match (self, f.kind) {
            // Only these two have a lossy or tighter encoding to re-save with.
            (Tool::Compress, Kind::Image) => matches!(f.ext.as_str(), "png" | "jpg"),
            (Tool::Compress, Kind::Video) => true,
            (Tool::Compress, Kind::Audio) => f.ext != "wav",
            (Tool::Crop, Kind::Image | Kind::Video) => true,
            (Tool::Trim, Kind::Audio | Kind::Video) => true,
            (Tool::Split, Kind::Audio | Kind::Video) => true,
            (Tool::Merge, Kind::Audio | Kind::Video) => true,
            (Tool::Merge, Kind::Document) => f.pandoc.is_some() || f.ext == "pdf",
            // Images merge into one PDF, a page each.
            (Tool::Merge, Kind::Image) => raster::reads(&f.ext),
            (Tool::Compress | Tool::Split, Kind::Document) => f.ext == "pdf",
            _ => false,
        }
    }
}

/// Lossy re-encode settings for Compress, by output type.
fn compress_args(ext: &str) -> Option<&'static [&'static str]> {
    Some(match ext {
        "mp3" => &["-c:a", "libmp3lame", "-b:a", "96k"],
        "ogg" => &["-c:a", "libvorbis", "-q:a", "2"],
        "opus" => &["-c:a", "libopus", "-b:a", "48k"],
        "m4a" => &["-c:a", "aac", "-b:a", "96k"],
        "flac" => &["-c:a", "flac", "-compression_level", "12"],
        "mp4" | "mov" => &[
            "-c:v", "libx264", "-preset", "slow", "-crf", "30", "-pix_fmt", "yuv420p", "-c:a",
            "aac", "-b:a", "96k",
        ],
        "mkv" => &[
            "-c:v", "libx264", "-preset", "slow", "-crf", "30", "-pix_fmt", "yuv420p", "-c:a",
            "libopus", "-b:a", "64k",
        ],
        "webm" => &[
            "-c:v",
            "libvpx-vp9",
            "-b:v",
            "0",
            "-crf",
            "42",
            "-row-mt",
            "1",
            "-c:a",
            "libopus",
            "-b:a",
            "64k",
        ],
        _ => return None,
    })
}

/// Silence below this level counts as silence for Trim.
const SILENCE_DB: &str = "-50dB";
/// What counts as a black frame for Trim: blackdetect with its default
/// pixel threshold, over at least a tenth of a second.
const BLACK_FILTER: &str = "blackdetect=d=0.1";
/// Seconds of video Crop looks at to find black bars.
const CROP_SCAN_SECONDS: f64 = 60.0;

impl Kiln {
    /// Tools that apply to all of `inputs`. Merge needs two or more files
    /// of the same type, or two or more images.
    pub fn tools_for(&self, inputs: &[&Path]) -> Vec<Tool> {
        let formats: Option<Vec<&Format>> = inputs
            .iter()
            .map(|p| self.registry().format_of(p))
            .collect();
        let Some(formats) = formats.filter(|f| !f.is_empty()) else {
            return vec![];
        };
        Tool::ALL
            .into_iter()
            .filter(|t| match t {
                Tool::Merge => {
                    let alike = formats.iter().all(|f| f.ext == formats[0].ext)
                        || formats.iter().all(|f| f.kind == Kind::Image);
                    formats.len() >= 2 && alike && formats.iter().all(|f| t.handles(f))
                }
                _ => formats.iter().all(|f| t.handles(f)),
            })
            .collect()
    }

    pub fn run_tool(&self, tool: Tool, inputs: &[PathBuf], opts: &Options) -> Result<Vec<PathBuf>> {
        self.run_tool_with(tool, inputs, opts, &mut |_| {}, &CancelToken::new())
    }

    /// Run `tool` and return the files it wrote. Merge combines all inputs
    /// into one file; the others work on each input in turn.
    pub fn run_tool_with(
        &self,
        tool: Tool,
        inputs: &[PathBuf],
        opts: &Options,
        progress: &mut dyn FnMut(Progress),
        cancel: &CancelToken,
    ) -> Result<Vec<PathBuf>> {
        let inputs = inputs
            .iter()
            .map(|p| checked_input(p))
            .collect::<Result<Vec<_>>>()?;
        let refs: Vec<&Path> = inputs.iter().map(PathBuf::as_path).collect();
        if !self.tools_for(&refs).contains(&tool) {
            return Err(self.unavailable(tool, &refs));
        }
        let mut ctx = Context { progress, cancel };

        let mut outputs = vec![];
        if tool == Tool::Merge {
            outputs = self.merge(&refs, opts, &mut ctx)?;
        } else {
            let n = refs.len() as f32;
            for (i, input) in refs.iter().enumerate() {
                let (lo, hi) = (i as f32 / n, (i + 1) as f32 / n);
                let outer = &mut *ctx.progress;
                let mut scaled = |p: Progress| {
                    outer(match p {
                        Progress::Fraction(f) => Progress::Fraction(lo + (hi - lo) * f),
                        other => other,
                    })
                };
                let mut sub = Context {
                    progress: &mut scaled,
                    cancel,
                };
                outputs.extend(self.one(tool, input, opts, &mut sub)?);
            }
        }
        (ctx.progress)(Progress::Fraction(1.0));
        Ok(outputs)
    }

    fn unavailable(&self, tool: Tool, inputs: &[&Path]) -> Error {
        if tool == Tool::Merge && inputs.len() < 2 {
            return Error::ToolUnavailable(
                "Merge needs two or more files of the same type.".to_owned(),
            );
        }
        let codes: Vec<String> = inputs
            .iter()
            .map(|p| match self.registry().format_of(p) {
                Some(f) => f.code(),
                None => p
                    .extension()
                    .map(|e| e.to_string_lossy().to_ascii_uppercase())
                    .unwrap_or_else(|| "these".to_owned()),
            })
            .collect();
        let mut unique = codes.clone();
        unique.dedup();
        if tool == Tool::Merge && unique.len() > 1 {
            return Error::ToolUnavailable("Merge needs files of the same type.".to_owned());
        }
        Error::ToolUnavailable(format!(
            "Kiln can't {} {} files.",
            tool.id(),
            unique.join(" and ")
        ))
    }

    fn one(
        &self,
        tool: Tool,
        input: &Path,
        opts: &Options,
        ctx: &mut Context<'_>,
    ) -> Result<Vec<PathBuf>> {
        let f = self
            .registry()
            .format_of(input)
            .expect("checked by tools_for");
        let out_dir = out_dir_for(input, opts)?;
        let work = WorkDir::for_options(opts)?;
        let stem = stem_of(input);
        let ext = f.ext.as_str();

        let named = |suffix: &str| format!("{stem} {suffix}");
        match (tool, f.kind) {
            (Tool::Compress, Kind::Document) => {
                let tmp = work.path().join("out.pdf");
                pdf::compress(input, &tmp, ctx)?;
                smaller_than(&tmp, input)?;
                Ok(vec![place(&tmp, &out_dir, &named(tool.suffix()), ext)?])
            }
            (Tool::Split, Kind::Document) => {
                let temps = pdf::split(input, &work.path().join("out.pdf"), ctx)?;
                temps
                    .iter()
                    .enumerate()
                    .map(|(i, tmp)| place(tmp, &out_dir, &format!("{stem} page {}", i + 1), ext))
                    .collect()
            }
            (Tool::Compress, Kind::Image) => {
                let img = raster::decode(input, ext)?;
                let tmp = work.path().join(format!("out.{ext}"));
                raster::encode(&img, &tmp, ext, 75)?;
                smaller_than(&tmp, input)?;
                Ok(vec![place(&tmp, &out_dir, &named(tool.suffix()), ext)?])
            }
            (Tool::Crop, Kind::Image) => {
                let img = raster::decode(input, ext)?;
                let (x, y, w, h) = border_box(&img).ok_or_else(|| {
                    Error::NothingToDo("There's no plain border to crop.".to_owned())
                })?;
                let tmp = work.path().join(format!("out.{ext}"));
                raster::encode(&img.crop_imm(x, y, w, h), &tmp, ext, opts.jpeg_quality)?;
                Ok(vec![place(&tmp, &out_dir, &named(tool.suffix()), ext)?])
            }
            (Tool::Compress, _) => {
                let args: Vec<OsString> = compress_args(ext)
                    .expect("handles() checked the type")
                    .iter()
                    .map(OsString::from)
                    .collect();
                let tmp = work.path().join(format!("out.{ext}"));
                let duration = ffmpeg::probe_duration(opts, input, ctx)?;
                media_args_run(opts, ctx, input, &args, &tmp, duration, (0.0, 1.0), &[])?;
                smaller_than(&tmp, input)?;
                Ok(vec![place(&tmp, &out_dir, &named(tool.suffix()), ext)?])
            }
            (Tool::Crop, Kind::Video) => {
                let log = ffmpeg::analyse(
                    opts,
                    ctx,
                    input,
                    "-vf",
                    "cropdetect=limit=24:round=2:reset=0",
                    Some(CROP_SCAN_SECONDS),
                )?;
                let size = ffmpeg::probe_size(opts, input, ctx)?;
                let crop = last_crop(&log)
                    .filter(|c| Some((c.0, c.1)) != size && c.0 > 0 && c.1 > 0)
                    .ok_or_else(|| {
                        Error::NothingToDo("There are no black bars to crop.".to_owned())
                    })?;
                let mut args: Vec<OsString> = vec![
                    "-vf".into(),
                    format!("crop={}:{}:{}:{}", crop.0, crop.1, crop.2, crop.3).into(),
                ];
                args.extend(f.ffmpeg.iter().map(OsString::from));
                let tmp = work.path().join(format!("out.{ext}"));
                let duration = ffmpeg::probe_duration(opts, input, ctx)?;
                media_args_run(opts, ctx, input, &args, &tmp, duration, (0.3, 1.0), &[])?;
                Ok(vec![place(&tmp, &out_dir, &named(tool.suffix()), ext)?])
            }
            (Tool::Trim, Kind::Video) => {
                let duration = ffmpeg::probe_duration(opts, input, ctx)?.ok_or_else(no_duration)?;
                let log = ffmpeg::analyse(opts, ctx, input, "-vf", BLACK_FILTER, None)?;
                let mut keep =
                    quiet_edges(&log, "black_start:", "black_end:", VIDEO_EDGE, duration);
                if ffmpeg::has_audio(opts, input, ctx)? {
                    let log = ffmpeg::analyse(
                        opts,
                        ctx,
                        input,
                        "-af",
                        &format!("silencedetect=noise={SILENCE_DB}:d=0.1"),
                        None,
                    )?;
                    let sound =
                        quiet_edges(&log, "silence_start: ", "silence_end: ", EDGE, duration);
                    // Only cut what is both black and silent, so a voice
                    // over a black opening stays.
                    keep = (keep.0.min(sound.0), keep.1.max(sound.1));
                }
                let (start, end) = worth_trimming(keep, duration).ok_or_else(|| {
                    Error::NothingToDo(
                        "There's no black, silent stretch at the start or end to trim.".to_owned(),
                    )
                })?;
                let pre = vec![OsString::from("-ss"), format!("{start:.3}").into()];
                let mut args: Vec<OsString> =
                    vec!["-t".into(), format!("{:.3}", end - start).into()];
                args.extend(f.ffmpeg.iter().map(OsString::from));
                let tmp = work.path().join(format!("out.{ext}"));
                media_args_run(
                    opts,
                    ctx,
                    input,
                    &args,
                    &tmp,
                    Some(end - start),
                    (0.3, 1.0),
                    &pre,
                )?;
                Ok(vec![place(&tmp, &out_dir, &named(tool.suffix()), ext)?])
            }
            (Tool::Trim, _) => {
                let duration = ffmpeg::probe_duration(opts, input, ctx)?.ok_or_else(no_duration)?;
                let log = ffmpeg::analyse(
                    opts,
                    ctx,
                    input,
                    "-af",
                    &format!("silencedetect=noise={SILENCE_DB}:d=0.1"),
                    None,
                )?;
                let (start, end) = sound_bounds(&log, duration).ok_or_else(|| {
                    Error::NothingToDo("There's no silence at the start or end to trim.".to_owned())
                })?;
                let pre = vec![OsString::from("-ss"), format!("{start:.3}").into()];
                let mut args: Vec<OsString> = vec![
                    "-t".into(),
                    format!("{:.3}", end - start).into(),
                    "-vn".into(),
                ];
                args.extend(f.ffmpeg.iter().map(OsString::from));
                let tmp = work.path().join(format!("out.{ext}"));
                media_args_run(
                    opts,
                    ctx,
                    input,
                    &args,
                    &tmp,
                    Some(end - start),
                    (0.3, 1.0),
                    &pre,
                )?;
                Ok(vec![place(&tmp, &out_dir, &named(tool.suffix()), ext)?])
            }
            (Tool::Split, _) => {
                let duration = ffmpeg::probe_duration(opts, input, ctx)?.ok_or_else(no_duration)?;
                let half = duration / 2.0;
                let mut temps = vec![];
                for (i, (pre, t)) in [(None, half), (Some(half), duration - half)]
                    .into_iter()
                    .enumerate()
                {
                    let pre: Vec<OsString> = match pre {
                        Some(s) => vec!["-ss".into(), format!("{s:.3}").into()],
                        None => vec![],
                    };
                    let mut args: Vec<OsString> = vec![
                        "-t".into(),
                        format!("{t:.3}").into(),
                        "-sn".into(),
                        "-dn".into(),
                    ];
                    args.extend(f.ffmpeg.iter().map(OsString::from));
                    let tmp = work.path().join(format!("part{}.{ext}", i + 1));
                    let span = (i as f32 * 0.5, (i + 1) as f32 * 0.5);
                    media_args_run(opts, ctx, input, &args, &tmp, Some(t), span, &pre)?;
                    temps.push(tmp);
                }
                temps
                    .iter()
                    .enumerate()
                    .map(|(i, tmp)| place(tmp, &out_dir, &format!("{stem} part {}", i + 1), ext))
                    .collect()
            }
            _ => unreachable!("tools_for() only offers supported pairs"),
        }
    }

    fn merge(
        &self,
        inputs: &[&Path],
        opts: &Options,
        ctx: &mut Context<'_>,
    ) -> Result<Vec<PathBuf>> {
        let first = inputs[0];
        let f = self
            .registry()
            .format_of(first)
            .expect("checked by tools_for");
        let out_dir = out_dir_for(first, opts)?;
        let work = WorkDir::for_options(opts)?;
        let tmp = work.path().join(format!("out.{}", f.ext));
        let name = format!("{} merged", stem_of(first));

        if f.kind == Kind::Image {
            let tmp = work.path().join("out.pdf");
            let images: Vec<(&Path, &str)> = inputs
                .iter()
                .map(|p| {
                    let ext = self.registry().format_of(p).expect("checked by tools_for");
                    (*p, ext.ext.as_str())
                })
                .collect();
            pdf::images_to_pdf(&images, &tmp, ctx)?;
            return Ok(vec![place(&tmp, &out_dir, &name, "pdf")?]);
        }
        if f.ext == "pdf" {
            pdf::merge(inputs, &tmp, ctx)?;
            return Ok(vec![place(&tmp, &out_dir, &name, "pdf")?]);
        }
        if f.kind == Kind::Document {
            (ctx.progress)(Progress::Indeterminate);
            let pandoc = f.pandoc.as_deref().expect("handles() checked pandoc");
            let mut cmd = std::process::Command::new(process::program(
                opts.engines.pandoc.as_deref(),
                "pandoc",
            ));
            cmd.arg("--sandbox")
                .args(["--from", pandoc, "--to", pandoc]);
            if f.standalone {
                cmd.arg("--standalone");
            }
            cmd.arg("--output").arg(&tmp).args(inputs);
            process::run("Pandoc", cmd, ctx.cancel, |_| {})?;
            ensure_written(&tmp, "Pandoc")?;
            return Ok(vec![place(&tmp, &out_dir, &name, &f.ext)?]);
        }

        let mut total = 0.0;
        for input in inputs {
            total += ffmpeg::probe_duration(opts, input, ctx)?.unwrap_or(0.0);
        }
        let n = inputs.len();
        let mut graph = String::new();
        if f.kind == Kind::Video {
            let (w, h) = ffmpeg::probe_size(opts, first, ctx)?.unwrap_or((1280, 720));
            for i in 0..n {
                graph.push_str(&format!(
                    "[{i}:v:0]scale={w}:{h}:force_original_aspect_ratio=decrease,\
                     pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps=30[v{i}];\
                     [{i}:a:0]aresample=48000[a{i}];"
                ));
            }
            for i in 0..n {
                graph.push_str(&format!("[v{i}][a{i}]"));
            }
            graph.push_str(&format!("concat=n={n}:v=1:a=1[v][a]"));
        } else {
            for i in 0..n {
                graph.push_str(&format!(
                    "[{i}:a:0]aresample=48000,aformat=channel_layouts=stereo[a{i}];"
                ));
            }
            for i in 0..n {
                graph.push_str(&format!("[a{i}]"));
            }
            graph.push_str(&format!("concat=n={n}:v=0:a=1[a]"));
        }
        let mut args: Vec<OsString> = vec!["-filter_complex".into(), graph.into()];
        if f.kind == Kind::Video {
            args.extend(["-map", "[v]"].map(OsString::from));
        }
        args.extend(["-map", "[a]"].map(OsString::from));
        args.extend(f.ffmpeg.iter().map(OsString::from));
        let job = Encode {
            inputs,
            pre_input: &[],
            args: &args,
            output: &tmp,
            duration: (total > 0.0).then_some(total),
            span: (0.0, 1.0),
        };
        job.run(opts, ctx)?;
        ensure_written(&tmp, "FFmpeg")?;
        Ok(vec![place(&tmp, &out_dir, &name, &f.ext)?])
    }
}

#[allow(clippy::too_many_arguments)]
fn media_args_run(
    opts: &Options,
    ctx: &mut Context<'_>,
    input: &Path,
    args: &[OsString],
    output: &Path,
    duration: Option<f64>,
    span: (f32, f32),
    pre_input: &[OsString],
) -> Result<()> {
    let mut all: Vec<OsString> = vec!["-map_metadata".into(), "0".into()];
    all.extend_from_slice(args);
    Encode {
        inputs: &[input],
        pre_input,
        args: &all,
        output,
        duration,
        span,
    }
    .run(opts, ctx)?;
    ensure_written(output, "FFmpeg")
}

fn no_duration() -> Error {
    Error::NothingToDo("Couldn't tell how long this file is.".to_owned())
}

/// A compressed copy that isn't smaller isn't worth keeping.
fn smaller_than(candidate: &Path, original: &Path) -> Result<()> {
    let a = std::fs::metadata(candidate)
        .map_err(|e| Error::io(candidate, e))?
        .len();
    let b = std::fs::metadata(original)
        .map_err(|e| Error::io(original, e))?
        .len();
    if a < b {
        Ok(())
    } else {
        Err(Error::NothingToDo(
            "This file is already as small as Kiln can make it.".to_owned(),
        ))
    }
}

/// The box inside a plain border, as (x, y, w, h). `None` if the corners
/// don't agree on a border colour, or there is no border to remove.
fn border_box(img: &DynamicImage) -> Option<(u32, u32, u32, u32)> {
    const TOLERANCE: u8 = 10;
    let (w, h) = img.dimensions();
    if w < 3 || h < 3 {
        return None;
    }
    let near =
        |a: Rgba<u8>, b: Rgba<u8>| a.0.iter().zip(b.0).all(|(x, y)| x.abs_diff(y) <= TOLERANCE);
    let border = img.get_pixel(0, 0);
    for (x, y) in [(w - 1, 0), (0, h - 1), (w - 1, h - 1)] {
        if !near(img.get_pixel(x, y), border) {
            return None;
        }
    }
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for y in 0..h {
        for x in 0..w {
            if !near(img.get_pixel(x, y), border) {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    if x0 > x1 || (x0, y0, x1, y1) == (0, 0, w - 1, h - 1) {
        return None;
    }
    Some((x0, y0, x1 - x0 + 1, y1 - y0 + 1))
}

/// The last `crop=w:h:x:y` that cropdetect reported.
fn last_crop(log: &str) -> Option<(u32, u32, u32, u32)> {
    let spec = log
        .rsplit("crop=")
        .next()
        .filter(|_| log.contains("crop="))?;
    let spec: String = spec
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ':')
        .collect();
    let n: Vec<u32> = spec.split(':').filter_map(|s| s.parse().ok()).collect();
    match n.as_slice() {
        [w, h, x, y] => Some((*w, *h, *x, *y)),
        _ => None,
    }
}

/// Seconds within which a quiet stretch counts as touching an edge.
const EDGE: f64 = 0.05;
/// The same for black video, which blackdetect ends at the last frame's
/// start, a frame short of the file's end.
const VIDEO_EDGE: f64 = 0.25;

/// Where the sound starts and ends, from silencedetect's log. `None` if
/// there's nothing to trim, or nothing but silence.
fn sound_bounds(log: &str, duration: f64) -> Option<(f64, f64)> {
    let keep = quiet_edges(log, "silence_start: ", "silence_end: ", EDGE, duration);
    worth_trimming(keep, duration)
}

/// Where the content starts and ends once quiet stretches touching either
/// edge are left out. `start_key` and `end_key` are how a detect filter
/// (silencedetect, blackdetect) labels each stretch in its log.
fn quiet_edges(log: &str, start_key: &str, end_key: &str, edge: f64, duration: f64) -> (f64, f64) {
    let value = |line: &str, key: &str| -> Option<f64> {
        let rest = line.split(key).nth(1)?;
        rest.split_whitespace()
            .next()?
            .trim_end_matches('|')
            .parse()
            .ok()
    };
    let mut silences: Vec<(f64, Option<f64>)> = vec![];
    for line in log.lines() {
        // blackdetect puts both ends on one line; silencedetect splits them.
        if let Some(s) = value(line, start_key) {
            silences.push((s.max(0.0), None));
        }
        if let Some(e) = value(line, end_key) {
            if let Some(last) = silences.last_mut() {
                last.1 = Some(e);
            }
        }
    }
    let mut start = 0.0;
    let mut end = duration;
    if let Some((s, Some(e))) = silences.first() {
        if *s <= edge {
            start = *e;
        }
    }
    if let Some((s, e)) = silences.last() {
        if e.is_none_or(|e| e >= duration - edge) && *s > start {
            end = *s;
        }
    }
    (start, end)
}

/// `keep` if it cuts anything and leaves something.
fn worth_trimming((start, end): (f64, f64), duration: f64) -> Option<(f64, f64)> {
    let trimmed = start > EDGE || end < duration - EDGE;
    (trimmed && end - start > 0.05).then_some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_ids_round_trip() {
        for t in Tool::ALL {
            assert_eq!(Tool::parse(t.id()), Some(t));
        }
        assert_eq!(Tool::parse("COMPRESS"), Some(Tool::Compress));
        assert_eq!(Tool::parse("explode"), None);
    }

    #[test]
    fn parses_cropdetect_output() {
        let log = "[Parsed_cropdetect_0 @ 0x1] x1:0 x2:63 y1:6 y2:41 w:64 h:36 x:0 y:6 pts:1 t:0.1 limit:0.09 crop=64:36:0:6\n\
                   [Parsed_cropdetect_0 @ 0x1] x1:0 x2:63 y1:6 y2:41 w:64 h:36 x:0 y:6 pts:2 t:0.2 limit:0.09 crop=64:36:0:6\n";
        assert_eq!(last_crop(log), Some((64, 36, 0, 6)));
        assert_eq!(last_crop("nothing here"), None);
    }

    #[test]
    fn finds_picture_between_black_stretches() {
        let log = "[blackdetect @ 0x1] black_start:0 black_end:0.5 black_duration:0.5\n\
                   [blackdetect @ 0x1] black_start:1.5 black_end:2 black_duration:0.5\n";
        let keep = quiet_edges(log, "black_start:", "black_end:", VIDEO_EDGE, 2.0);
        assert_eq!(worth_trimming(keep, 2.0), Some((0.5, 1.5)));
        let empty = quiet_edges("", "black_start:", "black_end:", VIDEO_EDGE, 2.0);
        assert_eq!(empty, (0.0, 2.0));
        assert_eq!(worth_trimming((0.0, 2.0), 2.0), None);
    }

    #[test]
    fn finds_sound_between_silences() {
        let log = "[silencedetect @ 0x1] silence_start: 0\n\
                   [silencedetect @ 0x1] silence_end: 0.5 | silence_duration: 0.5\n\
                   [silencedetect @ 0x1] silence_start: 1.5\n\
                   [silencedetect @ 0x1] silence_end: 2 | silence_duration: 0.5\n";
        assert_eq!(sound_bounds(log, 2.0), Some((0.5, 1.5)));
        // Trailing silence often has no end line at all.
        let open = "[silencedetect @ 0x1] silence_start: 1.5\n";
        assert_eq!(sound_bounds(open, 2.0), Some((0.0, 1.5)));
        // Silence only in the middle: nothing to trim.
        let middle = "[silencedetect @ 0x1] silence_start: 0.8\n\
                      [silencedetect @ 0x1] silence_end: 1.1 | silence_duration: 0.3\n";
        assert_eq!(sound_bounds(middle, 2.0), None);
        assert_eq!(sound_bounds("", 2.0), None);
    }

    #[test]
    fn border_box_finds_the_content() {
        let mut img = image::RgbImage::from_pixel(20, 10, image::Rgb([240, 240, 240]));
        for y in 3..7 {
            for x in 5..12 {
                img.put_pixel(x, y, image::Rgb([10, 20, 30]));
            }
        }
        let img = DynamicImage::ImageRgb8(img);
        assert_eq!(border_box(&img), Some((5, 3, 7, 4)));
        let plain =
            DynamicImage::ImageRgb8(image::RgbImage::from_pixel(8, 8, image::Rgb([1, 2, 3])));
        assert_eq!(border_box(&plain), None);
    }
}
