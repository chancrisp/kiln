//! Documents and EPUB through an external Pandoc process.

use std::process::Command;

use crate::converter::{Context, Converter, Progress, Request};
use crate::error::{Error, Result};
use crate::process;

pub struct PandocConverter;

const ENGINE: &str = "Pandoc";

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
        cmd.arg("--sandbox").args(["--from", from, "--to", to]);
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
