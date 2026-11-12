//! `%APPDATA%\Kiln\settings.toml`: where exports start, which key opens which
//! wheel, the theme, quality defaults and engine paths. Every field is
//! optional; anything left out takes its default.

use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use kiln_core::{Engines, Format, Options};

use crate::quality::QualityDefaults;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// Where Save dialogs start. Unset starts them in the last folder saved
    /// to, or the file's own folder. It no longer says where results go:
    /// that is `save_to`.
    #[serde(skip_serializing_if = "Option::is_none", with = "opt_path")]
    pub output_folder: Option<PathBuf>,
    /// The folder new files are written to. Unset writes each one next to
    /// its original.
    #[serde(skip_serializing_if = "Option::is_none", with = "opt_path")]
    pub save_to: Option<PathBuf>,
    pub theme: ThemeChoice,
    /// Whether Kiln looks at dropped files and suggests what to do.
    #[serde(default = "yes")]
    pub suggestions: bool,
    pub quality: QualityDefaults,
    pub engines: EnginePaths,
    /// Retired, and unused: the conversions Kiln once counted per source
    /// type to learn suggestions. It is still read and thrown away, so a
    /// file that has the table loads (unknown fields are refused), and never
    /// written, so the table leaves the file the next time settings are
    /// saved.
    #[serde(default, skip_serializing, rename = "habits")]
    pub(crate) retired_habits: Retired,
    /// Retired, and unused: the `[keys]` table that gave each wheel a key of
    /// its own. Shift converts and Ctrl opens tools, fixed. It is read and
    /// thrown away, as `habits` is.
    #[serde(default, skip_serializing, rename = "keys")]
    pub(crate) retired_keys: Retired,
    /// Retired, and unused: whether the tip about holding a key while
    /// dropping had been shown. The tip is gone; read and thrown away, as
    /// `habits` is.
    #[serde(default, skip_serializing, rename = "drop_tip_shown")]
    pub(crate) retired_drop_tip: Retired,
    /// Retired, and unused: whether results waited in Kiln until they were
    /// saved. Every result is now saved next to its original, or in the
    /// folder chosen in `save_to`, as soon as it is ready. Any value is
    /// read and thrown away, as `habits` is, so a file that has it loads
    /// with every other setting kept, and it leaves the file the next time
    /// settings are saved.
    #[serde(default, skip_serializing, rename = "save_next_to_original")]
    pub(crate) retired_save_next: Retired,
    /// Retired, and unused: whether Kiln had asked about saving next to the
    /// original when the first result was ready. It is still read, so a
    /// file that has it loads (unknown fields are refused), and never
    /// written, so it leaves the file the next time settings are saved.
    #[serde(default, skip_serializing)]
    pub save_asked: bool,
    /// Whether the drag-wheel is on: the wheel opens anywhere while a file
    /// is dragged, Kiln starts with Windows, and closing the window leaves
    /// Kiln in the notification area. Off for a new install, and always
    /// written. A file without the line reads as off.
    #[serde(default)]
    pub drag_wheel: bool,
    /// Whether this file is past the one-time turn-off of the drag-wheel
    /// (decision 3): before phase 3 it was on unless turned off, and it is
    /// now off unless turned on. A file without the line is from before,
    /// so [`Settings::parse`] turns the drag-wheel off once, with a notice,
    /// and sets this; every file written after says so.
    #[serde(default)]
    pub drag_wheel_reset: bool,
    /// Whether Kiln has said, the first time its window hid instead of
    /// quitting, that it is still running in the notification area.
    #[serde(default, skip_serializing_if = "is_false")]
    pub told_about_tray: bool,
    /// Whether the empty window's first-run help has been dismissed on
    /// this PC. Read from `welcome_shown` too, the welcome card's key that
    /// it replaces, and written under the new name.
    #[serde(default, skip_serializing_if = "is_false", alias = "welcome_shown")]
    pub first_run_dismissed: bool,
    /// Something loading the file changed that the person should be told
    /// once, as the window opens. Never written.
    #[serde(skip)]
    pub pending_notice: Option<Notice>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            output_folder: None,
            save_to: None,
            theme: ThemeChoice::default(),
            suggestions: true,
            quality: QualityDefaults::default(),
            engines: EnginePaths::default(),
            retired_habits: Retired,
            retired_keys: Retired,
            retired_drop_tip: Retired,
            retired_save_next: Retired,
            save_asked: false,
            drag_wheel: false,
            // A new install is past the turn-off: there is nothing to turn
            // off.
            drag_wheel_reset: true,
            told_about_tray: false,
            first_run_dismissed: false,
            pending_notice: None,
        }
    }
}

/// What loading the settings changed, to be said once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    /// The drag-wheel was on and has been turned off once (decision 3).
    DragWheelOff,
}

impl Notice {
    pub fn text(self) -> &'static str {
        match self {
            Notice::DragWheelOff => DRAG_WHEEL_NOW_OFF,
        }
    }
}

/// The one-time notice after the drag-wheel was turned off (decision 3).
pub const DRAG_WHEEL_NOW_OFF: &str =
    "The drag-wheel is now off. You can turn it back on in Settings.";

/// A value from a setting Kiln no longer has: read, whatever its shape, and
/// thrown away.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Retired;

impl<'de> Deserialize<'de> for Retired {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        serde::de::IgnoredAny::deserialize(d)?;
        Ok(Retired)
    }
}

fn yes() -> bool {
    true
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// The look Settings' Theme drop-down chooses. The file names it in
/// lowercase (`nord`, `solarized-dark`); a name Kiln doesn't know, from a
/// newer Kiln or a typo, is System, and the rest of the file is read as
/// it is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum ThemeChoice {
    /// Follow the Windows app mode.
    #[default]
    System,
    Light,
    Dark,
    Dracula,
    Gruvbox,
    Monokai,
    Nord,
    SolarizedDark,
    SolarizedLight,
}

impl ThemeChoice {
    /// In the order the Theme drop-down lists them.
    pub const ALL: [ThemeChoice; 9] = [
        ThemeChoice::System,
        ThemeChoice::Light,
        ThemeChoice::Dark,
        ThemeChoice::Dracula,
        ThemeChoice::Gruvbox,
        ThemeChoice::Monokai,
        ThemeChoice::Nord,
        ThemeChoice::SolarizedDark,
        ThemeChoice::SolarizedLight,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::System => "System",
            ThemeChoice::Light => "Light",
            ThemeChoice::Dark => "Dark",
            ThemeChoice::Dracula => "Dracula",
            ThemeChoice::Gruvbox => "Gruvbox",
            ThemeChoice::Monokai => "Monokai",
            ThemeChoice::Nord => "Nord",
            ThemeChoice::SolarizedDark => "Solarized Dark",
            ThemeChoice::SolarizedLight => "Solarized Light",
        }
    }

    /// The name settings.toml gives it.
    pub fn name(self) -> &'static str {
        match self {
            ThemeChoice::System => "system",
            ThemeChoice::Light => "light",
            ThemeChoice::Dark => "dark",
            ThemeChoice::Dracula => "dracula",
            ThemeChoice::Gruvbox => "gruvbox",
            ThemeChoice::Monokai => "monokai",
            ThemeChoice::Nord => "nord",
            ThemeChoice::SolarizedDark => "solarized-dark",
            ThemeChoice::SolarizedLight => "solarized-light",
        }
    }

    /// The choice a file's `name` stands for, if Kiln knows it.
    pub fn from_name(name: &str) -> Option<ThemeChoice> {
        ThemeChoice::ALL.into_iter().find(|t| t.name() == name)
    }

    /// Whether this is one of the six themes with a base of its own: they
    /// are drawn on it, with no Mica behind the window.
    pub fn is_named(self) -> bool {
        !matches!(
            self,
            ThemeChoice::System | ThemeChoice::Light | ThemeChoice::Dark
        )
    }
}

impl Serialize for ThemeChoice {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.name())
    }
}

impl<'de> Deserialize<'de> for ThemeChoice {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        /// A name, or whatever else was written there.
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Written {
            Name(String),
            Other(serde::de::IgnoredAny),
        }
        Ok(match Written::deserialize(d)? {
            Written::Name(n) => ThemeChoice::from_name(&n).unwrap_or_default(),
            Written::Other(_) => ThemeChoice::System,
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EnginePaths {
    #[serde(skip_serializing_if = "Option::is_none", with = "opt_path")]
    pub ffmpeg: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none", with = "opt_path")]
    pub pandoc: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none", with = "opt_path")]
    pub libreoffice: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none", with = "opt_path")]
    pub calibre: Option<PathBuf>,
}

/// The helper apps' official download pages.
pub const PANDOC_DOWNLOAD: &str = "https://pandoc.org/installing.html";
pub const LIBREOFFICE_DOWNLOAD: &str = "https://www.libreoffice.org/download/";
pub const CALIBRE_DOWNLOAD: &str = "https://calibre-ebook.com/download";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Engine {
    Ffmpeg,
    Pandoc,
    LibreOffice,
    Calibre,
}

impl Engine {
    pub const ALL: [Engine; 4] = [
        Engine::Ffmpeg,
        Engine::Pandoc,
        Engine::LibreOffice,
        Engine::Calibre,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Engine::Ffmpeg => "FFmpeg",
            Engine::Pandoc => "Pandoc",
            Engine::LibreOffice => "LibreOffice",
            Engine::Calibre => "Calibre",
        }
    }

    /// What the helper app is for, under its name in Settings > Helper
    /// apps (UX-12).
    pub fn purpose(self) -> &'static str {
        match self {
            Engine::Ffmpeg => "Other video formats, GIFs, and making videos smaller.",
            Engine::Pandoc => "Documents to EPUB, HTML and Markdown.",
            // No promise of Office to PDF through Microsoft Office while
            // that route isn't offered as working.
            Engine::LibreOffice => "Office files and documents to PDF.",
            Engine::Calibre => "E-books: EPUB, MOBI and AZW3.",
        }
    }

    /// The helper's official download page, which Get it opens in the
    /// browser. These three are the only addresses Kiln ever opens for a
    /// helper; Kiln itself never downloads anything. FFmpeg comes with
    /// Kiln, so it has none: Kiln never sends anyone to ffmpeg.org.
    pub fn download_page(self) -> Option<&'static str> {
        match self {
            Engine::Ffmpeg => None,
            Engine::Pandoc => Some(PANDOC_DOWNLOAD),
            Engine::LibreOffice => Some(LIBREOFFICE_DOWNLOAD),
            Engine::Calibre => Some(CALIBRE_DOWNLOAD),
        }
    }

