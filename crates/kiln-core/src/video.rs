//! Video through Kiln's own engine, `kiln_media::video`: MP4, MOV, M4V and
//! MKV converted among themselves, the picture copied where it can be and
//! encoded through Windows' Media Foundation where it can't.
//!
//! FFmpeg stays the fallback (ruling R52). When it is installed, these go
//! through it with formats.toml's arguments, retuned to the job's own video
//! settings (R57): a picture codec Windows has no decoder for (ProRes, say),
//! HEVC, AV1 or VP9 without Microsoft's Store extension that reads it, sound
//! the job has to refuse (R14, R50), and a PC without Media Foundation.
//! Without FFmpeg the job fails with kiln-media's own sentence, and an
//! extension's Store product travels with it for a Get it link
//! ([`Error::NeedsExtension`]).
//!
//! The facts of these four containers are Kiln's own too: the picture's
//! from the file's headers ([`media_info`]), and black bars from a few
//! decoded frames ([`bars`]).

use std::fs::File;
use std::path::Path;
use std::time::{Duration, Instant};

use kiln_media::video::{
    self, tune, Luma, ReadMode, Reader, SoundChoice, VCodec, VideoCodec, VideoInfo, VideoSetting,
    VideoTarget, VideoTune, TIMESCALE,
};
use kiln_media::{MediaError, SourceInfo};

use crate::audio;
use crate::converter::{Context, Converter, Options, Progress, Request};
use crate::error::{Error, Result};
use crate::facts::Bars;
use crate::ffmpeg::FfmpegConverter;
use crate::quality::{self, Limit, MediaInfo, Preset};
use crate::registry::{Format, Kind};

/// The backend's id in formats.toml.
pub(crate) const VIDEO: &str = "video";

/// The containers Kiln's own engine reads and writes.
pub(crate) const NATIVE: [&str; 4] = ["mp4", "m4v", "mov", "mkv"];

/// The details line of a job Automatic meant for a graphics card whose
/// encoder refused it, so Windows' software encoder wrote it (R58).
pub const ON_THE_PROCESSOR: &str = "Encoded on the processor.";

/// kiln-media's target for an extension, for the four containers.
pub(crate) fn target_of(ext: &str) -> Option<VideoTarget> {
    match ext {
        "mp4" => Some(VideoTarget::Mp4),
        "m4v" => Some(VideoTarget::M4v),
        "mov" => Some(VideoTarget::Mov),
        "mkv" => Some(VideoTarget::Mkv),
        _ => None,
    }
}

/// Whether Kiln's own engine reads the picture of files of this format.
pub(crate) fn reads_picture(f: &Format) -> bool {
    f.kind == Kind::Video && NATIVE.contains(&f.ext.as_str())
}

/// Whether Kiln's own engine converts `from` to `to`: one of the four
/// containers to another. This is what formats.toml's "video" route says;
/// a test holds the two together.
pub(crate) fn native_pair(from: &Format, to: &Format) -> bool {
    reads_picture(from) && reads_picture(to)
}

/// Converts video with kiln-media, handing what it can't read to FFmpeg.
pub struct VideoConverter {
    /// The fallback; `None` never falls back.
    ffmpeg: Option<FfmpegConverter>,
}

impl VideoConverter {
    /// The "video" backend, with FFmpeg as its fallback.
    pub const fn new() -> Self {
        VideoConverter {
            ffmpeg: Some(FfmpegConverter),
        }
    }

    /// The backend with no fallback, whatever is installed.
    pub const fn without_fallback() -> Self {
        VideoConverter { ffmpeg: None }
    }

    /// Runs the job through FFmpeg when it is installed, with the job's
    /// video settings (R57). Otherwise the job fails with `why`'s sentence,
    /// or, for a codec Windows can't decode at all (`why` is `None`), says
    /// FFmpeg is needed.
    fn fallback(
        &self,
        req: &Request<'_>,
        ctx: &mut Context<'_>,
        why: Option<MediaError>,
    ) -> Result<()> {
        let Some(ffmpeg) = self
            .ffmpeg
            .as_ref()
            .filter(|_| ffmpeg_installed(req.options))
        else {
            return Err(match why {
                Some(e) => audio::from_media(e, req.input, req.output),
                None => Error::EngineMissing { engine: "FFmpeg" },
            });
        };
        let target =
            target_of(&req.to.ext).ok_or_else(|| Error::UnknownFormat(req.to.ext.clone()))?;
        let fb = Fallback::plan(req, target)?;
        let retuned = Request {
            input: req.input,
            output: req.output,
            from: req.from,
            to: &fb.to,
            options: &fb.options,
        };
        ffmpeg.convert(&retuned, ctx)?;
        if let Some(fit) = &fb.fit {
            fit.check(req.output)?;
        }
        Ok(())
    }
}

impl Default for VideoConverter {
    fn default() -> Self {
        Self::new()
    }
}

impl Converter for VideoConverter {
    fn id(&self) -> &'static str {
        VIDEO
    }

    fn convert(&self, req: &Request<'_>, ctx: &mut Context<'_>) -> Result<()> {
        let target =
            target_of(&req.to.ext).ok_or_else(|| Error::UnknownFormat(req.to.ext.clone()))?;
        // Without Media Foundation (an N edition without the Media Feature
        // Pack) every job is FFmpeg's, or says the pack is missing (R59).
        if !audio::media_foundation_available() {
            // Off Windows nothing but FFmpeg can do it, and the row says so.
            let why = cfg!(windows).then_some(MediaError::NeedsMediaFoundation);
            return self.fallback(req, ctx, why);
        }
        // A picture codec Windows has no decoder for is FFmpeg's from the
        // start (R52). Any other failure to read the file is the job's to
        // report.
        if video::probe(req.input).is_ok_and(|info| undecodable(&info.codec)) {
            return self.fallback(req, ctx, None);
        }
        let q = &req.options.quality;
        let tune = quality::video_tune(q);
        // The job asks for the graphics card itself where Fit under's size
        // model needs it (R82); a copy never starts its encoder.
        let cancel = ctx.cancel;
        let done = {
            let progress = &mut ctx.progress;
            video::convert(
                req.input,
                req.output,
                target,
                audio::preset(q.preset),
                &tune,
                &mut |f| progress(Progress::Fraction(f)),
                &|| cancel.is_cancelled(),
            )
        };
        match done {
            Ok(report) => {
                if report.fell_back {
                    (ctx.progress)(Progress::Note(ON_THE_PROCESSOR));
                }
                Ok(())
            }
            // No decoder at all: like ProRes, FFmpeg's or nobody's.
            Err(MediaError::NoDecoder { .. }) => self.fallback(req, ctx, None),
            Err(e) if hands_over(&e) => self.fallback(req, ctx, Some(e)),
            Err(e) => Err(audio::from_media(e, req.input, req.output)),
        }
    }
}

