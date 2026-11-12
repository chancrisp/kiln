//! Running one conversion end to end: validate, convert in a private
//! working folder, then move the result to a name that overwrites nothing.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use kiln_media::SourceInfo;

use crate::audio::{self, AudioConverter};
use crate::calibre::CalibreConverter;
use crate::converter::{CancelToken, Context, Converter, Options, Progress, Request};
use crate::error::{Error, Result};
use crate::facts::Facts;
use crate::ffmpeg::FfmpegConverter;
use crate::held::{lock_file, probe_lock, LOCK_NAME};
use crate::libreoffice::LibreOfficeConverter;
use crate::office::{self, OfficeConverter};
use crate::output;
use crate::pandoc::PandocConverter;
use crate::pdf::PdfConverter;
use crate::process::find_on_path;
use crate::quality::MediaInfo;
use crate::raster::ImageConverter;
use crate::registry::{Format, Kind, Registry, Step};
use crate::suggest::Offered;
use crate::video::{self, VideoConverter};

/// Which backends' engines are installed, from `Kiln::availability` or
/// [`Availability::find`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Availability(HashMap<&'static str, bool>);

/// Every backend a [`Kiln`] has: the ids of the converters
/// `Kiln::with_registry` makes.
const BACKENDS: [&str; 10] = [
    "image",
    "ffmpeg",
    "pandoc",
    "office",
    "libreoffice",
    "calibre",
    "pdf",
    audio::AUDIO,
    audio::AUDIO_MF,
    crate::video::VIDEO,
];

/// The keys [`Availability`] keeps for each Office app beside "office"
/// itself (any of them): a step needs the app that opens its file.
const OFFICE_APPS: [&str; 3] = ["office-word", "office-excel", "office-powerpoint"];

impl Availability {
    /// Whether `backend`'s engine was found. Unknown backends weren't.
    /// "office" is any Office app; "office-word" and the others are each
    /// app.
    pub fn has(&self, backend: &str) -> bool {
        self.0.get(backend).copied().unwrap_or(false)
    }

    /// Whether `step` can run with what was found: on its backend, or on
    /// its fallback.
    pub fn runs(&self, step: &Step) -> bool {
        self.step_has(&step.backend, step)
            || step
                .fallback
                .as_deref()
                .is_some_and(|f| self.step_has(f, step))
    }

    /// Whether `backend` can do `step`: for Office, the app that opens the
    /// step's file.
    fn step_has(&self, backend: &str, step: &Step) -> bool {
        if backend == "office" {
            office::App::for_ext(&step.from).is_some_and(|a| self.has(a.key()))
        } else {
            self.has(backend)
        }
    }

    /// Every built-in engine found, for tests and for callers that want
    /// the unfiltered picture.
    pub fn all() -> Self {
        Availability(
            BACKENDS
                .into_iter()
                .chain(OFFICE_APPS)
                .map(|b| (b, true))
                .collect(),
        )
    }

    /// `backends` looked for with `opts`, and each Office app with
    /// "office".
    fn looked_for(backends: impl Iterator<Item = &'static str>, opts: &Options) -> Self {
        let mut found: HashMap<&'static str, bool> = HashMap::new();
        for b in backends {
            if b == "office" {
                let installed = office::usable();
                found.insert(b, installed.any());
                for app in office::App::ALL {
                    found.insert(app.key(), installed.has(app));
                }
            } else {
                found.insert(b, engine_available(opts, b));
            }
        }
        Availability(found)
    }

    /// Which engines are installed now, as `Kiln::availability` says, but
    /// without a `Kiln`, so a worker thread can look while the window
    /// thread keeps its own. It reads only `opts`, `PATH` as the registry
    /// holds it now (`process::current_path`), the usual
    /// install folders and Media Foundation's codec list, and holds no
    /// state of its own: it may be called from any thread, as often as
    /// wanted.
    ///
    /// It touches the disk (each configured path, each `PATH` folder and
    /// the install folders), and the first call in the process also starts
    /// Media Foundation to list its codecs (`kiln_media::mf_codecs`, kept
    /// after that until `kiln_media::mf_recheck`). So the window thread
    /// should not call it; a worker should, and send the answer back.
    pub fn find(opts: &Options) -> Self {
        Self::looked_for(BACKENDS.into_iter(), opts)
    }

    /// The same, with `backend` found too, if Kiln has it. "office"
    /// brings every Office app.
    pub fn with(mut self, backend: &str) -> Self {
        for b in BACKENDS.into_iter().chain(OFFICE_APPS) {
            if b == backend || (backend == "office" && OFFICE_APPS.contains(&b)) {
                self.0.insert(b, true);
            }
        }
        self
    }

    /// Everything found except the backends in `missing`. "office" takes
    /// every Office app with it.
    pub fn without(mut self, missing: &[&str]) -> Self {
        let office = missing.contains(&"office");
        self.0
            .retain(|b, _| !missing.contains(b) && !(office && OFFICE_APPS.contains(b)));
        self
    }
}