    /// The helper's package id in winget, which Install with winget runs.
    /// FFmpeg comes with Kiln, so it has none.
    pub fn winget_id(self) -> Option<&'static str> {
        match self {
            Engine::Ffmpeg => None,
            Engine::Pandoc => Some("JohnMacFarlane.Pandoc"),
            Engine::LibreOffice => Some("TheDocumentFoundation.LibreOffice"),
            Engine::Calibre => Some("calibre.calibre"),
        }
    }

    /// The program's file name, without `.exe`.
    pub fn program(self) -> &'static str {
        match self {
            Engine::Ffmpeg => "ffmpeg",
            Engine::Pandoc => "pandoc",
            Engine::LibreOffice => "soffice",
            Engine::Calibre => "ebook-convert",
        }
    }
}

impl EnginePaths {
    pub fn get(&self, e: Engine) -> Option<&Path> {
        match e {
            Engine::Ffmpeg => self.ffmpeg.as_deref(),
            Engine::Pandoc => self.pandoc.as_deref(),
            Engine::LibreOffice => self.libreoffice.as_deref(),
            Engine::Calibre => self.calibre.as_deref(),
        }
    }

    pub fn set(&mut self, e: Engine, path: Option<PathBuf>) {
        match e {
            Engine::Ffmpeg => self.ffmpeg = path,
            Engine::Pandoc => self.pandoc = path,
            Engine::LibreOffice => self.libreoffice = path,
            Engine::Calibre => self.calibre = path,
        }
    }
}

/// Where an engine will be found when a job needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineStatus {
    /// Set in settings.
    Chosen {
        path: PathBuf,
        exists: bool,
    },
    /// Not set, but on PATH.
    OnPath(PathBuf),
    /// Not set and not on PATH, but in the engine's default install
    /// folder. Only LibreOffice and Calibre are looked for this way.
    Installed(PathBuf),
    /// Not set, and the copy that comes with Kiln is there (FFmpeg only).
    /// It is used before one on PATH.
    Included(PathBuf),
    Missing,
}

impl EngineStatus {
    /// The line shown in Settings > Helper apps.
    pub fn describe(&self) -> String {
        match self {
            EngineStatus::Chosen { path, exists: true } => path.display().to_string(),
            EngineStatus::Chosen { path, .. } => format!("Not found at {}", path.display()),
            EngineStatus::OnPath(_) => "Found on PATH".to_owned(),
            EngineStatus::Installed(path) => path.display().to_string(),
            EngineStatus::Included(_) => "Included".to_owned(),
            EngineStatus::Missing => "Not installed".to_owned(),
        }
    }
}

pub use kiln_core::find_on_path;

pub fn engine_status(settings: &Settings, e: Engine, path_var: &OsStr) -> EngineStatus {
    let bundled = match e {
        Engine::Ffmpeg => kiln_core::ffmpeg::bundled_dir(),
        _ => None,
    };
    engine_status_in(settings, e, path_var, bundled.as_deref())
}

/// [`engine_status`] with `bundled` as the folder of the FFmpeg that comes
/// with Kiln (`kiln_core::ffmpeg::bundled_dir`).
pub fn engine_status_in(
    settings: &Settings,
    e: Engine,
    path_var: &OsStr,
    bundled: Option<&Path>,
) -> EngineStatus {
    let included = bundled
        .filter(|_| e == Engine::Ffmpeg)
        .map(|dir| dir.join("ffmpeg.exe"))
        .filter(|exe| exe.is_file());
    match (settings.engines.get(e), included) {
        (Some(p), _) => EngineStatus::Chosen {
            path: p.to_owned(),
            exists: p.is_file(),
        },
        (None, Some(exe)) => EngineStatus::Included(exe),
        (None, None) => match find_on_path(path_var, e.program()) {
            Some(p) => EngineStatus::OnPath(p),
            None => match e {
                Engine::LibreOffice => kiln_core::find_libreoffice()
                    .map_or(EngineStatus::Missing, EngineStatus::Installed),
                Engine::Calibre => {
                    kiln_core::find_calibre().map_or(EngineStatus::Missing, EngineStatus::Installed)
                }
                _ => EngineStatus::Missing,
            },
        },
    }
}

/// Microsoft Office's card in Settings > Helper apps (spec 4.2, mock).
pub const OFFICE_LABEL: &str = "Microsoft Office";
/// What Office is for, under its name.
pub const OFFICE_PURPOSE: &str = "Word, Excel and PowerPoint files to PDF.";

/// What Settings says of Microsoft Office: Found or Not found, the apps
/// found, and their folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfficeStatus {
    pub badge: &'static str,
    /// "Word, Excel and PowerPoint"; `None` when none was found.
    pub apps: Option<String>,
    pub folder: Option<PathBuf>,
}

/// Settings' Office line from what `kiln_core::office::find_installed`
/// found (a worker looks; it reads the registry and the disk).
pub fn office_status(found: &kiln_core::office::Installed) -> OfficeStatus {
    let names: Vec<&str> = found.apps().iter().map(|a| a.name()).collect();
    let apps = match names.as_slice() {
        [] => None,
        [one] => Some((*one).to_owned()),
        [rest @ .., last] => Some(format!("{} and {last}", rest.join(", "))),
    };
    OfficeStatus {
        badge: if apps.is_some() { "Found" } else { "Not found" },
        apps,
        folder: found.folder().map(Path::to_path_buf),
    }
}

/// Why settings.toml couldn't be used. The app carries on with defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsError(pub String);

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The drag-wheel's setting, on Settings > Drag-wheel (mock screen 6).
pub const DRAG_WHEEL_LABEL: &str = "Pick a format while dragging anywhere in Windows";
/// The drag-wheel setting's note: Tap Shift, in one line at 1100 DIPs
pub const DRAG_WHEEL_NOTE: &str = "Tap Shift while dragging files to open the wheel at the \
     pointer. Kiln then starts with Windows.";
/// The drag-wheel setting's note when Windows refused its hooks.
pub const DRAG_WHEEL_REFUSED: &str =
    "Windows didn't allow the drag-wheel to watch the mouse and keys.";

const HEADER: &str = "\
# Kiln settings. Kiln rewrites this file when you change settings in the
# app; comments you add here are not kept.
#
# save_to = 'C:\\Users\\you\\Converted'         # where new files go; unset:
#                                             # next to the original

# output_folder = 'C:\\Users\\you\\Converted'   # where Save dialogs start;
#                                             # unset: the last folder you
#                                             # exported to, or the file's own
# theme = 'system'                            # system, light, dark, dracula,
#                                             # gruvbox, monokai, nord,
#                                             # solarized-dark, solarized-light
# drag_wheel = false                          # true: the wheel opens anywhere
#                                             # while dragging, and Kiln
#                                             # starts with Windows
# suggestions = true
# [quality]                                   # video and images: small, high
#                                             # or best; resolution and
#                                             # image_size: original or a
#                                             # number; audio_kbps: auto or a
#                                             # number; page_dpi: 36 to 600
# [engines]                                   # full paths to ffmpeg.exe, pandoc.exe,
#                                             # soffice.exe (libreoffice) and
#                                             # ebook-convert.exe (calibre)

";

impl Settings {
    pub fn parse(text: &str) -> Result<Settings, SettingsError> {
        let s: Settings = toml::from_str(text).map_err(|e| {
            SettingsError(format!(
                "settings.toml couldn't be read: {}",
                e.message().trim_end_matches('.')
            ))
        })?;
        if !s.quality.is_valid() {
            return Err(SettingsError(
                "settings.toml has a quality value out of range".to_owned(),
            ));
        }
        Ok(s.migrated())
    }

    /// A file from before phase 3 had the drag-wheel on unless it was
    /// turned off. It is turned off once, with a notice if it was on; the
    /// file is past that from then on, so turning it on again keeps.
    fn migrated(mut self) -> Settings {
        if !self.drag_wheel_reset {
            if self.drag_wheel {
                self.drag_wheel = false;
                self.pending_notice = Some(Notice::DragWheelOff);
            }
            self.drag_wheel_reset = true;
        }
        self
    }

    pub fn to_toml(&self) -> String {
        let body = toml::to_string(self).expect("settings always serialize");
        format!("{HEADER}{body}")
    }

    /// Read settings from `path`. A missing file means defaults, quietly;
    /// a broken one means defaults, so results go next to their originals,
    /// and a message saying why. The broken file itself is never written
    /// over by a save Kiln makes on its own (`Store`).
    pub fn load(path: &Path) -> (Settings, Option<SettingsError>) {
        match std::fs::read_to_string(path) {
            Ok(text) => match Settings::parse(&text) {
                Ok(s) => (s, None),
                Err(e) => (Settings::default(), Some(e)),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Settings::default(), None),
            Err(e) => (
                Settings::default(),
                Some(SettingsError(format!(
                    "settings.toml couldn't be opened: {e}"
                ))),
            ),
        }
    }

    /// Write settings to `path`, creating its folder. The file is written
    /// beside the old one, flushed to the disk and renamed over it, so a
    /// crash or a power cut never leaves it half written. Each call writes
    /// a temp file of its own ([`temp_beside`]), so two writers at once
    /// never write into the same one. This runs on the calling thread; the
    /// app saves through [`Store`], whose writer thread calls it.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        use std::io::Write;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let (tmp, mut file) = new_temp(|| temp_beside(path))?;
        let written = file
            .write_all(self.to_toml().as_bytes())
            .and_then(|()| file.sync_all());
        drop(file);
        let moved = written.and_then(|()| std::fs::rename(&tmp, path));
        if moved.is_err() {
            // Only the temp file this call made, never the settings file
            // or a temp file another run left.
            let _ = std::fs::remove_file(&tmp);
        }
        moved
    }

    /// The options a conversion from `from` to `to` runs with, at the
    /// default quality for that kind of result.
    pub fn options_for(&self, from: &Format, to: &Format) -> Options {
        self.options_with(self.quality.quality_for(from, to))
    }

    /// The options with a quality chosen for one conversion.
    pub fn options_with(&self, quality: kiln_core::Quality) -> Options {
        Options {
            jpeg_quality: quality.preset.jpeg_quality(),
            quality,
            ..self.options()
        }
    }

    /// The options every job runs with. The output folder is not set here:
    /// the app points jobs at its held folder, and `output_folder` only
    /// says where Save dialogs start.
    pub fn options(&self) -> Options {
        Options {
            engines: Engines {
                ffmpeg: self.engines.ffmpeg.clone(),
                pandoc: self.engines.pandoc.clone(),
                libreoffice: self.engines.libreoffice.clone(),
                calibre: self.engines.calibre.clone(),
            },
            ..Options::default()
        }
    }
}

/// `%APPDATA%\Kiln\settings.toml`, if APPDATA is set.
pub fn default_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA").filter(|v| !v.is_empty())?;
    Some(PathBuf::from(appdata).join("Kiln").join("settings.toml"))
}

