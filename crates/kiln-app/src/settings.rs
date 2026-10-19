//! `%APPDATA%\Kiln\settings.toml`: where results go, which key opens which
//! wheel, the theme, quality defaults and engine paths. Every field is
//! optional; anything left out takes its default.

use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use kiln_core::{Engines, Format, Habits, Options};

use crate::quality::QualityDefaults;
use crate::wheel::WheelKind;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// Where results are written. Unset writes each result next to its
    /// input.
    #[serde(skip_serializing_if = "Option::is_none", with = "opt_path")]
    pub output_folder: Option<PathBuf>,
    pub theme: ThemeChoice,
    /// Whether Kiln looks at dropped files and suggests what to do.
    #[serde(default = "yes")]
    pub suggestions: bool,
    pub keys: KeyMap,
    pub quality: QualityDefaults,
    pub engines: EnginePaths,
    /// Conversions picked, counted per source type on this PC.
    #[serde(default, skip_serializing_if = "Habits::is_empty")]
    pub habits: Habits,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            output_folder: None,
            theme: ThemeChoice::default(),
            suggestions: true,
            keys: KeyMap::default(),
            quality: QualityDefaults::default(),
            engines: EnginePaths::default(),
            habits: Habits::default(),
        }
    }
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    /// Follow the Windows app mode.
    #[default]
    System,
    Dark,
    Light,
}

impl ThemeChoice {
    pub const ALL: [ThemeChoice; 3] = [ThemeChoice::System, ThemeChoice::Dark, ThemeChoice::Light];

    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::System => "System",
            ThemeChoice::Dark => "Dark",
            ThemeChoice::Light => "Light",
        }
    }

    /// Whether to draw the light palette, given the Windows app mode.
    pub fn is_light(self, system_light: bool) -> bool {
        match self {
            ThemeChoice::System => system_light,
            ThemeChoice::Dark => false,
            ThemeChoice::Light => true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Modifier {
    Shift,
    #[serde(alias = "control")]
    Ctrl,
    Alt,
}

impl Modifier {
    pub const ALL: [Modifier; 3] = [Modifier::Shift, Modifier::Ctrl, Modifier::Alt];

    /// As shown on a key chip: "SHIFT".
    pub fn code(self) -> &'static str {
        match self {
            Modifier::Shift => "SHIFT",
            Modifier::Ctrl => "CTRL",
            Modifier::Alt => "ALT",
        }
    }

    /// As written in a sentence: "Shift".
    pub fn name(self) -> &'static str {
        match self {
            Modifier::Shift => "Shift",
            Modifier::Ctrl => "Ctrl",
            Modifier::Alt => "Alt",
        }
    }

    fn next(self) -> Modifier {
        match self {
            Modifier::Shift => Modifier::Ctrl,
            Modifier::Ctrl => Modifier::Alt,
            Modifier::Alt => Modifier::Shift,
        }
    }
}

/// Which modifier opens which wheel while dragging.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct KeyMap {
    pub convert: Modifier,
    pub tools: Modifier,
    pub actions: Modifier,
}

impl Default for KeyMap {
    fn default() -> Self {
        Self {
            convert: Modifier::Shift,
            tools: Modifier::Ctrl,
            actions: Modifier::Alt,
        }
    }
}

impl KeyMap {
    pub fn key_for(&self, wheel: WheelKind) -> Modifier {
        match wheel {
            WheelKind::Convert => self.convert,
            WheelKind::Tools => self.tools,
            WheelKind::Actions => self.actions,
        }
    }

    fn slot(&mut self, wheel: WheelKind) -> &mut Modifier {
        match wheel {
            WheelKind::Convert => &mut self.convert,
            WheelKind::Tools => &mut self.tools,
            WheelKind::Actions => &mut self.actions,
        }
    }

    /// The wheel for the held modifiers. With more than one held, the
    /// first of Shift, Ctrl, Alt decides.
    pub fn wheel_for(&self, shift: bool, ctrl: bool, alt: bool) -> Option<WheelKind> {
        let held = [
            (Modifier::Shift, shift),
            (Modifier::Ctrl, ctrl),
            (Modifier::Alt, alt),
        ];
        let (m, _) = held.into_iter().find(|(_, down)| *down)?;
        WheelKind::ALL.into_iter().find(|w| self.key_for(*w) == m)
    }

    /// Give `wheel` the key `m`. The wheel that had `m` takes the old key,
    /// so every wheel keeps a key of its own.
    pub fn assign(&mut self, wheel: WheelKind, m: Modifier) {
        let old = self.key_for(wheel);
        if let Some(other) = WheelKind::ALL.into_iter().find(|w| self.key_for(*w) == m) {
            *self.slot(other) = old;
        }
        *self.slot(wheel) = m;
    }

    /// Move `wheel` to the next key, swapping as `assign` does.
    pub fn cycle(&mut self, wheel: WheelKind) {
        self.assign(wheel, self.key_for(wheel).next());
    }