/// Whether a backend's engine is installed: the configured path, or
/// where Kiln looks when none is set.
fn engine_available(opts: &Options, backend: &str) -> bool {
    let e = &opts.engines;
    let path_var = crate::process::current_path();
    let found = |configured: Option<&Path>, name: &str| match configured {
        Some(p) => p.is_file(),
        None => find_on_path(&path_var, name).is_some(),
    };
    match backend {
        "image" | "pdf" | audio::AUDIO => true,
        audio::AUDIO_MF => audio::media_foundation_available(),
        // Media Foundation reads and writes it; without it FFmpeg does
        // every job (R59).
        video::VIDEO => audio::media_foundation_available() || crate::ffmpeg::installed(opts),
        // Settings' path, then the copy that comes with Kiln, then PATH (R10).
        "ffmpeg" => crate::ffmpeg::installed(opts),
        "office" => office::usable().any(),
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

/// What a conversion that keeps only a PDF's text says it leaves out.
pub const KEEPS_TEXT_ONLY: &str = "Keeps the text only: no pictures, tables or layout.";

/// What a conversion to a video type says of a video's subtitle tracks.
pub const NO_SUBTITLES: &str = "Subtitles aren't kept.";

/// What a conversion to a video or sound type says of a video that has `n`
/// sound tracks, `n` more than one.
pub fn first_of_tracks(n: u16) -> String {
    format!("Keeps the first of {n} sound tracks.")
}

/// The registry plus the backends its routes name.
pub struct Kiln {
    registry: Registry,
    converters: HashMap<&'static str, Box<dyn Converter>>,
    /// What Kiln's own engine found in a file's sound, by path, as the
    /// last look at the file (`inspect`, `probe`, `audio_problem`) left it.
    /// Offering targets only reads this, so it never touches the disk.
    sound: Mutex<HashMap<PathBuf, Heard>>,
}

/// One file's sound as Kiln's own engine read it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Heard {
    /// The row's sentence when the sound can't be converted.
    problem: Option<String>,
    /// A video with no sound at all.
    silent: bool,
}

/// How many files' sound [`Kiln`] remembers before it starts again.
const HEARD_MAX: usize = 4096;

impl Default for Kiln {
    fn default() -> Self {
        Self::new()
    }
}

/// `sound`, the sound of `input` (a file of format `f`) as part 1 read it,
/// except that a video's sound part 1 names but can't decode (AC-3 and
/// E-AC-3 in Matroska, named through Media Foundation, R50) is
/// [`MediaError::Unsupported`], as DTS is, so it is flagged before any
/// conversion (R91). Only those codecs are decoded to check.
fn undecoded(
    f: &Format,
    input: &Path,
    sound: kiln_media::Result<SourceInfo>,
) -> kiln_media::Result<SourceInfo> {
    use kiln_media::{Codec, MediaError};
    let Ok(s) = &sound else {
        return sound;
    };
    if f.kind != Kind::Video || !matches!(s.codec, Codec::Ac3 | Codec::Other(_)) {
        return sound;
    }
    match kiln_media::video::job::first_block(input) {
        Err(MediaError::Read(_) | MediaError::Cancelled | MediaError::NeedsMediaFoundation)
        | Ok(()) => sound,
        Err(_) => Err(MediaError::Unsupported {
            codec: s.codec.display_name(),
        }),
    }
}

impl Kiln {
    /// The built-in registry and backends.
    pub fn new() -> Self {
        Self::with_registry(Registry::builtin()).expect("built-in routes use built-in backends")
    }

    /// Use a custom registry. Fails if a route names an unknown backend.
    pub fn with_registry(registry: Registry) -> Result<Self> {
        let backends: [Box<dyn Converter>; 10] = [
            Box::new(ImageConverter),
            Box::new(FfmpegConverter),
            Box::new(PandocConverter),
            Box::new(OfficeConverter),
            Box::new(LibreOfficeConverter),
            Box::new(CalibreConverter),
            Box::new(PdfConverter),
            Box::new(AudioConverter::PLAIN),
            Box::new(AudioConverter::MEDIA_FOUNDATION),
            Box::new(VideoConverter::new()),
        ];
        let converters: HashMap<_, _> = backends.into_iter().map(|c| (c.id(), c)).collect();
        if let Some(unknown) = registry.backends().find(|b| !converters.contains_key(b)) {
            return Err(Error::Registry(format!(
                "Kiln has no converter called \"{unknown}\"."
            )));
        }
        Ok(Self {
            registry,
            converters,
            sound: Mutex::default(),
        })
    }

    /// The same Kiln with `c` in place of the backend of the same id.
    #[cfg(test)]
    pub(crate) fn with_converter(mut self, c: Box<dyn Converter>) -> Self {
        self.converters.insert(c.id(), c);
        self
    }

    /// What Kiln's own engine finds wrong with the sound of `input`, an
    /// audio file or one of the video types it reads, as the row's plain
    /// sentence: "This MP3 is damaged and can't be read.", "This video has
    /// no sound to convert.". `None` when the sound is fine, and for other
    /// files. A file that needs Windows' Media Foundation to be read isn't
    /// a problem here: the "audio-mf" backend's availability says that.
    ///
    /// What an earlier look found is used again; otherwise this reads the
    /// file, so call it off the window's thread.
    pub fn audio_problem(&self, input: &Path) -> Option<String> {
        let known = self.sound.lock().ok()?.get(input).cloned();
        match known {
            Some(h) => h.problem,
            None => {
                self.hear(input);
                self.sound.lock().ok()?.get(input)?.problem.clone()
            }
        }
    }

    /// Tells this Kiln what `inspect` found for `input`, perhaps on another
    /// Kiln: a video with no sound then offers no audio targets. Reads
    /// nothing from the disk.
    pub fn learn(&self, input: &Path, facts: &Facts) {
        // A video whose sound only FFprobe counts is remembered when it has
        // none.
        let reads = self
            .registry
            .format_of(input)
            .is_some_and(|f| audio::reads_sound(f) || (f.kind == Kind::Video && facts.silent));
        if reads {
            self.remember(
                input,
                Heard {
                    problem: facts.audio_problem.clone(),
                    silent: facts.silent,
                },
            );
        }
    }

    /// Whether an earlier look found `input` to be a video Kiln's own
    /// engine reads that has no sound, so no audio target is offered for
    /// it. A file not looked at yet is taken to have sound. Reads nothing
    /// from the disk.
    fn silent(&self, input: &Path) -> bool {
        self.sound
            .lock()
            .ok()
            .and_then(|sound| sound.get(input).map(|h| h.silent))
            .unwrap_or(false)
    }

    /// Reads the sound of `input` with Kiln's own engine and remembers what
    /// it found. `None` for a file whose sound Kiln doesn't read.
    pub(crate) fn hear(&self, input: &Path) -> Option<kiln_media::Result<SourceInfo>> {
        let f = self.registry.format_of(input)?;
        if !audio::reads_sound(f) {
            return None;
        }
        let sound = undecoded(f, input, kiln_media::probe(input));
        let (problem, silent) = audio::problem_of(f, &sound);
        self.remember(input, Heard { problem, silent });
        Some(sound)
    }

    fn remember(&self, input: &Path, h: Heard) {
        if let Ok(mut sound) = self.sound.lock() {
            if sound.len() >= HEARD_MAX && !sound.contains_key(input) {
                sound.clear();
            }
            sound.insert(input.to_path_buf(), h);
        }
    }

    /// How many files' sound this Kiln remembers.
    #[cfg(test)]
    pub(crate) fn heard_count(&self) -> usize {
        self.sound.lock().unwrap().len()
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

    /// Output formats offered for `input`, split by whether the engines
    /// their routes need are installed: those that can run, and those that
    /// can't, each with the first missing backend ("ffmpeg"). A step with
    /// a fallback that can't run names the fallback: Kiln can say where to
    /// get LibreOffice, not Microsoft Office.
    pub fn targets_split(
        &self,
        input: &Path,
        available: &Availability,
    ) -> (Vec<&Format>, Vec<(&Format, String)>) {
        let (mut usable, mut blocked) = (vec![], vec![]);
        let Some(from) = self.registry.format_of(input) else {
            return (usable, blocked);
        };
        let silent = self.silent(input);
        for to in self.registry.targets(from) {
            if silent && to.kind == Kind::Audio {
                continue;
            }
            let missing = self.registry.route(from, to).and_then(|steps| {
                steps
                    .iter()
                    .find(|s| !available.runs(s))
                    .map(|s| helper_of(s.fallback.as_deref().unwrap_or(&s.backend)).to_string())
            });
            match missing {
                Some(b) => blocked.push((to, b)),
                None => usable.push(to),
            }
        }
        (usable, blocked)
    }

    /// The targets and tools Kiln can run on all of `paths` with the
    /// engines installed now. Targets are the extensions every file can
    /// reach, in the first file's order, through routes whose engines are
    /// all available. Tools need FFmpeg when any input is audio or video.
    pub fn offered(&self, paths: &[&Path], opts: &Options) -> Offered {
        self.offered_with(paths, &self.availability(opts))
    }

    /// Which engines are installed now. Finding them searches PATH and
    /// the usual install folders, so work it out once and pass it to
    /// `offered_with` for many files.
    pub fn availability(&self, opts: &Options) -> Availability {
        Availability::looked_for(self.converters.keys().copied(), opts)
    }

    /// `offered`, with engine availability already worked out.
    pub fn offered_with(&self, paths: &[&Path], available: &Availability) -> Offered {
        let Some(first) = paths.first() else {
            return Offered::default();
        };
        let routable = |from: &Format, to: &Format| {
            self.registry
                .route(from, to)
                .is_some_and(|steps| steps.iter().all(|s| available.runs(s)))
        };
        let froms: Option<Vec<&Format>> =
            paths.iter().map(|p| self.registry.format_of(p)).collect();
        let froms = froms.unwrap_or_default();
        let silent = paths.iter().any(|p| self.silent(p));
        let targets = if froms.len() == paths.len() {
            self.targets_for(first)
                .into_iter()
                .filter(|to| !(silent && to.kind == Kind::Audio))
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
        let tools = if media && !available.has("ffmpeg") {
            vec![]
        } else {
            self.tools_for(paths)
        };
        Offered { targets, tools }
    }

    /// What Kiln can say about a picture, sound or video: its size, frame
    /// rate, length, and its sound's codec, bitrate, channels and rate.
    /// For size estimates and for offering only the choices that fit the
    /// file. Sound comes from Kiln's own engine where it reads the file,
    /// and so does the picture of MP4, MOV, M4V and MKV; the rest from
    /// FFprobe. For a video, how many sound and subtitle tracks it holds
    /// (`sound_tracks`, `subtitle_tracks`): from the headers of the four
    /// containers Kiln reads, else FFprobe's, and whether Kiln's own engines
    /// would read a later sound track than the first (`later_sound`). `None`
    /// for other kinds, or when nothing can read the file.
    pub fn probe(&self, input: &Path, opts: &Options) -> Option<MediaInfo> {
        self.probe_with(input, opts, &CancelToken::new())
    }

    /// [`Kiln::probe`], with the FFprobe runs stopped by `cancel`.
    pub(crate) fn probe_with(
        &self,
        input: &Path,
        opts: &Options,
        cancel: &CancelToken,
    ) -> Option<MediaInfo> {
        let f = self.registry.format_of(input)?;
        if !matches!(f.kind, Kind::Image | Kind::Audio | Kind::Video) {
            return None;
        }
        video::media_info(f, input, self.hear(input), || {
            let mut ctx = Context {
                progress: &mut |_| {},
                cancel,
            };
            crate::ffmpeg::probe_media(opts, input, &mut ctx).ok()
        })
    }

    /// What converting `input` to `target` leaves out, as plain sentences
    /// to show with the job: "Keeps the first of 2 sound tracks.",
    /// "Subtitles aren't kept.", and for a PDF's text laid out as another
    /// document, "Keeps the text only: no pictures, tables or layout.".
    ///
    /// No engine takes a video's subtitles, and FFmpeg takes its first sound
    /// track, so those sentences always hold. Kiln's own engines (a video to
    /// a sound type, or to another of MP4, MOV, M4V and MKV) read the first
    /// sound track whose codec Symphonia can name, which is a later one when
    /// the first is AC-3, E-AC-3, DTS or TrueHD. `media.later_sound` says a
    /// file is one of those, and on such a route "the first of n" is left
    /// out: a missing note is better than a false one. [`Kiln::probe`] and
    /// [`Kiln::drops_reading`] set it; a caller that builds its own counts
    /// for a video should set it from `kiln_media::sym::takes_first_sound`.
    ///
    /// The track counts come from `media` (`MediaInfo::sound_tracks` and
    /// `subtitle_tracks`, 0 when not known), so a caller that has a file's
    /// `MediaInfo` already pays nothing. This reads nothing from the disk.
    pub fn drops(&self, input: &Path, target: &str, media: Option<&MediaInfo>) -> Vec<String> {
        let (Some(from), Some(to)) = (self.registry.format_of(input), self.registry.format(target))
        else {
            return vec![];
        };
        let Some(steps) = self.registry.route(from, to) else {
            return vec![];
        };
        // Kiln's own engines run the route when every step is theirs.
        let ours = steps
            .iter()
            .all(|s| audio::is_audio_backend(&s.backend) || s.backend == video::VIDEO);
        let mut notes = vec![];
        // Only a video has tracks to lose, and only a video or sound target
        // holds sound; subtitles are only said for a video target, as a
        // sound file has none to keep.
        if let (Kind::Video, Kind::Video | Kind::Audio, Some(media)) = (from.kind, to.kind, media) {
            if media.sound_tracks > 1 && !(ours && media.later_sound) {
                notes.push(first_of_tracks(media.sound_tracks));
            }
            if to.kind == Kind::Video && media.subtitle_tracks > 0 {
                notes.push(NO_SUBTITLES.to_owned());
            }
        }
        if self.registry.text_only(from, to) {
            notes.push(KEEPS_TEXT_ONLY.to_owned());
        }
        notes
    }

    /// [`Kiln::drops`] for a file on disk: a video's sound and subtitle
    /// tracks are counted from its headers (MP4, MOV, M4V, MKV and WebM) or,
    /// for the other video types, by FFprobe found where `Options::default()`
    /// looks. Reads the file, so call it off the window's thread, as the
    /// runner does.
    pub fn drops_reading(&self, input: &Path, target: &str) -> Vec<String> {
        self.drops_reading_with(input, target, &Options::default(), &CancelToken::new())
    }

    /// [`Kiln::drops_reading`] looking for FFprobe where `opts` says, with its
    /// runs stopped by `cancel`.
    ///
    /// A video that Kiln's own engines would read a later sound track of
    /// (see [`Kiln::drops`]) is marked so (`MediaInfo::later_sound`), and
    /// "Keeps the first of n sound tracks." is not said on their routes.
    pub fn drops_reading_with(
        &self,
        input: &Path,
        target: &str,
        opts: &Options,
        cancel: &CancelToken,
    ) -> Vec<String> {
        let (Some(from), Some(to)) = (self.registry.format_of(input), self.registry.format(target))
        else {
            return vec![];
        };
        // A file is read only when what it holds could be said.
        let tracks_matter =
            from.kind == Kind::Video && matches!(to.kind, Kind::Video | Kind::Audio);
        let media = tracks_matter
            .then(|| match kiln_media::video::track_counts(input) {
                Some(c) => Some(MediaInfo {
                    sound_tracks: c.sound,
                    subtitle_tracks: c.subtitles,
                    later_sound: video::later_sound(input, c.sound),
                    ..MediaInfo::default()
                }),
                None => self.probe_with(input, opts, cancel),
            })
            .flatten();
        self.drops(input, target, media.as_ref())
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
        // A file that isn't what its extension says is never handed to an
        // engine: one would treat a "DOCX" of random bytes as plain text
        // and report a "converted" file.
        if crate::check::content_is_wrong(&input, &from.ext) {
            return Err(Error::WrongContent {
                path: input.clone(),
                code: from.code(),
            });
        }
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
        // Where the last step writes, which is where it leaves its names.
        let mut last_out = PathBuf::new();
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
            if last {
                last_out.clone_from(&temp_out);
            }
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
                    other => other,
                })
            };
            let mut ctx = Context {
                progress: &mut step_progress,
                cancel,
            };
            let has = |backend: &str| step_engine_available(opts, backend, step);
            let used = self.run_step(step, &req, &mut ctx, &has)?;
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }

            let engine = engine_name(used);
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

        let unit = if from.kind == Kind::Presentation {
            "slide"
        } else {
            "page"
        };
        let finals = place_all(&written, &last_out, &out_dir, &stem, &to.ext, unit)?;
        progress(Progress::Fraction(1.0));
        Ok(finals)
    }
}

