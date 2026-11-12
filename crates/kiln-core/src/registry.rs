//! The format registry, loaded from formats.toml.

use std::collections::{BTreeMap, HashMap, HashSet};
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
    Subtitle,
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

/// One engine run inside a conversion: `from` to `to` with `backend`, or
/// with `fallback` when that backend's engine isn't there or can't take
/// the file now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub from: String,
    pub to: String,
    pub backend: String,
    pub fallback: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Registry {
    formats: BTreeMap<String, Format>,
    aliases: HashMap<String, String>,
    /// (from, to) -> the steps that convert it. A direct route has one
    /// step; a chain runs several, passing a file from one to the next.
    routes: BTreeMap<(String, String), Vec<Step>>,
    /// The pairs whose route keeps a PDF's text and nothing else (a chain's
    /// `note = "text-only"`).
    text_only: HashSet<(String, String)>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    formats: BTreeMap<String, RawFormat>,
    #[serde(default, rename = "route")]
    routes: Vec<RawRoute>,
    #[serde(default, rename = "chain")]
    chains: Vec<RawChain>,
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
    /// The backend that does the pairs when `backend` can't.
    fallback: Option<String>,
    from: Vec<String>,
    to: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawChain {
    from: Vec<String>,
    via: Vec<String>,
    to: Vec<String>,
    /// What the chain loses on the way, for the pairs it makes: only
    /// "text-only" (the text of the source is all that arrives).
    note: Option<String>,
}

/// The chain note that says a route keeps the text only.
const TEXT_ONLY: &str = "text-only";

fn bad(msg: String) -> Error {
    Error::Registry(msg)
}

/// "pandoc", or "pandoc, then libreoffice" for a chain.
fn describe(steps: &[Step]) -> String {
    let names: Vec<&str> = steps.iter().map(|s| s.backend.as_str()).collect();
    names.join(", then ")
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
            if route.fallback.as_ref() == Some(&route.backend) {
                return Err(bad(format!(
                    "The {} route falls back to itself.",
                    route.backend
                )));
            }
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
                    let step = Step {
                        from: from.clone(),
                        to: to.clone(),
                        backend: route.backend.clone(),
                        fallback: route.fallback.clone(),
                    };
                    if let Some(prev) = routes.insert(key, vec![step]) {
                        return Err(bad(format!(
                            "{} to {} is declared twice (backends {} and {}).",
                            from.to_ascii_uppercase(),
                            to.to_ascii_uppercase(),
                            describe(&prev),
                            route.backend
                        )));
                    }
                }
            }
        }

        // Chains are read in order, so a chain may build on an earlier one.
        let mut text_only = HashSet::new();
        for chain in raw.chains {
            if let Some(note) = chain.note.as_deref().filter(|n| *n != TEXT_ONLY) {
                return Err(bad(format!(
                    "A chain has the note \"{note}\", which Kiln doesn't know."
                )));
            }
            for ext in chain.from.iter().chain(&chain.via).chain(&chain.to) {
                if !formats.contains_key(ext) {
                    return Err(bad(format!(
                        "A chain uses \"{ext}\", which isn't a declared format."
                    )));
                }
            }
            if chain.via.is_empty() {
                return Err(bad("A chain needs at least one type in via.".to_owned()));
            }
            for from in &chain.from {
                for to in &chain.to {
                    if from == to {
                        continue;
                    }
                    let code = |e: &str| e.to_ascii_uppercase();
                    if chain.via.contains(from) || chain.via.contains(to) {
                        return Err(bad(format!(
                            "The chain from {} to {} passes through one of its own ends.",
                            code(from),
                            code(to)
                        )));
                    }
                    let mut hops = vec![from];
                    hops.extend(&chain.via);
                    hops.push(to);
                    let mut steps = Vec::new();
                    for leg in hops.windows(2) {
                        let Some(leg_steps) = routes.get(&(leg[0].clone(), leg[1].clone())) else {
                            return Err(bad(format!(
                                "The chain from {} to {} needs {} to {}, which isn't supported.",
                                code(from),
                                code(to),
                                code(leg[0]),
                                code(leg[1])
                            )));
                        };
                        steps.extend(leg_steps.iter().cloned());
                    }
                    let key = (from.clone(), to.clone());
                    if chain.note.is_some() {
                        text_only.insert(key.clone());
                    }
                    if let Some(prev) = routes.insert(key, steps) {
                        return Err(bad(format!(
                            "{} to {} is declared twice (backends {} and a chain).",
                            code(from),
                            code(to),
                            describe(&prev)
                        )));
                    }
                }
            }
        }

        Ok(Self {
            formats,
            aliases,
            routes,
            text_only,
        })
    }

    /// Look up a format by extension or alias, ignoring case and a leading dot.
    pub fn format(&self, ext: &str) -> Option<&Format> {
        let ext = ext.trim_start_matches('.').to_ascii_lowercase();
        let ext = self.aliases.get(&ext).unwrap_or(&ext);
        self.formats.get(ext)
    }

    /// Every extension the registry reads or writes: each format's own, and
    /// its aliases. Lowercase, no dot.
    pub fn extensions(&self) -> impl Iterator<Item = &str> {
        self.formats
            .keys()
            .chain(self.aliases.keys())
            .map(String::as_str)
    }

    /// Every extension of a format Kiln can convert from, that is one with
    /// at least one target: its own, and its aliases. Lowercase, no dot,
    /// sorted, without duplicates. The installer lists these for Open with.
    pub fn input_extensions(&self) -> Vec<String> {
        let mut exts: Vec<String> = self
            .formats
            .values()
            .filter(|f| !self.targets(f).is_empty())
            .flat_map(|f| {
                let own = std::iter::once(f.ext.clone());
                let aliases = self
                    .aliases
                    .iter()
                    .filter(move |(_, ext)| **ext == f.ext)
                    .map(|(alias, _)| alias.clone());
                own.chain(aliases)
            })
            .collect();
        exts.sort_unstable();
        exts.dedup();
        exts
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

    /// The steps that convert a pair, if it is supported.
    pub fn route(&self, from: &Format, to: &Format) -> Option<&[Step]> {
        self.routes
            .get(&(from.ext.clone(), to.ext.clone()))
            .map(Vec::as_slice)
    }

    /// Whether the route from `from` to `to` keeps the text of the source
    /// and nothing else: a PDF's pictures, tables and layout don't arrive
    /// (a chain with `note = "text-only"`).
    pub fn text_only(&self, from: &Format, to: &Format) -> bool {
        self.text_only.contains(&(from.ext.clone(), to.ext.clone()))
    }

    /// The backend id for a pair one engine converts directly. Chains and
    /// unsupported pairs give `None`.
    pub fn backend(&self, from: &Format, to: &Format) -> Option<&str> {
        match self.route(from, to)? {
            [step] => Some(step.backend.as_str()),
            _ => None,
        }
    }

    /// Every supported pair, as (from, to, steps).
    pub fn pairs(&self) -> impl Iterator<Item = (&Format, &Format, &[Step])> {
        self.routes
            .iter()
            .map(|((f, t), s)| (&self.formats[f], &self.formats[t], s.as_slice()))
    }

    /// Every backend a route names, first or as its fallback.
    pub fn backends(&self) -> impl Iterator<Item = &str> {
        let mut ids: Vec<&str> = self
            .routes
            .values()
            .flatten()
            .flat_map(|s| std::iter::once(s.backend.as_str()).chain(s.fallback.as_deref()))
            .collect();
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
        assert_eq!(
            codes,
            [
                "avif", "bmp", "gif", "ico", "jpg", "jxl", "pdf", "ppm", "qoi", "tga", "tiff",
                "webp"
            ]
        );
        assert_eq!(r.backend(jpg, png), Some("image"));
        assert_eq!(r.format("tif").unwrap().ext, "tiff");
        assert!(r.backends().eq([
            "audio",
            "audio-mf",
            "calibre",
            "ffmpeg",
            "image",
            "libreoffice",
            "office",
            "pandoc",
            "pdf",
            "video"
        ]));
    }

    #[test]
    fn input_extensions_are_what_kiln_converts_from() {
        let r = Registry::builtin();
        let exts = r.input_extensions();
        for e in ["png", "jpg", "jpeg", "tif", "tiff", "mp4", "docx"] {
            assert!(exts.iter().any(|x| x == e), "{e}");
        }
        assert!(
            exts.windows(2).all(|w| w[0] < w[1]),
            "sorted, no duplicates"
        );
        for e in &exts {
            let f = r.format(e).unwrap();
            assert!(!r.targets(f).is_empty(), "{e} has no target");
        }
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
        assert_eq!(r.backend(mp4, mp3), Some("audio"));
        assert_eq!(r.backend(mp3, mp4), None);
        let avi = r.format("avi").unwrap();
        assert_eq!(r.backend(avi, mp3), Some("ffmpeg"));
    }

    #[test]
    fn office_pairs_leave_pandoc_its_own() {
        let r = Registry::builtin();
        let pair = |a: &str, b: &str| r.backend(r.format(a).unwrap(), r.format(b).unwrap());
        assert_eq!(pair("docx", "odt"), Some("pandoc"));
        // PDF goes through Microsoft Office first (its own test below).
        assert_eq!(pair("docx", "pdf"), Some("office"));
        assert_eq!(pair("doc", "docx"), Some("libreoffice"));
        assert_eq!(pair("xlsx", "csv"), Some("libreoffice"));
        assert_eq!(pair("pptx", "odp"), Some("libreoffice"));
        // Every type LibreOffice writes says how, in chains too.
        for (_, _, steps) in r.pairs() {
            let lo = |s: &&Step| {
                s.backend == "libreoffice" || s.fallback.as_deref() == Some("libreoffice")
            };
            for step in steps.iter().filter(lo) {
                let to = r.format(&step.to).unwrap();
                assert!(
                    to.libreoffice.is_some(),
                    "{} has no libreoffice filter",
                    to.ext
                );
            }
        }
    }

    #[test]
    fn a_route_may_fall_back_to_a_second_backend() {
        let r = Registry::from_toml(
            r#"
            [formats.a]
            kind = "document"
            [formats.b]
            kind = "document"
            [[route]]
            backend = "x"
            fallback = "y"
            from = ["a"]
            to = ["b"]
        "#,
        )
        .unwrap();
        let (a, b) = (r.format("a").unwrap(), r.format("b").unwrap());
        let steps = r.route(a, b).unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].backend, "x");
        assert_eq!(steps[0].fallback.as_deref(), Some("y"));
        assert_eq!(r.backend(a, b), Some("x"));
        // A Kiln must have both.
        assert!(r.backends().eq(["x", "y"]));
        // A chain through the pair keeps the fallback on its leg.
        let r = Registry::from_toml(&format!(
            "{CHAIN_BASE}
            [[route]]
            backend = \"z\"
            fallback = \"w\"
            from = [\"c\"]
            to = [\"d\"]
            [[chain]]
            from = [\"a\"]
            via = [\"b\", \"c\"]
            to = [\"d\"]"
        ))
        .unwrap();
        let steps = r
            .route(r.format("a").unwrap(), r.format("d").unwrap())
            .unwrap();
        assert_eq!(steps[0].fallback, None);
        assert_eq!(steps[2].fallback.as_deref(), Some("w"));
    }

    #[test]
    fn a_route_may_not_fall_back_to_itself() {
        let msg = err(r#"
            [formats.a]
            kind = "image"
            [formats.b]
            kind = "image"
            [[route]]
            backend = "x"
            fallback = "x"
            from = ["a"]
            to = ["b"]
        "#);
        assert_eq!(msg, "The x route falls back to itself.");
    }

    #[test]
    fn office_files_go_to_pdf_through_office_then_libreoffice() {
        let r = Registry::builtin();
        let get = |e: &str| r.format(e).unwrap();
        let pdf = get("pdf");
        for from in [
            "doc", "docx", "odt", "rtf", "txt", "xlsx", "xls", "ods", "csv", "pptx", "ppt", "odp",
        ] {
            let steps = r.route(get(from), pdf).unwrap();
            assert_eq!(steps.len(), 1, "{from}");
            assert_eq!(steps[0].backend, "office", "{from}");
            assert_eq!(steps[0].fallback.as_deref(), Some("libreoffice"), "{from}");
        }
        // Types Office doesn't open stay LibreOffice's alone.
        for from in ["pages", "wpd", "numbers", "key", "vsdx", "pub"] {
            let steps = r.route(get(from), pdf).unwrap();
            assert_eq!(steps[0].backend, "libreoffice", "{from}");
            assert_eq!(steps[0].fallback, None, "{from}");
        }
        // The other office pairs are LibreOffice's, as before.
        assert_eq!(r.backend(get("doc"), get("docx")), Some("libreoffice"));
        assert_eq!(r.backend(get("xlsx"), get("csv")), Some("libreoffice"));
        // A Markdown file's PDF is laid out by Word too, when it's there.
        let md = r.route(get("md"), pdf).unwrap();
        assert_eq!(md.last().unwrap().backend, "office");
        assert_eq!(md.last().unwrap().fallback.as_deref(), Some("libreoffice"));
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
    fn heic_to_png_is_the_image_backend_s_alone() {
        let r = Registry::builtin();
        let heic = r.format("heic").unwrap();
        assert_eq!(r.backend(heic, r.format("png").unwrap()), Some("image"));
        // FFmpeg's old route for it can't come back beside it.
        let msg = err(&format!(
            "{DEFAULT_FORMATS}\n[[route]]\nbackend = \"ffmpeg\"\nfrom = [\"heic\"]\nto = [\"png\"]\n"
        ));
        assert_eq!(
            msg,
            "HEIC to PNG is declared twice (backends image and ffmpeg)."
        );
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

    const CHAIN_BASE: &str = r#"
        [formats.a]
        kind = "document"
        [formats.b]
        kind = "document"
        [formats.c]
        kind = "document"
        [formats.d]
        kind = "document"
        [[route]]
        backend = "x"
        from = ["a"]
        to = ["b"]
        [[route]]
        backend = "y"
        from = ["b"]
        to = ["c"]
    "#;

    #[test]
    fn chains_join_their_legs() {
        let r = Registry::from_toml(&format!(
            "{CHAIN_BASE}
            [[chain]]
            from = [\"a\"]
            via = [\"b\"]
            to = [\"c\"]"
        ))
        .unwrap();
        let (a, c) = (r.format("a").unwrap(), r.format("c").unwrap());
        let steps: Vec<(&str, &str, &str)> = r
            .route(a, c)
            .unwrap()
            .iter()
            .map(|s| (s.from.as_str(), s.to.as_str(), s.backend.as_str()))
            .collect();
        assert_eq!(steps, [("a", "b", "x"), ("b", "c", "y")]);
        assert_eq!(r.backend(a, c), None);
        assert_eq!(r.backend(a, r.format("b").unwrap()), Some("x"));
        let codes: Vec<&str> = r.targets(a).iter().map(|f| f.ext.as_str()).collect();
        assert_eq!(codes, ["b", "c"]);
    }

    #[test]
    fn chains_need_every_leg() {
        let msg = err(&format!(
            "{CHAIN_BASE}
            [[chain]]
            from = [\"a\"]
            via = [\"b\"]
            to = [\"d\"]"
        ));
        assert_eq!(
            msg,
            "The chain from A to D needs B to D, which isn't supported."
        );
    }

    #[test]
    fn chains_may_not_repeat_a_route() {
        let msg = err(&format!(
            "{CHAIN_BASE}
            [[route]]
            backend = \"z\"
            from = [\"a\"]
            to = [\"c\"]
            [[chain]]
            from = [\"a\"]
            via = [\"b\"]
            to = [\"c\"]"
        ));
        assert!(msg.contains("declared twice"), "{msg}");
    }

    #[test]
    fn chains_may_not_pass_through_their_ends() {
        let msg = err(&format!(
            "{CHAIN_BASE}
            [[chain]]
            from = [\"a\"]
            via = [\"a\", \"b\"]
            to = [\"c\"]"
        ));
        assert!(msg.contains("its own ends"), "{msg}");
    }

    #[test]
    fn every_chain_in_the_builtin_registry_resolves() {
        let r = Registry::builtin();
        for (from, to, steps) in r.pairs() {
            assert_eq!(steps.first().unwrap().from, from.ext);
            assert_eq!(steps.last().unwrap().to, to.ext);
            for w in steps.windows(2) {
                assert_eq!(w[0].to, w[1].from, "{} to {}", from.ext, to.ext);
            }
        }
    }

    #[test]
    fn a_chain_may_note_that_it_keeps_the_text_only() {
        let r = Registry::from_toml(&format!(
            "{CHAIN_BASE}
            [[chain]]
            from = [\"a\"]
            via = [\"b\"]
            to = [\"c\"]
            note = \"text-only\""
        ))
        .unwrap();
        let get = |e: &str| r.format(e).unwrap();
        assert!(r.text_only(get("a"), get("c")));
        // A direct route, and the legs of the chain, have no note.
        assert!(!r.text_only(get("a"), get("b")));
        assert!(!r.text_only(get("b"), get("c")));
        // An unsupported pair has none either.
        assert!(!r.text_only(get("c"), get("a")));

        let plain = Registry::from_toml(&format!(
            "{CHAIN_BASE}
            [[chain]]
            from = [\"a\"]
            via = [\"b\"]
            to = [\"c\"]"
        ))
        .unwrap();
        assert!(!plain.text_only(plain.format("a").unwrap(), plain.format("c").unwrap()));
    }

    #[test]
    fn a_chain_note_must_be_one_kiln_knows() {
        let msg = err(&format!(
            "{CHAIN_BASE}
            [[chain]]
            from = [\"a\"]
            via = [\"b\"]
            to = [\"c\"]
            note = \"lossy\""
        ));
        assert_eq!(
            msg,
            "A chain has the note \"lossy\", which Kiln doesn't know."
        );
    }
}