/// A picture codec Media Foundation has no decoder for, whatever is
/// installed.
fn undecodable(codec: &VideoCodec) -> bool {
    matches!(codec, VideoCodec::Other(_))
}

/// The failures FFmpeg takes over when it is installed (R52): the picture
/// needs a Store extension to be read, or HEVC one to be written (FFmpeg's
/// HEVC encoder writes it then, R93), the sound has to be refused, Windows has no
/// decoder for the picture, or no Media Foundation at all.
fn hands_over(e: &MediaError) -> bool {
    matches!(
        e,
        MediaError::NeedsExtension { .. }
            | MediaError::NeedsExtensionToWrite { .. }
            | MediaError::Unsupported { .. }
            | MediaError::NeedsMediaFoundation
            | MediaError::NoDecoder { .. }
    )
}

/// The CRF libx264 gets for a quality step (R57): round(51 − 0.4·q), held
/// to 16..35; libx265 gets 4 more, as its scale runs that much lower for the
/// same look.
fn crf(q: u8, codec: VCodec) -> u32 {
    let base = (51.0 - 0.4 * f64::from(q)).round().clamp(16.0, 35.0) as u32;
    match codec {
        VCodec::H264 => base,
        VCodec::Hevc => base + 4,
    }
}

/// Sets the value after `flag` in `args`, adding the pair when it is
/// missing.
fn set_arg(args: &mut Vec<String>, flag: &str, value: String) {
    match args.iter().position(|a| a == flag) {
        Some(i) if i + 1 < args.len() => args[i + 1] = value,
        _ => args.extend([flag.to_owned(), value]),
    }
}

/// Takes `flag` and its value out of `args`.
fn drop_arg(args: &mut Vec<String>, flag: &str) {
    if let Some(i) = args.iter().position(|a| a == flag) {
        args.drain(i..(i + 2).min(args.len()));
    }
}

/// How FFmpeg writes a fallback job (R57): formats.toml's arguments for the
/// target, retuned to the job's video settings.
struct Fallback {
    /// The target with its FFmpeg arguments retuned.
    to: Format,
    /// The job's options with the limits and the sound's bitrate as FFmpeg's
    /// backend reads them.
    options: Options,
    /// Fit under's limit, checked once FFmpeg is done.
    fit: Option<Fit>,
}

/// Fit under on the fallback: the limit, and what is needed to say how far
/// down the file can go when FFmpeg's one pass lands over it (R45).
struct Fit {
    target: VideoTarget,
    limit: u64,
    /// The size model's estimate for the bitrate asked for.
    estimate: Option<u64>,
    /// The smallest file the size model allows.
    floor: u64,
}

impl Fallback {
    /// FFmpeg's arguments for `req` into `t`:
    /// - the resolution and frame rate limits as the backend's scale and fps
    ///   filters (never enlarging, both sides rounded down to even);
    /// - HEVC as FFmpeg's HEVC encoder (tagged hvc1 outside MKV, and an M4V
    ///   written by the MP4 muxer, as the iPod one has no HEVC), H.264 as
    ///   its H.264 encoder (`@hevc` and `@h264`, made real by the FFmpeg
    ///   backend: libx265 and libx264, or Windows' own, R11);
    /// - a quality step as a CRF ([`crf`]); Original (or none chosen) as
    ///   formats.toml's own values for the preset;
    /// - Fit under as one pass at [`tune::fit_under`]'s answer: its bitrate,
    ///   its quality step, or, when the source already fits, the picture
    ///   copied;
    /// - the sound as Kiln's own path writes it, with or without Fit under
    ///   (R97): Keep copies it (`-c:a copy`) where the target holds it, and
    ///   otherwise it is written at the bitrate the size model counts
    ///   ([`tune::sound_kbps`]: the preset's for Keep, R95, or the one
    ///   chosen), never at Settings' saved audio default (R55). A file whose
    ///   headers Kiln can't read keeps formats.toml's sound.
    fn plan(req: &Request<'_>, t: VideoTarget) -> Result<Fallback> {
        let tune = quality::video_tune(&req.options.quality);
        let mut options = req.options.clone();
        let mut to = req.to.clone();
        let q = &mut options.quality;
        q.size = tune.max_lines.map_or(Limit::Original, Limit::Max);
        q.fps = tune.max_fps.map_or(Limit::Original, Limit::Max);
        q.audio_kbps = match tune.sound {
            SoundChoice::Kbps(k) => Some(k),
            SoundChoice::Keep | SoundChoice::Off => None,
        };
        let args = &mut to.ffmpeg;
        if tune.sound == SoundChoice::Off {
            // Left out, as Kiln's own path leaves it.
            drop_arg(args, "-c:a");
            drop_arg(args, "-b:a");
            args.push("-an".to_owned());
        }
        // R97: the sound as Kiln's own path writes it.
        let info = video::probe(req.input).ok();
        if let Some(info) = info.as_ref().filter(|_| tune.sound != SoundChoice::Off) {
            let copies = tune.sound == SoundChoice::Keep
                && info
                    .sound
                    .as_ref()
                    .is_some_and(|s| tune::can_copy_sound(t, s));
            if copies {
                set_arg(args, "-c:a", "copy".to_owned());
                drop_arg(args, "-b:a");
                q.audio_kbps = None;
            } else {
                let sound = tune::sound_kbps(t, info, &tune).round() as u32;
                if sound > 0 {
                    q.audio_kbps = Some(sound);
                }
            }
        }
        let mut fit = None;
        // Whether the picture written is HEVC, and whether it is copied.
        let mut hevc = tune.codec == VCodec::Hevc;
        let mut copied = false;
        let setting = match tune.fit_bytes {
            Some(limit) => {
                let info = video::probe(req.input).map_err(|e| from(e, req))?;
                let s = tune::fit_under(t, limit, &info, &tune).map_err(|e| from(e, req))?;
                let floor = match tune::fit_under(t, 0, &info, &tune) {
                    Err(MediaError::TooSmall { min_bytes, .. }) => min_bytes,
                    _ => 0,
                };
                fit = Some(Fit {
                    target: t,
                    limit,
                    estimate: tune::estimate_bytes(t, &s, &info, &tune),
                    floor,
                });
                let copies = s == VideoSetting::Original
                    && tune::default_setting(t, &info, &tune) == VideoSetting::Original;
                if copies {
                    // The source already fits: copy the picture as it is.
                    copied = true;
                    hevc = info.codec == VideoCodec::Hevc;
                    set_arg(args, "-c:v", "copy".to_owned());
                    for flag in ["-crf", "-preset", "-pix_fmt", "-tag:v", "-b:v"] {
                        drop_arg(args, flag);
                    }
                    q.size = Limit::Original;
                    q.fps = Limit::Original;
                    q.preset = Preset::High;
                    None
                } else {
                    Some(s)
                }
            }
            None => tune.setting,
        };
        let encodes_hevc = tune.codec == VCodec::Hevc && !copied;
        if encodes_hevc {
            set_arg(args, "-c:v", crate::ffmpeg::HEVC.to_owned());
        }
        if hevc && t != VideoTarget::Mkv {
            set_arg(args, "-tag:v", "hvc1".to_owned());
        }
        if hevc && t == VideoTarget::M4v {
            // The iPod muxer FFmpeg picks for .m4v has no HEVC.
            set_arg(args, "-f", "mp4".to_owned());
        }
        match setting {
            None | Some(VideoSetting::Original) => {
                // formats.toml's CRF is libx264's; libx265's runs lower.
                if encodes_hevc {
                    if let Some(i) = args.iter().position(|a| a == "-crf") {
                        if let Some(v) = args.get(i + 1).and_then(|v| v.parse::<u32>().ok()) {
                            args[i + 1] = (v + 4).to_string();
                        }
                    }
                }
            }
            Some(VideoSetting::Quality(qv)) => {
                set_arg(args, "-crf", crf(qv, tune.codec).to_string());
                // The step is the whole choice: no preset moves it.
                q.preset = Preset::High;
            }
            Some(VideoSetting::Kbps(k)) => {
                drop_arg(args, "-crf");
                set_arg(args, "-b:v", format!("{k}k"));
                q.preset = Preset::High;
            }
        }
        Ok(Fallback { to, options, fit })
    }
}