impl Kiln {
    /// Run one step on its backend, or on its fallback when `has` says the
    /// backend's engine isn't there, when Office's app is already open
    /// (decision 4), or when the backend leaves the file to LibreOffice
    /// ([`Error::ForLibreOffice`]). Returns the backend that did it.
    fn run_step<'s>(
        &self,
        step: &'s Step,
        req: &Request<'_>,
        ctx: &mut Context<'_>,
        has: &dyn Fn(&str) -> bool,
    ) -> Result<&'s str> {
        let first = step.backend.as_str();
        let Some(fallback) = step.fallback.as_deref() else {
            self.converters[first]
                .convert(req, ctx)
                .map_err(needs_libreoffice)?;
            return Ok(first);
        };
        if !has(first) {
            self.converters[fallback].convert(req, ctx)?;
            return Ok(fallback);
        }
        match self.converters[first].convert(req, ctx) {
            Err(Error::OfficeInUse { .. }) if has(fallback) => {
                self.converters[fallback].convert(req, ctx)?;
                Ok(fallback)
            }
            Err(Error::ForLibreOffice) if fallback == "libreoffice" && has(fallback) => {
                self.converters[fallback].convert(req, ctx)?;
                Ok(fallback)
            }
            done => done.map(|()| first).map_err(needs_libreoffice),
        }
    }
}