/// `%LOCALAPPDATA%\Kiln\held`, where finished results wait to be exported,
/// if LOCALAPPDATA is set.
pub fn held_root() -> Option<PathBuf> {
    held_root_in(std::env::var_os("LOCALAPPDATA"))
}

fn held_root_in(local: Option<std::ffi::OsString>) -> Option<PathBuf> {
    kiln_local_in(local, "held")
}

/// Where `winget.exe` is: on `path_var`, or in the WindowsApps folder under
/// `local` (LOCALAPPDATA's value), where Windows puts it for the user.
pub fn find_winget(path_var: &OsStr, local: Option<&OsStr>) -> Option<PathBuf> {
    find_on_path(path_var, "winget").or_else(|| {
        let exe = Path::new(local.filter(|v| !v.is_empty())?)
            .join("Microsoft")
            .join("WindowsApps")
            .join("winget.exe");
        exe.is_file().then_some(exe)
    })
}

/// The command line after `cmd.exe` that runs `winget install` for package
/// `id` in a console that stays open for its output. It is passed as it is,
/// not quoted by Rust, because `cmd` has its own rules: `/s` makes it strip
/// exactly the outer pair of quotes, so a path with a space, `&`, `^` or
/// brackets stays one word, and `/d` skips any AutoRun command. `--source
/// winget` keeps the Store's source from asking which one is meant.
pub fn winget_console_args(winget: &Path, id: &str) -> std::ffi::OsString {
    let mut line = std::ffi::OsString::from("/d /s /k \"\"");
    line.push(winget);
    line.push("\" install -e --id ");
    line.push(id);
    line.push(" --source winget\"");
    line
}

/// `%LOCALAPPDATA%\Kiln\logs`, where `kiln.log` and crash files are
/// written, if LOCALAPPDATA is set.
pub fn logs_dir() -> Option<PathBuf> {
    logs_dir_in(std::env::var_os("LOCALAPPDATA"))
}

fn logs_dir_in(local: Option<std::ffi::OsString>) -> Option<PathBuf> {
    kiln_local_in(local, "logs")
}

/// `name` in Kiln's folder under `local`, LOCALAPPDATA's value.
fn kiln_local_in(local: Option<std::ffi::OsString>, name: &str) -> Option<PathBuf> {
    let local = local.filter(|v| !v.is_empty())?;
    Some(PathBuf::from(local).join("Kiln").join(name))
}

/// How long the settings writer waits after a save for another before it
/// writes the latest.
pub const SAVE_DEBOUNCE: Duration = Duration::from_millis(250);

/// The longest a save waits while more keep coming within the debounce, so a
/// setting changed again and again is still written.
const SAVE_WAIT_MAX: Duration = Duration::from_secs(1);

/// How long a write refused by another program (an antivirus or a backup
/// holding the file) waits before its one more try.
const RETRY_AFTER: Duration = Duration::from_millis(100);

/// Where settings are kept, whether the file there could be read, and the
/// thread that writes it.
///
/// A file that couldn't be read is never written over by a setting Kiln
/// changed itself. An explicit change in Settings still saves, but copies the
/// file aside first, so a hand edit with a mistake in it is never lost.
///
/// Saving never touches the disk on the calling thread. `save` and
/// `save_background` hand the settings to a writer thread, started on the
/// first save, which waits [`SAVE_DEBOUNCE`] for more and then writes the
/// latest: a temp file, `sync_all`, a rename. [`Store::flush`] blocks until
/// every save made before it is written, and dropping the store flushes, so
/// the process must flush (or drop the store) before it ends. A
/// [`Flusher`] from [`Store::flusher`] flushes from outside whatever owns
/// the store. A write that fails is logged, its line kept for
/// [`Store::take_error`], and its settings kept to be tried again by the
/// next flush, unless a newer save replaces them.
///
/// Keep one `Store` per settings file in a process: two would share the
/// temp file beside it.
pub struct Store {
    shared: Arc<Shared>,
    writer: Option<JoinHandle<()>>,
}

impl fmt::Debug for Store {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Store")
            .field("path", &self.shared.path)
            .field("writer", &self.writer.is_some())
            .finish_non_exhaustive()
    }
}

/// A handle that flushes a [`Store`]'s writer from anywhere: kept beside
/// the app, it works while the app (and the store in it) is borrowed.
/// Cheap to clone; it never starts a writer itself.
#[derive(Clone)]
pub struct Flusher(Arc<Shared>);

impl fmt::Debug for Flusher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Flusher").field(&self.0.path).finish()
    }
}

/// What the store, its flushers and its writer thread share.
struct Shared {
    path: Option<PathBuf>,
    state: Mutex<State>,
    /// Signalled on every change to `state`: a save, a flush, a write done,
    /// the writer gone.
    changed: Condvar,
    /// Called on the writer thread after each write, whatever its outcome.
    on_written: Mutex<Option<WrittenHook>>,
}

type WrittenHook = Box<dyn Fn() + Send>;

#[derive(Default)]
struct State {
    /// The latest settings not yet written. With `first` unset it is a
    /// write that failed, kept for the next flush or save.
    pending: Option<Pending>,
    /// Counts the saves made; `done` counts those the writer has finished
    /// with, written or failed.
    queued: u64,
    done: u64,
    /// Every save up to this count is written at once, without waiting.
    flush_to: u64,
    /// When the oldest pending save and the latest one were made.
    first: Option<Instant>,
    last: Option<Instant>,
    /// The writer stops once nothing is due.
    quit: bool,
    /// Whether a writer thread is running. It clears this however it ends.
    alive: bool,
    /// The file on disk couldn't be read (see `Store`). Changed only by
    /// `load` and by writes.
    unreadable: bool,
    /// The last write's failure, until taken or a later write succeeds.
    error: Option<String>,
    /// How many times the file was written, for tests.
    writes: u64,
    /// Tests: the writer thread panics when it next wakes.
    #[cfg(test)]
    die: bool,
    /// Tests: the writer never finds a save due on its own, only flushed.
    #[cfg(test)]
    hold: bool,
    /// Tests: each write first sleeps this long.
    #[cfg(test)]
    stall: Option<Duration>,
}

/// One coalesced save.
#[derive(Clone)]
struct Pending {
    settings: Settings,
    /// Whether any save folded into it was an explicit change in Settings,
    /// which may write over an unreadable file once it is copied aside.
    explicit: bool,
}

fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The line for a save with nowhere to go.
const NO_APPDATA: &str = "Settings can't be saved because APPDATA isn't set.";

impl Store {
    pub fn new(path: Option<PathBuf>) -> Store {
        Store {
            shared: Arc::new(Shared {
                path,
                state: Mutex::new(State::default()),
                changed: Condvar::new(),
                on_written: Mutex::new(None),
            }),
            writer: None,
        }
    }

    /// Read the settings, remembering whether the file was unreadable. This
    /// reads the disk on the calling thread: it is for start-up, before the
    /// window shows.
    pub fn load(&mut self) -> (Settings, Option<SettingsError>) {
        let Some(path) = &self.shared.path else {
            return (Settings::default(), None);
        };
        let (s, err) = Settings::load(path);
        lock(&self.shared.state).unreadable = err.is_some();
        (s, err)
    }

    /// Save after an explicit change in Settings. The settings are handed
    /// to the writer thread and written shortly after; this returns at once
    /// and never touches the disk. The only error here is that there is
    /// nowhere to save, the line for the status bar; a write that fails
    /// later is kept for [`Store::take_error`].
    pub fn save(&mut self, s: &Settings) -> Result<(), String> {
        if self.shared.path.is_none() {
            return Err(NO_APPDATA.to_owned());
        }
        self.queue(s, true);
        Ok(())
    }

    /// Save a setting Kiln changed itself, not the person in Settings. While
    /// the file on disk couldn't be read, it is changed in memory only. As
    /// `save`, this returns at once and never touches the disk.
    pub fn save_background(&mut self, s: &Settings) -> Result<(), String> {
        if self.shared.path.is_none() {
            return Err(NO_APPDATA.to_owned());
        }
        {
            let st = lock(&self.shared.state);
            // An explicit save not yet written may still make the file
            // readable; the writer checks again when it writes.
            let explicit_waiting = st.pending.as_ref().is_some_and(|p| p.explicit);
            let writing = st.done < st.queued;
            if st.unreadable && !explicit_waiting && !writing {
                return Ok(());
            }
        }
        self.queue(s, false);
        Ok(())
    }

    /// Block until every save made before this call is written, or has
    /// failed, without waiting out the debounce. Quit calls it before the
    /// process ends. A write that failed before and is still kept is tried
    /// again. The error is the kept one, if the last write failed; it stays
    /// kept.
    pub fn flush(&self) -> Result<(), String> {
        flush_shared(&self.shared, None).unwrap_or(Ok(()))
    }

    /// `flush`, but giving up after `wait`: `None` when the write wasn't
    /// done by then, which is logged. For `WM_ENDSESSION`, where Windows
    /// may end the process soon after.
    pub fn flush_timeout(&self, wait: Duration) -> Option<Result<(), String>> {
        flush_shared(&self.shared, Some(wait))
    }

    /// A handle that flushes this store from outside whatever owns it.
    pub fn flusher(&self) -> Flusher {
        Flusher(self.shared.clone())
    }

    /// The last write's failure, as the line for the status bar ("Settings
    /// couldn't be saved: …"), once: it is cleared when taken, and when a
    /// later write succeeds. Every failure is logged as well.
    pub fn take_error(&self) -> Option<String> {
        lock(&self.shared.state).error.take()
    }

    /// Call `f` on the writer thread after each write, whether it worked or
    /// not, so the window can be woken to call [`Store::take_error`]. `f`
    /// must not call back into the store. A panic in `f` is caught and
    /// logged.
    pub fn on_written(&mut self, f: impl Fn() + Send + 'static) {
        *self
            .shared
            .on_written
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Box::new(f));
    }

    /// How many times the file has been written.
    #[cfg(test)]
    fn writes(&self) -> u64 {
        lock(&self.shared.state).writes
    }

    /// Hand `s` to the writer, starting it on the first save, and again
    /// if it has stopped.
    fn queue(&mut self, s: &Settings, explicit: bool) {
        {
            let mut st = lock(&self.shared.state);
            let explicit = explicit || st.pending.as_ref().is_some_and(|p| p.explicit);
            st.pending = Some(Pending {
                settings: s.clone(),
                explicit,
            });
            st.queued += 1;
            let now = Instant::now();
            // A failed write kept for later has no `first`: this save is
            // the start of the wait.
            st.first.get_or_insert(now);
            st.last = Some(now);
        }
        self.shared.changed.notify_all();
        // A writer that has stopped (it never should) is replaced.
        if self.writer.as_ref().is_some_and(|h| h.is_finished()) {
            if let Some(h) = self.writer.take() {
                let _ = h.join();
            }
            crate::diag::log("The settings writer stopped; starting it again.");
        }
        if self.writer.is_none() {
            let shared = self.shared.clone();
            lock(&shared.state).alive = true;
            match std::thread::Builder::new()
                .name("kiln-settings".into())
                .spawn(move || write_loop(&shared))
            {
                Ok(h) => self.writer = Some(h),
                // No thread: write here and now rather than not at all.
                Err(e) => {
                    lock(&self.shared.state).alive = false;
                    crate::diag::log(&format!("The settings writer couldn't start: {e}"));
                    write_next(&self.shared);
                }
            }
        }
    }
}

