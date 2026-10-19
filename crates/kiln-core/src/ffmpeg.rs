//! Audio and video through an external FFmpeg process.

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use crate::converter::{Context, Converter, Options, Progress, Request};
use crate::error::{Error, Result};
use crate::process;
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
        let duration = probe_duration(req.options, req.input, ctx)?;
        let mut args: Vec<OsString> = vec!["-map_metadata".into(), "0".into()];
        args.extend(["-sn", "-dn"].map(OsString::from));
        if req.to.kind == Kind::Audio {
            args.push("-vn".into());
        }
        args.extend(req.to.ffmpeg.iter().map(OsString::from));
        let job = Encode {
            inputs: &[req.input],
            pre_input: &[],
            args: &args,
            output: req.output,
            duration,
            span: (0.0, 1.0),
        };
        job.run(req.options, ctx).map(|_| ())
    }
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
    let mut cmd = Command::new(process::program(opts.engines.ffmpeg.as_deref(), "ffmpeg"));
    cmd.args(["-hide_banner", "-nostdin", "-nostats", "-loglevel", "info"])
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
    let mut cmd = Command::new(process::sibling(opts.engines.ffmpeg.as_deref(), "ffprobe"));
    cmd.args(["-v", "error", "-protocol_whitelist", "file"])
        .args(["-show_entries", entries])
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
