//! Running one conversion end to end: validate, convert in a private
//! working folder, then move the result to a name that overwrites nothing.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::converter::{CancelToken, Context, Converter, Options, Progress, Request};
use crate::error::{Error, Result};
use crate::ffmpeg::FfmpegConverter;
use crate::libreoffice::LibreOfficeConverter;
use crate::output;
use crate::pandoc::PandocConverter;
use crate::raster::ImageConverter;
use crate::registry::{Format, Registry};

/// The registry plus the backends its routes name.
pub struct Kiln {
    registry: Registry,
    converters: HashMap<&'static str, Box<dyn Converter>>,
}

impl Default for Kiln {
    fn default() -> Self {
        Self::new()
    }
}

impl Kiln {
    /// The built-in registry and backends.
    pub fn new() -> Self {
        Self::with_registry(Registry::builtin()).expect("built-in routes use built-in backends")
    }

    /// Use a custom registry. Fails if a route names an unknown backend.
    pub fn with_registry(registry: Registry) -> Result<Self> {
        let backends: [Box<dyn Converter>; 4] = [
            Box::new(ImageConverter),
            Box::new(FfmpegConverter),
            Box::new(PandocConverter),
            Box::new(LibreOfficeConverter),
        ];
        let converters: HashMap<_, _> = backends.into_iter().map(|c| (c.id(), c)).collect();
        if let Some(unknown) = registry.backends().find(|b| !converters.contains_key(b)) {
            return Err(Error::Registry(format!(
                "\"{unknown}\" isn't a backend Kiln has."
            )));
        }
        Ok(Self {
            registry,
            converters,
        })
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Output formats offered for a file, judged by its extension.
    pub fn targets_for(&self, input: &Path) -> Vec<&Format> {
        self.registry
            .format_of(input)
            .map(|f| self.registry.targets(f))
            .unwrap_or_default()
    }

    /// Convert `input` to `target` (an extension such as "jpg") and return
    /// the path written.
    pub fn convert(&self, input: &Path, target: &str, opts: &Options) -> Result<PathBuf> {
        self.convert_with(input, target, opts, &mut |_| {}, &CancelToken::new())
    }

    /// Like [`Kiln::convert`], reporting progress and honouring `cancel`.
    /// Nothing is left behind on failure or cancel: no partial output and
    /// no working folder.
    pub fn convert_with(
        &self,
        input: &Path,
        target: &str,
        opts: &Options,
        progress: &mut dyn FnMut(Progress),
        cancel: &CancelToken,
    ) -> Result<PathBuf> {
        if !(1..=100).contains(&opts.jpeg_quality) {
            return Err(Error::InvalidQuality(opts.jpeg_quality));
        }
        let input = checked_input(input)?;

        let from = self.registry.format_of(&input).ok_or_else(|| {
            Error::UnsupportedInput(
                input
                    .extension()
                    .map(|e| e.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            )
        })?;
        let to = self
            .registry
            .format(target)
            .ok_or_else(|| Error::UnknownFormat(target.trim_start_matches('.').to_owned()))?;
        let backend = self
            .registry
            .backend(from, to)
            .ok_or_else(|| Error::UnsupportedPair {
                from: from.code(),
                to: to.code(),
                supported: self
                    .registry
                    .targets(from)
                    .iter()
                    .map(|f| f.code())
                    .collect(),
            })?;
        let converter = &self.converters[backend];

        let out_dir = out_dir_for(&input, opts)?;
        let work = WorkDir::for_options(opts)?;
        let temp_out = work.path().join(format!("output.{}", to.ext));

        let req = Request {
            input: &input,
            output: &temp_out,
            from,
            to,
            options: opts,
        };
        let mut ctx = Context { progress, cancel };
        converter.convert(&req, &mut ctx)?;
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }

        ensure_written(&temp_out, engine_name(backend))?;
        let final_path = place(&temp_out, &out_dir, &stem_of(&input), &to.ext)?;
        (ctx.progress)(Progress::Fraction(1.0));
        Ok(final_path)
    }
}

/// Where outputs for `input` go: the configured folder, or next to it.
pub(crate) fn out_dir_for(input: &Path, opts: &Options) -> Result<PathBuf> {
    let dir = match &opts.out_dir {
        Some(dir) => dir.clone(),
        None => input
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(".")),
    };
    if dir.is_dir() {
        Ok(dir)
    } else {
        Err(Error::OutputDirNotFound(dir))
    }
}

pub(crate) fn stem_of(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// An engine that exits cleanly but writes nothing is still a failure.
pub(crate) fn ensure_written(path: &Path, engine: &'static str) -> Result<()> {
    match fs::metadata(path) {
        Ok(m) if m.len() > 0 => Ok(()),
        _ => Err(Error::EngineFailed {
            engine,
            summary: "it finished without writing a file.".to_owned(),
            details: String::new(),
        }),
    }
}

pub(crate) fn engine_name(backend: &str) -> &'static str {
    match backend {
        "ffmpeg" => "FFmpeg",
        "pandoc" => "Pandoc",
        "libreoffice" => "LibreOffice",
        _ => "Kiln",
    }
}

/// The input as an absolute path to an existing file.
pub(crate) fn checked_input(input: &Path) -> Result<PathBuf> {
    let meta = match fs::metadata(input) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(Error::InputNotFound(input.to_path_buf()))
        }
        Err(e) => return Err(Error::io(input, e)),
    };
    if !meta.is_file() {
        return Err(Error::InputNotAFile(input.to_path_buf()));
    }
    // Absolute paths can never be mistaken for an engine's options, even
    // for a file named "-y.wav".
    std::path::absolute(input).map_err(|e| Error::io(input, e))
}

/// Move the finished file from the working folder to a free name.
pub(crate) fn place(temp: &Path, dir: &Path, stem: &str, ext: &str) -> Result<PathBuf> {
    let (placeholder, path) = output::create_unique(dir, stem, ext)?;
    // Close the placeholder first: Windows can't replace an open file.
    drop(placeholder);

    if fs::rename(temp, &path).is_ok() {
        return Ok(path);
    }
    // Different volume (or another rename failure): copy instead.
    let copied = File::open(temp).and_then(|mut src| {
        let mut dst = OpenOptions::new().write(true).truncate(true).open(&path)?;
        io::copy(&mut src, &mut dst)?;
        dst.sync_all()
    });
    match copied {
        Ok(()) => Ok(path),
        Err(e) => {
            let _ = fs::remove_file(&path);
            Err(Error::io(path, e))
        }
    }
}

/// A private per-job folder, removed when dropped (finish, error or cancel).
pub(crate) struct WorkDir(PathBuf);

impl WorkDir {
    pub(crate) fn for_options(opts: &Options) -> Result<Self> {
        let root = opts
            .temp_root
            .clone()
            .unwrap_or_else(|| std::env::temp_dir().join("Kiln"));
        Self::create(&root)
    }

    fn create(root: &Path) -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        fs::create_dir_all(root).map_err(|e| Error::io(root, e))?;
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        loop {
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir = root.join(format!("job-{}-{n}-{nanos:x}", std::process::id()));
            match fs::create_dir(&dir) {
                Ok(()) => return Ok(Self(dir)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(Error::io(dir, e)),
            }
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