impl Flusher {
    /// [`Store::flush`], from outside the store.
    pub fn flush(&self) -> Result<(), String> {
        flush_shared(&self.0, None).unwrap_or(Ok(()))
    }

    /// [`Store::flush_timeout`], from outside the store.
    pub fn flush_timeout(&self, wait: Duration) -> Option<Result<(), String>> {
        flush_shared(&self.0, Some(wait))
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        let Some(writer) = self.writer.take() else {
            return;
        };
        let _ = self.flush();
        lock(&self.shared.state).quit = true;
        self.shared.changed.notify_all();
        let _ = writer.join();
    }
}

/// Flush `shared`'s writer, waiting at most `wait` when given: `None` when
/// it gave up. When no writer runs, what is due is written on this thread.
fn flush_shared(shared: &Shared, wait: Option<Duration>) -> Option<Result<(), String>> {
    let deadline = wait.map(|w| Instant::now() + w);
    let mut st = lock(&shared.state);
    // A failed write still kept is due again, as a save of its own.
    if st.pending.is_some() && st.first.is_none() {
        st.queued += 1;
        let now = Instant::now();
        st.first = Some(now);
        st.last = Some(now);
    }
    let target = st.queued;
    st.flush_to = st.flush_to.max(target);
    shared.changed.notify_all();
    while st.done < target {
        if !st.alive {
            // Nobody else will write it.
            drop(st);
            write_next(shared);
            st = lock(&shared.state);
            if st.done < target && st.pending.is_none() {
                break;
            }
            continue;
        }
        st = match deadline {
            None => shared
                .changed
                .wait(st)
                .unwrap_or_else(PoisonError::into_inner),
            Some(at) => {
                let now = Instant::now();
                if now >= at {
                    crate::diag::log("Settings weren't written in time to flush them.");
                    return None;
                }
                shared
                    .changed
                    .wait_timeout(st, at - now)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0
            }
        };
    }
    Some(st.error.clone().map_or(Ok(()), Err))
}

/// Clears `alive` when the writer thread ends, even by a panic, so a flush
/// waiting on it writes for itself.
struct Gone<'a>(&'a Shared);

impl Drop for Gone<'_> {
    fn drop(&mut self) {
        lock(&self.0.state).alive = false;
        self.0.changed.notify_all();
    }
}

/// The writer thread: wait for a save, wait out the debounce, write the
/// latest; until the store is dropped.
fn write_loop(shared: &Shared) {
    let _gone = Gone(shared);
    loop {
        {
            let mut st = lock(&shared.state);
            loop {
                #[cfg(test)]
                if std::mem::take(&mut st.die) {
                    panic!("the settings writer was told to stop");
                }
                if let (Some(_), Some(first), Some(last)) = (&st.pending, st.first, st.last) {
                    let due = (last + SAVE_DEBOUNCE).min(first + SAVE_WAIT_MAX);
                    #[cfg(test)]
                    let due = if st.hold {
                        Instant::now() + Duration::from_secs(3600)
                    } else {
                        due
                    };
                    let now = Instant::now();
                    if st.flush_to > st.done || st.quit || now >= due {
                        break;
                    }
                    st = shared
                        .changed
                        .wait_timeout(st, due - now)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0;
                } else if st.quit {
                    return;
                } else {
                    st = shared
                        .changed
                        .wait(st)
                        .unwrap_or_else(PoisonError::into_inner);
                }
            }
        }
        write_next(shared);
    }
}

/// Write the pending settings, if any, and record how it went.
fn write_next(shared: &Shared) {
    let Some(path) = &shared.path else {
        return;
    };
    let (job, seq, unreadable) = {
        let mut st = lock(&shared.state);
        let Some(job) = st.pending.take() else {
            return;
        };
        st.first = None;
        st.last = None;
        #[cfg(test)]
        if let Some(d) = st.stall {
            drop(st);
            std::thread::sleep(d);
            st = lock(&shared.state);
        }
        (job, st.queued, st.unreadable)
    };
    // A panic here must not leave `flush` waiting for ever.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        write_job(path, &job, unreadable)
    }))
    .unwrap_or_else(|_| Outcome {
        wrote: false,
        unreadable,
        error: Some("Settings couldn't be saved: the writer stopped.".to_owned()),
    });
    if let Some(e) = &outcome.error {
        crate::diag::log(e);
    }
    {
        let mut st = lock(&shared.state);
        st.done = seq;
        st.unreadable = outcome.unreadable;
        if outcome.wrote {
            st.writes += 1;
            st.error = None;
        } else if outcome.error.is_some() && st.pending.is_none() {
            // Nothing newer came: keep these settings for the next flush
            // or save, without a time, so the writer doesn't try them
            // again on its own.
            st.pending = Some(job);
        }
        if outcome.error.is_some() {
            st.error = outcome.error;
        }
    }
    shared.changed.notify_all();
    let hook = shared
        .on_written
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Some(f) = hook.as_ref() {
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err() {
            crate::diag::log("The settings writer's wake-up panicked.");
        }
    }
}

/// What one write did.
struct Outcome {
    wrote: bool,
    /// Whether the file is still one that couldn't be read.
    unreadable: bool,
    error: Option<String>,
}

/// Whether a write failed because another program held the file for a
/// moment: access denied, or a sharing or lock violation.
fn held_by_another(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::PermissionDenied || matches!(e.raw_os_error(), Some(32 | 33))
}

/// Write `job` to `path`, copying an unreadable file aside first when the
/// person changed a setting, and leaving it alone when Kiln did.
fn write_job(path: &Path, job: &Pending, unreadable: bool) -> Outcome {
    let failed = |e: std::io::Error, unreadable| Outcome {
        wrote: false,
        unreadable,
        error: Some(format!("Settings couldn't be saved: {e}")),
    };
    if unreadable {
        if !job.explicit {
            return Outcome {
                wrote: false,
                unreadable: true,
                error: None,
            };
        }
        match keep_unreadable(path) {
            Ok(_) => {}
            // Gone since it was read: there's nothing to keep.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            // Not copied, so not written over: still unreadable.
            Err(e) => return failed(e, true),
        }
    }
    let saved = match job.settings.save(path) {
        Err(e) if held_by_another(&e) => {
            std::thread::sleep(RETRY_AFTER);
            job.settings.save(path)
        }
        other => other,
    };
    match saved {
        Ok(()) => Outcome {
            wrote: true,
            unreadable: false,
            error: None,
        },
        // Copied aside already, so the next save may write.
        Err(e) => failed(e, false),
    }
}

/// Copy an unreadable settings file to `settings.toml.bad` beside it, or
/// `settings.toml.bad (1)`, `settings.toml.bad (2)` and so on when that's
/// taken. It never overwrites or deletes anything.
pub fn keep_unreadable(path: &Path) -> std::io::Result<PathBuf> {
    use std::io::Write;
    let bytes = std::fs::read(path)?;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    for n in 0..1000 {
        let bad = if n == 0 {
            path.with_file_name(format!("{name}.bad"))
        } else {
            path.with_file_name(format!("{name}.bad ({n})"))
        };
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&bad)
        {
            Ok(mut f) => {
                f.write_all(&bytes)?;
                f.sync_all()?;
                return Ok(bad);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "every settings.toml.bad name is taken",
    ))
}