/// A file a backend left to LibreOffice ([`Error::ForLibreOffice`]) when
/// LibreOffice can't take it: the job needs LibreOffice, as any job does
/// whose helper is missing.
fn needs_libreoffice(e: Error) -> Error {
    match e {
        Error::ForLibreOffice => Error::EngineMissing {
            engine: "LibreOffice",
        },
        e => e,
    }
}

/// Whether `backend`'s engine can do `step` now: for Office, whether the
/// app that opens the step's file is installed.
fn step_engine_available(opts: &Options, backend: &str, step: &Step) -> bool {
    if backend == "office" {
        office::App::for_ext(&step.from).is_some_and(|a| office::usable().has(a))
    } else {
        engine_available(opts, backend)
    }
}

/// The helper a missing backend stands for. Kiln's video engine is missing
/// only when Windows' Media Feature Pack is, and FFmpeg too (R59), so on
/// Windows the row asks for the pack; elsewhere only FFmpeg can do the work,
/// so it asks for FFmpeg (R60).
fn helper_of(backend: &str) -> &str {
    match backend {
        video::VIDEO if cfg!(windows) => audio::AUDIO_MF,
        video::VIDEO => "ffmpeg",
        other => other,
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

/// Move what the last step wrote to its final names in `dir`: `{stem}.{ext}`
/// for one file. For several, `{stem}-{name}.{ext}` when the step named its
/// parts (see [`Request::names`]), each in the order of the files, and
/// otherwise `{stem} {unit} {n}.{ext}`.
fn place_all(
    written: &[PathBuf],
    output: &Path,
    dir: &Path,
    stem: &str,
    ext: &str,
    unit: &str,
) -> Result<Vec<PathBuf>> {
    if let [only] = written {
        return Ok(vec![place(only, dir, stem, ext)?]);
    }
    let names = part_names(output, written.len());
    written
        .iter()
        .enumerate()
        .map(|(n, path)| {
            // `place` makes the whole name safe to use, a sheet's name too.
            let part = match names.as_ref().map(|names| names[n].trim()) {
                Some(name) if !name.is_empty() => format!("{stem}-{name}"),
                _ => format!("{stem} {unit} {}", n + 1),
            };
            place(path, dir, &part, ext)
        })
        .collect()
}

/// The names a step gave its `count` parts, if it gave one to each. A names
/// file that is missing, or that has a different number of names, names
/// nothing: the parts are told apart by their numbers.
fn part_names(output: &Path, count: usize) -> Option<Vec<String>> {
    let text = fs::read_to_string(Request::names(output)).ok()?;
    let names: Vec<String> = text.lines().map(str::to_owned).collect();
    (names.len() == count).then_some(names)
}

/// Where outputs for `input` go: the configured folder, or next to it. It
/// must be there and take new files, which is found out now, before any
/// work is done on a result with nowhere to go.
pub(crate) fn out_dir_for(input: &Path, opts: &Options) -> Result<PathBuf> {
    let dir = match &opts.out_dir {
        Some(dir) => dir.clone(),
        None => input
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(".")),
    };
    if !dir.is_dir() {
        return Err(Error::OutputDirNotFound(dir));
    }
    output::check_writable(&dir)?;
    Ok(dir)
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
        "office" => "Microsoft Office",
        "libreoffice" => "LibreOffice",
        "calibre" => "Calibre",
        // Kiln's own sound engine, with or without Media Foundation.
        audio::AUDIO | audio::AUDIO_MF => "Kiln",
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
///
/// It holds a `lock` file, open with no sharing for as long as the folder
/// lives. A job a crash or a kill cut short can't close it, but Windows
/// does, so [`sweep_stale_jobs`] tells a folder a running job is using from
/// one nobody is.
pub(crate) struct WorkDir {
    dir: PathBuf,
    /// Taken (closed) before the folder is removed: Windows can't delete an
    /// open file.
    lock: Option<File>,
}

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
                Ok(()) => {
                    let lock = dir.join(LOCK_NAME);
                    return match lock_file(&lock) {
                        Ok(file) => Ok(Self {
                            dir,
                            lock: Some(file),
                        }),
                        Err(e) => {
                            let _ = fs::remove_dir(&dir);
                            Err(Error::io(lock, e))
                        }
                    };
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(Error::io(dir, e)),
            }
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.dir
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        drop(self.lock.take());
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Remove the job folders under `root` (`%TEMP%\Kiln`) that a crash or a
/// kill left, and return them. Only direct children named as [`WorkDir`]
/// names them (`job-<process id>-<n>-<hex>`) are looked at, and no link is
/// followed.
///
/// A folder with a `lock` is removed only if that lock opens with no
/// sharing, which a running job's never does. One with no lock, as when its
/// job ended between making the folder and the lock, is removed only if the
/// process id in its name isn't a running process.
pub fn sweep_stale_jobs(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut removed = Vec::new();
    for entry in entries.flatten() {
        // `DirEntry::file_type` doesn't follow links, junctions included.
        match entry.file_type() {
            Ok(t) if t.is_dir() => {}
            _ => continue,
        }
        let Some(pid) = entry.file_name().to_str().and_then(job_pid) else {
            continue;
        };
        let dir = entry.path();
        if sweep_job(&dir, pid) {
            removed.push(dir);
        }
    }
    removed
}

/// The process id in the name of a job folder, or `None` if `name` isn't
/// one: `job-<id>-<n>-<hex>`.
fn job_pid(name: &str) -> Option<u32> {
    let mut parts = name.strip_prefix("job-")?.split('-');
    let (pid, n, nanos) = (parts.next()?, parts.next()?, parts.next()?);
    let digits = |s: &str, radix| !s.is_empty() && s.chars().all(|c| c.is_digit(radix));
    if parts.next().is_some() || !digits(pid, 10) || !digits(n, 10) || !digits(nanos, 16) {
        return None;
    }
    pid.parse().ok()
}

/// Remove one job folder if no running job is using it. False if one is,
/// if it isn't a folder of ours, or if it couldn't be removed.
fn sweep_job(dir: &Path, pid: u32) -> bool {
    let lock = dir.join(LOCK_NAME);
    match fs::symlink_metadata(&lock) {
        Ok(m) if m.is_file() => {
            // Only Windows can open a file with no sharing, so only there
            // does an open lock say a job is running. Elsewhere none is
            // trusted, and nothing is removed on its word.
            if cfg!(not(windows)) {
                return false;
            }
            // Opened with no sharing: fails while a job holds it.
            match probe_lock(&lock) {
                // Closed again so the folder, lock and all, can go.
                Ok(probe) => drop(probe),
                Err(_) => return false,
            }
        }
        // A link or a folder where the lock goes isn't a job folder of ours.
        Ok(_) => return false,
        // No lock: whether its job still runs is for its process to say.
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if process_alive(pid) {
                return false;
            }
        }
        Err(_) => return false,
    }
    fs::remove_dir_all(dir).is_ok()
}

/// Whether a process with this id is running. A process this user can't
/// open is counted as running, since it is there.
#[cfg(windows)]
pub(crate) fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_ACCESS_DENIED, STILL_ACTIVE,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    // SAFETY: plain Win32 calls. The handle is this function's own, closed
    // once, and `code` outlives the call that writes it.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            // An id nobody has is "invalid parameter"; a process of another
            // user, or an elevated one, is "access denied".
            return GetLastError() == ERROR_ACCESS_DENIED;
        }
        let mut code = 0u32;
        let read = GetExitCodeProcess(process, &mut code);
        CloseHandle(process);
        // A process that has ended can stay openable while others hold it.
        read == 0 || code == STILL_ACTIVE as u32
    }
}

