//! Documents and EPUB through an external Pandoc process.

use std::fs;
use std::process::Command;

use crate::converter::{Context, Converter, Progress, Request};
use crate::error::{Error, Result};
use crate::process;

pub struct PandocConverter;

const ENGINE: &str = "Pandoc";

/// Pandoc's filter that keeps the pictures in the input's folder and drops
/// the rest. `--sandbox` stops Pandoc reading any picture at all, and the
/// filter, which isn't under the sandbox, reads the ones it admits itself.
const KEEP_LOCAL: &str = include_str!("keep-local.lua");

/// Pandoc's names for the targets that put the pictures inside the file they
/// write, so a picture Pandoc can't read is a picture lost. These get the
/// filter. Every other target only names its pictures, fetches nothing, and
/// keeps each reference as written.
const EMBEDDING_TARGETS: &[&str] = &["docx", "odt", "epub", "pptx", "rtf", "fb2", "ipynb"];

impl Converter for PandocConverter {
    fn id(&self) -> &'static str {
        "pandoc"
    }

    fn convert(&self, req: &Request<'_>, ctx: &mut Context<'_>) -> Result<()> {
        let (Some(from), Some(to)) = (&req.from.pandoc, &req.to.pandoc) else {
            return Err(Error::Registry(format!(
                "{} to {} uses Pandoc, but a format has no pandoc name.",
                req.from.code(),
                req.to.code()
            )));
        };
        (ctx.progress)(Progress::Indeterminate);

        let mut cmd = Command::new(process::program(
            req.options.engines.pandoc.as_deref(),
            "pandoc",
        ));
        // --sandbox limits Pandoc to the files named here: no network
        // fetches and no reading other files through include directives.
        cmd.arg("--sandbox");
        if EMBEDDING_TARGETS.contains(&to.as_str()) {
            // The filter goes in the job's working folder, as LibreOffice's
            // profile does, and goes with it. It lets back in the pictures
            // next to the input.
            let work = req
                .output
                .parent()
                .expect("the output is inside the job's working folder");
            let filter = work.join("keep-local.lua");
            fs::write(&filter, KEEP_LOCAL).map_err(|e| Error::io(&filter, e))?;
            cmd.arg("--lua-filter").arg(&filter);
        }
        cmd.args(["--from", from, "--to", to]);
        if req.to.standalone {
            cmd.arg("--standalone");
        }
        cmd.arg("--output")
            .arg(req.output)
            // Paths are absolute, so they can't be mistaken for options.
            .arg(req.input);

        process::run(ENGINE, cmd, ctx.cancel, |_| {}).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Registry;

    /// Pandoc's names for the targets that write only text.
    const TEXT_TARGETS: &[&str] = &[
        "markdown",
        "html",
        "rst",
        "latex",
        "plain",
        "org",
        "asciidoc",
        "typst",
        "mediawiki",
        "textile",
        "docbook",
    ];

    #[test]
    fn every_pandoc_target_is_either_embedding_or_text() {
        // A new Pandoc type in formats.toml has to be sorted into one list.
        let registry = Registry::builtin();
        let names: Vec<&str> = registry
            .extensions()
            .filter_map(|ext| registry.format(ext))
            .filter_map(|f| f.pandoc.as_deref())
            .collect();
        for name in &names {
            let embedding = EMBEDDING_TARGETS.contains(name);
            let text = TEXT_TARGETS.contains(name);
            assert!(embedding != text, "{name} is in both lists or neither");
        }
        for name in EMBEDDING_TARGETS.iter().chain(TEXT_TARGETS) {
            assert!(names.contains(name), "{name} is not a Pandoc type");
        }
    }
}