    /// The empty window's hint, naming the keys as they are now set.
    pub fn hint(&self) -> String {
        format!(
            "Hold {} while dropping to convert, {} for tools, or {} for file actions.",
            self.convert.name(),
            self.tools.name(),
            self.actions.name()
        )
    }

    fn is_valid(&self) -> bool {
        self.convert != self.tools && self.tools != self.actions && self.convert != self.actions
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

/// The engines Kiln runs as separate programs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    Missing,
}

impl EngineStatus {
    /// The line shown in Settings > Engines.
    pub fn describe(&self) -> String {
        match self {
            EngineStatus::Chosen { path, exists: true } => path.display().to_string(),
            EngineStatus::Chosen { path, .. } => format!("Not found at {}", path.display()),
            EngineStatus::OnPath(_) => "Found on PATH".to_owned(),
            EngineStatus::Installed(path) => path.display().to_string(),
            EngineStatus::Missing => "Not installed".to_owned(),
        }
    }
}

pub use kiln_core::find_on_path;

pub fn engine_status(settings: &Settings, e: Engine, path_var: &OsStr) -> EngineStatus {
    match settings.engines.get(e) {
        Some(p) => EngineStatus::Chosen {
            path: p.to_owned(),
            exists: p.is_file(),
        },
        None => match find_on_path(path_var, e.program()) {
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

/// Why settings.toml couldn't be used. The app carries on with defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsError(pub String);

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

const HEADER: &str = "\
# Kiln settings. Kiln rewrites this file when you change settings in the
# app; comments you add here are not kept.
#
# output_folder = 'C:\\Users\\you\\Converted'   # unset: next to each file
# theme = 'system'                            # system, dark or light
# suggestions = true
# [keys]                                      # shift, ctrl or alt, one each
# [quality]                                   # video and images: small, high
#                                             # or best; resolution, fps and
#                                             # image_size: original or a
#                                             # number; audio_kbps: auto or a
#                                             # number; page_dpi: 36 to 600
# [engines]                                   # full paths to ffmpeg.exe, pandoc.exe,
#                                             # soffice.exe (libreoffice) and
#                                             # ebook-convert.exe (calibre)
# [habits]                                    # picks per source type, counted on this PC

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
        if !s.keys.is_valid() {
            return Err(SettingsError(
                "settings.toml gives two wheels the same key".to_owned(),
            ));
        }
        Ok(s)
    }

    pub fn to_toml(&self) -> String {
        let body = toml::to_string(self).expect("settings always serialize");
        format!("{HEADER}{body}")
    }

    /// Read settings from `path`. A missing file means defaults, quietly;
    /// a broken one means defaults and a message saying why.
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
    /// beside the old one and renamed over it, so a crash never leaves it
    /// half written.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, self.to_toml())?;
        std::fs::rename(&tmp, path)
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

    /// The options every job runs with.
    pub fn options(&self) -> Options {
        Options {
            out_dir: self.output_folder.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hint_follows_the_keys() {
        let mut keys = KeyMap::default();
        assert_eq!(
            keys.hint(),
            "Hold Shift while dropping to convert, Ctrl for tools, or Alt for file actions."
        );
        keys.assign(WheelKind::Convert, Modifier::Alt);
        assert_eq!(
            keys.hint(),
            "Hold Alt while dropping to convert, Ctrl for tools, or Shift for file actions."
        );
    }

    #[test]
    fn an_empty_file_means_defaults() {
        let s = Settings::parse("").unwrap();
        assert_eq!(s, Settings::default());
        assert_eq!(s.output_folder, None);
        assert_eq!(s.theme, ThemeChoice::System);
        assert_eq!(s.keys.convert, Modifier::Shift);
        assert_eq!(s.keys.tools, Modifier::Ctrl);
        assert_eq!(s.keys.actions, Modifier::Alt);
        assert_eq!(s.engines, EnginePaths::default());
        assert_eq!(s.options(), Options::default());
    }

    #[test]
    fn every_field_parses() {
        let s = Settings::parse(
            r#"
            output_folder = 'C:\Users\me\Converted'
            theme = "light"
            [keys]
            convert = "alt"
            tools = "control"
            actions = "shift"
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
        assert_eq!(s.keys.convert, Modifier::Alt);
        assert_eq!(s.keys.tools, Modifier::Ctrl);
        assert_eq!(s.keys.actions, Modifier::Shift);
        assert_eq!(
            s.engines.ffmpeg,
            Some(PathBuf::from(r"D:\tools\ffmpeg.exe"))
        );
        assert_eq!(s.engines.pandoc, None);
        let o = s.options();
        assert_eq!(o.out_dir, s.output_folder);
        assert_eq!(o.engines.ffmpeg, s.engines.ffmpeg);
    }

    #[test]
    fn partial_sections_keep_their_other_defaults() {
        let s = Settings::parse("[keys]\nconvert = 'ctrl'\ntools = 'shift'").unwrap();
        assert_eq!(s.keys.actions, Modifier::Alt);
        assert_eq!(s.theme, ThemeChoice::System);
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
            ("theme = 'sepia'", "sepia"),
            ("colour = 'dark'", "colour"),
            ("[keys]\nconvert = 'win'", "win"),
            ("theme = ", "couldn't be read"),
        ] {
            let e = Settings::parse(text).unwrap_err();
            assert!(e.0.contains(needle), "{text:?} gave {e}");
            assert!(e.0.starts_with("settings.toml"));
            assert!(!e.0.contains('!'));
        }
        let e = Settings::parse("[keys]\nconvert = 'ctrl'").unwrap_err();
        assert_eq!(e.0, "settings.toml gives two wheels the same key");
    }

    #[test]
    fn saved_settings_read_back_the_same() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Kiln").join("settings.toml");
        assert_eq!(Settings::load(&path), (Settings::default(), None));

        let mut s = Settings {
            output_folder: Some(dir.path().join("out")),
            theme: ThemeChoice::Dark,
            ..Settings::default()
        };
        s.keys.assign(WheelKind::Actions, Modifier::Shift);
        s.engines
            .set(Engine::Pandoc, Some(PathBuf::from(r"C:\pandoc\pandoc.exe")));
        s.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# Kiln settings."));
        assert_eq!(Settings::load(&path), (s, None));
        assert!(!path.with_extension("toml.tmp").exists());

        // The defaults write a file that parses back to the defaults.
        assert_eq!(
            Settings::parse(&Settings::default().to_toml()).unwrap(),
            Settings::default()
        );
    }

    #[test]
    fn a_broken_file_falls_back_to_defaults_with_a_reason() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "theme = 'dark'\ntheme = 'light'").unwrap();
        let (s, e) = Settings::load(&path);
        assert_eq!(s, Settings::default());
        assert!(e.unwrap().0.contains("couldn't be read"));
    }

    #[test]
    fn modifiers_pick_wheels_through_the_map() {
        let k = KeyMap::default();
        assert_eq!(k.wheel_for(true, false, false), Some(WheelKind::Convert));
        assert_eq!(k.wheel_for(false, true, false), Some(WheelKind::Tools));
        assert_eq!(k.wheel_for(false, false, true), Some(WheelKind::Actions));
        assert_eq!(k.wheel_for(false, false, false), None);
        assert_eq!(k.wheel_for(true, true, false), Some(WheelKind::Convert));

        let mut k = KeyMap::default();
        k.assign(WheelKind::Convert, Modifier::Alt);
        assert_eq!(k.wheel_for(false, false, true), Some(WheelKind::Convert));
        assert_eq!(k.wheel_for(true, false, false), Some(WheelKind::Actions));
    }

    #[test]
    fn assigning_a_key_swaps_so_each_wheel_keeps_one() {
        let mut k = KeyMap::default();
        k.assign(WheelKind::Tools, Modifier::Shift);
        assert_eq!(
            (k.convert, k.tools, k.actions),
            (Modifier::Ctrl, Modifier::Shift, Modifier::Alt)
        );
        assert!(k.is_valid());
        for _ in 0..7 {
            k.cycle(WheelKind::Actions);
            assert!(k.is_valid());
        }
        k.assign(WheelKind::Convert, k.convert);
        assert!(k.is_valid());
    }

    #[test]
    fn theme_follows_windows_unless_overridden() {
        assert!(ThemeChoice::System.is_light(true));
        assert!(!ThemeChoice::System.is_light(false));
        assert!(!ThemeChoice::Dark.is_light(true));
        assert!(ThemeChoice::Light.is_light(false));
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
    fn habits_and_suggestions_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let mut s = Settings {
            suggestions: false,
            ..Settings::default()
        };
        for _ in 0..3 {
            s.habits.record("heic", "png");
        }
        s.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("suggestions = false"), "{text}");
        assert!(text.contains("[habits]"), "{text}");
        assert_eq!(Settings::load(&path), (s, None));
    }

    #[test]
    fn missing_suggestions_means_on() {
        let s = Settings::parse("theme = 'dark'").unwrap();
        assert!(s.suggestions);
        assert!(s.habits.is_empty());
        assert!(Settings::default().suggestions);
        // No picks yet: no [habits] table is written.
        assert!(!Settings::default().to_toml().contains(
            "
[habits]"
        ));
    }

    #[test]
    fn a_bad_habit_is_a_settings_error() {
        let e = Settings::parse(
            "[habits]
heic = 3",
        )
        .unwrap_err();
        assert!(e.0.starts_with("settings.toml couldn't be read"), "{e}");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(
            &path,
            "[habits]
heic = 3",
        )
        .unwrap();
        let (s, e) = Settings::load(&path);
        assert_eq!(s, Settings::default());
        assert!(matches!(e, Some(SettingsError(_))));
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