/// Elsewhere there is no asking, so every process is taken to be running
/// and nothing is removed on a process id alone.
#[cfg(not(windows))]
pub(crate) fn process_alive(_pid: u32) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_that_need_a_missing_helper_are_split_off_with_the_helper() {
        let kiln = Kiln::new();
        let all = Availability::all();
        let (usable, blocked) = kiln.targets_split(Path::new("a.mp4"), &all);
        assert!(!usable.is_empty() && blocked.is_empty());

        let no_ffmpeg = Availability::all().without(&["ffmpeg"]);
        let (usable, blocked) = kiln.targets_split(Path::new("a.avi"), &no_ffmpeg);
        assert!(usable.is_empty(), "{usable:?}");
        assert!(blocked.iter().all(|(_, b)| b == "ffmpeg"));
        assert!(blocked.iter().any(|(f, _)| f.ext == "mp3"));
        // An MP4's sound is Kiln's own work, and so is its picture among
        // MP4, MOV, M4V and MKV; the other video types need FFmpeg.
        let (usable, blocked) = kiln.targets_split(Path::new("a.mp4"), &no_ffmpeg);
        let video = ["m4v", "mov", "mkv"];
        assert!(
            usable
                .iter()
                .all(|f| f.kind == Kind::Audio || video.contains(&f.ext.as_str())),
            "{usable:?}"
        );
        assert!(usable.iter().any(|f| f.ext == "mp3"));
        assert!(video.iter().all(|v| usable.iter().any(|f| f.ext == *v)));
        assert!(blocked.iter().all(|(_, b)| b == "ffmpeg"));
        assert!(blocked.iter().any(|(f, _)| f.ext == "webm"));

        // An image needs no helper for most of its formats.
        let (usable, _) = kiln.targets_split(Path::new("a.png"), &no_ffmpeg);
        assert!(usable.iter().any(|f| f.ext == "jpg"));
        // A type nobody knows has nothing either way.
        let (usable, blocked) = kiln.targets_split(Path::new("a.xyz"), &all);
        assert!(usable.is_empty() && blocked.is_empty());
    }

    #[test]
    fn a_password_protected_office_file_never_reaches_an_engine_while_office_is_off() {
        // LibreOffice runs with no time limit, and some versions wait on a
        // password-protected file (M3): it is refused before any engine,
        // in the plain words a damaged file gets.
        if crate::office::OFFICE_TO_PDF {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let locked = crate::office::compound_file(&["EncryptionInfo", "EncryptedPackage"]);
        let kiln = Kiln::new();
        for ext in ["docx", "xlsx", "pptx"] {
            let path = dir.path().join(format!("locked.{ext}"));
            std::fs::write(&path, &locked).unwrap();
            let err = kiln.convert(&path, "pdf", &Options::default()).unwrap_err();
            assert!(
                matches!(&err, Error::WrongContent { code, .. } if *code == ext.to_uppercase()),
                "{ext}: {err:?}"
            );
        }
        // Nothing was written beside them.
        let mut left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, ["locked.docx", "locked.pptx", "locked.xlsx"]);
    }

    #[test]
    fn a_missing_video_engine_asks_for_what_would_do_the_work() {
        // On Windows the Media Feature Pack; elsewhere only FFmpeg (R60).
        let want = if cfg!(windows) { "audio-mf" } else { "ffmpeg" };
        assert_eq!(helper_of("video"), want);
        assert_eq!(helper_of("ffmpeg"), "ffmpeg");
        assert_eq!(helper_of("audio-mf"), "audio-mf");
    }

    #[test]
    fn availability_can_be_found_on_a_worker_without_a_kiln() {
        // A configured engine that is there, and one that isn't, so the
        // answer is not just PATH's.
        let dir = tempfile::tempdir().unwrap();
        let pandoc = dir.path().join("pandoc.exe");
        std::fs::write(&pandoc, b"").unwrap();
        let mut opts = Options::default();
        opts.engines.pandoc = Some(pandoc);
        opts.engines.ffmpeg = Some(dir.path().join("gone.exe"));
        let here = Kiln::new().availability(&opts);
        // The same answer from a thread of its own, with no Kiln, twice.
        let worker = {
            let opts = opts.clone();
            std::thread::spawn(move || (Availability::find(&opts), Availability::find(&opts)))
        };
        let (first, again) = worker.join().unwrap();
        assert_eq!(first, here);
        assert_eq!(again, here);
        assert!(here.has("pandoc"));
        assert!(!here.has("ffmpeg"));
        // Every backend is answered, as `all` lists them, and each Office
        // app on its own.
        for b in BACKENDS.into_iter().chain(OFFICE_APPS) {
            assert!(Availability::all().has(b));
            assert!(first.0.contains_key(b), "{b}");
        }
        assert_eq!(first.0.len(), BACKENDS.len() + OFFICE_APPS.len());
    }

    #[test]
    fn office_to_pdf_switched_off_leaves_office_files_to_libreoffice() {
        // Decision D14: Office to PDF is off until review 9/10/14's
        // C1 and I3 land. Whatever is installed, Kiln never counts Office
        // as a converter, so an Office file's PDF is LibreOffice's or
        // waits on it.
        if office::OFFICE_TO_PDF {
            return;
        }
        assert_eq!(office::usable(), office::Installed::default());
        let opts = Options::default();
        let found = Availability::find(&opts);
        assert!(!found.has("office"));
        for app in office::App::ALL {
            assert!(!found.has(app.key()), "{app:?}");
        }
        assert!(!Kiln::new().availability(&opts).has("office"));
        assert!(!engine_available(&opts, "office"));
        let kiln = Kiln::new();
        let docx = kiln.registry().format_of(Path::new("a.docx")).unwrap();
        let pdf = kiln.registry().format("pdf").unwrap();
        let steps = kiln.registry().route(docx, pdf).unwrap();
        assert!(!step_engine_available(&opts, "office", &steps[0]));
        // Without LibreOffice too, the row names LibreOffice.
        let no_lo = found.without(&["libreoffice"]);
        let (_, blocked) = kiln.targets_split(Path::new("a.docx"), &no_lo);
        assert!(
            blocked
                .iter()
                .any(|(f, b)| f.ext == "pdf" && b == "libreoffice"),
            "{blocked:?}"
        );
    }

    #[test]
    fn office_pairs_run_on_libreoffice_when_office_is_missing() {
        let kiln = Kiln::new();
        let pdf_of = |file: &str, available: &Availability| {
            let (usable, blocked) = kiln.targets_split(Path::new(file), available);
            let usable = usable.iter().any(|f| f.ext == "pdf");
            let waits = blocked
                .iter()
                .find(|(f, _)| f.ext == "pdf")
                .map(|(_, b)| b.clone());
            (usable, waits)
        };
        let all = Availability::all();
        assert_eq!(pdf_of("a.docx", &all), (true, None));
        // Office alone, LibreOffice alone: either makes the PDF.
        let office = Availability::all().without(&["libreoffice"]);
        assert_eq!(pdf_of("a.docx", &office), (true, None));
        let lo = Availability::all().without(&["office"]);
        assert!(!lo.has("office-word"));
        for file in ["a.docx", "a.xlsx", "a.pptx", "a.csv", "a.doc"] {
            assert_eq!(pdf_of(file, &lo), (true, None), "{file}");
        }
        // Neither: the row names LibreOffice, which Kiln can point to.
        let neither = Availability::all().without(&["office", "libreoffice"]);
        assert_eq!(
            pdf_of("a.docx", &neither),
            (false, Some("libreoffice".to_owned()))
        );
        // Word without PowerPoint: a presentation waits on LibreOffice.
        let word = Availability::all().without(&["office-powerpoint", "libreoffice"]);
        assert_eq!(pdf_of("a.docx", &word), (true, None));
        assert_eq!(
            pdf_of("a.pptx", &word),
            (false, Some("libreoffice".to_owned()))
        );
        // offered_with agrees.
        let offered = kiln.offered_with(&[Path::new("a.pptx")], &word);
        assert!(!offered.targets.iter().any(|t| t == "pdf"), "{offered:?}");
        let offered = kiln.offered_with(&[Path::new("a.pptx")], &lo);
        assert!(offered.targets.iter().any(|t| t == "pdf"), "{offered:?}");
        // `with` brings every app back.
        let back = neither.with("office");
        assert!(back.has("office-word") && back.has("office-powerpoint"));
    }

    /// A converter that says what it was asked and answers as told.
    struct Told {
        id: &'static str,
        calls: std::sync::Arc<AtomicU64>,
        /// The error it answers with, or none to write its output.
        refuse: Option<fn() -> Error>,
    }

    impl Converter for Told {
        fn id(&self) -> &'static str {
            self.id
        }

        fn convert(&self, req: &Request<'_>, _: &mut Context<'_>) -> Result<()> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(refuse) = self.refuse {
                return Err(refuse());
            }
            fs::write(req.output, self.id).map_err(|e| Error::io(req.output, e))
        }
    }

    #[test]
    fn a_step_falls_back_when_its_first_backend_cant_take_it() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("talk.pptx");
        fs::write(&input, b"PK").unwrap();
        let output = dir.path().join("output.pdf");
        let office_calls = std::sync::Arc::new(AtomicU64::new(0));
        let lo_calls = std::sync::Arc::new(AtomicU64::new(0));
        let in_use = || Error::OfficeInUse { app: "PowerPoint" };
        let kiln_with = |refuse: Option<fn() -> Error>| {
            Kiln::new()
                .with_converter(Box::new(Told {
                    id: "office",
                    calls: office_calls.clone(),
                    refuse,
                }))
                .with_converter(Box::new(Told {
                    id: "libreoffice",
                    calls: lo_calls.clone(),
                    refuse: None,
                }))
        };
        let kiln = |busy: bool| kiln_with(busy.then_some(in_use as fn() -> Error));
        let reg = Registry::builtin();
        let (pptx, pdf) = (reg.format("pptx").unwrap(), reg.format("pdf").unwrap());
        let step = reg.route(pptx, pdf).unwrap()[0].clone();
        let opts = Options::default();
        let run = |k: &Kiln, has: &dyn Fn(&str) -> bool| {
            let _ = fs::remove_file(&output);
            let req = Request {
                input: &input,
                output: &output,
                from: pptx,
                to: pdf,
                options: &opts,
            };
            let cancel = CancelToken::new();
            let mut ctx = Context {
                progress: &mut |_| {},
                cancel: &cancel,
            };
            k.run_step(&step, &req, &mut ctx, has)
                .map(str::to_owned)
                .map_err(|e| e.to_string())
        };
        let count = |c: &AtomicU64| c.load(Ordering::SeqCst);

        // Office there and free: Office does it.
        assert_eq!(run(&kiln(false), &|_| true), Ok("office".to_owned()));
        assert_eq!(fs::read_to_string(&output).unwrap(), "office");
        // No Office: LibreOffice, and Office is never asked.
        let before = count(&office_calls);
        assert_eq!(
            run(&kiln(false), &|b| b != "office"),
            Ok("libreoffice".to_owned())
        );
        assert_eq!(count(&office_calls), before);
        assert_eq!(fs::read_to_string(&output).unwrap(), "libreoffice");
        // PowerPoint already open: LibreOffice takes it (decision 4).
        assert_eq!(run(&kiln(true), &|_| true), Ok("libreoffice".to_owned()));
        // ...and without LibreOffice the row says what to do.
        let lo_before = count(&lo_calls);
        assert_eq!(
            run(&kiln(true), &|b| b != "libreoffice"),
            Err("Close PowerPoint to convert this, or install LibreOffice.".to_owned())
        );
        assert_eq!(count(&lo_calls), lo_before);

        // Office sends the file to LibreOffice: LibreOffice takes it...
        let to_lo = kiln_with(Some(|| Error::ForLibreOffice));
        assert_eq!(run(&to_lo, &|_| true), Ok("libreoffice".to_owned()));
        assert_eq!(fs::read_to_string(&output).unwrap(), "libreoffice");
        // ...and without LibreOffice the row says LibreOffice is needed.
        let lo_before = count(&lo_calls);
        assert_eq!(
            run(&to_lo, &|b| b != "libreoffice"),
            Err("LibreOffice isn't installed. Get it from Settings > Helper apps.".to_owned())
        );
        assert_eq!(count(&lo_calls), lo_before);
    }

    #[test]
    fn with_finds_a_backend_again() {
        let none = Availability::all().without(&["pandoc", "ffmpeg"]);
        let back = none.clone().with("pandoc");
        assert!(back.has("pandoc") && !back.has("ffmpeg"));
        assert_eq!(
            none.with("nothing"),
            Availability::all().without(&["pandoc", "ffmpeg"])
        );
    }

    #[test]
    fn without_media_foundation_only_mf_targets_are_blocked() {
        let kiln = Kiln::new();
        let no_mf = Availability::all().without(&["audio-mf"]);
        let (usable, blocked) = kiln.targets_split(Path::new("a.mp3"), &no_mf);
        let usable: Vec<&str> = usable.iter().map(|f| f.ext.as_str()).collect();
        assert_eq!(usable, ["aiff", "flac", "ogg", "opus", "wav", "wv"]);
        let blocked: Vec<(&str, &str)> = blocked
            .iter()
            .map(|(f, b)| (f.ext.as_str(), b.as_str()))
            .collect();
        assert_eq!(
            blocked,
            [
                ("aac", "audio-mf"),
                ("ac3", "audio-mf"),
                ("m4a", "audio-mf"),
                ("wma", "audio-mf")
            ]
        );
        // With it, everything runs.
        let (usable, blocked) = kiln.targets_split(Path::new("a.mp3"), &Availability::all());
        assert_eq!(usable.len(), 10);
        assert!(blocked.is_empty());
        // A WMA needs Windows to be read at all.
        let (usable, _) = kiln.targets_split(Path::new("a.wma"), &no_mf);
        assert!(usable.is_empty());
    }

    /// Make `root\name` holding each of `files`, and a `lock` (closed) when
    /// `locked`.
    fn job_folder(root: &Path, name: &str, locked: bool, files: &[&str]) -> PathBuf {
        let dir = root.join(name);
        fs::create_dir(&dir).unwrap();
        for file in files {
            fs::write(dir.join(file), b"left behind").unwrap();
        }
        if locked {
            fs::write(dir.join("lock"), b"").unwrap();
        }
        dir
    }

    fn sorted(mut paths: Vec<PathBuf>) -> Vec<PathBuf> {
        paths.sort();
        paths
    }

    /// A job folder whose last step wrote `parts` as `output-1.csv`,
    /// `output-2.csv` and so on, each holding its text, and `names` as its
    /// names file when given; and the folder the results go to.
    fn several_parts(parts: &[&str], names: Option<&str>) -> (tempfile::TempDir, PathBuf) {
        let work = tempfile::tempdir().unwrap();
        let output = work.path().join("output.csv");
        for (n, text) in parts.iter().enumerate() {
            fs::write(Request::numbered(&output, n + 1), text).unwrap();
        }
        if let Some(names) = names {
            fs::write(Request::names(&output), names).unwrap();
        }
        // A job folder holds more than the results.
        fs::write(work.path().join(LOCK_NAME), b"").unwrap();
        (work, output)
    }

    /// Place what `output` was given, as the last step of a conversion of
    /// `book.xlsx` to CSV, into a new folder: the file names there, and the
    /// text of each file, in the order placed.
    fn placed(output: &Path) -> (Vec<String>, Vec<String>) {
        let out = tempfile::tempdir().unwrap();
        let written = written_by(output);
        let finals = place_all(&written, output, out.path(), "book", "csv", "page").unwrap();
        let names = finals
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        let texts = finals
            .iter()
            .map(|p| fs::read_to_string(p).unwrap())
            .collect();
        (names, texts)
    }

    #[test]
    fn a_names_file_names_the_parts() {
        let (work, output) = several_parts(&["1", "2"], Some("Sales\nNotes"));
        // The names file and the lock are not results.
        assert_eq!(
            written_by(&output),
            [Request::numbered(&output, 1), Request::numbered(&output, 2)]
        );
        let (names, texts) = placed(&output);
        assert_eq!(names, ["book-Sales.csv", "book-Notes.csv"]);
        assert_eq!(texts, ["1", "2"]);
        drop(work);
    }

    #[test]
    fn parts_without_names_are_numbered() {
        let (_work, output) = several_parts(&["a", "b"], None);
        assert_eq!(placed(&output).0, ["book page 1.csv", "book page 2.csv"]);
        // So are parts whose names file has too few names, or too many.
        for names in ["Sales", "Sales\nNotes\nMore", ""] {
            let (_work, output) = several_parts(&["a", "b"], Some(names));
            assert_eq!(placed(&output).0, ["book page 1.csv", "book page 2.csv"]);
        }
    }

    #[test]
    fn a_part_with_a_blank_name_is_numbered() {
        let (_work, output) = several_parts(&["a", "b", "c"], Some("Sales\n  \nNotes"));
        assert_eq!(
            placed(&output).0,
            ["book-Sales.csv", "book page 2.csv", "book-Notes.csv"]
        );
    }

    #[test]
    fn one_part_is_named_as_before_whatever_the_names_file_says() {
        let (_work, output) = several_parts(&["a"], Some("Sales"));
        assert_eq!(placed(&output).0, ["book.csv"]);
    }

    #[test]
    fn part_names_are_made_safe_to_use() {
        let (_work, output) = several_parts(
            &["a", "b", "c", "d"],
            Some("Q1/Q2\nNotes.\nCON\nÅrsrapport 2024"),
        );
        assert_eq!(
            placed(&output).0,
            [
                "book-Q1_Q2.csv",
                "book-Notes.csv",
                "book-CON.csv",
                "book-Årsrapport 2024.csv"
            ]
        );
        // Names that come to the same name are told apart, and a file that
        // is there already is kept.
        let (_work, output) = several_parts(&["a", "b"], Some("a?\na*"));
        let out = tempfile::tempdir().unwrap();
        fs::write(out.path().join("book-a_.csv"), "mine").unwrap();
        let written = written_by(&output);
        let finals = place_all(&written, &output, out.path(), "book", "csv", "page").unwrap();
        assert_eq!(
            finals,
            [
                out.path().join("book-a_ (1).csv"),
                out.path().join("book-a_ (2).csv")
            ]
        );
        assert_eq!(
            fs::read_to_string(out.path().join("book-a_.csv")).unwrap(),
            "mine"
        );
    }

    /// A junction at `link` to `target`, which needs no privilege.
    #[cfg(windows)]
    fn junction(link: &Path, target: &Path) {
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        // Without the junction there's nothing to test: say so, don't pass.
        assert!(made, "couldn't make a junction with `cmd /C mklink /J`");
    }

    #[cfg(windows)]
    #[test]
    fn a_job_folder_is_locked_while_it_lives() {
        let root = tempfile::tempdir().unwrap();
        let work = WorkDir::create(root.path()).unwrap();
        let lock = work.path().join("lock");
        assert!(lock.is_file());
        // Open with no sharing, so another process can't delete it either.
        assert!(fs::remove_file(&lock).is_err(), "the lock could be removed");
        assert!(lock.is_file());

        // Dropped, the folder goes, lock and all.
        let dir = work.path().to_path_buf();
        fs::write(dir.join("output.mp4"), b"partial").unwrap();
        drop(work);
        assert!(!dir.exists(), "the folder outlived its job");
        assert!(root.path().read_dir().unwrap().next().is_none());
    }

    #[cfg(windows)]
    #[test]
    fn stale_job_folders_are_swept_and_live_ones_kept() {
        let root = tempfile::tempdir().unwrap();
        let live = WorkDir::create(root.path()).unwrap();
        fs::write(live.path().join("output.mp4"), b"in progress").unwrap();
        let live_name = live.path().file_name().unwrap().to_owned();
        let live_name = live_name.to_str().unwrap();

        // A job a crash left: a lock nobody holds, and a half-made result.
        let crashed = job_folder(root.path(), "job-999999-0-0", true, &["output.mp4"]);
        // One that stopped between making its folder and its lock: with no
        // lock to ask, its process decides. This process is running, ...
        let own = format!("job-{}-1-0", std::process::id());
        let starting = job_folder(root.path(), &own, false, &["output.png"]);
        // ... and this one's isn't.
        let dead = job_folder(root.path(), "job-4000000000-0-0", false, &["output.png"]);
        // Folders that aren't job folders are left alone, even named alike.
        let other = job_folder(root.path(), "other", true, &["keep.txt"]);
        let alike = job_folder(root.path(), "job-notes", true, &["keep.txt"]);
        let file = root.path().join("job-1-0-0");
        fs::write(&file, b"a file").unwrap();

        let removed = sweep_stale_jobs(root.path());
        assert_eq!(sorted(removed), sorted(vec![crashed.clone(), dead.clone()]));
        assert!(!crashed.exists() && !dead.exists());
        assert!(starting.join("output.png").is_file(), "a live process's");
        assert!(root.path().join(live_name).join("output.mp4").is_file());
        assert!(other.join("keep.txt").is_file());
        assert!(alike.join("keep.txt").is_file());
        assert!(file.is_file());

        // Nothing left to sweep, and a root that isn't there is no trouble.
        assert!(sweep_stale_jobs(root.path()).is_empty());
        assert!(sweep_stale_jobs(&root.path().join("nope")).is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn sweep_does_not_follow_a_junction() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("Kiln");
        fs::create_dir(&root).unwrap();
        // One folder with a lock, one with none: each is a place the sweep
        // would remove from if it followed a link to it.
        let with_lock = job_folder(base.path(), "outside-1", true, &["mine.txt"]);
        let without = job_folder(base.path(), "outside-2", false, &["mine.txt"]);
        junction(&root.join("job-999999-0-0"), &with_lock);
        junction(&root.join("job-4000000000-0-0"), &without);

        assert!(sweep_stale_jobs(&root).is_empty());
        assert!(with_lock.join("lock").is_file());
        assert!(with_lock.join("mine.txt").is_file());
        assert!(without.join("mine.txt").is_file());
    }

    #[cfg(windows)]
    #[test]
    fn a_folder_that_takes_no_files_stops_a_job_before_any_work() {
        let base = tempfile::tempdir().unwrap();
        let input = base.path().join("photo.png");
        image::RgbImage::new(4, 4).save(&input).unwrap();
        let out = base.path().join("card");
        fs::create_dir(&out).unwrap();
        // Declared after `base`, so it is undone before `base` is removed.
        let _locked = output::test_support::Locked::new(&out);
        let work = base.path().join("work");
        let opts = Options {
            out_dir: Some(out.clone()),
            temp_root: Some(work.clone()),
            ..Options::default()
        };

        let kiln = Kiln::new();
        let err = kiln.convert(&input, "jpg", &opts).unwrap_err();
        assert!(matches!(&err, Error::CantSaveIn(d) if d == &out), "{err:?}");
        assert!(!work.exists(), "work was started");

        // So does a tool, which writes to the same place.
        let err = kiln
            .run_tool(crate::Tool::Smaller, std::slice::from_ref(&input), &opts)
            .unwrap_err();
        assert!(matches!(&err, Error::CantSaveIn(d) if d == &out), "{err:?}");
        assert!(!work.exists(), "work was started");

        // A folder that isn't there is still said to be missing.
        let opts = Options {
            out_dir: Some(base.path().join("gone")),
            ..opts
        };
        let err = kiln.convert(&input, "jpg", &opts).unwrap_err();
        assert!(matches!(err, Error::OutputDirNotFound(_)), "{err:?}");
    }
}