/// Optional paths where an empty string also means unset.
mod opt_path {
    use std::path::PathBuf;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(p: &Option<PathBuf>, s: S) -> Result<S::Ok, S::Error> {
        match p {
            Some(p) => s.serialize_str(&p.to_string_lossy()),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<PathBuf>, D::Error> {
        let s = Option::<String>::deserialize(d)?;
        Ok(s.filter(|s| !s.trim().is_empty()).map(PathBuf::from))
    }
}

/// A temp file beside `path` that no other save uses: named for this
/// process and a count, so the writer thread and a flush at the end of a
/// session, or two Kilns, never write into the same one.
fn temp_beside(path: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    path.with_file_name(format!("{name}.{}-{n}.tmp", std::process::id()))
}

/// Make a new temp file at the first of `names` that nothing is at. A
/// name taken already (a crashed run with the same pid left it) is passed
/// over and left be. Gives up after 100 names taken.
fn new_temp(mut names: impl FnMut() -> PathBuf) -> std::io::Result<(PathBuf, std::fs::File)> {
    let mut taken = 0;
    loop {
        let tmp = names();
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
        {
            Ok(file) => return Ok((tmp, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && taken < 100 => taken += 1,
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_engine_says_what_it_is_for() {
        let lines: Vec<_> = Engine::ALL.iter().map(|e| e.purpose()).collect();
        assert_eq!(
            lines,
            [
                "Other video formats, GIFs, and making videos smaller.",
                "Documents to EPUB, HTML and Markdown.",
                // No promise of Office to PDF through Microsoft Office.
                "Office files and documents to PDF.",
                "E-books: EPUB, MOBI and AZW3.",
            ]
        );
    }

    #[test]
    fn a_missing_helper_sends_people_to_get_it_only_where_there_is_a_page() {
        // kiln-core's sentence for a missing helper says where to get it
        // only for a helper with a Get it here.
        for e in Engine::ALL {
            assert_eq!(
                kiln_core::has_download_page(e.label()),
                e.download_page().is_some(),
                "{e:?}"
            );
        }
    }

    #[test]
    fn get_it_opens_only_the_official_pages() {
        let pages: Vec<_> = Engine::ALL.iter().map(|e| e.download_page()).collect();
        assert_eq!(
            pages,
            [
                // FFmpeg comes with Kiln: nothing sends anyone to its site.
                None,
                Some("https://pandoc.org/installing.html"),
                Some("https://www.libreoffice.org/download/"),
                Some("https://calibre-ebook.com/download"),
            ]
        );
    }

    #[test]
    fn an_empty_file_means_defaults() {
        let s = Settings::parse("").unwrap();
        assert_eq!(s, Settings::default());
        assert_eq!(s.output_folder, None);
        assert_eq!(s.theme, ThemeChoice::System);
        assert_eq!(s.engines, EnginePaths::default());
        assert_eq!(s.options(), Options::default());
    }

    #[test]
    fn every_field_parses() {
        let s = Settings::parse(
            r#"
            output_folder = 'C:\Users\me\Converted'
            theme = "light"
            [engines]
            ffmpeg = 'D:\tools\ffmpeg.exe'
            "#,
        )
        .unwrap();
        assert_eq!(
            s.output_folder,
            Some(PathBuf::from(r"C:\Users\me\Converted"))
        );
        assert_eq!(s.theme, ThemeChoice::Light);
        assert_eq!(
            s.engines.ffmpeg,
            Some(PathBuf::from(r"D:\tools\ffmpeg.exe"))
        );
        assert_eq!(s.engines.pandoc, None);
        let o = s.options();
        // The folder only says where Save dialogs start; jobs write to
        // the held folder, which the app sets.
        assert_eq!(o.out_dir, None);
        assert_eq!(o.engines.ffmpeg, s.engines.ffmpeg);
    }

    #[test]
    fn results_are_held_under_local_app_data() {
        assert_eq!(
            held_root_in(Some(r"C:\Users\me\AppData\Local".into())),
            Some(PathBuf::from(r"C:\Users\me\AppData\Local\Kiln\held"))
        );
        assert_eq!(held_root_in(Some("".into())), None);
        assert_eq!(held_root_in(None), None);
    }

    #[test]
    fn winget_ids_are_for_the_three_helpers_that_are_not_included() {
        assert_eq!(Engine::Ffmpeg.winget_id(), None);
        assert_eq!(Engine::Pandoc.winget_id(), Some("JohnMacFarlane.Pandoc"));
        assert_eq!(
            Engine::LibreOffice.winget_id(),
            Some("TheDocumentFoundation.LibreOffice")
        );
        assert_eq!(Engine::Calibre.winget_id(), Some("calibre.calibre"));
    }

    #[test]
    fn winget_is_found_on_path_or_in_windows_apps() {
        let dir = std::env::temp_dir().join(format!("kiln-winget-{}", std::process::id()));
        let apps = dir.join("Microsoft").join("WindowsApps");
        std::fs::create_dir_all(&apps).unwrap();
        let nowhere = std::ffi::OsString::from(dir.join("empty"));
        assert_eq!(find_winget(&nowhere, Some(dir.as_os_str())), None);
        assert_eq!(find_winget(&nowhere, None), None);
        std::fs::write(apps.join("winget.exe"), b"").unwrap();
        let exe = apps.join("winget.exe");
        assert_eq!(
            find_winget(&nowhere, Some(dir.as_os_str())),
            Some(exe.clone())
        );
        assert_eq!(find_winget(&nowhere, Some("".as_ref())), None);
        // On PATH, that is the one used.
        let on_path = std::env::join_paths([&apps]).unwrap();
        assert_eq!(find_winget(&on_path, None), Some(exe));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn winget_installs_by_exact_id_in_a_console_that_stays_open() {
        let line = |p: &str| winget_console_args(Path::new(p), "calibre.calibre");
        assert_eq!(
            line(r"C:\Apps\winget.exe"),
            r#"/d /s /k ""C:\Apps\winget.exe" install -e --id calibre.calibre --source winget""#
        );
        // `&`, and a space with brackets, stay inside the quotes.
        assert_eq!(
            line(r"C:\R&D\winget.exe"),
            r#"/d /s /k ""C:\R&D\winget.exe" install -e --id calibre.calibre --source winget""#
        );
        assert_eq!(
            line(r"C:\Program Files (x86)\A@b\winget.exe"),
            r#"/d /s /k ""C:\Program Files (x86)\A@b\winget.exe" install -e --id calibre.calibre --source winget""#
        );
    }

    #[test]
    fn logs_are_kept_under_local_app_data() {
        assert_eq!(
            logs_dir_in(Some(r"C:\Users\me\AppData\Local".into())),
            Some(PathBuf::from(r"C:\Users\me\AppData\Local\Kiln\logs"))
        );
        assert_eq!(logs_dir_in(Some("".into())), None);
        assert_eq!(logs_dir_in(None), None);
    }

    #[test]
    fn output_folder_only_says_where_exports_start() {
        // Files from before the change still load, and the key stays.
        let s = Settings::parse(r"output_folder = 'C:\Exports'").unwrap();
        assert_eq!(s.output_folder, Some(PathBuf::from(r"C:\Exports")));
        assert_eq!(s.options().out_dir, None);
        let text = s.to_toml();
        assert!(text.contains("\noutput_folder = "), "{text}");
        assert_eq!(Settings::parse(&text).unwrap(), s);
        // The comment no longer promises output next to each file.
        let header = text
            .lines()
            .find(|l| l.contains("# output_folder"))
            .unwrap();
        assert!(header.contains("Save dialogs start"), "{header}");
        assert!(!header.contains("next to each file"), "{header}");
    }

    #[test]
    fn empty_paths_mean_unset() {
        let s = Settings::parse("output_folder = ''\n[engines]\npandoc = '  '").unwrap();
        assert_eq!(s.output_folder, None);
        assert_eq!(s.engines.pandoc, None);
    }

    #[test]
    fn mistakes_are_reported_plainly() {
        for (text, needle) in [
            ("colour = 'dark'", "colour"),
            ("drag_wheel = 'sepia'", "sepia"),
            ("theme = ", "couldn't be read"),
        ] {
            let e = Settings::parse(text).unwrap_err();
            assert!(e.0.contains(needle), "{text:?} gave {e}");
            assert!(e.0.starts_with("settings.toml"));
            assert!(!e.0.contains('!'));
        }
    }

    /// How many temp files saving left in `dir`.
    fn temps_in(dir: &Path) -> usize {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .count()
    }

    #[test]
    fn each_save_writes_a_temp_file_of_its_own() {
        let path = Path::new(r"C:\x\settings.toml");
        let (a, b) = (temp_beside(path), temp_beside(path));
        assert_ne!(a, b);
        assert_eq!(a.parent(), path.parent());
        let pid = std::process::id().to_string();
        assert!(a.to_string_lossy().contains(&pid));
        assert!(a.to_string_lossy().ends_with(".tmp"));
    }

    #[test]
    fn a_temp_name_a_crashed_run_left_is_passed_over_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        // A crash between the write and the rename left the first name;
        // a later Kiln with the same pid comes to it.
        let left = dir.path().join("settings.toml.7-0.tmp");
        std::fs::write(&left, "left").unwrap();
        let mut n = 0;
        let names = || {
            n += 1;
            dir.path().join(format!("settings.toml.7-{}.tmp", n - 1))
        };
        let (tmp, file) = new_temp(names).unwrap();
        drop(file);
        assert_eq!(tmp, dir.path().join("settings.toml.7-1.tmp"));
        assert_eq!(std::fs::read_to_string(&left).unwrap(), "left");
        let _ = std::fs::remove_file(&tmp);
        // And a save beside it goes ahead, leaving it be.
        Settings::default().save(&path).unwrap();
        assert!(path.is_file());
        assert_eq!(std::fs::read_to_string(&left).unwrap(), "left");
    }

    #[test]
    fn two_writers_at_once_leave_whole_settings_and_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let one = Settings {
            suggestions: false,
            ..Settings::default()
        };
        let other = Settings::default();
        let writers: Vec<_> = [one.clone(), other.clone()]
            .into_iter()
            .map(|s| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for _ in 0..50 {
                        // A rename over a file the other is renaming can
                        // fail for a moment; a whole file is what counts.
                        let _ = s.save(&path);
                    }
                })
            })
            .collect();
        for w in writers {
            w.join().unwrap();
        }
        let (read, error) = Settings::load(&path);
        assert_eq!(error, None);
        assert!(read == one || read == other);
        assert_eq!(temps_in(dir.path()), 0);
    }

    #[test]
    fn saved_settings_read_back_the_same() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Kiln").join("settings.toml");
        assert_eq!(Settings::load(&path), (Settings::default(), None));

        // Each field away from its default, so each is shown to come back.
        let mut s = Settings {
            output_folder: Some(dir.path().join("out")),
            theme: ThemeChoice::Dark,
            told_about_tray: true,
            first_run_dismissed: true,
            save_to: Some(dir.path().join("Converted files")),
            ..Settings::default()
        };
        s.engines
            .set(Engine::Pandoc, Some(PathBuf::from(r"C:\pandoc\pandoc.exe")));
        s.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# Kiln settings."));
        assert_eq!(Settings::load(&path), (s, None));
        assert_eq!(temps_in(path.parent().unwrap()), 0);

        // The defaults write a file that parses back to the defaults.
        assert_eq!(
            Settings::parse(&Settings::default().to_toml()).unwrap(),
            Settings::default()
        );
    }

    #[test]
    fn a_file_from_when_kiln_asked_about_saving_still_loads() {
        // Kiln once asked, the first time a result was ready, and wrote
        // `save_asked`. Such a file still loads, and saving it again drops
        // the retired fields.
        for text in [
            "save_next_to_original = true\nsave_asked = true\n",
            "save_asked = true\n",
            "save_asked = false\n",
        ] {
            let s = Settings::parse(text).unwrap();
            let again = s.to_toml();
            assert!(!again.contains("save_asked"), "{again}");
            assert!(!again.contains("save_next_to_original"), "{again}");
            let back = Settings::parse(&again).unwrap();
            assert_eq!(
                back,
                Settings {
                    save_asked: false,
                    ..s
                }
            );
        }
    }

    /// A settings.toml from when results could wait in Kiln, with the
    /// setting off and every other key away from its default.
    const HOLDING: &str = r#"
output_folder = 'D:\Converted'
save_to = 'D:\Chosen'
theme = "light"
suggestions = false
save_next_to_original = false
drag_wheel = true
drag_wheel_reset = true
told_about_tray = true
first_run_dismissed = true

[quality]
video = "small"
images = "best"

[engines]
pandoc = 'D:\tools\pandoc.exe'
"#;

    #[test]
    fn an_old_file_with_the_hold_setting_loads_with_every_other_value_kept() {
        let s = Settings::parse(HOLDING).unwrap();
        assert_eq!(s.output_folder, Some(PathBuf::from(r"D:\Converted")));
        assert_eq!(s.save_to, Some(PathBuf::from(r"D:\Chosen")));
        assert_eq!(s.theme, ThemeChoice::Light);
        assert!(!s.suggestions);
        assert!(s.drag_wheel, "past the turn-off, it stays on");
        assert!(s.drag_wheel_reset);
        assert!(s.told_about_tray);
        assert!(s.first_run_dismissed);
        assert_eq!(s.quality.video, kiln_core::Preset::Small);
        assert_eq!(s.quality.images, kiln_core::Preset::Best);
        assert_eq!(
            s.engines.pandoc,
            Some(PathBuf::from(r"D:\tools\pandoc.exe"))
        );
        assert_eq!(s.pending_notice, None);
        // On, or a value of the wrong kind, loads too, the same.
        for line in [
            "save_next_to_original = true",
            "save_next_to_original = 'yes'",
            "save_next_to_original = 3",
        ] {
            let text = HOLDING.replace("save_next_to_original = false", line);
            assert_eq!(Settings::parse(&text).unwrap(), s, "{line}");
        }
    }

    #[test]
    fn the_retired_hold_setting_is_never_written() {
        let s = Settings::parse(HOLDING).unwrap();
        let text = s.to_toml();
        assert!(!text.contains("save_next_to_original"), "{text}");
        assert!(!HEADER.contains("save_next_to_original"));
        assert!(!HEADER.contains("keep results in Kiln"));
        assert_eq!(Settings::parse(&text).unwrap(), s);
    }
    #[test]
    fn the_drag_wheel_and_the_tray_notice_read_back() {
        // A file written before the drag-wheel existed: nothing has been
        // said about the notification area yet.
        let old = "theme = 'dark'\nsave_next_to_original = true\n[keys]\nconvert = 'shift'\n";
        let s = Settings::parse(old).unwrap();
        assert!(!s.told_about_tray);

        // Turned off, it stays off; told, it stays told.
        let mut s = Settings {
            drag_wheel: false,
            told_about_tray: true,
            ..Settings::default()
        };
        let back = Settings::parse(&s.to_toml()).unwrap();
        assert_eq!(back, s);
        assert!(s.to_toml().contains("drag_wheel = false"));
        assert!(s.to_toml().contains("told_about_tray = true"));

        // Not yet told is the default and isn't written.
        s.told_about_tray = false;
        assert!(!s.to_toml().contains("told_about_tray"));
    }

    /// A settings.toml as phase 2 wrote it, with every key it had set
    /// away from its default.
    const PHASE_2: &str = r#"
output_folder = 'D:\Converted'
theme = "dark"
suggestions = false
drop_tip_shown = true
save_next_to_original = false
drag_wheel = true
told_about_tray = true
welcome_shown = true

[quality]
video = "small"
resolution = 720
fps = 30
images = "best"
image_size = 1920
audio_kbps = 192
page_dpi = 300

[engines]
ffmpeg = 'D:\tools\ffmpeg.exe'
pandoc = 'D:\tools\pandoc.exe'
libreoffice = 'C:\Program Files\LibreOffice\program\soffice.exe'
calibre = 'C:\Program Files\Calibre2\ebook-convert.exe'
"#;

    #[test]
    fn a_phase_2_file_loads_with_every_kept_setting() {
        let s = Settings::parse(PHASE_2).unwrap();
        assert_eq!(s.output_folder, Some(PathBuf::from(r"D:\Converted")));
        assert_eq!(s.theme, ThemeChoice::Dark);
        assert!(!s.suggestions);
        assert!(s.told_about_tray);
        assert!(s.first_run_dismissed);
        assert_eq!(s.save_to, None);
        assert_eq!(s.quality.video, kiln_core::Preset::Small);
        assert_eq!(s.quality.resolution, kiln_core::Limit::Max(720));
        assert_eq!(s.quality.images, kiln_core::Preset::Best);
        assert_eq!(s.quality.image_size, kiln_core::Limit::Max(1920));
        assert_eq!(s.quality.audio_kbps, Some(192));
        assert_eq!(s.quality.page_dpi, 300);
        assert_eq!(
            s.engines.ffmpeg,
            Some(PathBuf::from(r"D:\tools\ffmpeg.exe"))
        );
        assert_eq!(
            s.engines.pandoc,
            Some(PathBuf::from(r"D:\tools\pandoc.exe"))
        );
        assert_eq!(
            s.engines.libreoffice,
            Some(PathBuf::from(
                r"C:\Program Files\LibreOffice\program\soffice.exe"
            ))
        );
        assert_eq!(
            s.engines.calibre,
            Some(PathBuf::from(
                r"C:\Program Files\Calibre2\ebook-convert.exe"
            ))
        );

        // The frame rate and the drop tip are read and thrown away: saved
        // again, neither is written, and what is written reads back the
        // same.
        let written = s.to_toml();
        let body: Vec<&str> = written.lines().filter(|l| !l.starts_with('#')).collect();
        for gone in ["fps", "drop_tip_shown", "welcome_shown"] {
            assert!(
                !body.iter().any(|l| l.contains(gone)),
                "{gone} in {written}"
            );
        }
        assert!(
            written.contains("\nfirst_run_dismissed = true"),
            "{written}"
        );
        let back = Settings::parse(&written).unwrap();
        assert_eq!(
            back,
            Settings {
                pending_notice: None,
                ..s.clone()
            }
        );
    }

    #[test]
    fn a_new_install_has_the_drag_wheel_off() {
        let s = Settings::default();
        assert!(!s.drag_wheel);
        assert_eq!(s.save_to, None);
        assert!(!s.first_run_dismissed);
        assert_eq!(s.pending_notice, None);
        // No file at all is a new install.
        let dir = tempfile::tempdir().unwrap();
        let (s, err) = Settings::load(&dir.path().join("settings.toml"));
        assert_eq!((s, err), (Settings::default(), None));
        // A new install's file is past the one-time turn-off, so the
        // drag-wheel turned on later stays on.
        let on = Settings {
            drag_wheel: true,
            ..Settings::default()
        };
        let back = Settings::parse(&on.to_toml()).unwrap();
        assert!(back.drag_wheel);
        assert_eq!(back.pending_notice, None);
    }

    #[test]
    fn the_drag_wheel_is_turned_off_once_with_a_notice() {
        // A file from before phase 3 with the drag-wheel on: off, once,
        // with the notice to show.
        let s = Settings::parse(PHASE_2).unwrap();
        assert!(!s.drag_wheel);
        assert_eq!(s.pending_notice, Some(Notice::DragWheelOff));
        assert_eq!(
            Notice::DragWheelOff.text(),
            "The drag-wheel is now off. You can turn it back on in Settings."
        );
        // Saved, the turn-off is remembered...
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        s.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\ndrag_wheel_reset = true"), "{text}");
        assert!(text.contains("\ndrag_wheel = false"), "{text}");
        let (again, err) = Settings::load(&path);
        assert_eq!(err, None);
        assert!(!again.drag_wheel);
        assert_eq!(again.pending_notice, None);
        // ...so turned back on, it stays on, with nothing more to say.
        let on = Settings {
            drag_wheel: true,
            ..again
        };
        on.save(&path).unwrap();
        let (back, _) = Settings::load(&path);
        assert!(back.drag_wheel);
        assert_eq!(back.pending_notice, None);

        // Off already, there is nothing to say, and the turn-off is
        // remembered all the same.
        let off = Settings::parse("drag_wheel = false\n").unwrap();
        assert!(!off.drag_wheel);
        assert_eq!(off.pending_notice, None);
        assert!(off.to_toml().contains("\ndrag_wheel_reset = true"));
        // Without the line (a file from before the drag-wheel) it reads as
        // off, quietly.
        let none = Settings::parse("theme = 'dark'\n").unwrap();
        assert!(!none.drag_wheel);
        assert_eq!(none.pending_notice, None);
        // The notice is never written to the file.
        assert!(!s.to_toml().contains("notice"));
    }

    #[test]
    fn welcome_shown_reads_as_first_run_dismissed() {
        let s = Settings::parse("welcome_shown = true\n").unwrap();
        assert!(s.first_run_dismissed);
        let text = s.to_toml();
        assert!(text.contains("\nfirst_run_dismissed = true"), "{text}");
        assert!(
            !text.lines().any(|l| l.starts_with("welcome_shown")),
            "{text}"
        );
        assert!(Settings::parse(&text).unwrap().first_run_dismissed);
        // Not dismissed is the default and isn't written.
        assert!(!Settings::parse("").unwrap().first_run_dismissed);
        assert!(!Settings::default()
            .to_toml()
            .contains("\nfirst_run_dismissed"));
    }

    #[test]
    fn save_to_round_trips() {
        for folder in [
            r"D:\Converted",
            r"C:\Users\me\My Pictures\Converted files",
            r"C:\Users\Zoë\Fotos é ü 写真",
        ] {
            let s = Settings {
                save_to: Some(PathBuf::from(folder)),
                ..Settings::default()
            };
            let text = s.to_toml();
            assert!(text.contains("\nsave_to = "), "{text}");
            assert_eq!(Settings::parse(&text).unwrap(), s, "{folder}");
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("settings.toml");
            s.save(&path).unwrap();
            assert_eq!(Settings::load(&path), (s, None));
        }
        // Unset isn't written; an empty string means unset.
        assert!(!Settings::default().to_toml().contains("\nsave_to"));
        assert_eq!(Settings::parse("save_to = ''").unwrap().save_to, None);
        assert_eq!(Settings::parse("save_to = '  '").unwrap().save_to, None);
    }

    #[test]
    fn a_broken_file_falls_back_to_defaults_with_a_reason() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "theme = 'dark'\ntheme = 'light'").unwrap();
        let (s, e) = Settings::load(&path);
        // The defaults: results go next to their originals.
        assert_eq!(s, Settings::default());
        assert!(e.unwrap().0.contains("couldn't be read"));
    }

    #[test]
    fn an_unreadable_file_means_defaults_and_results_go_next_to_the_original() {
        let dir = tempfile::tempdir().unwrap();
        // A file there that can't be parsed: the defaults, with the reason.
        let broken = dir.path().join("broken.toml");
        std::fs::write(&broken, "theme = ").unwrap();
        let (s, e) = Settings::load(&broken);
        assert_eq!(s, Settings::default());
        assert_eq!(s.save_to, None, "next to the original");
        assert!(e.is_some());
        // Nor can one be opened: here a folder has the file's name.
        let folder = dir.path().join("folder.toml");
        std::fs::create_dir(&folder).unwrap();
        let (s, e) = Settings::load(&folder);
        assert_eq!(s, Settings::default());
        assert!(e.unwrap().0.contains("couldn't be opened"));
        // With no file at all it is a new install, quietly.
        let (s, e) = Settings::load(&dir.path().join("missing.toml"));
        assert_eq!(s, Settings::default());
        assert_eq!(e, None);
    }
    #[test]
    fn a_background_save_never_writes_over_an_unreadable_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let bad = b"colour = 'dark'\noutput_folder = 'D:\\Mine'\n";
        std::fs::write(&path, bad).unwrap();
        let mut store = Store::new(Some(path.clone()));
        let (mut s, e) = store.load();
        assert!(e.is_some());
        s.told_about_tray = true;
        assert_eq!(store.save_background(&s), Ok(()));
        // Written on the writer thread: wait for it.
        store.flush().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bad);
        assert!(!dir.path().join("settings.toml.bad").exists());
        assert_eq!(temps_in(path.parent().unwrap()), 0);
    }

    #[test]
    fn an_explicit_save_copies_an_unreadable_file_aside_first() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let bad = b"colour = 'dark'\n";
        std::fs::write(&path, bad).unwrap();
        // Copies from an earlier time are kept too.
        let first = dir.path().join("settings.toml.bad");
        std::fs::write(&first, b"older").unwrap();
        let mut store = Store::new(Some(path.clone()));
        let (mut s, _) = store.load();
        s.theme = ThemeChoice::Dark;
        store.save(&s).unwrap();
        store.flush().unwrap();
        assert_eq!(std::fs::read(&first).unwrap(), b"older");
        let kept = dir.path().join("settings.toml.bad (1)");
        assert_eq!(std::fs::read(&kept).unwrap(), bad);
        assert_eq!(Settings::load(&path), (s.clone(), None));
        // Readable now: a background save is written, and nothing more is copied.
        s.told_about_tray = true;
        store.save_background(&s).unwrap();
        store.flush().unwrap();
        assert_eq!(Settings::load(&path), (s, None));
        assert!(!dir.path().join("settings.toml.bad (2)").exists());
    }

    #[test]
    fn a_readable_file_saves_in_the_background() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let mut store = Store::new(Some(path.clone()));
        let (mut s, e) = store.load();
        assert!(e.is_none());
        s.told_about_tray = true;
        store.save_background(&s).unwrap();
        store.flush().unwrap();
        assert_eq!(Settings::load(&path), (s, None));
        assert!(Store::new(None).save(&Settings::default()).is_err());
    }

    #[test]
    fn ten_saves_in_a_row_are_one_write_of_the_last() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let mut store = Store::new(Some(path.clone()));
        let (mut s, _) = store.load();
        let started = Instant::now();
        for i in 0..10 {
            s.theme = ThemeChoice::ALL[i % ThemeChoice::ALL.len()];
            s.told_about_tray = i % 2 == 1;
            if i % 2 == 0 {
                store.save(&s).unwrap();
            } else {
                store.save_background(&s).unwrap();
            }
        }
        // Saving returned at once: nothing is written yet.
        let (writes, there) = (store.writes(), path.exists());
        assert!(started.elapsed() < SAVE_DEBOUNCE, "{:?}", started.elapsed());
        assert_eq!(writes, 0);
        assert!(!there);
        // Once the debounce has passed, the writer writes the latest, once.
        let deadline = Instant::now() + Duration::from_secs(10);
        while store.writes() == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(started.elapsed() >= SAVE_DEBOUNCE);
        std::thread::sleep(SAVE_DEBOUNCE * 2);
        assert_eq!(store.writes(), 1);
        assert_eq!(Settings::load(&path), (s.clone(), None));
        assert_eq!(store.flush(), Ok(()));
        assert_eq!(store.writes(), 1, "nothing was left to flush");
        assert_eq!(temps_in(path.parent().unwrap()), 0);
    }

    #[test]
    fn flush_writes_without_waiting_out_the_debounce() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Kiln").join("settings.toml");
        let mut store = Store::new(Some(path.clone()));
        let (mut s, _) = store.load();
        // A flush with nothing saved returns at once.
        assert_eq!(store.flush(), Ok(()));
        // Held, the writer never writes on its own: only the flush can.
        lock(&store.shared.state).hold = true;
        s.first_run_dismissed = true;
        store.save(&s).unwrap();
        std::thread::sleep(SAVE_DEBOUNCE * 2);
        assert_eq!(store.writes(), 0);
        assert_eq!(store.flush(), Ok(()));
        assert_eq!(store.writes(), 1);
        assert_eq!(Settings::load(&path), (s.clone(), None));
        // And again for the next save.
        s.told_about_tray = true;
        store.save_background(&s).unwrap();
        store.flush().unwrap();
        assert_eq!(store.writes(), 2);
        assert_eq!(Settings::load(&path), (s, None));
    }

    #[test]
    fn dropping_the_store_writes_what_is_waiting() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let s = Settings {
            theme: ThemeChoice::Light,
            ..Settings::default()
        };
        {
            let mut store = Store::new(Some(path.clone()));
            store.load();
            store.save(&s).unwrap();
            assert!(!path.exists());
        }
        assert_eq!(Settings::load(&path), (s, None));
        // A store that never saved starts no writer and writes nothing.
        let other = dir.path().join("other.toml");
        drop(Store::new(Some(other.clone())));
        assert!(!other.exists());
    }

    #[test]
    fn a_failed_write_is_logged_and_kept_for_settings_to_show() {
        let dir = tempfile::tempdir().unwrap();
        // The folder settings.toml should go in is a file.
        let blocker = dir.path().join("Kiln");
        std::fs::write(&blocker, b"not a folder").unwrap();
        let path = blocker.join("settings.toml");
        let mut store = Store::new(Some(path.clone()));
        let (tx, rx) = std::sync::mpsc::channel();
        store.on_written(move || {
            let _ = tx.send(());
        });
        let mut s = Settings::default();
        store.save(&s).unwrap();
        let e = store.flush().unwrap_err();
        assert!(e.starts_with("Settings couldn't be saved: "), "{e}");
        // The window was told, and the line waits until it is taken, once.
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(store.take_error(), Some(e));
        assert_eq!(store.take_error(), None);
        assert_eq!(store.writes(), 0);
        assert_eq!(std::fs::read(&blocker).unwrap(), b"not a folder");
        // A later write that works clears a line not yet taken.
        store.save(&s).unwrap();
        assert!(store.flush().is_err());
        std::fs::remove_file(&blocker).unwrap();
        s.theme = ThemeChoice::Dark;
        store.save(&s).unwrap();
        assert_eq!(store.flush(), Ok(()));
        assert_eq!(store.take_error(), None);
        assert_eq!(Settings::load(&path), (s, None));
    }

    #[test]
    fn an_unreadable_file_that_cant_be_copied_aside_is_not_written_over() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let bad = b"colour = 'dark'\n";
        std::fs::write(&path, bad).unwrap();
        let mut store = Store::new(Some(path.clone()));
        let (mut s, e) = store.load();
        assert!(e.is_some());
        // Every name a copy could take is taken by a folder.
        std::fs::create_dir(dir.path().join("settings.toml.bad")).unwrap();
        for n in 1..1000 {
            std::fs::create_dir(dir.path().join(format!("settings.toml.bad ({n})"))).unwrap();
        }
        s.theme = ThemeChoice::Dark;
        store.save(&s).unwrap();
        // A background save behind it is folded in, and not written either.
        s.told_about_tray = true;
        store.save_background(&s).unwrap();
        assert!(store.flush().is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bad);
        assert_eq!(store.writes(), 0);
        // The person's change is kept and tried again by the next flush,
        // which still can't copy the file aside, so still doesn't write.
        store.take_error();
        assert!(store.flush().is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bad);
        // Once a copy can be made, the kept change is written.
        std::fs::remove_dir(dir.path().join("settings.toml.bad")).unwrap();
        assert_eq!(store.flush(), Ok(()));
        assert_eq!(
            std::fs::read(dir.path().join("settings.toml.bad")).unwrap(),
            bad
        );
        assert_eq!(Settings::load(&path), (s, None));
        assert_eq!(store.writes(), 1);
    }

    #[test]
    fn a_background_save_alone_leaves_an_unreadable_file_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let bad = b"colour = 'dark'\n";
        std::fs::write(&path, bad).unwrap();
        let mut store = Store::new(Some(path.clone()));
        let (mut s, _) = store.load();
        s.told_about_tray = true;
        store.save_background(&s).unwrap();
        assert_eq!(store.flush(), Ok(()));
        assert_eq!(std::fs::read(&path).unwrap(), bad);
        assert_eq!(store.writes(), 0);
    }

    #[test]
    fn a_panicking_wake_up_leaves_the_writer_writing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let mut store = Store::new(Some(path.clone()));
        store.on_written(|| panic!("a wake-up that panics"));
        let mut s = Settings::default();
        store.save(&s).unwrap();
        assert_eq!(store.flush(), Ok(()));
        s.theme = ThemeChoice::Light;
        store.save(&s).unwrap();
        assert_eq!(store.flush(), Ok(()));
        assert_eq!(store.writes(), 2);
        assert_eq!(Settings::load(&path), (s, None));
        assert!(!store.writer.as_ref().unwrap().is_finished());
    }

    #[test]
    fn a_writer_that_stopped_is_written_for_and_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let mut store = Store::new(Some(path.clone()));
        let mut s = Settings::default();
        // The writer dies as it wakes for this save.
        lock(&store.shared.state).die = true;
        store.save(&s).unwrap();
        // The flush writes it itself, once the writer is gone.
        assert_eq!(store.flush(), Ok(()));
        assert_eq!(Settings::load(&path), (s.clone(), None));
        let deadline = Instant::now() + Duration::from_secs(10);
        while !store.writer.as_ref().unwrap().is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(store.writer.as_ref().unwrap().is_finished());
        // The next save starts a new writer, which writes as before.
        s.theme = ThemeChoice::Dark;
        store.save(&s).unwrap();
        assert!(!store.writer.as_ref().unwrap().is_finished());
        assert_eq!(store.flush(), Ok(()));
        assert_eq!(Settings::load(&path), (s.clone(), None));
        // And dropping it flushes too.
        s.theme = ThemeChoice::Light;
        store.save(&s).unwrap();
        drop(store);
        assert_eq!(Settings::load(&path), (s, None));
    }

    #[test]
    fn a_flusher_flushes_from_another_thread_while_the_store_is_held() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let mut store = Store::new(Some(path.clone()));
        let flusher = store.flusher();
        // Nothing saved yet: it returns at once.
        assert_eq!(flusher.flush(), Ok(()));
        let s = Settings {
            theme: ThemeChoice::Dark,
            ..Settings::default()
        };
        lock(&store.shared.state).hold = true;
        store.save(&s).unwrap();
        // The store stays borrowed here while another thread flushes.
        let held = &mut store;
        let other = flusher.clone();
        let r = std::thread::spawn(move || other.flush_timeout(Duration::from_secs(5)))
            .join()
            .unwrap();
        assert_eq!(r, Some(Ok(())));
        assert_eq!(held.writes(), 1);
        assert_eq!(Settings::load(&path), (s, None));
    }

    #[test]
    fn a_flush_with_a_timeout_gives_up_and_the_write_still_happens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let mut store = Store::new(Some(path.clone()));
        lock(&store.shared.state).stall = Some(Duration::from_millis(600));
        let s = Settings {
            theme: ThemeChoice::Light,
            ..Settings::default()
        };
        store.save(&s).unwrap();
        assert_eq!(store.flush_timeout(Duration::from_millis(50)), None);
        // The write goes on, and a blocking flush waits for it.
        assert_eq!(store.flush(), Ok(()));
        assert_eq!(Settings::load(&path), (s, None));
    }

    #[test]
    fn a_failed_write_is_kept_and_written_by_the_next_flush() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("Kiln");
        std::fs::write(&blocker, b"not a folder").unwrap();
        let path = blocker.join("settings.toml");
        let mut store = Store::new(Some(path.clone()));
        let s = Settings {
            theme: ThemeChoice::Dark,
            ..Settings::default()
        };
        store.save(&s).unwrap();
        assert!(store.flush().is_err());
        // The writer doesn't keep trying on its own.
        std::thread::sleep(SAVE_DEBOUNCE * 2);
        assert_eq!(store.writes(), 0);
        // Once it can be written, a flush alone writes the kept settings.
        std::fs::remove_file(&blocker).unwrap();
        assert_eq!(store.flush(), Ok(()));
        assert_eq!(store.writes(), 1);
        assert_eq!(Settings::load(&path), (s, None));
    }

    #[test]
    fn every_theme_has_a_file_name_that_reads_back() {
        let names: Vec<&str> = ThemeChoice::ALL.iter().map(|t| t.name()).collect();
        assert_eq!(
            names,
            [
                "system",
                "light",
                "dark",
                "dracula",
                "gruvbox",
                "monokai",
                "nord",
                "solarized-dark",
                "solarized-light"
            ]
        );
        for t in ThemeChoice::ALL {
            let text = format!("theme = '{}'", t.name());
            assert_eq!(Settings::parse(&text).unwrap().theme, t, "{text}");
            let s = Settings {
                theme: t,
                ..Settings::default()
            };
            let written = toml::to_string(&s).unwrap();
            assert!(
                written.contains(&format!("theme = \"{}\"", t.name())),
                "{written}"
            );
            assert_eq!(Settings::parse(&written).unwrap().theme, t);
        }
    }

    #[test]
    fn an_unknown_theme_name_is_system_and_keeps_the_rest_of_the_file() {
        for odd in ["sepia", "Nord", "", "solarized dark"] {
            let text = format!(
                "theme = '{odd}'\nsuggestions = false\nsave_to = 'D:\\Mine'\n[quality]\npage_dpi = 300\n"
            );
            let s = Settings::parse(&text).unwrap_or_else(|e| panic!("{odd:?}: {e}"));
            assert_eq!(s.theme, ThemeChoice::System, "{odd:?}");
            assert!(!s.suggestions, "{odd:?}");
            assert_eq!(s.save_to, Some(PathBuf::from(r"D:\Mine")), "{odd:?}");
            assert_ne!(s.quality, QualityDefaults::default(), "{odd:?}");
        }
        // A value that isn't even a word is read the same way.
        let s = Settings::parse("theme = 7\nsuggestions = false").unwrap();
        assert_eq!((s.theme, s.suggestions), (ThemeChoice::System, false));
        // Loading it reports nothing wrong.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "theme = 'sepia'\nsuggestions = false\n").unwrap();
        let (s, e) = Settings::load(&path);
        assert_eq!(
            (s.theme, s.suggestions, e),
            (ThemeChoice::System, false, None)
        );
    }

    #[test]
    fn named_themes_are_the_ones_with_a_base_of_their_own() {
        use ThemeChoice::*;
        for t in [System, Light, Dark] {
            assert!(!t.is_named(), "{t:?}");
        }
        for t in [
            Dracula,
            Gruvbox,
            Monokai,
            Nord,
            SolarizedDark,
            SolarizedLight,
        ] {
            assert!(t.is_named(), "{t:?}");
        }
    }

    #[test]
    fn engines_are_found_in_settings_then_on_path() {
        let dir = tempfile::tempdir().unwrap();
        let name = if cfg!(windows) {
            "ffmpeg.exe"
        } else {
            "ffmpeg"
        };
        let exe = dir.path().join(name);
        std::fs::write(&exe, b"").unwrap();
        let path_var = std::env::join_paths([dir.path()]).unwrap();
        let empty = std::ffi::OsString::new();

        let mut s = Settings::default();
        assert_eq!(
            engine_status(&s, Engine::Ffmpeg, &path_var),
            EngineStatus::OnPath(exe.clone())
        );
        assert_eq!(
            engine_status(&s, Engine::Pandoc, &path_var),
            EngineStatus::Missing
        );
        assert_eq!(
            engine_status(&s, Engine::Ffmpeg, &empty).describe(),
            "Not installed"
        );

        s.engines.set(Engine::Ffmpeg, Some(exe.clone()));
        let st = engine_status(&s, Engine::Ffmpeg, &empty);
        assert_eq!(
            st,
            EngineStatus::Chosen {
                path: exe.clone(),
                exists: true
            }
        );
        assert_eq!(st.describe(), exe.display().to_string());

        s.engines
            .set(Engine::Ffmpeg, Some(dir.path().join("gone.exe")));
        assert!(engine_status(&s, Engine::Ffmpeg, &path_var)
            .describe()
            .starts_with("Not found at"));
    }

    #[test]
    fn office_says_which_apps_it_found_and_where() {
        use kiln_core::office::Installed;
        let folder = Path::new(r"C:\Program Files\Microsoft Office\root\Office16");
        let all = Installed {
            word: Some(folder.join("WINWORD.EXE")),
            excel: Some(folder.join("EXCEL.EXE")),
            powerpoint: Some(folder.join("POWERPNT.EXE")),
        };
        let s = office_status(&all);
        assert_eq!(s.badge, "Found");
        assert_eq!(s.apps.as_deref(), Some("Word, Excel and PowerPoint"));
        assert_eq!(s.folder.as_deref(), Some(folder));
        assert_eq!(OFFICE_LABEL, "Microsoft Office");
        assert_eq!(OFFICE_PURPOSE, "Word, Excel and PowerPoint files to PDF.");

        let two = Installed {
            powerpoint: None,
            ..all.clone()
        };
        assert_eq!(office_status(&two).apps.as_deref(), Some("Word and Excel"));
        let word = Installed {
            word: all.word.clone(),
            ..Installed::default()
        };
        assert_eq!(office_status(&word).apps.as_deref(), Some("Word"));

        let none = office_status(&Installed::default());
        assert_eq!(none.badge, "Not found");
        assert_eq!(none.apps, None);
        assert_eq!(none.folder, None);
    }

    #[test]
    fn the_ffmpeg_that_comes_with_kiln_is_included() {
        // Settings' path, then the copy beside Kiln, then PATH.
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("app").join("ffmpeg").join("bin");
        let on_path = root.path().join("path");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&on_path).unwrap();
        for dir in [&bin, &on_path] {
            std::fs::write(dir.join("ffmpeg.exe"), b"").unwrap();
        }
        let path_var = std::env::join_paths([&on_path]).unwrap();
        let mut s = Settings::default();

        let st = engine_status_in(&s, Engine::Ffmpeg, &path_var, Some(&bin));
        assert_eq!(st, EngineStatus::Included(bin.join("ffmpeg.exe")));
        assert_eq!(st.describe(), "Included");
        // Without it, PATH as before.
        assert_eq!(
            engine_status_in(&s, Engine::Ffmpeg, &path_var, None),
            EngineStatus::OnPath(on_path.join("ffmpeg.exe"))
        );
        // A bundled folder means nothing to the other helpers.
        assert_eq!(
            engine_status_in(&s, Engine::Pandoc, &path_var, Some(&bin)),
            EngineStatus::Missing
        );
        // A chosen copy still comes first.
        let chosen = on_path.join("ffmpeg.exe");
        s.engines.set(Engine::Ffmpeg, Some(chosen.clone()));
        assert_eq!(
            engine_status_in(&s, Engine::Ffmpeg, &path_var, Some(&bin)),
            EngineStatus::Chosen {
                path: chosen,
                exists: true
            }
        );
    }

    #[test]
    fn suggestions_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let s = Settings {
            suggestions: false,
            ..Settings::default()
        };
        s.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("suggestions = false"), "{text}");
        assert_eq!(Settings::load(&path), (s, None));
    }

    #[test]
    fn missing_suggestions_means_on() {
        let s = Settings::parse("theme = 'dark'").unwrap();
        assert!(s.suggestions);
        assert!(Settings::default().suggestions);
    }

    /// A settings.toml from the first phase, with every key since removed,
    /// loads with every other setting as it was, and saving it afterwards
    /// writes none of the removed keys.
    #[test]
    fn an_old_file_with_removed_keys_keeps_every_other_setting() {
        let old = r#"
output_folder = 'D:\Converted'
theme = "dark"
suggestions = false
drag_wheel = false
drop_tip_shown = true
save_next_to_original = false
told_about_tray = true
welcome_shown = true
save_asked = true

[keys]
convert = "alt"
tools = "shift"
actions = "ctrl"

[habits.heic]
jpg = 5
png = 2

[habits.png]
webp = 3

[quality]
video = "small"

[engines]
ffmpeg = 'D:\tools\ffmpeg.exe'
"#;
        let s = Settings::parse(old).unwrap();
        assert_eq!(s.output_folder, Some(PathBuf::from(r"D:\Converted")));
        assert_eq!(s.theme, ThemeChoice::Dark);
        assert!(!s.suggestions);
        assert!(!s.drag_wheel);
        assert!(s.told_about_tray);
        assert!(s.first_run_dismissed);
        assert_eq!(
            s.engines.ffmpeg,
            Some(PathBuf::from(r"D:\tools\ffmpeg.exe"))
        );
        assert_eq!(s.quality.video, kiln_core::Preset::Small);

        let written = s.to_toml();
        for gone in [
            "habits",
            "keys",
            "actions",
            "heic",
            "save_asked",
            "drop_tip_shown",
            "welcome_shown",
        ] {
            assert!(!written.contains(gone), "{gone} in {written}");
        }
        let back = Settings::parse(&written).unwrap();
        // `save_asked` is the one retired field the file had read as true.
        assert_eq!(
            back,
            Settings {
                save_asked: false,
                ..s.clone()
            }
        );
        assert_eq!(back.theme, ThemeChoice::Dark);

        // A table of the removed kind that is not even the right shape
        // is thrown away too.
        let odd = Settings::parse(
            "theme = 'light'
habits = 3",
        )
        .unwrap();
        assert_eq!(odd.theme, ThemeChoice::Light);
        assert!(!odd.to_toml().contains("habits"));
    }

    #[test]
    fn the_settings_file_lives_in_appdata() {
        // Read-only check of the shape; the variable itself belongs to the
        // machine running the tests.
        if let Some(p) = default_path() {
            assert!(p.ends_with(Path::new("Kiln").join("settings.toml")));
        }
    }
}
