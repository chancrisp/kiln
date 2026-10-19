//! Audio and video through an external FFmpeg process.

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use crate::converter::{Context, Converter, Progress, Request};
use crate::error::Result;
use crate::process;
use crate::registry::Kind;

pub struct FfmpegConverter;

const ENGINE: &str = "FFmpeg";

/// FFmpeg treats "name:" prefixes as protocols. Saying "file:" explicitly
/// means a file called "http:x.mp4" or "concat:a|b" is just a file.
fn file_url(path: &Path) -> OsString {
    let mut s = OsString::from("file:");
    s.push(path.as_os_str());
    s
}

impl Converter for FfmpegConverter {
    fn id(&self) -> &'static str {
        "ffmpeg"
    }

    fn convert(&self, req: &Request<'_>, ctx: &mut Context<'_>) -> Result<()> {
        let configured = req.options.engines.ffmpeg.as_deref();
        let duration = probe_duration(configured, req.input, ctx);
        if let Err(crate::Error::Cancelled) = duration {
            return Err(crate::Error::Cancelled);
        }
        let duration_us = duration.ok().flatten().map(|s| s * 1_000_000.0);
        (ctx.progress)(match duration_us {
            Some(_) => Progress::Fraction(0.0),
            None => Progress::Indeterminate,
        });

        let mut cmd = Command::new(process::program(configured, "ffmpeg"));
        cmd.args(["-hide_banner", "-nostdin", "-nostats", "-loglevel", "error"])
            .args(["-progress", "pipe:1"])
            // Local files only: no network, even if a playlist asks for it.
            .args(["-protocol_whitelist", "file"])
            .arg("-i")
            .arg(file_url(req.input))
            .args(["-map_metadata", "0", "-sn", "-dn"]);
        if req.to.kind == Kind::Audio {
            cmd.arg("-vn");
        }
        cmd.args(&req.to.ffmpeg)
            // Never overwrite. The output path is fresh, so this only
            // guards against mistakes.
            .arg("-n")
            .arg(file_url(req.output));

        let progress = &mut *ctx.progress;
        process::run(ENGINE, cmd, ctx.cancel, |line| {
            let Some(total) = duration_us else { return };
            if let Some(v) = line.strip_prefix("out_time_us=") {
                if let Ok(done) = v.trim().parse::<f64>() {
                    progress(Progress::Fraction((done / total).clamp(0.0, 1.0) as f32));
                }
            }
        })
    }
}

/// Media duration in seconds via ffprobe, if it can be found and the file
/// has one. Only cancellation is an error here: without a duration the job
/// still runs, it just reports indeterminate progress.
fn probe_duration(
    configured: Option<&Path>,
    input: &Path,
    ctx: &mut Context<'_>,
) -> Result<Option<f64>> {
    let mut cmd = Command::new(process::sibling(configured, "ffprobe"));
    cmd.args(["-v", "error", "-protocol_whitelist", "file"])
        .args(["-show_entries", "format=duration"])
        .args(["-of", "default=noprint_wrappers=1:nokey=1"])
        .arg(file_url(input));
    let mut seconds = None;
    match process::run("FFprobe", cmd, ctx.cancel, |line| {
        if let Ok(s) = line.trim().parse::<f64>() {
            if s.is_finite() && s > 0.0 {
                seconds = Some(s);
            }
        }
    }) {
        Err(crate::Error::Cancelled) => Err(crate::Error::Cancelled),
        _ => Ok(seconds),
    }
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
