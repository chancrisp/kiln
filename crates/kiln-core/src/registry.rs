//! The format registry, loaded from formats.toml.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde::Deserialize;

use crate::error::{Error, Result};

/// The registry that ships with Kiln.
pub const DEFAULT_FORMATS: &str = include_str!("../formats.toml");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Image,
    Audio,
    Video,
    Document,
    Spreadsheet,
    Presentation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Format {
    /// Canonical extension, lowercase, no dot. Used for output names.
    pub ext: String,
    pub kind: Kind,
    /// FFmpeg arguments for writing this type.
    pub ffmpeg: Vec<String>,
    /// Pandoc's name for this type.
    pub pandoc: Option<String>,
    pub standalone: bool,
    /// LibreOffice's `--convert-to` value for writing this type.
    pub libreoffice: Option<String>,
    /// LibreOffice's `--infilter` value for reading this type, when it
    /// can't tell on its own.
    pub libreoffice_infilter: Option<String>,
}

impl Format {
    /// The format code shown to users: "PNG", "MP4", "DOCX".
    pub fn code(&self) -> String {
        self.ext.to_ascii_uppercase()
    }
}

#[derive(Debug, Clone)]
pub struct Registry {
    formats: BTreeMap<String, Format>,
    aliases: HashMap<String, String>,
    /// (from, to) -> backend id.
    routes: BTreeMap<(String, String), String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    formats: BTreeMap<String, RawFormat>,
    #[serde(default, rename = "route")]
    routes: Vec<RawRoute>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFormat {
    kind: Kind,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    ffmpeg: Vec<String>,
    pandoc: Option<String>,
    #[serde(default)]
    standalone: bool,
    libreoffice: Option<String>,
    libreoffice_infilter: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRoute {
    backend: String,
    from: Vec<String>,
    to: Vec<String>,
}

fn bad(msg: String) -> Error {
    Error::Registry(msg)
}

fn valid_ext(ext: &str) -> bool {
    !ext.is_empty()
        && ext
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

impl Registry {
    pub fn builtin() -> Self {
        Self::from_toml(DEFAULT_FORMATS).expect("built-in formats.toml is valid")
    }

    /// Parse a registry. Duplicate extensions, aliases or pairs are errors.
    pub fn from_toml(text: &str) -> Result<Self> {
        let raw: RawFile = toml::from_str(text).map_err(|e| bad(e.to_string()))?;

        let mut formats = BTreeMap::new();
        let mut aliases: HashMap<String, String> = HashMap::new();
        for (ext, f) in raw.formats {
            if !valid_ext(&ext) {
                return Err(bad(format!(
                    "\"{ext}\" isn't a valid extension. Use lowercase letters and digits."
                )));
            }
            for alias in f.aliases {
                if !valid_ext(&alias) {
                    return Err(bad(format!("\"{alias}\" isn't a valid alias.")));
                }
                if let Some(prev) = aliases.insert(alias.clone(), ext.clone()) {
                    return Err(bad(format!(
                        "The alias \"{alias}\" is declared for both {prev} and {ext}."
                    )));
                }
            }
            formats.insert(
                ext.clone(),
                Format {
                    ext,
                    kind: f.kind,
                    ffmpeg: f.ffmpeg,
                    pandoc: f.pandoc,
                    standalone: f.standalone,
                    libreoffice: f.libreoffice,
                    libreoffice_infilter: f.libreoffice_infilter,
                },
            );
        }
        for (alias, ext) in &aliases {
            if formats.contains_key(alias) {
                return Err(bad(format!(
                    "\"{alias}\" is both a format and an alias of {ext}."
                )));
            }
        }

        let mut routes = BTreeMap::new();
        for route in raw.routes {
            for ext in route.from.iter().chain(&route.to) {
                if !formats.contains_key(ext) {
                    return Err(bad(format!(
                        "The {} route uses \"{ext}\", which isn't a declared format.",
                        route.backend
                    )));
                }
            }
            for from in &route.from {
                for to in &route.to {
                    if from == to {
                        continue;
                    }
                    let key = (from.clone(), to.clone());
                    if let Some(prev) = routes.insert(key, route.backend.clone()) {
                        return Err(bad(format!(
                            "{} to {} is declared twice (backends {prev} and {}).",
                            from.to_ascii_uppercase(),
                            to.to_ascii_uppercase(),
                            route.backend
                        )));
                    }
                }
            }
        }

        Ok(Self {
            formats,
            aliases,
            routes,
        })
    }

    /// Look up a format by extension or alias, ignoring case and a leading dot.
    pub fn format(&self, ext: &str) -> Option<&Format> {
        let ext = ext.trim_start_matches('.').to_ascii_lowercase();
        let ext = self.aliases.get(&ext).unwrap_or(&ext);
        self.formats.get(ext)
    }

    /// The format of a file, judged by its extension.
    pub fn format_of(&self, path: &Path) -> Option<&Format> {
        self.format(path.extension()?.to_str()?)
    }

    /// Output formats a file of type `from` can be converted to, in
    /// extension order.
    pub fn targets(&self, from: &Format) -> Vec<&Format> {
        self.routes
            .keys()
            .filter(|(f, _)| *f == from.ext)
            .map(|(_, t)| &self.formats[t])
            .collect()
    }

    /// The backend id for a pair, if it is supported.
    pub fn backend(&self, from: &Format, to: &Format) -> Option<&str> {
        self.routes
            .get(&(from.ext.clone(), to.ext.clone()))
            .map(String::as_str)
    }

    /// Every supported pair, as (from, to, backend).
    pub fn pairs(&self) -> impl Iterator<Item = (&Format, &Format, &str)> {
        self.routes
            .iter()
            .map(|((f, t), b)| (&self.formats[f], &self.formats[t], b.as_str()))
    }

    pub fn backends(&self) -> impl Iterator<Item = &str> {
        let mut ids: Vec<&str> = self.routes.values().map(String::as_str).collect();
        ids.sort_unstable();
        ids.dedup();
        ids.into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(text: &str) -> String {
        match Registry::from_toml(text) {
            Err(Error::Registry(msg)) => msg,
            other => panic!("expected a registry error, got {other:?}"),
        }
    }

    #[test]
    fn builtin_registry_loads() {
        let r = Registry::builtin();
        let png = r.format("png").unwrap();
        let jpg = r.format("jpg").unwrap();
        assert_eq!(r.backend(png, jpg), Some("image"));
        let codes: Vec<&str> = r.targets(png).iter().map(|f| f.ext.as_str()).collect();
        assert_eq!(codes, ["bmp", "gif", "jpg", "tiff", "webp"]);
        assert_eq!(r.backend(jpg, png), Some("image"));
        assert_eq!(r.format("tif").unwrap().ext, "tiff");
        assert!(r
            .backends()
            .eq(["ffmpeg", "image", "libreoffice", "pandoc"]));
    }

    #[test]
    fn lookup_ignores_case_dots_and_aliases() {
        let r = Registry::builtin();
        for spelling in ["jpg", "JPG", ".jpg", "jpeg", ".JPEG"] {
            assert_eq!(r.format(spelling).unwrap().ext, "jpg", "{spelling}");
        }
        assert_eq!(r.format_of(Path::new("a/b/Song.MP3")).unwrap().ext, "mp3");
        assert!(r.format_of(Path::new("noext")).is_none());
        assert!(r.format("xyz").is_none());
    }

    #[test]
    fn identity_pairs_are_skipped() {
        let r = Registry::builtin();
        assert!(r.pairs().all(|(f, t, _)| f.ext != t.ext));
    }

    #[test]
    fn video_can_become_audio_but_not_the_reverse() {
        let r = Registry::builtin();
        let mp4 = r.format("mp4").unwrap();
        let mp3 = r.format("mp3").unwrap();
        assert_eq!(r.backend(mp4, mp3), Some("ffmpeg"));
        assert_eq!(r.backend(mp3, mp4), None);
    }

    #[test]
    fn office_pairs_leave_pandoc_its_own() {
        let r = Registry::builtin();
        let pair = |a: &str, b: &str| r.backend(r.format(a).unwrap(), r.format(b).unwrap());
        assert_eq!(pair("docx", "odt"), Some("pandoc"));
        assert_eq!(pair("docx", "pdf"), Some("libreoffice"));
        assert_eq!(pair("doc", "docx"), Some("libreoffice"));
        assert_eq!(pair("xlsx", "csv"), Some("libreoffice"));
        assert_eq!(pair("pptx", "odp"), Some("libreoffice"));
        // PDF is written, never read.
        assert!(r.targets(r.format("pdf").unwrap()).is_empty());
        // Every type LibreOffice writes says how.
        for (_, to, backend) in r.pairs() {
            if backend == "libreoffice" {
                assert!(
                    to.libreoffice.is_some(),
                    "{} has no libreoffice filter",
                    to.ext
                );
            }
        }
        assert_eq!(r.pairs().filter(|p| p.2 == "libreoffice").count(), 37);
    }

    #[test]
    fn duplicate_pairs_are_rejected() {
        let msg = err(r#"
            [formats.a]
            kind = "image"
            [formats.b]
            kind = "image"
            [[route]]
            backend = "x"
            from = ["a"]
            to = ["b"]
            [[route]]
            backend = "y"
            from = ["a"]
            to = ["b"]
        "#);
        assert_eq!(msg, "A to B is declared twice (backends x and y).");
    }

    #[test]
    fn duplicate_pairs_inside_one_route_are_rejected() {
        let msg = err(r#"
            [formats.a]
            kind = "image"
            [formats.b]
            kind = "image"
            [[route]]
            backend = "x"
            from = ["a"]
            to = ["b", "b"]
        "#);
        assert!(msg.contains("declared twice"), "{msg}");
    }

    #[test]
    fn duplicate_format_tables_are_rejected() {
        let msg = err(r#"
            [formats.a]
            kind = "image"
            [formats.a]
            kind = "audio"
        "#);
        assert!(!msg.is_empty());
    }

    #[test]
    fn duplicate_aliases_are_rejected() {
        let msg = err(r#"
            [formats.a]
            kind = "image"
            aliases = ["z"]
            [formats.b]
            kind = "image"
            aliases = ["z"]
        "#);
        assert!(msg.contains("\"z\""), "{msg}");
    }

    #[test]
    fn alias_may_not_shadow_a_format() {
        let msg = err(r#"
            [formats.a]
            kind = "image"
            aliases = ["b"]
            [formats.b]
            kind = "image"
        "#);
        assert!(msg.contains("both a format and an alias"), "{msg}");
    }

    #[test]
    fn routes_must_use_declared_formats() {
        let msg = err(r#"
            [formats.a]
            kind = "image"
            [[route]]
            backend = "x"
            from = ["a"]
            to = ["nope"]
        "#);
        assert!(msg.contains("\"nope\""), "{msg}");
    }

    #[test]
    fn extensions_must_be_lowercase() {
        let msg = err(r#"
            [formats.PNG]
            kind = "image"
        "#);
        assert!(msg.contains("valid extension"), "{msg}");
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let msg = err(r#"
            [formats.a]
            kind = "image"
            colour = "blue"
        "#);
        assert!(msg.contains("colour"), "{msg}");
    }
}
