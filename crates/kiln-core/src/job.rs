//! Running one conversion end to end: validate, convert in a private
//! working folder, then move the result to a name that overwrites nothing.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::calibre::CalibreConverter;
use crate::converter::{CancelToken, Context, Converter, Options, Progress, Request};
use crate::error::{Error, Result};
use crate::ffmpeg::FfmpegConverter;
use crate::libreoffice::LibreOfficeConverter;
use crate::output;
use crate::pandoc::PandocConverter;
use crate::pdf::PdfConverter;
use crate::process::find_on_path;
use crate::raster::ImageConverter;
use crate::registry::{Format, Kind, Registry};
use crate::suggest::Offered;

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
        let backends: [Box<dyn Converter>; 6] = [
            Box::new(ImageConverter),
            Box::new(FfmpegConverter),
            Box::new(PandocConverter),
            Box::new(LibreOfficeConverter),
            Box::new(CalibreConverter),
            Box::new(PdfConverter),
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

    /// The targets and tools Kiln can run on all of `paths` with the
    /// engines installed now. Targets are the extensions every file can
    /// reach, in the first file's order, through routes whose engines are
    /// all available. Tools need FFmpeg when any input is audio or video.
    pub fn offered(&self, paths: &[&Path], opts: &Options) -> Offered {
        let Some(first) = paths.first() else {
            return Offered::default();
        };
        let available: HashMap<&str, bool> = self
            .converters
            .keys()
            .map(|&b| (b, self.available(opts, b)))
            .collect();
        let routable = |from: &Format, to: &Format| {
            self.registry.route(from, to).is_some_and(|steps| {
                steps
                    .iter()
                    .all(|s| available.get(s.backend.as_str()).copied().unwrap_or(false))
            })
        };
        let froms: Option<Vec<&Format>> =
            paths.iter().map(|p| self.registry.format_of(p)).collect();
        let froms = froms.unwrap_or_default();
        let targets = if froms.len() == paths.len() {
            self.targets_for(first)
                .into_iter()
                .filter(|to| {
                    froms.iter().all(|from| {
                        self.registry.targets(from).iter().any(|t| t.ext == to.ext)
                            && routable(from, to)
                    })
                })
                .map(|to| to.ext.clone())
                .collect()
        } else {
            vec![]
        };
        let media = froms
            .iter()
            .any(|f| matches!(f.kind, Kind::Audio | Kind::Video));
        let tools = if media && !available["ffmpeg"] {
            vec![]
        } else {
            self.tools_for(paths)
        };
        Offered { targets, tools }
    }

    /// Whether a backend's engine is installed: the configured path, or
    /// where Kiln looks when none is set.
    fn available(&self, opts: &Options, backend: &str) -> bool {
        let e = &opts.engines;
        let path_var = std::env::var_os("PATH").unwrap_or_default();
        let found = |configured: Option<&Path>, name: &str| match configured {
            Some(p) => p.is_file(),
            None => find_on_path(&path_var, name).is_some(),
        };
        match backend {
            "image" | "pdf" => true,
            "ffmpeg" => found(e.ffmpeg.as_deref(), "ffmpeg"),
            "pandoc" => found(e.pandoc.as_deref(), "pandoc"),
            "libreoffice" => match e.libreoffice.as_deref() {
                Some(p) => p.is_file(),
                None => crate::find_libreoffice().is_some(),
            },
            "calibre" => match e.calibre.as_deref() {
                Some(p) => p.exists(),
                None => crate::find_calibre().is_some(),
            },
            _ => false,
        }
    }

    /// What FFprobe can say about a picture, sound or video: its size,
    /// frame rate, length and whether it has sound. For size estimates and
    /// for offering only the choices that fit the file. `None` for other
    /// kinds, or when FFprobe isn't installed or can't read the file.
    pub fn probe(&self, input: &Path, opts: &Options) -> Option<crate::quality::MediaInfo> {
        let f = self.registry.format_of(input)?;
        if !matches!(f.kind, Kind::Image | Kind::Audio | Kind::Video) {
            return None;
        }
        let cancel = CancelToken::new();
        let mut ctx = Context {
            progress: &mut |_| {},
            cancel: &cancel,
        };
        let info = crate::ffmpeg::probe_media(opts, input, &mut ctx).ok()?;
        (info.duration.is_some() || info.width.is_some()).then_some(info)
    }

    /// Convert `input` to `target` (an extension such as "jpg") and return
    /// the path written. When a conversion writes several files, such as
    /// one image per PDF page, this is the first; see
    /// [`Kiln::convert_outputs`].
    pub fn convert(&self, input: &Path, target: &str, opts: &Options) -> Result<PathBuf> {
        self.convert_with(input, target, opts, &mut |_| {}, &CancelToken::new())
    }

    /// Like [`Kiln::convert`], reporting progress and honouring `cancel`.
    pub fn convert_with(
        &self,
        input: &Path,
        target: &str,
        opts: &Options,
        progress: &mut dyn FnMut(Progress),
        cancel: &CancelToken,
    ) -> Result<PathBuf> {
        let mut outputs = self.convert_outputs(input, target, opts, progress, cancel)?;
        Ok(outputs.remove(0))
    }

    /// Convert and return every file written, in order: one for most
    /// conversions, one per page or slide when the target is an image of
    /// a paged document. A chained route runs each engine in turn inside
    /// the job's working folder. Nothing is left behind on failure or
    /// cancel: no partial output and no working folder.
    pub fn convert_outputs(
        &self,
        input: &Path,
        target: &str,
        opts: &Options,
        progress: &mut dyn FnMut(Progress),
        cancel: &CancelToken,
    ) -> Result<Vec<PathBuf>> {
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
        let steps = self
            .registry
            .route(from, to)
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

        let out_dir = out_dir_for(&input, opts)?;
        let work = WorkDir::for_options(opts)?;
        let stem = stem_of(&input);
        let count = steps.len();
        let mut current = input.clone();
        let mut written = Vec::new();
        for (i, step) in steps.iter().enumerate() {
            let step_from = self
                .registry
                .format(&step.from)
                .expect("routes use known formats");
            let step_to = self
                .registry
                .format(&step.to)
                .expect("routes use known formats");
            let last = i + 1 == count;
            // Each step gets its own folder, so engines that keep files
            // next to their output (LibreOffice's profile) never meet.
            // Files between steps keep the input's name, since some
            // engines take a title from it.
            let temp_out = if last {
                work.path().join(format!("output.{}", step_to.ext))
            } else {
                let dir = work.path().join(format!("step-{}", i + 1));
                fs::create_dir(&dir).map_err(|e| Error::io(&dir, e))?;
                dir.join(format!("{stem}.{}", step_to.ext))
            };
            let req = Request {
                input: &current,
                output: &temp_out,
                from: step_from,
                to: step_to,
                options: opts,
            };
            let mut step_progress = |p: Progress| {
                progress(match p {
                    Progress::Fraction(f) => Progress::Fraction((i as f32 + f) / count as f32),
                    Progress::Indeterminate => Progress::Indeterminate,
                })
            };
            let mut ctx = Context {
                progress: &mut step_progress,
                cancel,
            };
            self.converters[step.backend.as_str()].convert(&req, &mut ctx)?;
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }

            let engine = engine_name(&step.backend);
            written = written_by(&temp_out);
            if written.is_empty() {
                written.push(temp_out.clone());
            }
            for path in &written {
                ensure_written(path, engine)?;
            }
            if !last {
                if written.len() > 1 {
                    return Err(Error::EngineFailed {
                        engine,
                        summary: "it wrote several files where the next step needs one.".to_owned(),
                        details: String::new(),
                    });
                }
                current = written.remove(0);
            }
        }

        let finals = if written.len() == 1 {
            vec![place(&written[0], &out_dir, &stem, &to.ext)?]
        } else {
            let unit = if from.kind == Kind::Presentation {
                "slide"
            } else {
                "page"
            };
            written
                .iter()
                .enumerate()
                .map(|(n, path)| {
                    place(path, &out_dir, &format!("{stem} {unit} {}", n + 1), &to.ext)
                })
                .collect::<Result<Vec<_>>>()?
        };
        progress(Progress::Fraction(1.0));
        Ok(finals)
    }
}

/// What a step wrote: its output, or the numbered files a backend writes
/// instead when it makes several (see [`Request::numbered`]).
fn written_by(output: &Path) -> Vec<PathBuf> {
    if output.exists() {
        return vec![output.to_path_buf()];
    }
    (1..)
        .map(|n| Request::numbered(output, n))
        .take_while(|p| p.exists())
        .collect()
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
        "calibre" => "Calibre",
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
