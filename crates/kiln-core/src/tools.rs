//! File tools: make smaller, resize, remove location data, remove borders
//! or black bars, remove quiet ends, cut in half, save each page and merge.
//! Each one writes new files next to its inputs and never touches the
//! originals.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use image::{DynamicImage, GenericImageView, Rgba};

use crate::converter::{CancelToken, Context, Options, Progress};
use crate::error::{Error, Result};
use crate::ffmpeg::{self, Encode};
use crate::job::{checked_input, ensure_written, out_dir_for, place, stem_of, Kiln, WorkDir};
use crate::registry::{Format, Kind};
use crate::{pdf, photo, process, raster, video};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tool {
    /// Make a video, photo or PDF smaller, in the same format.
    Smaller,
    /// Scale a photo down to a long side.
    Resize,
    /// Take where a photo was taken and the camera details out of it.
    RemoveLocation,
    /// Remove a photo's plain border, or a video's black bars.
    RemoveEdges,
    /// Remove silence from the ends of sound, and stretches that are both
    /// black and silent from the ends of a video.
    RemoveQuietEnds,
    /// Cut sound or video into two halves.
    CutInHalf,
    /// Save each page of a PDF as a PDF of its own.
    SaveEachPage,
    /// Join two or more files into one.
    Merge,
}

/// What a tool needs to know beyond the files. Nothing is set by default;
/// each tool says what it does with a value left unset.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolOptions {
    /// Make smaller: the largest the result may be, in bytes.
    pub max_bytes: Option<u64>,
    /// Make smaller and Resize: the longest side of a photo, in pixels.
    pub long_side: Option<u32>,
    /// Make smaller and Resize: also remove location data.
    pub remove_location: bool,
    /// Make smaller for video: keep the sound. Unset keeps it.
    pub keep_sound: Option<bool>,
    /// Make smaller for video: the most lines the picture's short side may
    /// have (720 for 720p); it is scaled down to it, never up.
    pub picture_lines: Option<u32>,
}

impl Tool {
    pub const ALL: [Tool; 8] = [
        Tool::Smaller,
        Tool::Resize,
        Tool::RemoveLocation,
        Tool::RemoveEdges,
        Tool::RemoveQuietEnds,
        Tool::CutInHalf,
        Tool::SaveEachPage,
        Tool::Merge,
    ];

