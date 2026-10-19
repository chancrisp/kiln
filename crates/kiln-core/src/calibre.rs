//! Kindle ebooks (MOBI, AZW3) through Calibre's ebook-convert, run as an
//! external process. Calibre is optional: without it, these pairs say it
//! isn't installed.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::converter::{Context, Converter, Progress, Request};
use crate::error::{Error, Result};
use crate::process;

pub struct CalibreConverter;

const ENGINE: &str = "Calibre";

impl Converter for CalibreConverter {
    fn id(&self) -> &'static str {
        "calibre"
    }

    fn convert(&self, req: &Request<'_>, ctx: &mut Context<'_>) -> Result<()> {
        (ctx.progress)(Progress::Indeterminate);

        // Settings and scratch files of its own, so a job never reads or
        // changes the user's Calibre preferences, and goes with the job.
        let work = req
            .output
            .parent()
            .expect("the output is inside the job's working folder");
        let config = work.join("calibre-config");
        let temp = work.join("calibre-temp");
        for dir in [&config, &temp] {
            fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
        }

        let mut cmd = Command::new(program(req.options.engines.calibre.as_deref()));
        // ebook-convert picks both types from the extensions. Paths are
        // absolute, so they can't be mistaken for options.
        cmd.arg(req.input)
            .arg(req.output)
            .env("CALIBRE_CONFIG_DIRECTORY", &config)
            .env("CALIBRE_TEMP_DIR", &temp)
            .env("CALIBRE_NO_NATIVE_FILEDIALOGS", "1");

        let progress = &mut *ctx.progress;
        process::run(ENGINE, cmd, ctx.cancel, |line| {
            if let Some(f) = percent(line) {
                progress(Progress::Fraction(f));
            }
        })
        .map(|_| ())
    }
}

/// ebook-convert reports "34% Converting input to HTML...".
fn percent(line: &str) -> Option<f32> {
    let (num, _) = line.trim_start().split_once('%')?;
    let n: f32 = num.parse().ok()?;
    (0.0..=100.0).contains(&n).then_some(n / 100.0)
}

/// The ebook-convert to run: the configured path, or the one found on
/// PATH or in Calibre's install folder.
fn program(configured: Option<&Path>) -> PathBuf {
    configured
        .map(Path::to_path_buf)
        .or_else(find_installed)
        .unwrap_or_else(|| PathBuf::from("ebook-convert"))
}

/// Where ebook-convert is when no path is set: on PATH, or in Calibre's
/// default install folder. `None` if it isn't in either.
pub fn find_installed() -> Option<PathBuf> {
    let name = format!("ebook-convert{}", std::env::consts::EXE_SUFFIX);
    let on_path = std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join(&name))
            .find(|p| p.is_file())
    });
    on_path.or_else(|| {
        ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"]
            .into_iter()
            .filter_map(std::env::var_os)
            .map(|dir| Path::new(&dir).join("Calibre2").join(&name))
            .find(|p| p.is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_lines_are_read() {
        assert_eq!(percent("34% Converting input to HTML..."), Some(0.34));
        assert_eq!(percent("  100% Output saved"), Some(1.0));
        assert_eq!(percent("Converting input to HTML..."), None);
        assert_eq!(percent("250% nonsense"), None);
    }
}
