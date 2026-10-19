//! The `Converter` trait every backend implements, and what it is handed.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::error::Result;
use crate::quality::Quality;
use crate::registry::Format;

/// Paths to external engines. `None` means "find it on PATH".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Engines {
    pub ffmpeg: Option<PathBuf>,
    pub pandoc: Option<PathBuf>,
    /// soffice.exe or soffice.com. `None` also looks in LibreOffice's
    /// default install folder, since its installer doesn't touch PATH.
    pub libreoffice: Option<PathBuf>,
    /// Calibre's ebook-convert.exe. `None` also looks in Calibre's default
    /// install folder.
    pub calibre: Option<PathBuf>,
}

/// Settings that apply to one conversion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Where to write the result. `None` writes next to the input.
    pub out_dir: Option<PathBuf>,
    /// JPG quality, 1 to 100.
    pub jpeg_quality: u8,
    /// Compression, picture size, frame rate, audio bitrate and page
    /// resolution, for the pairs they apply to.
    pub quality: Quality,
    pub engines: Engines,
    /// Parent of the per-job working folders. `None` uses `%TEMP%\Kiln`.
    pub temp_root: Option<PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            out_dir: None,
            jpeg_quality: 90,
            quality: Quality::default(),
            engines: Engines::default(),
            temp_root: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Progress {
    /// Running, but the backend can't say how far along it is.
    Indeterminate,
    /// Fraction done, 0.0 to 1.0.
    Fraction(f32),
}

/// Shared flag a caller sets to stop a job. Cheap to clone.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// One conversion, as a backend sees it.
pub struct Request<'a> {
    /// Absolute path of the source file.
    pub input: &'a Path,
    /// Where to write. It is inside the job's private working folder and
    /// does not exist yet; Kiln moves it to its final name afterwards. A
    /// backend that makes several files, such as one image per page,
    /// writes [`Request::numbered`] paths 1, 2, 3 and so on instead.
    pub output: &'a Path,
    pub from: &'a Format,
    pub to: &'a Format,
    pub options: &'a Options,
}

impl Request<'_> {
    /// The `n`th of several outputs: `output-1.png` next to `output.png`.
    pub fn numbered(output: &Path, n: usize) -> PathBuf {
        let stem = output.file_stem().unwrap_or_default().to_string_lossy();
        let name = match output.extension() {
            Some(ext) => format!("{stem}-{n}.{}", ext.to_string_lossy()),
            None => format!("{stem}-{n}"),
        };
        output.with_file_name(name)
    }
}

/// Lets a backend report progress and notice cancellation.
pub struct Context<'a> {
    pub progress: &'a mut dyn FnMut(Progress),
    pub cancel: &'a CancelToken,
}

pub trait Converter: Send + Sync {
    /// The id routes use in formats.toml, such as "ffmpeg".
    fn id(&self) -> &'static str;

    /// Write `req.output` from `req.input`. Return `Error::Cancelled` as
    /// soon as practical after `ctx.cancel` is set.
    fn convert(&self, req: &Request<'_>, ctx: &mut Context<'_>) -> Result<()>;
}