    /// The tool's name in menus for files of `kind`: "Remove borders" for
    /// photos, "Remove black bars" for video.
    pub fn label(self, kind: Kind) -> &'static str {
        match (self, kind) {
            (Tool::Smaller, _) => "Make smaller",
            (Tool::Resize, _) => "Resize",
            (Tool::RemoveLocation, _) => "Remove location data",
            (Tool::RemoveEdges, Kind::Video) => "Remove black bars",
            (Tool::RemoveEdges, _) => "Remove borders",
            (Tool::RemoveQuietEnds, Kind::Video) => "Remove black at the ends",
            (Tool::RemoveQuietEnds, _) => "Remove silence at the ends",
            (Tool::CutInHalf, _) => "Cut in half",
            (Tool::SaveEachPage, _) => "Save each page as a PDF",
            (Tool::Merge, _) => "Merge",
        }
    }

    /// The tool as a sentence round `what` it works on, lowercase:
    /// "make photo.png smaller", "remove borders from 3 files".
    pub fn phrase(self, kind: Kind, what: &str) -> String {
        match self {
            Tool::Smaller => format!("make {what} smaller"),
            Tool::Resize => format!("resize {what}"),
            Tool::CutInHalf => format!("cut {what} in half"),
            Tool::SaveEachPage => format!("save each page of {what} as a PDF"),
            Tool::Merge => format!("merge {what}"),
            Tool::RemoveLocation | Tool::RemoveEdges => {
                format!("{} from {what}", self.label(kind).to_lowercase())
            }
            Tool::RemoveQuietEnds => {
                format!("{} of {what}", self.label(kind).to_lowercase())
            }
        }
    }

    /// The id scripts and the command line use.
    pub fn id(self) -> &'static str {
        match self {
            Tool::Smaller => "smaller",
            Tool::Resize => "resize",
            Tool::RemoveLocation => "remove-location",
            Tool::RemoveEdges => "remove-edges",
            Tool::RemoveQuietEnds => "remove-quiet-ends",
            Tool::CutInHalf => "cut-in-half",
            Tool::SaveEachPage => "save-each-page",
            Tool::Merge => "merge",
        }
    }

    /// The tool for an id, or one of the ids the tools had before they
    /// were named for what they do (`compress`, `crop`, `trim`, `split`).
    pub fn parse(s: &str) -> Option<Tool> {
        let s = s.trim();
        let old = [
            ("compress", Tool::Smaller),
            ("crop", Tool::RemoveEdges),
            ("trim", Tool::RemoveQuietEnds),
            ("split", Tool::CutInHalf),
        ];
        Tool::ALL
            .into_iter()
            .map(|t| (t.id(), t))
            .chain(old)
            .find(|(id, _)| id.eq_ignore_ascii_case(s))
            .map(|(_, t)| t)
    }

    /// What is added to output names: "photo cropped.jpg", "photo
    /// (smaller).jpg". Resize puts the long side before its own: "photo
    /// (1920 px).jpg".
    fn suffix(self) -> &'static str {
        match self {
            Tool::Smaller => "(smaller)",
            Tool::Resize => "px",
            Tool::RemoveLocation => "(no location)",
            Tool::RemoveEdges => "cropped",
            Tool::RemoveQuietEnds => "trimmed",
            Tool::CutInHalf => "part",
            Tool::SaveEachPage => "page",
            Tool::Merge => "merged",
        }
    }

    /// Whether this tool can work on one file of this format.
    fn handles(self, f: &Format) -> bool {
        let ext = f.ext.as_str();
        match (self, f.kind) {
            // Only formats Kiln can write (R2).
            (Tool::Smaller, Kind::Image) => matches!(ext, "jpg" | "png" | "webp"),
            (Tool::Smaller, Kind::Video) => matches!(ext, "mp4" | "mov" | "m4v" | "mkv"),
            (Tool::Smaller | Tool::SaveEachPage, Kind::Document) => ext == "pdf",
            (Tool::Resize | Tool::RemoveLocation, Kind::Image) => matches!(
                ext,
                "jpg" | "png" | "webp" | "tiff" | "bmp" | "gif" | "heic" | "avif"
            ),
            (Tool::RemoveEdges, Kind::Image | Kind::Video) => true,
            (Tool::RemoveQuietEnds | Tool::CutInHalf, Kind::Audio | Kind::Video) => true,
            (Tool::Merge, Kind::Audio | Kind::Video) => true,
            (Tool::Merge, Kind::Document) => f.pandoc.is_some() || ext == "pdf",
            // Images merge into one PDF, a page each.
            (Tool::Merge, Kind::Image) => raster::reads(ext),
            _ => false,
        }
    }
}

/// Whether Make smaller's work for `f` is written: video, photos and PDF.
fn smaller_ready(f: &Format) -> bool {
    matches!(
        f.ext.as_str(),
        "mp4" | "mov" | "m4v" | "mkv" | "jpg" | "png" | "webp" | "pdf"
    )
}

/// "Kiln can't make videos smaller yet." for `tool` on files like `f`.
fn not_ready_for(tool: Tool, f: &Format) -> Error {
    let what = match f.kind {
        Kind::Video => "videos".to_owned(),
        Kind::Image if tool != Tool::Smaller || smaller_ready(f) => "photos".to_owned(),
        _ => format!("{} files", f.code()),
    };
    Error::ToolUnavailable(format!("Kiln can't {} yet.", tool.phrase(f.kind, &what)))
}

/// Silence below this level counts as silence for Remove quiet ends.
const SILENCE_DB: &str = "-50dB";
/// What counts as a black frame for Remove quiet ends: blackdetect with its default
/// pixel threshold, over at least a tenth of a second.
const BLACK_FILTER: &str = "blackdetect=d=0.1";
/// Seconds of video Remove edges looks at to find black bars.
const CROP_SCAN_SECONDS: f64 = 60.0;