impl Fit {
    /// R45 after FFmpeg's one pass: a file over the limit is removed, and
    /// the job is `TooSmall`, with the floor the size model gives once
    /// corrected by how far this pass landed from its estimate.
    fn check(&self, output: &Path) -> Result<()> {
        let size = std::fs::metadata(output)
            .map_err(|e| Error::io(output, e))?
            .len();
        if size <= self.limit {
            return Ok(());
        }
        let _ = std::fs::remove_file(output);
        let ratio = match self.estimate {
            Some(e) if e > 0 => size as f64 / e as f64,
            _ => 1.0,
        };
        let corrected = (self.floor as f64 * ratio).ceil() as u64;
        Err(Error::Media(
            MediaError::TooSmall {
                format: self.target.name().to_string(),
                min_bytes: corrected.max(self.limit + 1),
            }
            .to_string(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Make smaller

/// The sound's bitrate when Make smaller has to encode it to reach a size:
/// the Small preset's AAC for MP4, MOV and M4V, its Opus for MKV.
fn small_sound_kbps(t: VideoTarget) -> u32 {
    if t == VideoTarget::Mkv {
        64
    } else {
        96
    }
}

/// "This file is already as small as Kiln can make it."
fn as_small_as_it_gets() -> Error {
    Error::NothingToDo("This file is already as small as Kiln can make it.".to_owned())
}

/// Make smaller for a video (spec 3.2): `input`, an MP4, MOV, M4V or MKV,
/// written again in its own container into `work`, through Kiln's own
/// engine on the graphics card as conversions are (no FFmpeg, ENG-07).
/// Returns the file written there.
///
/// With `opts.tool.max_bytes`, the picture gets the bitrate Fit under finds
/// for the size, and a first pass that lands over it is encoded again with
/// the size model corrected by how far it missed (R45). The sound is copied
/// when the container holds it, unless that leaves the size out of reach:
/// then it is encoded at the Small preset's bitrate. `picture_lines` caps
/// the picture's short side, and `keep_sound: Some(false)` leaves the sound
/// out. Without a size the picture is encoded at the Small step.
///
/// Never a file over the size or one as large as the original (R4): a size
/// the file can't reach saves nothing and is [`Error::OutOfReach`] with the
/// smallest Kiln can make; a file already under it is `NothingToDo`, unless
/// a smaller picture or no sound was asked for too. A size for a file whose
/// length isn't known is turned down before anything is encoded.
/// What Make smaller works out for a video before anything is encoded:
/// its container, the original's size, what the file holds, the tuning,
/// the size asked and the size aimed at (never the original's or over).
struct SmallerPlan {
    target: VideoTarget,
    original: u64,
    info: VideoInfo,
    tune: VideoTune,
    max: Option<u64>,
    aim: Option<u64>,
}

/// [`smaller`]'s plan for `input`, turning down what it turns down before
/// encoding; `out` is where its file would go, for the errors' words.
fn smaller_plan(input: &Path, ext: &str, opts: &Options, out: &Path) -> Result<SmallerPlan> {
    let target = target_of(ext).ok_or_else(|| Error::UnknownFormat(ext.to_owned()))?;
    let original = std::fs::metadata(input)
        .map_err(|e| Error::io(input, e))?
        .len();
    let media = |e: MediaError| audio::from_media(e, input, out);
    let info = video::probe(input).map_err(media)?;
    let tool = &opts.tool;
    let sound_off = tool.keep_sound == Some(false);
    let has_sound = info.sound.is_some() || info.sound_unreadable.is_some();
    // Settings' codec and encoder, and nothing else of Settings' video: the
    // size, the picture's lines and the sound are Make smaller's own.
    let mut tune = VideoTune {
        codec: opts.quality.video.codec,
        encoder: opts.quality.video.encoder,
        max_lines: tool.picture_lines,
        sound: if sound_off {
            SoundChoice::Off
        } else {
            SoundChoice::Keep
        },
        ..VideoTune::default()
    };
    let max = tool.max_bytes;
    if let Some(max) = max.filter(|&m| m >= original) {
        // A video under the size is left alone, unless a smaller picture or
        // no sound was asked for too.
        let fewer_lines = tool
            .picture_lines
            .is_some_and(|l| l < info.width.min(info.height));
        if !fewer_lines && !(sound_off && has_sound) {
            return Err(Error::NothingToDo(format!(
                "This file is already smaller than {}.",
                crate::suggest::about(max)
            )));
        }
    }
    // A size needs the file's length: without it Fit under can't pick a
    // bitrate, and the whole file would be encoded for nothing.
    let known = info.duration.is_some_and(|d| d.is_finite() && d > 0.0);
    if max.is_some() && !known {
        return Err(crate::tools::no_duration());
    }
    // Never larger than the original, whatever the size asked.
    let aim = max.map(|m| m.min(original.saturating_sub(1)));
    tune.fit_bytes = aim;
    if let Some(aim) = aim {
        // The sound copied as it is may leave too little for the picture;
        // the Small preset's bitrate may not.
        let copied_too_big = matches!(
            tune::fit_under(target, aim, &info, &tune),
            Err(MediaError::TooSmall { .. })
        );
        if copied_too_big && tune.sound == SoundChoice::Keep && has_sound {
            let encoded = VideoTune {
                sound: SoundChoice::Kbps(small_sound_kbps(target)),
                ..tune
            };
            if tune::sound_kbps(target, &info, &encoded) < tune::sound_kbps(target, &info, &tune) {
                tune = encoded;
            }
        }
    }
    Ok(SmallerPlan {
        target,
        original,
        info,
        tune,
        max,
        aim,
    })
}

/// How big [`smaller`] would make the video at `input`, an MP4, MOV, M4V
/// or MKV, for `opts.tool.max_bytes`: the Make smaller dialog's estimate,
/// writing nothing. The picture's bitrate is what Fit under picks for the
/// size; where the size holds the best the picture can be, that quality
/// step's estimate is scaled by a short sample encode of the file (spec
/// 3.3), or used as it is when no sample can be made. Its refusals are
/// `smaller`'s: a size out of reach is [`Error::OutOfReach`] with the
/// smallest Kiln can make, a file already under it `NothingToDo`.
/// Slow (the sample starts the encoder): never on the window's thread.
pub(crate) fn smaller_size(
    input: &Path,
    ext: &str,
    opts: &Options,
    cancel: &crate::converter::CancelToken,
) -> Result<u64> {
    let gone = || -> Result<()> {
        if cancel.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    };
    gone()?;
    let SmallerPlan {
        target,
        original,
        info,
        tune,
        max,
        aim,
    } = smaller_plan(input, ext, opts, input)?;
    let (Some(max), Some(aim)) = (max, aim) else {
        return Err(Error::ToolUnavailable(
            "Make smaller needs a size for its estimate.".to_owned(),
        ));
    };
    let setting = match tune::fit_under(target, aim, &info, &tune) {
        Ok(s) => s,
        Err(MediaError::TooSmall { min_bytes, .. }) => {
            return Err(if aim < max || min_bytes >= original {
                as_small_as_it_gets()
            } else {
                Error::OutOfReach {
                    max_bytes: max,
                    smallest: min_bytes,
                }
            });
        }
        Err(e) => return Err(audio::from_media(e, input, input)),
    };
    let chosen = VideoTune {
        setting: Some(setting),
        ..tune
    };
    // Only a quality step's size depends on the picture; a bitrate is what
    // it is. No sample (none can be made, or the encoder failed) leaves the
    // table's estimate.
    let measured = match setting {
        VideoSetting::Quality(_) | VideoSetting::Original => {
            gone()?;
            match video::sample(input, &chosen, &|| cancel.is_cancelled()) {
                Ok(m) => m,
                Err(MediaError::Cancelled) => return Err(Error::Cancelled),
                Err(_) => None,
            }
        }
        VideoSetting::Kbps(_) => None,
    };
    gone()?;
    let bytes = tune::estimate_measured(target, &setting, &info, &chosen, measured.as_ref())
        .ok_or_else(crate::tools::no_duration)?;
    // Never said to be over the size asked, or the original's.
    Ok(bytes.min(aim))
}

pub(crate) fn smaller(
    input: &Path,
    ext: &str,
    opts: &Options,
    work: &Path,
    ctx: &mut Context<'_>,
) -> Result<std::path::PathBuf> {
    let tmp = work.join(format!("out.{ext}"));
    let media = |e: MediaError| audio::from_media(e, input, &tmp);
    let SmallerPlan {
        target,
        original,
        tune,
        max,
        aim,
        ..
    } = smaller_plan(input, ext, opts, &tmp)?;
    let cancel = ctx.cancel;
    let done = {
        let progress = &mut ctx.progress;
        video::convert(
            input,
            &tmp,
            target,
            kiln_media::Preset::Small,
            &tune,
            &mut |f| progress(Progress::Fraction(f)),
            &|| cancel.is_cancelled(),
        )
    };
    match done {
        Ok(report) => {
            if report.fell_back {
                (ctx.progress)(Progress::Note(ON_THE_PROCESSOR));
            }
        }
        Err(MediaError::TooSmall { min_bytes, .. }) => {
            return Err(match (max, aim) {
                // Out of reach only because of the original's own size.
                (Some(m), Some(a)) if a < m || min_bytes >= original => as_small_as_it_gets(),
                (Some(m), _) => Error::OutOfReach {
                    max_bytes: m,
                    smallest: min_bytes,
                },
                (None, _) => as_small_as_it_gets(),
            });
        }
        Err(e) => return Err(media(e)),
    }
    // Never a file over the size asked, or one as large as the original.
    let written = std::fs::metadata(&tmp)
        .map_err(|e| Error::io(&tmp, e))?
        .len();
    let over = max.is_some_and(|m| written > m);
    if over || written >= original {
        let _ = std::fs::remove_file(&tmp);
        return Err(match max {
            Some(m) if over && written < original => Error::OutOfReach {
                max_bytes: m,
                smallest: written,
            },
            _ => as_small_as_it_gets(),
        });
    }
    Ok(tmp)
}

/// A kiln-media failure on the fallback's way in, as Kiln's error.
fn from(e: MediaError, req: &Request<'_>) -> Error {
    audio::from_media(e, req.input, req.output)
}

/// Whether FFmpeg is installed: the configured path, the copy that comes
/// with Kiln, or on PATH (R10).
pub(crate) fn ffmpeg_installed(opts: &Options) -> bool {
    crate::ffmpeg::installed(opts)
}

// ---------------------------------------------------------------------------
// The pool

/// Where a video job runs (R53).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lane {
    /// The processor pool: an encode on the processor, or a job FFmpeg
    /// takes. It weighs the whole processor budget.
    Processor,
    /// The processor pool, for a copy that encodes nothing.
    Copy,
    /// The one slot of the graphics card with this LUID, whose hardware
    /// encoder writes the picture.
    Card(u64),
}

/// Where converting `input` to `to` runs: on the graphics card whose
/// encoder [`video::encoder_adapter`] picks when the picture is encoded
/// there, else in the processor pool. Reads the file's headers, and the
/// first call per codec starts Media Foundation's encoders (up to about
/// 400 ms), so never call this on the window's thread.
pub(crate) fn lane(input: &Path, to: &str, opts: &Options) -> Lane {
    let Some(t) = target_of(to) else {
        return Lane::Processor;
    };
    // Without Media Foundation the job is FFmpeg's (R59).
    if !audio::media_foundation_available() {
        return Lane::Processor;
    }
    let Ok(info) = video::probe(input) else {
        return Lane::Processor;
    };
    if undecodable(&info.codec) {
        return Lane::Processor;
    }
    let tune = quality::video_tune(&opts.quality);
    let card = || video::encoder_adapter(tune.codec, tune.encoder);
    // Fit under may encode even when its first answer is a copy: a copy
    // that lands over the limit is encoded again (R45), so the job waits
    // for its card (R53). Its estimate reads only what is known of the card
    // (R82), so the card is asked first.
    if tune.fit_bytes.is_some() {
        return card().map_or(Lane::Processor, Lane::Card);
    }
    let Ok(setting) = tune::chosen_setting(t, audio::preset(opts.quality.preset), &tune, &info)
    else {
        return Lane::Processor;
    };
    let chosen = VideoTune {
        setting: Some(setting),
        ..tune
    };
    // A copy never starts the card's encoder.
    if tune::can_copy_video(t, &info, &chosen) {
        return Lane::Copy;
    }
    card().map_or(Lane::Processor, Lane::Card)
}

// ---------------------------------------------------------------------------
// Facts

/// What Kiln knows about `path`, a file of format `f`, given what
/// kiln-media read of its `sound` (see [`audio::media_info`]). The picture
/// of MP4, MOV, M4V and MKV is read from the file's own headers, so it needs
/// no FFprobe; the length is the picture's alone (a fragmented MP4 has none,
/// R22). Anything else, or a file whose picture can't be read that way, is
/// [`audio::media_info`]'s. A video with more than one sound track also says
/// whether Kiln's own engines would read a later one ([`later_sound`]).
pub(crate) fn media_info(
    f: &Format,
    path: &Path,
    sound: Option<kiln_media::Result<SourceInfo>>,
    ffprobe: impl FnOnce() -> Option<MediaInfo>,
) -> Option<MediaInfo> {
    let mut info = if reads_picture(f) {
        match video::probe(path) {
            Ok(v) => Some(from_video(&v, video::track_counts(path))),
            Err(_) => audio::media_info(f, sound, ffprobe),
        }
    } else {
        audio::media_info(f, sound, ffprobe)
    }?;
    if f.kind == Kind::Video {
        info.later_sound = later_sound(path, info.sound_tracks);
    }
    Some(info)
}

/// Whether Kiln's own sound and video engines read a sound track other than
/// the first of the video at `path`, which holds `sound_tracks` of them:
/// `MediaInfo::later_sound`. Only a video with more than one has a first to
/// say; for it, this is false only when the engines are known to read the
/// first (`kiln_media::sym::takes_first_sound`), so a file they can't be
/// told of is counted with those that read a later one, and the note about
/// the first is left out. Reads the file's headers: not for the window's
/// thread.
pub(crate) fn later_sound(path: &Path, sound_tracks: u16) -> bool {
    sound_tracks > 1 && kiln_media::sym::takes_first_sound(path) != Some(true)
}

/// `MediaInfo` from what kiln-media found in a video, and `tracks`, how many
/// sound and subtitle tracks its headers list (`None` when they can't be
/// counted).
pub(crate) fn from_video(v: &VideoInfo, tracks: Option<video::TrackCounts>) -> MediaInfo {
    let tracks = tracks.unwrap_or_default();
    let mut info = MediaInfo {
        duration: v.duration,
        width: Some(v.width),
        height: Some(v.height),
        fps: v.fps,
        video_codec: Some(v.codec.clone()),
        rotation: v.rotation,
        hdr: Some(v.hdr),
        bit_depth: Some(v.bit_depth),
        video_kbps: v.video_kbps,
        needs_extension: video::missing_extension(&v.codec),
        lead_in: v.lead_in,
        sound_tracks: tracks.sound,
        subtitle_tracks: tracks.subtitles,
        ..MediaInfo::default()
    };
    if let Some(sound) = &v.sound {
        audio::fill(&mut info, sound);
        // The length is the picture's, even when the sound says one.
        info.duration = v.duration;
    } else if let Some(codec) = &v.sound_unreadable {
        info.has_audio = true;
        info.codec = Some(codec.clone());
    }
    info
}

/// The frames the black-bar check looks at.
const BAR_FRAMES: usize = 5;
/// How far into the video they are spread, in seconds.
const BAR_SECONDS: f64 = 3.0;
/// The brightness at or under which a line counts as black, for 8-bit
/// pictures: FFmpeg's `cropdetect=limit=24`, which the check for the other
/// video types runs.
const BLACK_LIMIT: u32 = 24;

/// Which black bars `path` has: [`BAR_FRAMES`] frames spread across the
/// first [`BAR_SECONDS`] from its first frame, decoded in memory, judged as
/// FFmpeg's cropdetect judges them (a line is black when its average
/// brightness is at most 24 in 8 bits, 96 in 10) and by [`Bars::of`]'s 4%
/// rule. Only each frame's lit edges are kept, never the frame. The bars are
/// as the picture is shown, so a turned picture's sides are its top and
/// bottom. `None` when the file can't be decoded, or `cancel` or `limit`
/// comes first.
pub(crate) fn bars(path: &Path, limit: Duration, cancel: &dyn Fn() -> bool) -> Option<Bars> {
    let start = Instant::now();
    let over = || cancel() || start.elapsed() >= limit;
    let file = File::open(path).ok()?;
    let mut reader = Reader::open(file, ReadMode::Decoded { d3d: None }).ok()?;
    let info = reader.info().clone();
    let span = info
        .duration
        .filter(|d| d.is_finite() && *d > 0.0)
        .map_or(BAR_SECONDS, |d| d.min(BAR_SECONDS));
    let mut seen: Vec<Lit> = Vec::with_capacity(BAR_FRAMES);
    // Times count from the first frame: a file may start anywhere.
    let mut first_pts = None;
    while seen.len() < BAR_FRAMES {
        if over() {
            return None;
        }
        let Ok(Some(frame)) = reader.next_frame() else {
            break;
        };
        let from = *first_pts.get_or_insert(frame.pts);
        let at = frame.pts.saturating_sub(from) as f64 / TIMESCALE as f64;
        if at > BAR_SECONDS {
            break;
        }
        if at + 1e-6 >= span * seen.len() as f64 / BAR_FRAMES as f64 {
            seen.push(lit(&frame.luma().ok()?));
        }
    }
    let frame = seen.first()?.size;
    let bars = crop_of(&seen).map_or(Bars::None, |crop| Bars::of(frame, crop));
    Some(if info.rotation % 180 == 90 {
        match bars {
            Bars::Sides => Bars::AboveBelow,
            Bars::AboveBelow => Bars::Sides,
            other => other,
        }
    } else {
        bars
    })
}

/// What the bar check keeps of one frame: its size, and the first and last
/// lines that aren't black, as (left, top, right, bottom), inclusive.
/// `None` edges when every line is black.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Lit {
    size: (u32, u32),
    edges: Option<(u32, u32, u32, u32)>,
}

/// The lit edges of a frame, by cropdetect's rule.
pub(crate) fn lit(f: &Luma) -> Lit {
    let (w, h) = (f.width, f.height);
    let limit = u64::from(BLACK_LIMIT) * (u64::from(f.max) + 1) / 256;
    let row_lit =
        |y: u32| f.row(y).iter().map(|&v| u64::from(v)).sum::<u64>() > limit * u64::from(w);
    let col_lit = |x: u32| {
        (0..h).map(|y| u64::from(f.row(y)[x as usize])).sum::<u64>() > limit * u64::from(h)
    };
    let edges = (0..h).find(|&y| row_lit(y)).map(|top| {
        let bottom = (0..h).rev().find(|&y| row_lit(y)).unwrap_or(top);
        let left = (0..w).find(|&x| col_lit(x)).unwrap_or(0);
        let right = (0..w).rev().find(|&x| col_lit(x)).unwrap_or(w - 1);
        (left, top, right, bottom)
    });
    Lit {
        size: (w, h),
        edges,
    }
}

/// The size of the picture inside the black lines around it, across all of
/// `seen` (each the size of the first), rounded down to even as
/// cropdetect's `round=2` does. `None` when every line is black.
pub(crate) fn crop_of(seen: &[Lit]) -> Option<(u32, u32)> {
    let size = seen.first()?.size;
    let (l, t, r, b) = seen
        .iter()
        .filter(|s| s.size == size)
        .filter_map(|s| s.edges)
        .reduce(|(l, t, r, b), (l2, t2, r2, b2)| (l.min(l2), t.min(t2), r.max(r2), b.max(b2)))?;
    let even = |n: u32| n & !1;
    Some((even(r.saturating_sub(l) + 1), even(b.saturating_sub(t) + 1)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Registry;

    /// A `w` x `h` 8-bit frame, black outside the rows `top..bottom` and
    /// columns `left..right`, which are mid-grey.
    fn framed(w: u32, h: u32, left: u32, top: u32, right: u32, bottom: u32) -> Luma {
        let mut samples = vec![16u16; (w * h) as usize];
        for y in top..bottom {
            for x in left..right {
                samples[(y * w + x) as usize] = 128;
            }
        }
        Luma {
            width: w,
            height: h,
            max: 255,
            samples,
        }
    }

    /// `crop_of` over whole frames.
    fn crop(frames: &[Luma]) -> Option<(u32, u32)> {
        crop_of(&frames.iter().map(lit).collect::<Vec<_>>())
    }

    #[test]
    fn an_edit_lists_lead_in_reaches_the_sheet() {
        // R69: the facts carry it, so the sheet and the estimate know MKV
        // can't copy such a picture, as the job does.
        let picture = MediaInfo {
            width: Some(640),
            height: Some(360),
            fps: Some(24.0),
            video_codec: Some(VideoCodec::H264),
            duration: Some(20.0),
            lead_in: Some(8.08),
            ..MediaInfo::default()
        };
        let v = quality::video_info(&picture).unwrap();
        assert_eq!(v.lead_in, Some(8.08));
        assert_eq!(from_video(&v, None).lead_in, Some(8.08));
        let none = VideoTune::default();
        let copies = |t| tune::can_copy_video(t, &v, &none);
        assert!(!copies(VideoTarget::Mkv));
        assert!(copies(VideoTarget::Mp4));
    }

    #[test]
    fn the_route_and_native_pair_agree() {
        let r = Registry::builtin();
        for (from, to, steps) in r.pairs() {
            let ours = steps.iter().all(|s| s.backend == VIDEO);
            assert_eq!(
                ours,
                native_pair(from, to),
                "{} -> {}: {steps:?}",
                from.ext,
                to.ext
            );
            if ours {
                assert_eq!(steps.len(), 1);
                assert!(target_of(&to.ext).is_some());
            }
        }
    }

    #[test]
    fn letterbox_lines_are_found_like_cropdetect() {
        // 64x48 with 6 black lines above and below: 36 lines of picture.
        let f = framed(64, 48, 0, 6, 64, 42);
        assert_eq!(crop(std::slice::from_ref(&f)), Some((64, 36)));
        assert_eq!(Bars::of((64, 48), (64, 36)), Bars::AboveBelow);
        // The picture across frames is their union.
        let g = framed(64, 48, 0, 2, 64, 42);
        assert_eq!(crop(&[f.clone(), g]), Some((64, 40)));
        // An odd extent rounds down to even.
        assert_eq!(crop(&[framed(64, 48, 3, 0, 64, 48)]), Some((60, 48)));
        // All black is no picture at all, and adds nothing to a union.
        let black = framed(64, 48, 0, 0, 0, 0);
        assert_eq!(crop(std::slice::from_ref(&black)), None);
        assert_eq!(crop(&[black, f]), Some((64, 36)));
        // What is kept of a frame is its size and four edges.
        assert_eq!(
            lit(&framed(64, 48, 0, 6, 64, 42)),
            Lit {
                size: (64, 48),
                edges: Some((0, 6, 63, 41))
            }
        );
    }

    #[test]
    fn a_fit_under_job_waits_for_its_card_even_when_it_would_copy() {
        // Item 6 of the final review, R53: Fit under may encode after all
        // (R45), so it takes its card's queue; a plain copy never does.
        let input =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../kiln-media/tests/data/video/h264.mp4");
        let mut opts = Options::default();
        let plain = if audio::media_foundation_available() {
            Lane::Copy
        } else {
            Lane::Processor
        };
        assert_eq!(lane(&input, "mov", &opts), plain);
        opts.quality.video.fit_bytes = Some(4_000_000_000);
        let want = match video::encoder_adapter(VCodec::H264, Default::default()) {
            Some(luid) if audio::media_foundation_available() => Lane::Card(luid),
            _ => Lane::Processor,
        };
        assert_eq!(lane(&input, "mov", &opts), want);
    }

    #[test]
    fn the_fallback_keeps_sound_as_kilns_own_path_does() {
        // R97: without Fit under too, Keep copies sound the target holds
        // and writes the rest at the preset's bitrate (R95), the same the
        // size model counts.
        let data = |name: &str| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../kiln-media/tests/data/video")
                .join(name)
        };
        let presets = [
            (Preset::Small, 96, 64),
            (Preset::High, 160, 96),
            (Preset::Best, 192, 160),
        ];
        let counted = |preset| VideoTune {
            sound_preset: audio::preset(preset),
            ..VideoTune::default()
        };
        // ProRes with AAC into MP4: the sound is copied at every preset.
        for (preset, _, _) in presets {
            let a = fallback_args_for(&data("prores.mov"), "mp4", VideoTune::default(), preset);
            assert_eq!(arg(&a, "-c:a"), Some("copy"), "{preset:?}");
            assert_eq!(arg(&a, "-b:a"), None, "{preset:?}");
        }
        // Opus in an MKV can't go into MP4: AAC at the preset's bitrate.
        let opus = data("h264.mkv");
        let info = video::probe(&opus).unwrap();
        for (preset, aac, _) in presets {
            let a = fallback_args_for(&opus, "mp4", VideoTune::default(), preset);
            assert_eq!(arg(&a, "-c:a"), Some("aac"), "{preset:?}");
            assert_eq!(
                arg(&a, "-b:a"),
                Some(format!("{aac}k").as_str()),
                "{preset:?}"
            );
            let got = tune::sound_kbps(VideoTarget::Mp4, &info, &counted(preset));
            assert_eq!(got, aac as f64, "{preset:?}");
        }
        // PCM into MP4, and into MKV as Opus, from files FFmpeg makes.
        if crate::process::find_on_path(&std::env::var_os("PATH").unwrap_or_default(), "ffmpeg")
            .is_none()
        {
            eprintln!("the_fallback_keeps_sound_as_kilns_own_path_does: PCM skipped, no FFmpeg");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        for (ext, to, opus) in [("mkv", "mp4", false), ("mov", "mkv", true)] {
            let pcm = dir.path().join(format!("pcm.{ext}"));
            let made = std::process::Command::new("ffmpeg")
                .args(["-v", "error", "-y", "-i"])
                .arg(data("h264.mp4"))
                .args(["-c:v", "copy", "-c:a", "pcm_s16le"])
                .arg(&pcm)
                .status()
                .unwrap();
            assert!(made.success());
            let info = video::probe(&pcm).unwrap();
            assert!(info.sound.is_some(), "Kiln reads the PCM in the {ext}");
            let t = target_of(to).unwrap();
            for (preset, aac, op) in presets {
                let want = if opus { op } else { aac };
                let a = fallback_args_for(&pcm, to, VideoTune::default(), preset);
                let said = format!("{want}k");
                assert_eq!(arg(&a, "-b:a"), Some(said.as_str()), "{to} {preset:?}");
                let tuned = counted(preset);
                assert_eq!(
                    tune::sound_kbps(t, &info, &tuned),
                    want as f64,
                    "{to} {preset:?}"
                );
                // The estimate grows by that bitrate over the length.
                let q = VideoSetting::Quality(65);
                let silent = VideoInfo {
                    sound: None,
                    ..info.clone()
                };
                let grows = tune::estimate_bytes(t, &q, &info, &tuned).unwrap()
                    - tune::estimate_bytes(t, &q, &silent, &tuned).unwrap();
                let want_bytes = want as f64 * 1000.0 / 8.0 * info.duration.unwrap();
                assert!((grows as f64 - want_bytes).abs() <= 1.0, "{to} {preset:?}");
            }
        }
    }

    #[test]
    fn hevc_with_no_encoder_goes_to_ffmpegs_hevc_encoder() {
        // R93: no HEVC encoder on this PC is FFmpeg's job when FFmpeg is
        // there, as reading is (R52): its HEVC encoder, made real by the
        // FFmpeg backend (libx265 or hevc_mf, R11).
        assert!(hands_over(&MediaError::NeedsExtensionToWrite {
            codec: "HEVC",
            product: "9N4WGH0Z6VHQ"
        }));
        let hevc = VideoTune {
            codec: VCodec::Hevc,
            ..VideoTune::default()
        };
        let a = fallback_args("h264.mp4", "mp4", hevc);
        assert_eq!(arg(&a, "-c:v"), Some(crate::ffmpeg::HEVC));
    }

    #[test]
    fn quality_steps_become_crf() {
        // round(51 - 0.4 q), held to 16..35; HEVC 4 more.
        assert_eq!(crf(45, VCodec::H264), 33);
        assert_eq!(crf(65, VCodec::H264), 25);
        assert_eq!(crf(90, VCodec::H264), 16);
        assert_eq!(crf(100, VCodec::H264), 16);
        assert_eq!(crf(20, VCodec::H264), 35);
        assert_eq!(crf(0, VCodec::H264), 35);
        assert_eq!(crf(65, VCodec::Hevc), 29);
    }

    #[test]
    fn arguments_are_set_and_dropped_in_place() {
        let mut a: Vec<String> = ["-c:v", "libx264", "-crf", "20", "-c:a", "aac"]
            .map(String::from)
            .to_vec();
        set_arg(&mut a, "-crf", "33".into());
        set_arg(&mut a, "-b:v", "500k".into());
        drop_arg(&mut a, "-c:a");
        assert_eq!(a, ["-c:v", "libx264", "-crf", "33", "-b:v", "500k"]);
        drop_arg(&mut a, "-tag:v");
        assert_eq!(a.len(), 6);
    }

    /// FFmpeg's arguments for a fallback job from kiln-media's test video
    /// `name` into `to`, with `video` as the job's settings, as FFmpeg's
    /// backend would give them (after the quality tuning it applies).
    fn fallback_args(name: &str, to: &str, video: VideoTune) -> Vec<String> {
        let input = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../kiln-media/tests/data/video")
            .join(name);
        fallback_args_for(&input, to, video, Preset::High)
    }

    /// FFmpeg's arguments for `input` into `to` with `video` at `preset`.
    fn fallback_args_for(input: &Path, to: &str, video: VideoTune, preset: Preset) -> Vec<String> {
        let r = Registry::builtin();
        let input = input.to_path_buf();
        let mut options = Options::default();
        options.quality.video = video;
        options.quality.preset = preset;
        let from = r.format_of(&input).unwrap();
        let req = Request {
            input: &input,
            output: Path::new("unused"),
            from,
            to: r.format(to).unwrap(),
            options: &options,
        };
        let fb = Fallback::plan(&req, target_of(to).unwrap()).unwrap();
        quality::tune(&fb.to.ffmpeg, &fb.options.quality)
    }

    /// The value after `flag`.
    fn arg<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
        let i = args.iter().position(|a| a == flag)?;
        args.get(i + 1).map(String::as_str)
    }

    #[test]
    fn sound_off_leaves_the_sound_out_of_ffmpeg_s_file_too() {
        let off = VideoTune {
            sound: SoundChoice::Off,
            ..VideoTune::default()
        };
        for to in ["mp4", "mkv"] {
            let a = fallback_args("h264.mp4", to, off);
            assert!(a.iter().any(|x| x == "-an"), "{to}: {a:?}");
            assert_eq!(arg(&a, "-c:a"), None, "{to}: {a:?}");
            assert_eq!(arg(&a, "-b:a"), None, "{to}: {a:?}");
        }
    }

    #[test]
    fn hevc_into_m4v_uses_the_mp4_muxer() {
        let hevc = VideoTune {
            setting: Some(VideoSetting::Quality(60)),
            codec: VCodec::Hevc,
            ..VideoTune::default()
        };
        let a = fallback_args("h264.mp4", "m4v", hevc);
        assert_eq!(arg(&a, "-c:v"), Some(crate::ffmpeg::HEVC));
        assert_eq!(arg(&a, "-tag:v"), Some("hvc1"));
        assert_eq!(arg(&a, "-f"), Some("mp4"));
        // H.264 keeps FFmpeg's own choice for .m4v, and MKV has no tag.
        let a = fallback_args("h264.mp4", "m4v", VideoTune::default());
        assert_eq!(arg(&a, "-f"), None);
        let a = fallback_args("h264.mp4", "mkv", hevc);
        assert_eq!((arg(&a, "-tag:v"), arg(&a, "-f")), (None, None));
    }

    #[test]
    fn fit_under_a_source_already_fits_copies_it() {
        // 67 KB of H.264 and AAC under 1 MB: the picture and the sound are
        // copied, with nothing left of the encoder's settings.
        let fit = |limit, sound| VideoTune {
            fit_bytes: Some(limit),
            sound,
            ..VideoTune::default()
        };
        let a = fallback_args("h264.mp4", "mov", fit(1_000_000, SoundChoice::Keep));
        assert_eq!(arg(&a, "-c:v"), Some("copy"));
        assert_eq!(arg(&a, "-c:a"), Some("copy"));
        for flag in ["-crf", "-preset", "-pix_fmt", "-b:a", "-b:v"] {
            assert_eq!(arg(&a, flag), None, "{flag}: {a:?}");
        }
        // A sound bitrate asked for is written; the picture still copied.
        let a = fallback_args("h264.mp4", "mov", fit(1_000_000, SoundChoice::Kbps(96)));
        assert_eq!(arg(&a, "-c:v"), Some("copy"));
        assert_eq!(
            (arg(&a, "-c:a"), arg(&a, "-b:a")),
            (Some("aac"), Some("96k"))
        );
        // Opus can't go into MOV: it is encoded at what the model counts
        // for it, the bitrate Kiln's own AAC encoder would be given for
        // stereo, not the source's (R90).
        let a = fallback_args("h264.mkv", "mov", fit(1_000_000, SoundChoice::Keep));
        assert_eq!(arg(&a, "-c:v"), Some("copy"));
        assert_eq!(
            (arg(&a, "-c:a"), arg(&a, "-b:a")),
            (Some("aac"), Some("160k"))
        );
    }

    #[test]
    fn fit_under_writes_the_sound_the_model_counted() {
        // ProRes can't be copied, and its picture is small enough that the
        // top quality step fits a generous limit: that step's CRF. Its AAC
        // fits MP4, so the sound is copied, as the model counted (R90, R97).
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../kiln-media/tests/data/video/prores.mov");
        let info = video::probe(&path).unwrap();
        let tune = VideoTune {
            fit_bytes: Some(1_000_000),
            ..VideoTune::default()
        };
        assert_eq!(
            tune::fit_under(VideoTarget::Mp4, 1_000_000, &info, &tune).unwrap(),
            VideoSetting::Quality(90)
        );
        let a = fallback_args("prores.mov", "mp4", tune);
        assert_eq!(arg(&a, "-c:a"), Some("copy"));
        assert_eq!(arg(&a, "-b:a"), None);
        let counted = tune::sound_kbps(VideoTarget::Mp4, &info, &tune);
        // The source's own bitrate, 128 where it doesn't say.
        let own = info.sound.as_ref().and_then(|s| s.kbps);
        assert_eq!(counted, own.map_or(128.0, f64::from));
        assert_eq!(arg(&a, "-crf"), Some("16"));
        assert_eq!(arg(&a, "-c:v"), Some(crate::ffmpeg::H264));
    }

    #[test]
    fn a_fallback_over_its_limit_is_removed_and_too_small() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("output.mp4");
        std::fs::write(&out, vec![0u8; 3_000]).unwrap();
        let fit = |limit| Fit {
            target: VideoTarget::Mp4,
            limit,
            estimate: Some(2_000),
            floor: 1_000,
        };
        assert!(fit(3_000).check(&out).is_ok());
        assert!(out.exists());
        // 3000 written for an estimate of 2000: the floor is 1.5 times the
        // model's, and never under the limit asked for.
        let err = fit(2_500).check(&out).unwrap_err();
        assert!(!out.exists());
        assert_eq!(
            err.to_string(),
            MediaError::TooSmall {
                format: "MP4".into(),
                min_bytes: 2_501
            }
            .to_string()
        );
    }

    #[test]
    fn ten_bit_lines_use_the_same_limit() {
        // 10-bit black (64) is under 96; a 10-bit grey (512) is not.
        let mut f = framed(32, 32, 0, 8, 32, 24);
        f.max = 1023;
        for v in f.samples.iter_mut() {
            *v = if *v == 16 { 64 } else { 512 };
        }
        assert_eq!(crop(&[f]), Some((32, 16)));
    }

    #[test]
    fn what_ffmpeg_takes_over() {
        assert!(hands_over(&MediaError::NeedsExtension {
            codec: "HEVC",
            product: "9N4WGH0Z6VHQ"
        }));
        assert!(hands_over(&MediaError::Unsupported {
            codec: "DTS".into()
        }));
        assert!(hands_over(&MediaError::NeedsMediaFoundation));
        assert!(hands_over(&MediaError::NoDecoder {
            codec: "MPEG-4".into()
        }));
        // An encoder's words are never read for meaning.
        assert!(!hands_over(&MediaError::Encoder(
            "Windows has no decoder for MPEG-4 video".into()
        )));
        assert!(!hands_over(&MediaError::Damaged {
            code: "video".into()
        }));
        assert!(!hands_over(&MediaError::NoHardwareEncoder {
            codec: "HEVC"
        }));
        assert!(!hands_over(&MediaError::Cancelled));
        assert!(undecodable(&VideoCodec::Other("ProRes".into())));
        assert!(!undecodable(&VideoCodec::Hevc));
    }
}