impl Kiln {
    /// Tools that apply to all of `inputs`, which must be of one kind.
    /// Merge needs two or more files of the same type, or two or more
    /// images.
    pub fn tools_for(&self, inputs: &[&Path]) -> Vec<Tool> {
        let formats: Option<Vec<&Format>> = inputs
            .iter()
            .map(|p| self.registry().format_of(p))
            .collect();
        let Some(formats) = formats.filter(|f| !f.is_empty()) else {
            return vec![];
        };
        // A tool's name depends on the kind of file it works on, so files
        // of different kinds share none.
        if formats.iter().any(|f| f.kind != formats[0].kind) {
            return vec![];
        }
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

    /// How big Make smaller would make `input` for `opts.tool.max_bytes`
    /// (with its other options): a photo or PDF by the same trials the
    /// tool makes, in memory; a video by Fit under's bitrate, or a short
    /// sample where the size holds its best picture. Writes nothing, but
    /// for FFmpeg's picture of a JPG only FFmpeg can read, which goes to a
    /// private folder removed once it is read. Its
    /// refusals are the tool's: [`Error::OutOfReach`] with the smallest
    /// Kiln can make, and `NothingToDo` for a file already under the
    /// size. Slow: never on the window's thread.
    pub fn smaller_size(&self, input: &Path, opts: &Options, cancel: &CancelToken) -> Result<u64> {
        let input = checked_input(input)?;
        let f = self.registry().format_of(&input).cloned();
        let Some(f) = f.filter(|f| Tool::Smaller.handles(f) && smaller_ready(f)) else {
            return Err(self.unavailable(Tool::Smaller, &[input.as_path()]));
        };
        let Some(max) = opts.tool.max_bytes else {
            return Err(Error::ToolUnavailable(
                "Make smaller needs a size for its estimate.".to_owned(),
            ));
        };
        match f.kind {
            Kind::Document => pdf::smaller_size(&input, max, cancel),
            Kind::Video => video::smaller_size(&input, &f.ext, opts, cancel),
            _ => photo::smaller_size(&input, &f.ext, opts, cancel),
        }
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
        if !self.built(tool, &refs) {
            return Err(self.not_ready(tool, &refs));
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

    /// Whether the work for `tool` on `inputs` is written yet. Make smaller
    /// works on MP4, MOV, M4V, MKV, JPG, PNG, WEBP and PDF.
    fn built(&self, tool: Tool, inputs: &[&Path]) -> bool {
        match tool {
            Tool::Smaller => inputs
                .iter()
                .all(|p| self.registry().format_of(p).is_some_and(smaller_ready)),
            _ => true,
        }
    }

    /// What [`Self::built`] refused, said plainly: "Kiln can't make
    /// videos smaller yet." It names the first file whose format isn't
    /// ready.
    fn not_ready(&self, tool: Tool, inputs: &[&Path]) -> Error {
        let formats: Vec<&Format> = inputs
            .iter()
            .filter_map(|p| self.registry().format_of(p))
            .collect();
        let waiting = formats
            .iter()
            .find(|f| tool == Tool::Smaller && !smaller_ready(f));
        match waiting.or(formats.first()) {
            Some(f) => not_ready_for(tool, f),
            None => Error::ToolUnavailable(format!(
                "Kiln can't {} yet.",
                tool.phrase(Kind::Document, "these files")
            )),
        }
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
        let kind = inputs
            .first()
            .and_then(|p| self.registry().format_of(p))
            .map_or(Kind::Document, |f| f.kind);
        let what = format!("{} files", unique.join(" and "));
        Error::ToolUnavailable(format!("Kiln can't {}.", tool.phrase(kind, &what)))
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
            (Tool::Smaller, Kind::Document) => {
                let tmp = work.path().join("out.pdf");
                pdf::smaller(input, &tmp, opts.tool.max_bytes, ctx)?;
                Ok(vec![place(&tmp, &out_dir, &named(tool.suffix()), ext)?])
            }
            (Tool::SaveEachPage, Kind::Document) => {
                let temps = pdf::split(input, &work.path().join("out.pdf"), ctx)?;
                temps
                    .iter()
                    .enumerate()
                    .map(|(i, tmp)| {
                        place(
                            tmp,
                            &out_dir,
                            &format!("{stem} {} {}", tool.suffix(), i + 1),
                            ext,
                        )
                    })
                    .collect()
            }
            (Tool::Smaller, Kind::Video) => {
                let tmp = video::smaller(input, ext, opts, work.path(), ctx)?;
                Ok(vec![place(&tmp, &out_dir, &named(tool.suffix()), ext)?])
            }
            (Tool::Smaller, Kind::Image) if smaller_ready(f) => {
                (ctx.progress)(Progress::Indeterminate);
                let (tmp, saved_as) = photo::smaller(input, ext, opts, work.path(), ctx.cancel)?;
                Ok(vec![place(
                    &tmp,
                    &out_dir,
                    &named(tool.suffix()),
                    &saved_as,
                )?])
            }
            (Tool::Resize, Kind::Image) => {
                (ctx.progress)(Progress::Indeterminate);
                let (tmp, saved_as, side) =
                    photo::resize(input, ext, opts, work.path(), ctx.cancel)?;
                let name = named(&format!("({side} {})", tool.suffix()));
                Ok(vec![place(&tmp, &out_dir, &name, &saved_as)?])
            }
            (Tool::RemoveLocation, Kind::Image) => {
                (ctx.progress)(Progress::Indeterminate);
                let (tmp, saved_as) =
                    photo::without_location(input, ext, opts, work.path(), ctx.cancel)?;
                Ok(vec![place(
                    &tmp,
                    &out_dir,
                    &named(tool.suffix()),
                    &saved_as,
                )?])
            }
            (Tool::RemoveEdges, Kind::Image) => {
                let fallback = raster::Fallback {
                    options: opts,
                    work: work.path(),
                };
                let raster::Decoded { image, icc, .. } =
                    raster::decode_with(input, ext, ctx.cancel, Some(fallback))?;
                let (x, y, w, h) = border_box(&image).ok_or_else(|| {
                    Error::NothingToDo("There's no plain border to remove.".to_owned())
                })?;
                let tmp = work.path().join(format!("out.{ext}"));
                let cropped = image.crop_imm(x, y, w, h);
                raster::encode(&cropped, &tmp, ext, opts.jpeg_quality, icc.as_deref())?;
                Ok(vec![place(&tmp, &out_dir, &named(tool.suffix()), ext)?])
            }
            (Tool::RemoveEdges, Kind::Video) => {
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
                        Error::NothingToDo("There are no black bars to remove.".to_owned())
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
            (Tool::RemoveQuietEnds, Kind::Video) => {
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
                        "There's no black, silent stretch at the start or end to remove."
                            .to_owned(),
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
            (Tool::RemoveQuietEnds, _) => {
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
                    Error::NothingToDo(
                        "There's no silence at the start or end to remove.".to_owned(),
                    )
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
            (Tool::CutInHalf, _) => {
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
                    .map(|(i, tmp)| {
                        place(
                            tmp,
                            &out_dir,
                            &format!("{stem} {} {}", tool.suffix(), i + 1),
                            ext,
                        )
                    })
                    .collect()
            }
            // `tools_for()` and `built()` let through only the work that
            // is written; anything else says what isn't ready.
            _ => Err(not_ready_for(tool, f)),
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
            let fallback = raster::Fallback {
                options: opts,
                work: work.path(),
            };
            pdf::images_to_pdf(&images, &tmp, fallback, ctx)?;
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

/// "Couldn't tell how long this file is."
pub(crate) fn no_duration() -> Error {
    Error::NothingToDo("Couldn't tell how long this file is.".to_owned())
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
pub(crate) fn last_crop(log: &str) -> Option<(u32, u32, u32, u32)> {
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
    fn work_not_written_yet_is_refused_not_a_panic() {
        // `built()` keeps this from `one()`; reached all the same, it says
        // what isn't ready rather than end Kiln.
        let dir = tempfile::tempdir().unwrap();
        let sized = Options {
            temp_root: Some(dir.path().join("work")),
            tool: ToolOptions {
                long_side: Some(1_000),
                ..ToolOptions::default()
            },
            ..Options::default()
        };
        let input = dir.path().join("clip.mp4");
        std::fs::write(&input, b"x").unwrap();
        let mut progress = |_| {};
        let cancel = CancelToken::new();
        let mut ctx = Context {
            progress: &mut progress,
            cancel: &cancel,
        };
        let err = Kiln::new()
            .one(Tool::Resize, &input, &sized, &mut ctx)
            .unwrap_err();
        let want = "Kiln can't resize videos yet.";
        assert!(
            matches!(&err, Error::ToolUnavailable(m) if m == want),
            "{err:?}"
        );
    }

    #[test]
    fn tool_ids_round_trip() {
        for t in Tool::ALL {
            assert_eq!(Tool::parse(t.id()), Some(t));
        }
        let ids: Vec<&str> = Tool::ALL.iter().map(|t| t.id()).collect();
        assert_eq!(
            ids,
            [
                "smaller",
                "resize",
                "remove-location",
                "remove-edges",
                "remove-quiet-ends",
                "cut-in-half",
                "save-each-page",
                "merge"
            ]
        );
        // The old ids still work, so scripts keep running.
        assert_eq!(Tool::parse("COMPRESS"), Some(Tool::Smaller));
        assert_eq!(Tool::parse("crop"), Some(Tool::RemoveEdges));
        assert_eq!(Tool::parse("trim"), Some(Tool::RemoveQuietEnds));
        assert_eq!(Tool::parse("split"), Some(Tool::CutInHalf));
        assert_eq!(Tool::parse("explode"), None);
    }

    #[test]
    fn tools_are_named_for_what_they_do_to_each_kind() {
        use Kind::*;
        assert_eq!(
            Tool::RemoveQuietEnds.label(Audio),
            "Remove silence at the ends"
        );
        assert_eq!(
            Tool::RemoveQuietEnds.label(Video),
            "Remove black at the ends"
        );
        assert_eq!(Tool::RemoveEdges.label(Image), "Remove borders");
        assert_eq!(Tool::RemoveEdges.label(Video), "Remove black bars");
        assert_eq!(Tool::CutInHalf.label(Audio), "Cut in half");
        assert_eq!(Tool::CutInHalf.label(Video), "Cut in half");
        assert_eq!(
            Tool::SaveEachPage.label(Document),
            "Save each page as a PDF"
        );
        assert_eq!(Tool::Smaller.label(Video), "Make smaller");
        assert_eq!(Tool::Resize.label(Image), "Resize");
        assert_eq!(Tool::RemoveLocation.label(Image), "Remove location data");
        assert_eq!(Tool::Merge.label(Audio), "Merge");
    }

    #[test]
    fn a_tool_reads_as_a_sentence_round_what_it_works_on() {
        use Kind::*;
        let say = |t: Tool, k| t.phrase(k, "photo.png");
        assert_eq!(say(Tool::Smaller, Image), "make photo.png smaller");
        assert_eq!(say(Tool::Resize, Image), "resize photo.png");
        assert_eq!(
            say(Tool::RemoveLocation, Image),
            "remove location data from photo.png"
        );
        assert_eq!(
            say(Tool::RemoveEdges, Image),
            "remove borders from photo.png"
        );
        assert_eq!(
            say(Tool::RemoveEdges, Video),
            "remove black bars from photo.png"
        );
        assert_eq!(
            say(Tool::RemoveQuietEnds, Audio),
            "remove silence at the ends of photo.png"
        );
        assert_eq!(
            say(Tool::RemoveQuietEnds, Video),
            "remove black at the ends of photo.png"
        );
        assert_eq!(say(Tool::CutInHalf, Audio), "cut photo.png in half");
        assert_eq!(
            say(Tool::SaveEachPage, Document),
            "save each page of photo.png as a PDF"
        );
        assert_eq!(say(Tool::Merge, Audio), "merge photo.png");
    }

    #[test]
    fn tool_options_start_with_nothing_set() {
        let o = crate::Options::default().tool;
        assert_eq!(o, ToolOptions::default());
        assert_eq!(o.max_bytes, None);
        assert_eq!(o.long_side, None);
        assert!(!o.remove_location);
        assert_eq!(o.keep_sound, None);
        assert_eq!(o.picture_lines, None);
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
