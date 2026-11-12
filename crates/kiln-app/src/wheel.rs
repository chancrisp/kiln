//! The format wheel: which items it offers and where they sit. Drawing and
//! input live in `win`; this is the platform-independent geometry.
//!
//! One flat ring (look spec, mock screen 7): `R_INNER` to `R_OUTER`, its
//! segments `GAP` apart with corners rounded `CORNER`, round a hub
//! `R_HUB` across. More opens a list beside the ring (`FlyoutPlace`).

use std::f32::consts::{PI, TAU};
use std::path::{Path, PathBuf};
use std::time::Instant;

use kiln_core::{Availability, Kiln, Kind, Offered, Suggested, Tool};

use crate::anim;
use crate::layout::Rect;
use crate::theme::fluent::{motion as fluent_motion, space};

/// Outer and inner radius of the ring, in DIPs.
pub const R_OUTER: f32 = 136.0;
pub const R_INNER: f32 = 56.0;
/// The hub's radius: 104 DIPs across, a gap inside the ring.
pub const R_HUB: f32 = 52.0;
/// The space between two segments, and their corners' radius.
pub const GAP: f32 = space::WHEEL_GAP;
pub const CORNER: f32 = crate::theme::fluent::radius::WHEEL_INNER;
/// Where a segment's code or glyph is centred: a little past the middle of
/// the ring, as the mock sets it.
pub const R_LABEL: f32 = (R_INNER + R_OUTER) / 2.0 + 2.0;
/// The kind's arc runs this far inside the outer edge, `space::KIND_ARC`
/// wide.
pub const KIND_ARC_INSET: f32 = 3.0;
/// How far past the outer edge the pointer still counts as on an item.
pub const REACH: f32 = 48.0;
/// Space kept between the wheel and the edge of the list it keeps to.
pub const EDGE_GAP: f32 = 8.0;
/// How far the ring's shadow reaches from it (the flyout shadow's 8 DIP
/// drop and its 16 DIP blur): kept inside the content layer on every side,
/// so no edge cuts it off.
pub const SHADOW_GAP: f32 = 24.0;
/// Most formats the ring shows for files whose kind has no list of its
/// own ([`Order::ring`]): the first this many, most popular first.
pub const RING_FORMATS: usize = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WheelKind {
    /// Convert to another format.
    Convert,
    /// The file tools, such as Remove borders and Merge (Ctrl+Enter).
    Tools,
}

impl WheelKind {
    pub const ALL: [WheelKind; 2] = [WheelKind::Convert, WheelKind::Tools];

    /// The label in the wheel's centre.
    pub fn title(self) -> &'static str {
        match self {
            WheelKind::Convert => "Convert",
            WheelKind::Tools => "Tools",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Convert to this extension.
    Convert(String),
    /// Run a tool on files of this kind, which names it.
    Tool(Tool, Kind),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// Format code ("JPG") or tool name ("Remove borders").
    pub label: String,
    /// Whether `label` is a format code, drawn in monospace.
    pub is_code: bool,
    pub action: Action,
    /// The target's kind, which groups the outer ring. `None` for tools.
    pub kind: Option<Kind>,
}

/// A place on the wheel the pointer or the keyboard can be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spot {
    /// A segment of the ring. One past the last item is More.
    Inner(usize),
    /// Item `j` of the list More opens, counted through its groups.
    Flyout(usize),
    /// The list's last line, "Options…", for the format last lit.
    Options,
}

/// Where a wheel was opened: over Kiln's own window, or floating over
/// another app while a file is dragged there (the drag-wheel).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Origin {
    #[default]
    Window,
    Drag,
}

/// One group of More's list: a heading and what is under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    /// "Photos", "Sound", "Tools".
    pub title: &'static str,
    pub items: Vec<Item>,
}

/// The heading over the tools in More.
pub const TOOLS_GROUP: &str = "Tools";

/// The footer row of More's list, "Options for JPG…": the options dialog
/// of the format `ext` ([`Wheel::options_subject`]).
pub fn options_line(ext: &str) -> String {
    format!(
        "Options for {}\u{2026}",
        ext.trim_start_matches('.').to_ascii_uppercase()
    )
}

/// A line of More's list, top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlyoutRow<'a> {
    /// A group's heading.
    Heading(&'static str),
    /// A format or a tool, and its spot.
    Item(Spot, &'a Item),
    /// The list's footer, "Options for JPG…", for the format
    /// [`Wheel::options_subject`] names, by its extension. It is only there
    /// when some format has something to set, and then it is always usable.
    Options(&'a str),
}

/// What More's group of formats of `kind` is called.
pub fn group_title(kind: Kind) -> &'static str {
    match kind {
        Kind::Image => "Photos",
        Kind::Audio => "Sound",
        Kind::Video => "Video",
        Kind::Document => "Documents",
        Kind::Spreadsheet => "Sheets",
        Kind::Presentation => "Slides",
        Kind::Subtitle => "Subtitles",
    }
}

/// `items` in More's groups, in the order given: each run of formats of
/// one kind under that kind's heading, and tools under Tools.
pub fn grouped(items: Vec<Item>) -> Vec<Group> {
    let mut groups: Vec<Group> = vec![];
    for item in items {
        let title = item.kind.map_or(TOOLS_GROUP, group_title);
        match groups.last_mut() {
            Some(g) if g.title == title => g.items.push(item),
            _ => groups.push(Group {
                title,
                items: vec![item],
            }),
        }
    }
    groups
}

#[derive(Debug, Clone, PartialEq)]
pub struct Wheel {
    pub kind: WheelKind,
    /// Where it was opened. Only a drag-wheel converts on a pick.
    pub origin: Origin,
    /// The inner ring.
    pub items: Vec<Item>,
    /// What More holds: formats grouped by kind, then the tools. Empty
    /// when everything is on the inner ring, and then there is no More
    /// segment. [`Spot::Flyout`] counts through it as one list.
    pub flyout: Vec<Group>,
    /// Centre in window DIPs.
    pub center: (f32, f32),
    /// Below 1 only when the window is too small for the whole wheel.
    pub scale: f32,
    /// The source type, shown muted under the title: "PNG", or "MIXED".
    pub source_code: String,
    /// The kind every file is, whose glyph the hub shows; `None` when they
    /// are of several kinds, or of none Kiln knows.
    pub source_kind: Option<Kind>,
    /// Only the list of formats and tools, with no ring: the footer's
    /// Convert, the pane's More formats and tools and the row menu's
    /// Convert to… open it beside what asked for it.
    pub list_only: bool,
    /// The formats on the ring and in More that have something to set,
    /// each by its extension, the one to offer first at the head: the
    /// suggestion when it is among them, else the first in the list. The
    /// footer row's dialog is for the lit format if it is among them,
    /// else for the head ([`Wheel::options_subject`]).
    pub settable: Vec<String>,
}

impl Wheel {
    /// Whether there is a More segment: More has something in it.
    pub fn has_more(&self) -> bool {
        self.flyout.iter().any(|g| !g.items.is_empty())
    }

    /// What More holds, as one list, top to bottom: the order
    /// [`Spot::Flyout`] counts in.
    pub fn more_items(&self) -> impl Iterator<Item = &Item> + Clone {
        self.flyout.iter().flat_map(|g| g.items.iter())
    }

    /// How many items More holds.
    pub fn more_len(&self) -> usize {
        self.flyout.iter().map(|g| g.items.len()).sum()
    }

    /// Segments on the inner ring: the items, and More if there is one.
    pub fn segments(&self) -> usize {
        if self.list_only {
            return 0;
        }
        self.items.len() + usize::from(self.has_more())
    }

    /// Whether inner segment `i` is More.
    pub fn is_more(&self, i: usize) -> bool {
        self.has_more() && i == self.items.len()
    }

    pub fn item(&self, spot: Spot) -> Option<&Item> {
        match spot {
            Spot::Inner(i) => self.items.get(i),
            Spot::Flyout(i) => self.more_items().nth(i),
            Spot::Options => None,
        }
    }

    /// The item at `spot` if it is a format: its extension.
    pub fn format_at(&self, spot: Spot) -> Option<&str> {
        match &self.item(spot)?.action {
            Action::Convert(ext) => Some(ext),
            Action::Tool(..) => None,
        }
    }

    /// Whether `spot` is the More segment.
    pub fn is_more_spot(&self, spot: Spot) -> bool {
        matches!(spot, Spot::Inner(i) if self.is_more(i))
    }

    /// What the hub says while `lit` is lit, for `files` files: two lines,
    /// the pair in Body Strong and a hint in Caption (mock screen 7).
    /// - Nothing lit: the source's code ("JPG", "JPG · 3") and "Pick a
    ///   format" ("Pick a tool" on the tools wheel).
    /// - A format: "JPG → PNG", and for one file the size `estimate` gives
    ///   ("about 1.1 MB"), for several how many.
    /// - Make smaller: "Make smaller", "Keeps JPG".
    /// - More: "More", "Formats, tools".
    /// - Another tool: its name, and how many files when several.
    pub fn hub_lines(
        &self,
        lit: Option<Spot>,
        files: usize,
        estimate: &dyn Fn(&str) -> Option<u64>,
    ) -> HubLines {
        let code = &self.source_code;
        let many = |n: usize| (n > 1).then(|| format!("{n} files"));
        let lines = |pair: String, hint: Option<String>| HubLines {
            pair,
            hint: hint.unwrap_or_default(),
        };
        let shown = lit.filter(|&s| self.item(s).is_some() || self.is_more_spot(s));
        let Some(spot) = shown else {
            let pair = if files > 1 {
                format!("{code} \u{b7} {files}")
            } else {
                code.clone()
            };
            let hint = match self.kind {
                WheelKind::Tools => "Pick a tool",
                WheelKind::Convert => "Pick a format",
            };
            return lines(pair, Some(hint.to_owned()));
        };
        let label = self.item(spot).map_or("", |i| i.label.as_str()).to_owned();
        match self.item(spot).map(|i| &i.action) {
            None => lines("More".to_owned(), Some("Formats, tools".to_owned())),
            Some(Action::Tool(Tool::Smaller, _)) => {
                lines("Make smaller".to_owned(), Some(format!("Keeps {code}")))
            }
            Some(Action::Tool(..)) => lines(label, many(files)),
            Some(Action::Convert(ext)) => {
                let hint = many(files).or_else(|| {
                    estimate(ext)
                        .filter(|&b| b >= crate::inspector::MIN_ESTIMATE)
                        .map(|b| format!("about {}", kiln_core::about(b)))
                });
                lines(format!("{code} \u{2192} {label}"), hint)
            }
        }
    }

    /// The format the footer row's options are for: the format last lit,
    /// `lit`, if it has something to set, else the head of
    /// [`Wheel::settable`] (the suggestion, or else the first format in
    /// the list that has settings). `None` when no format has.
    pub fn options_subject(&self, lit: Option<Spot>) -> Option<&str> {
        let lit = lit.and_then(|s| self.format_at(s));
        lit.and_then(|ext| self.settable.iter().find(|e| *e == ext))
            .or_else(|| self.settable.first())
            .map(String::as_str)
    }

    /// More's list, top to bottom: each group's heading and its items,
    /// then the footer row, "Options for JPG…", for the format
    /// [`Wheel::options_subject`] gives for `lit`, if the list holds formats
    /// and some has settings and the wheel isn't the drag-wheel, whose
    /// files can't take options.
    pub fn flyout_rows(&self, lit: Option<Spot>) -> Vec<FlyoutRow<'_>> {
        let mut rows = vec![];
        let mut j = 0;
        for g in self.flyout.iter().filter(|g| !g.items.is_empty()) {
            rows.push(FlyoutRow::Heading(g.title));
            for item in &g.items {
                rows.push(FlyoutRow::Item(Spot::Flyout(j), item));
                j += 1;
            }
        }
        let formats = self
            .more_items()
            .any(|i| matches!(i.action, Action::Convert(_)));
        if formats && self.origin != Origin::Drag {
            if let Some(ext) = self.options_subject(lit) {
                rows.push(FlyoutRow::Options(ext));
            }
        }
        rows
    }

    /// What the hub names while `spot` is lit: its item's label, or More.
    pub fn hub_name(&self, spot: Spot) -> Option<&str> {
        match (self.item(spot), spot) {
            (Some(item), _) => Some(item.label.as_str()),
            (None, Spot::Inner(i)) if self.is_more(i) => Some("More"),
            _ => None,
        }
    }

    /// How far the wheel reaches from its centre at full size: the ring.
    /// More's list goes beside it wherever there is room
    /// ([`flyout_place`]).
    pub fn extent(&self) -> f32 {
        if self.list_only {
            0.0
        } else {
            R_OUTER
        }
    }
}

/// The hub's two lines (mock `.pair` and `.hint`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubLines {
    pub pair: String,
    pub hint: String,
}

/// The order kinds are grouped in on the outer ring.
fn kind_order(kind: Option<Kind>) -> usize {
    match kind {
        Some(Kind::Image) => 0,
        Some(Kind::Video) => 1,
        Some(Kind::Audio) => 2,
        Some(Kind::Document) => 3,
        Some(Kind::Spreadsheet) => 4,
        Some(Kind::Presentation) => 5,
        Some(Kind::Subtitle) => 6,
        None => 7,
    }
}

/// Targets people reach for most, first: the order of the formats off a
/// kind's ring ([`Order::rank`]), and of the whole ring for kinds that
/// have none of their own.
const POPULAR: &[&str] = &[
    "jpg", "png", "pdf", "mp4", "mp3", "docx", "webp", "gif", "wav", "txt", "html", "md", "mov",
    "mkv", "webm", "m4a", "flac", "xlsx", "csv", "pptx", "epub", "avif", "ogg", "aac", "opus",
    "srt", "odt", "rtf", "tiff", "bmp", "ico", "jxl", "mobi", "azw3", "ods", "odp", "vtt", "avi",
    "wmv", "doc", "xls", "ppt", "ass", "ts", "m4v", "flv", "3gp", "mpg", "ogv", "apng", "aiff",
    "wma", "ac3", "wv", "tga", "qoi", "ppm", "tex", "rst", "org", "adoc", "typ", "wiki", "textile",
    "dbk", "fb2", "ipynb",
];

/// Where `ext` comes in [`POPULAR`]; one past its end for a format not
/// in it.
fn rank(ext: &str) -> usize {
    POPULAR
        .iter()
        .position(|p| *p == ext)
        .unwrap_or(POPULAR.len())
}

/// What a format is for, in a few words: "Small image for the web". The one
/// source of it, for the side pane's rows and every list of formats.
pub fn format_words(ext: &str) -> Option<&'static str> {
    kiln_core::describe_format(ext)
}

/// The Segoe Fluent Icons glyph `tool` is drawn with, on the ring and in
/// More's list (mock `FORMATS`, where it has one).
pub fn glyph_for_tool(tool: Tool) -> char {
    match tool {
        Tool::Smaller => crate::theme::fluent::glyph::MAKE_SMALLER,
        Tool::Resize => '\u{E740}',
        Tool::RemoveLocation => '\u{E707}',
        Tool::RemoveEdges | Tool::RemoveQuietEnds => '\u{E7A8}',
        Tool::CutInHalf => '\u{E8C6}',
        Tool::SaveEachPage => '\u{E7C3}',
        Tool::Merge => '\u{E8F1}',
    }
}

/// What `tool` does, under its name in More's list (mock `FORMATS`).
pub fn tool_note(tool: Tool, kind: Kind) -> &'static str {
    match (tool, kind) {
        (Tool::Smaller, _) => "Same format, smaller file",
        (Tool::Resize, _) => "Pick the longest side; keeps the format",
        (Tool::RemoveLocation, _) => "Removes where it was taken and the camera details",
        (Tool::RemoveEdges, Kind::Video) => "Cuts black bars at the sides or top",
        (Tool::RemoveEdges, _) => "Cuts plain edges around the picture",
        (Tool::RemoveQuietEnds, Kind::Video) => "Cuts black frames at the start and end",
        (Tool::RemoveQuietEnds, _) => "Cuts silence at the start and end",
        (Tool::CutInHalf, _) => "Makes two files, each half as long",
        (Tool::SaveEachPage, _) => "Makes one PDF for each page",
        (Tool::Merge, _) => "Joins the files into one",
    }
}

/// What Kiln offers for `paths` with the helpers in `available`.
pub fn offered(kiln: &Kiln, paths: &[&Path], available: &Availability) -> Offered {
    kiln.offered_with(paths, available)
}

/// The subtitle formats, which a video is offered only when it has a
/// subtitle track (spec 3.7).
pub const SUBTITLES: &[&str] = &["srt", "vtt", "ass"];

/// For files Kiln knows nothing more about yet: none has a subtitle track.
pub fn no_subtitles(_: &Path) -> bool {
    false
}

/// The ring's formats for each kind of file, in order: the mock's rings
/// (spec 3.7). A file's own format is never offered, so it drops out.
const RING_IMAGE: &[&str] = &["jpg", "png", "webp", "pdf", "avif", "gif", "tiff"];
const RING_VIDEO: &[&str] = &["mp4", "mp3", "gif", "mov", "webm", "mkv", "m4a"];
const RING_AUDIO: &[&str] = &["mp3", "m4a", "wav", "flac", "ogg", "aac"];
const RING_DOCUMENT: &[&str] = &["pdf", "docx", "txt", "html", "odt", "rtf", "epub"];
const RING_PDF: &[&str] = &["docx", "png", "jpg", "txt"];

/// What a wheel's formats are ordered for: PDFs, which have a ring of
/// their own, files of one kind, or files of several kinds. The wheel,
/// the inspector and the row's helper app all order formats this way, so
/// they agree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    /// Every file is a PDF.
    Pdf,
    /// Every file is of this kind.
    Kind(Kind),
    /// Files of several kinds, or of none Kiln knows.
    Any,
}

impl Order {
    /// The order for files whose formats are `formats`: each one's own
    /// extension and kind.
    pub fn of_formats<'a>(formats: impl IntoIterator<Item = (&'a str, Kind)>) -> Order {
        let mut order = None;
        for (ext, kind) in formats {
            let this = if ext == "pdf" {
                Order::Pdf
            } else {
                Order::Kind(kind)
            };
            let documents = |o: Order| matches!(o, Order::Pdf | Order::Kind(Kind::Document));
            order = Some(match order {
                None => this,
                Some(o) if o == this => o,
                // PDFs among other documents go by the documents' ring.
                Some(o) if documents(o) && documents(this) => Order::Kind(Kind::Document),
                Some(_) => return Order::Any,
            });
        }
        order.unwrap_or(Order::Any)
    }

    /// The order for the files at `paths`.
    pub fn of(kiln: &Kiln, paths: &[PathBuf]) -> Order {
        let formats: Option<Vec<_>> = paths
            .iter()
            .map(|p| {
                kiln.registry()
                    .format_of(p)
                    .map(|f| (f.ext.as_str(), f.kind))
            })
            .collect();
        formats.map_or(Order::Any, Order::of_formats)
    }

    /// The formats the ring shows, in order. Empty for kinds the spec
    /// gives no ring of their own: theirs is their first [`RING_FORMATS`]
    /// formats.
    pub fn ring(self) -> &'static [&'static str] {
        match self {
            Order::Pdf => RING_PDF,
            Order::Kind(Kind::Image) => RING_IMAGE,
            Order::Kind(Kind::Video) => RING_VIDEO,
            Order::Kind(Kind::Audio) => RING_AUDIO,
            Order::Kind(Kind::Document) => RING_DOCUMENT,
            Order::Kind(_) | Order::Any => &[],
        }
    }

    /// Where `ext` comes: the ring's formats in their order, then the rest
    /// most popular first ([`POPULAR`]).
    pub fn rank(self, ext: &str) -> usize {
        let ring = self.ring();
        ring.iter()
            .position(|r| *r == ext)
            .unwrap_or_else(|| ring.len() + rank(ext))
    }

    /// `exts`, lowercase extensions, in this order ([`Order::rank`]), and
    /// those that rank alike alphabetically.
    pub fn sort(self, exts: &mut [String]) {
        exts.sort_by(|a, b| self.rank(a).cmp(&self.rank(b)).then_with(|| a.cmp(b)));
    }
}

/// The order More's groups of formats come in: the files' own kind first,
/// then the others as [`kind_order`] has them.
fn group_order(own: Option<Kind>, kind: Option<Kind>) -> (bool, usize) {
    (kind != own, kind_order(kind))
}

/// What the files at `paths` can become with the helpers in `available`,
/// as one list: for files of one kind, the formats any of them can become
/// (UX-06), and a file already in the one picked, or that can't become
/// it, is left out of the job ([`split_for`]); for files of several kinds,
/// only those every one can. A video is offered subtitle formats only when `subtitles` says
/// it holds a subtitle track.
fn formats_for(
    kiln: &Kiln,
    available: &Availability,
    paths: &[PathBuf],
    subtitles: &dyn Fn(&Path) -> bool,
) -> Vec<String> {
    let any = source_kind(kiln, paths).is_some();
    let mut common: Option<Vec<String>> = None;
    for p in paths {
        let targets = targets_of(kiln, available, p, subtitles);
        common = Some(match common {
            None => targets,
            Some(mut c) if any => {
                for t in targets {
                    if !c.contains(&t) {
                        c.push(t);
                    }
                }
                c
            }
            Some(c) => c.into_iter().filter(|e| targets.contains(e)).collect(),
        });
    }
    common.unwrap_or_default()
}

/// The formats the file at `p` can become with `available`; a video
/// subtitle formats only when `subtitles` says it holds a subtitle track.
fn targets_of(
    kiln: &Kiln,
    available: &Availability,
    p: &Path,
    subtitles: &dyn Fn(&Path) -> bool,
) -> Vec<String> {
    kiln.targets_split(p, available)
        .0
        .iter()
        .filter(|f| keeps_kind(kiln, p, f.kind, subtitles))
        .map(|f| f.ext.clone())
        .collect()
}

/// Files picked for a conversion to one format, split three ways (UX-06).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Split {
    /// Those that can become it: the job's.
    pub convert: Vec<PathBuf>,
    /// Those in it already, left alone: "Already JPG".
    pub already: Vec<PathBuf>,
    /// Those that can't become it with any helper, left out of the job:
    /// "Can't become WEBP".
    pub cant: Vec<PathBuf>,
    /// Those that could with a helper app that isn't there, each with
    /// the helper's backend ("ffmpeg"), left out of the job: "Needs
    /// FFmpeg to become MOV".
    pub needs: Vec<(PathBuf, String)>,
}

/// `paths` split for a conversion to `ext`: a file already in it is left
/// alone (`model::already_in`), and one that can't become it with
/// `available`, by the rule the wheel offers formats by
/// ([`formats_for`]), is left out. With `available` not known yet
/// (`None`), nothing is left out for that: core says why per file.
pub fn split_for(
    kiln: &Kiln,
    available: Option<&Availability>,
    paths: &[PathBuf],
    ext: &str,
    subtitles: &dyn Fn(&Path) -> bool,
) -> Split {
    let (rest, already) = crate::model::already_in(kiln, paths, ext);
    let Some(available) = available else {
        return Split {
            convert: rest,
            already,
            ..Split::default()
        };
    };
    let to = kiln
        .registry()
        .format(ext)
        .map_or_else(|| ext.to_ascii_lowercase(), |f| f.ext.clone());
    let mut split = Split {
        already,
        ..Split::default()
    };
    for p in rest {
        if targets_of(kiln, available, &p, subtitles).contains(&to) {
            split.convert.push(p);
        } else if let Some(helper) = helper_for(kiln, available, &p, &to, subtitles) {
            split.needs.push((p, helper));
        } else {
            split.cant.push(p);
        }
    }
    split
}

/// The helper app ("ffmpeg") the file at `p` waits on to become `to`,
/// when that is all it lacks: `to` is among the formats it would have
/// with the helper, by the rule [`targets_of`] goes by.
fn helper_for(
    kiln: &Kiln,
    available: &Availability,
    p: &Path,
    to: &str,
    subtitles: &dyn Fn(&Path) -> bool,
) -> Option<String> {
    kiln.targets_split(p, available)
        .1
        .into_iter()
        .find(|(f, _)| f.ext == to && keeps_kind(kiln, p, f.kind, subtitles))
        .map(|(_, helper)| helper)
}

/// Whether a file at `p` is offered formats of kind `to`: subtitle
/// formats only from a subtitle file, or a video `subtitles` says holds a
/// subtitle track.
fn keeps_kind(kiln: &Kiln, p: &Path, to: Kind, subtitles: &dyn Fn(&Path) -> bool) -> bool {
    let from = kiln.registry().format_of(p).map(|f| f.kind);
    to != Kind::Subtitle || from == Some(Kind::Subtitle) || subtitles(p)
}

/// The line of a row left out of a conversion it can't make: "Can't
/// become WEBP".
pub fn cant_line(ext: &str) -> String {
    format!(
        "Can't become {}",
        ext.trim_start_matches('.').to_ascii_uppercase()
    )
}

/// The line of a row left out of a conversion only because helper app
/// `backend` isn't there: "Needs FFmpeg to become MOV".
pub fn needs_line(ext: &str, backend: &str) -> String {
    format!(
        "Needs {} to become {}",
        crate::model::helper_name(backend),
        ext.trim_start_matches('.').to_ascii_uppercase()
    )
}

/// What a drag-wheel drop's notice adds for the files `split` left out of
/// a conversion to `ext`, whose rows the hidden window can't show: "a.jpg
/// is already JPG.", "3 files can't become JPG.", "b.avi needs FFmpeg to
/// become MOV.".
pub fn left_out_lines(split: &Split, ext: &str) -> Vec<String> {
    let code = ext.trim_start_matches('.').to_ascii_uppercase();
    let name = |p: &PathBuf| {
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let mut lines = vec![];
    match split.already.as_slice() {
        [] => {}
        [one] => lines.push(format!("{} is already {code}.", name(one))),
        many => lines.push(format!("{} files are already {code}.", many.len())),
    }
    match split.cant.as_slice() {
        [] => {}
        [one] => lines.push(format!("{} can't become {code}.", name(one))),
        many => lines.push(format!("{} files can't become {code}.", many.len())),
    }
    // Those waiting on a helper, by helper in the order first met.
    let mut helpers: Vec<&str> = vec![];
    for (_, h) in &split.needs {
        if !helpers.contains(&h.as_str()) {
            helpers.push(h);
        }
    }
    for h in helpers {
        let name_of = crate::model::helper_name(h);
        let theirs: Vec<&PathBuf> = split
            .needs
            .iter()
            .filter(|(_, b)| b == h)
            .map(|(p, _)| p)
            .collect();
        match theirs.as_slice() {
            [one] => lines.push(format!("{} needs {name_of} to become {code}.", name(one))),
            many => lines.push(format!(
                "{} files need {name_of} to become {code}.",
                many.len()
            )),
        }
    }
    lines
}

/// The ring and More for these files.
/// - Convert: Make smaller first where R2 allows it, then the files'
///   ring of formats ([`Order::ring`]). More holds the other formats,
///   grouped by kind with the files' own kind first, then the other tools.
/// - Tools: every tool on the ring, and no More.
fn build(
    kind: WheelKind,
    kiln: &Kiln,
    available: &Availability,
    paths: &[PathBuf],
    subtitles: &dyn Fn(&Path) -> bool,
) -> (Vec<Item>, Vec<Group>) {
    let refs: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
    // Tools are offered only for files of one kind, and named for it.
    let own = source_kind(kiln, paths);
    let tools: Vec<Item> = match own {
        Some(k) => kiln
            .offered_with(&refs, available)
            .tools
            .into_iter()
            .map(|t| Item {
                label: t.label(k).to_owned(),
                is_code: false,
                action: Action::Tool(t, k),
                kind: None,
            })
            .collect(),
        None => vec![],
    };
    if kind == WheelKind::Tools {
        return (tools, vec![]);
    }

    let order = Order::of(kiln, paths);
    let mut exts = formats_for(kiln, available, paths, subtitles);
    order.sort(&mut exts);
    let ring_list = order.ring();
    let (mut ring, mut rest): (Vec<Item>, Vec<Item>) = (vec![], vec![]);
    for ext in exts {
        let Some(f) = kiln.registry().format(&ext) else {
            continue;
        };
        let on_ring = if ring_list.is_empty() {
            ring.len() < RING_FORMATS
        } else {
            ring_list.contains(&ext.as_str())
        };
        let item = Item {
            label: f.code(),
            is_code: true,
            action: Action::Convert(ext),
            kind: Some(f.kind),
        };
        if on_ring {
            ring.push(item);
        } else {
            rest.push(item);
        }
    }
    // Make smaller, where it is offered, comes first; the other tools go
    // last in More.
    let (smaller, others): (Vec<Item>, Vec<Item>) = tools
        .into_iter()
        .partition(|t| matches!(t.action, Action::Tool(Tool::Smaller, _)));
    ring.splice(0..0, smaller);
    // Stable, so each group keeps the order its formats were sorted in.
    rest.sort_by_key(|item| group_order(own, item.kind));
    rest.extend(others);
    (ring, grouped(rest))
}

/// Everything a wheel offers as one list's groups: the ring's items and
/// More's, each kind's formats under that kind's heading, the files' own
/// kind (`own`) first and then the others as More has them, the tools last.
pub fn all_groups(ring: Vec<Item>, flyout: Vec<Group>, own: Option<Kind>) -> Vec<Group> {
    let mut groups: Vec<Group> = vec![];
    let mut tools: Vec<Item> = vec![];
    for item in ring
        .into_iter()
        .chain(flyout.into_iter().flat_map(|g| g.items))
    {
        let Some(kind) = item.kind else {
            tools.push(item);
            continue;
        };
        let title = group_title(kind);
        match groups.iter_mut().find(|g| g.title == title) {
            Some(g) => g.items.push(item),
            None => groups.push(Group {
                title,
                items: vec![item],
            }),
        }
    }
    // Stable, so each group keeps the order its formats were met in.
    let kind_of = |g: &Group| g.items.first().and_then(|i| i.kind);
    groups.sort_by_key(|g| group_order(own, kind_of(g)));
    if !tools.is_empty() {
        groups.push(Group {
            title: TOOLS_GROUP,
            items: tools,
        });
    }
    groups
}

/// The ring and More the app shows for these files ([`build`]).
pub fn offering(
    kind: WheelKind,
    kiln: &Kiln,
    available: &Availability,
    paths: &[PathBuf],
    subtitles: &dyn Fn(&Path) -> bool,
) -> (Vec<Item>, Vec<Group>) {
    build(kind, kiln, available, paths, subtitles)
}

/// Everything the wheel offers for these files ([`offering`]) as one
/// list, the ring then More.
pub fn items_for(
    kind: WheelKind,
    kiln: &Kiln,
    available: &Availability,
    paths: &[PathBuf],
    subtitles: &dyn Fn(&Path) -> bool,
) -> Vec<Item> {
    let (mut ring, flyout) = offering(kind, kiln, available, paths, subtitles);
    ring.extend(flyout.into_iter().flat_map(|g| g.items));
    ring
}

/// What a wheel opened for the selection has to offer, worked out before
/// it opens so that no empty wheel is ever shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Offer {
    /// Nothing is selected.
    Nothing,
    /// The wheel has something for these files: one kind of file, or
    /// several with something in common.
    Choices,
    /// Several kinds of file with nothing in common. The kinds present as
    /// (type code, files), most files first.
    Mixed(Vec<(String, usize)>),
    /// One kind of file that nothing applies to.
    Unsupported(String),
    /// Files whose formats all wait on a helper app that isn't installed:
    /// its backend ("ffmpeg") and the type that needs it.
    NeedsHelper(String, String),
}

/// The type code a file shows under: "MP4", or its extension, or "Other"
/// for a file without one.
fn type_code(kiln: &Kiln, path: &Path) -> String {
    match kiln.registry().format_of(path) {
        Some(f) => f.code(),
        None => path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_uppercase())
            .filter(|e| !e.is_empty())
            .unwrap_or_else(|| "Other".to_owned()),
    }
}

/// Decide what `kind`'s wheel can offer for `paths`.
pub fn offer(kind: WheelKind, kiln: &Kiln, available: &Availability, paths: &[PathBuf]) -> Offer {
    if paths.is_empty() {
        return Offer::Nothing;
    }
    if !items_for(kind, kiln, available, paths, &no_subtitles).is_empty() {
        return Offer::Choices;
    }
    // A file that has formats, none of which can run for want of a helper.
    for p in paths {
        let (usable, blocked) = kiln.targets_split(p, available);
        if let (true, Some((_, backend))) = (usable.is_empty(), blocked.first()) {
            return Offer::NeedsHelper(backend.clone(), type_code(kiln, p));
        }
    }
    let mut kinds: Vec<(String, usize)> = vec![];
    for p in paths {
        let code = type_code(kiln, p);
        match kinds.iter_mut().find(|(c, _)| *c == code) {
            Some((_, n)) => *n += 1,
            None => kinds.push((code, 1)),
        }
    }
    // Most files first; ties keep the order they were met in.
    kinds.sort_by_key(|a| std::cmp::Reverse(a.1));
    match kinds.as_slice() {
        [(code, _)] => Offer::Unsupported(code.clone()),
        _ => Offer::Mixed(kinds),
    }
}

/// The kinds present, "MP4 (3), DOCX (2)", the first four and a count of
/// the rest.
fn kinds_list(kinds: &[(String, usize)]) -> String {
    const SHOWN: usize = 4;
    let mut parts: Vec<String> = kinds
        .iter()
        .take(SHOWN)
        .map(|(c, n)| format!("{c} ({n})"))
        .collect();
    if kinds.len() > SHOWN {
        parts.push(format!("{} more", kinds.len() - SHOWN));
    }
    parts.join(", ")
}

/// The footer's sentence for an offer that has no wheel, with what to do
/// next. `None` when the wheel can open.
pub fn offer_sentence(kind: WheelKind, offer: &Offer) -> Option<String> {
    let verb = if kind == WheelKind::Tools {
        "use a tool on"
    } else {
        "convert"
    };
    match offer {
        Offer::Choices => None,
        Offer::Nothing => Some("Select a file first.".to_owned()),
        Offer::Unsupported(code) => Some(format!("Kiln can't {verb} {code} files.")),
        // Part of Windows, added in Settings rather than downloaded: the
        // row's Get Media Feature Pack opens the page.
        Offer::NeedsHelper(backend, _) if backend == "audio-mf" => {
            Some("Needs Windows' Media Feature Pack, which isn't installed.".to_owned())
        }
        Offer::NeedsHelper(backend, code) => {
            let name = crate::model::helper_name(backend);
            Some(match crate::model::get_link(backend) {
                Some(_) => format!(
                    "{code} files need {name}, which isn't installed. Use Get {name} to download it."
                ),
                // FFmpeg comes with Kiln: there is nowhere to send anyone.
                None => format!("{code} files need {name}, which isn't installed."),
            })
        }
        Offer::Mixed(kinds) => {
            let none = if kind == WheelKind::Tools {
                "No tool fits all of these files"
            } else {
                "These files have no format in common"
            };
            Some(format!(
                "{none}: {}. Select one kind at a time.",
                kinds_list(kinds)
            ))
        }
    }
}

/// "PNG" when every file shares a type, "MIXED" otherwise.
pub fn source_code(paths: &[PathBuf]) -> String {
    let codes: Vec<String> = paths
        .iter()
        .map(|p| {
            p.extension()
                .map(|e| e.to_string_lossy().to_ascii_uppercase())
                .unwrap_or_default()
        })
        .collect();
    match codes.first() {
        Some(first) if codes.iter().all(|c| c == first) && !first.is_empty() => first.clone(),
        Some(_) => "MIXED".to_owned(),
        None => String::new(),
    }
}

/// How many kinds of file `paths` are, by their formats. A file of a type
/// Kiln doesn't know counts as a kind of its own, one for each such type.
pub fn kinds_in(kiln: &Kiln, paths: &[PathBuf]) -> usize {
    let mut seen: Vec<Result<Kind, String>> = vec![];
    for p in paths {
        let k = kiln.registry().format_of(p).map(|f| f.kind).ok_or_else(|| {
            p.extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default()
        });
        if !seen.contains(&k) {
            seen.push(k);
        }
    }
    seen.len()
}

/// The kind every one of `paths` is, by its format; `None` when they are
/// of several kinds or one Kiln doesn't know.
pub fn source_kind(kiln: &Kiln, paths: &[PathBuf]) -> Option<Kind> {
    let mut kinds = paths
        .iter()
        .map(|p| kiln.registry().format_of(p).map(|f| f.kind));
    let first = kinds.next()??;
    kinds.all(|k| k == Some(first)).then_some(first)
}

/// Where a wheel reaching `extent` DIPs from its centre goes on `area`
/// (the window's content layer), and its scale: as near the point as it
/// fits whole, and smaller only when the area can't hold it at full size.
/// The room kept on every side is the shadow's (`SHADOW_GAP`), so no edge
/// of the layer cuts it off.
pub fn place(x: f32, y: f32, area: Rect, extent: f32) -> ((f32, f32), f32) {
    let room = (area.w.min(area.h) / 2.0 - SHADOW_GAP).max(1.0);
    let scale = (room / extent).min(1.0);
    let r = extent * scale + SHADOW_GAP;
    let across = |at: f32, from: f32, len: f32| {
        if len >= 2.0 * r {
            at.clamp(from + r, from + len - r)
        } else {
            from + len / 2.0
        }
    };
    (
        (across(x, area.x, area.w), across(y, area.y, area.h)),
        scale,
    )
}

/// Where a wheel goes that pours over the list beside the inspector, and
/// its scale: as [`place`] puts it on `area`, then moved
/// across so all it reaches stays between `across`, the list's left and
/// right edges, and made smaller when the list is narrower than it. So it
/// never covers the inspector, even once More's ring pours open.
pub fn place_within(
    x: f32,
    y: f32,
    area: Rect,
    extent: f32,
    across: (f32, f32),
) -> ((f32, f32), f32) {
    let ((cx, cy), scale) = place(x, y, area, extent);
    let (left, right) = across;
    let room = ((right - left) / 2.0 - EDGE_GAP).max(1.0);
    // Never larger than the window allowed: a smaller wheel still fits
    // where `place` put it up and down.
    let scale = scale.min(room / extent);
    let r = extent * scale + EDGE_GAP;
    let cx = if right - left >= 2.0 * r {
        cx.clamp(left + r, right - r)
    } else {
        (left + right) / 2.0
    };
    ((cx, cy), scale)
}

/// How a wheel was asked for, which alone decides whether it keeps to the
/// list. Set by whoever opens the wheel, never worked out from where the
/// pointer is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pour {
    /// From a key or beside a row: the Enter key, a row's link, the
    /// right-click menu, the footer's Convert. It keeps to the list, so it
    /// never covers the inspector.
    OverList,
    /// Under the pointer of a modifier-drag. It opens at the pointer, as
    /// far as the window allows, so a release there picks nothing.
    AtPointer,
}

/// Where a wheel reaching `extent` DIPs from its centre goes, poured as
/// `pour` says, and its scale. `list` is the list's left and right edges
/// while the inspector shows beside it, `None` when there is no inspector
/// to keep clear of.
pub fn place_pour(
    pour: Pour,
    x: f32,
    y: f32,
    area: Rect,
    extent: f32,
    list: Option<(f32, f32)>,
) -> ((f32, f32), f32) {
    match (pour, list) {
        (Pour::OverList, Some(across)) => place_within(x, y, area, extent, across),
        _ => place(x, y, area, extent),
    }
}

/// Keep a plain wheel on `area`.
pub fn clamp_center(x: f32, y: f32, area: Rect) -> (f32, f32) {
    place(x, y, area, R_OUTER).0
}

/// Start and end angle of item `i` of `n`, in radians, clockwise from the
/// positive x axis (screen coordinates, y down). Item 0 is centred at the
/// top and items go clockwise.
pub fn segment(i: usize, n: usize) -> (f32, f32) {
    let span = TAU / n as f32;
    let mid = -PI / 2.0 + i as f32 * span;
    (mid - span / 2.0, mid + span / 2.0)
}

/// Where item `i`'s code, or its glyph and caption, is centred, relative
/// to the wheel: on its middle line at `R_LABEL`.
pub fn label_point(i: usize, n: usize) -> (f32, f32) {
    if n == 1 {
        return (0.0, -R_LABEL);
    }
    let (a0, a1) = segment(i, n);
    let mid = (a0 + a1) / 2.0;
    (R_LABEL * mid.cos(), R_LABEL * mid.sin())
}

/// On a ring too crowded for the full gap and corners, they shrink
/// together until a segment's inner corners take at most this share of its
/// inner edge's half-angle.
const SEGMENT_FIT: f32 = 0.9;

/// One segment's outline (look spec, Spacing and shape): the annular
/// sector from `a0` to `a1` and `r0` to `r1`, its straight sides `d` in
/// from the sector's own so that neighbours stand `2d` apart (`GAP`), and
/// its four corners rounded `rc` (`CORNER`). A ring of one item is the
/// whole annulus, with no sides or corners.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmentShape {
    pub a0: f32,
    pub a1: f32,
    pub r0: f32,
    pub r1: f32,
    pub d: f32,
    pub rc: f32,
    pub full: bool,
}

impl SegmentShape {
    /// Segment `i` of `n` on the ring.
    pub fn of(i: usize, n: usize) -> Self {
        let (a0, a1) = segment(i, n.max(1));
        Self::new(a0, a1, R_INNER, R_OUTER, n <= 1)
    }

    /// The segment for a slot of a ring: `GAP` from its neighbours with
    /// corners of `CORNER`, both shrunk together on a slot too narrow for
    /// them, and the corners never more than half the ring's depth.
    pub fn new(a0: f32, a1: f32, r0: f32, r1: f32, full: bool) -> Self {
        let half = ((a1 - a0) / 2.0).min(PI / 2.0);
        let d = GAP / 2.0;
        let rc = CORNER.min((r1 - r0) / 2.0 - 0.5).max(0.0);
        // The corners' centres must sit inside the slot: (d + rc) at most
        // SEGMENT_FIT of the way across its half-angle at radius r0 + rc.
        let room = SEGMENT_FIT * half.sin();
        let need = d + rc - room * rc;
        let k = if need > 0.0 {
            (room * r0 / need).min(1.0)
        } else {
            1.0
        };
        Self {
            a0,
            a1,
            r0,
            r1,
            d: d * k,
            rc: rc * k,
            full,
        }
    }

    /// The angle where the corner of radius `rc` touches the arc at `r`
    /// on the `a0` side; the `a1` side's is as far in from `a1`.
    pub fn inset(&self, r: f32) -> f32 {
        ((self.d + self.rc) / r).clamp(-1.0, 1.0).asin()
    }

    /// The centre of the corner on side `a` (`toward` +1 for `a0`, -1 for
    /// `a1`) whose circle touches the arc at `r`.
    pub fn corner(&self, a: f32, toward: f32, r: f32) -> (f32, f32) {
        let dc = self.d + self.rc;
        let along = (r * r - dc * dc).max(0.0).sqrt();
        let (s, c) = a.sin_cos();
        (along * c - toward * dc * s, along * s + toward * dc * c)
    }

    /// Its middle angle.
    pub fn mid(&self) -> f32 {
        (self.a0 + self.a1) / 2.0
    }

    /// Whether `(x, y)`, relative to the wheel, is inside the outline: on
    /// the ring, and at least `d` from both of its sides. The corners'
    /// rounding is left out.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        let r = x.hypot(y);
        if r < self.r0 || r > self.r1 {
            return false;
        }
        if self.full {
            return true;
        }
        let a = y.atan2(x);
        let rel = (a - self.a0).rem_euclid(TAU);
        if rel > self.a1 - self.a0 {
            return false;
        }
        // Its distance from each side's line through the centre.
        let from = |side: f32| (side.cos() * y - side.sin() * x).abs();
        from(self.a0) >= self.d && from(self.a1) >= self.d
    }

    /// The kind's arc: just inside the outer edge, clear of the corners.
    pub fn kind_arc(&self) -> (f32, f32, f32) {
        let r = self.r1 - KIND_ARC_INSET;
        if self.full {
            return (0.0, TAU, r);
        }
        let trim = ((self.d + self.rc + 1.0) / r).min((self.a1 - self.a0) / 3.0);
        (self.a0 + trim, self.a1 - trim, r)
    }
}

/// Which of `n` equal slots an angle falls in, measured clockwise from
/// the top with slot 0 centred there.
fn slot_at(from_top: f32, n: usize) -> usize {
    let span = TAU / n as f32;
    ((from_top + span / 2.0) / span).floor() as usize % n
}

/// What choosing a spot on the wheel does, however it was chosen: a
/// click, a drop or a release over it, Enter, or UI Automation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Picked {
    /// More opens its list.
    More,
    /// The options dialog for this format, whose Convert converts: Tab on
    /// a lit format, or More's "Options…".
    Options(String),
    /// A format converts with Settings' quality, or a tool runs.
    Run(Action),
}

/// What choosing `spot` on `w` does. `None` where there is nothing, and
/// for "Options…", which is for the format last lit ([`picked_options`]).
pub fn picked(w: &Wheel, spot: Spot) -> Option<Picked> {
    if w.is_more_spot(spot) {
        return Some(Picked::More);
    }
    Some(pick_item(w.item(spot)?))
}

/// What choosing `item` does, on either wheel: it runs.
pub fn pick_item(item: &Item) -> Picked {
    Picked::Run(item.action.clone())
}

/// What "Options…" or Tab does for `spot`: the options for its format.
/// `None` when `spot` is no format.
pub fn picked_options(w: &Wheel, spot: Option<Spot>) -> Option<Picked> {
    Some(Picked::Options(w.format_at(spot?)?.to_owned()))
}

/// Which segment of the ring is under a point (window DIPs): by angle,
/// anywhere from the inner edge to `REACH` past the outer one. The hub
/// selects nothing, and a point in the gap between two segments still
/// hits by its angle. More's list is hit by [`flyout_hit`].
pub fn hit(w: &Wheel, x: f32, y: f32) -> Option<Spot> {
    let n = w.segments();
    if n == 0 {
        return None;
    }
    let s = w.scale.max(0.01);
    let (dx, dy) = ((x - w.center.0) / s, (y - w.center.1) / s);
    let d = dx.hypot(dy);
    let from_top = (dy.atan2(dx) + PI / 2.0).rem_euclid(TAU);
    (R_INNER..=R_OUTER + REACH)
        .contains(&d)
        .then(|| Spot::Inner(slot_at(from_top, n)))
}

/// More's list: this wide, its items inside this much padding, and this
/// far from the ring.
pub const FLYOUT_W: f32 = 360.0;
/// The drag-wheel's list stays this wide: its window is square, and a wider
/// list would make it taller than a 1080p screen.
pub const FLYOUT_W_DRAG: f32 = 300.0;
pub const FLYOUT_PAD: f32 = 4.0;
pub const FLYOUT_GAP: f32 = 8.0;
/// A group's heading (Caption, 10 above and 4 below); a format's row, whose
/// description wraps to two lines; a tool's row (its name, then what it
/// does over two lines); and the footer row, "Options for JPG…", which
/// stays at the bottom.
pub const FLYOUT_HEADING: f32 = 30.0;
pub const FLYOUT_ITEM: f32 = 44.0;
pub const FLYOUT_TOOL: f32 = 60.0;
pub const FLYOUT_FOOTER: f32 = 40.0;

/// How wide `w`'s list is.
pub fn flyout_width(w: &Wheel) -> f32 {
    if w.origin == Origin::Drag {
        FLYOUT_W_DRAG
    } else {
        FLYOUT_W
    }
}

/// How tall a line of More's list is.
pub fn flyout_row_height(row: &FlyoutRow) -> f32 {
    match row {
        FlyoutRow::Heading(_) => FLYOUT_HEADING,
        FlyoutRow::Item(_, item) if matches!(item.action, Action::Tool(..)) => FLYOUT_TOOL,
        FlyoutRow::Item(..) => FLYOUT_ITEM,
        FlyoutRow::Options(_) => FLYOUT_FOOTER,
    }
}

/// Where More's list is: its frame, each line's box (scrolled), how tall
/// its scrolling lines are together, and how far it is scrolled. The
/// footer row is not among the scrolling lines: it stays at the bottom.
#[derive(Debug, Clone, PartialEq)]
pub struct FlyoutPlace {
    pub rect: Rect,
    /// One box per line, in the order of the rows it was laid out for.
    pub rows: Vec<Rect>,
    pub content: f32,
    pub scroll: f32,
    /// The box the scrolling lines show in.
    pub body: Rect,
    /// The footer row's box, when there is one.
    pub footer: Option<Rect>,
}

impl FlyoutPlace {
    /// The box the scrolling lines show in.
    pub fn view(&self) -> Rect {
        self.body
    }

    /// The most it scrolls: 0 when every line fits.
    pub fn max_scroll(&self) -> f32 {
        (self.content - self.body.h).max(0.0)
    }

    /// The scroll that keeps line `i` in view, from `self.scroll`. The
    /// footer row never scrolls.
    pub fn scroll_to(&self, i: usize) -> f32 {
        let Some(r) = self.rows.get(i) else {
            return self.scroll;
        };
        if self.footer == Some(*r) {
            return self.scroll;
        }
        let v = self.view();
        let mut s = self.scroll;
        if r.y < v.y {
            s -= v.y - r.y;
        } else if r.bottom() > v.bottom() {
            s += r.bottom() - v.bottom();
        }
        s.clamp(0.0, self.max_scroll())
    }
}

/// How tall the scrolling lines `rows` show in `room`: all of them if they
/// fit, else whole lines only, so the list ends on a line's edge, and never
/// on a heading with nothing under it.
fn snapped_height(rows: &[&FlyoutRow], room: f32) -> f32 {
    let total: f32 = rows.iter().map(|r| flyout_row_height(r)).sum();
    if total <= room {
        return total;
    }
    let (mut sum, mut kept) = (0.0, 0.0);
    for r in rows {
        let rh = flyout_row_height(r);
        if sum + rh > room {
            break;
        }
        sum += rh;
        if !matches!(r, FlyoutRow::Heading(_)) {
            kept = sum;
        }
    }
    if kept > 0.0 {
        kept
    } else {
        room
    }
}

/// More's list for `w`, lines `rows`, in `bounds` (window DIPs), scrolled
/// `scroll`: beside the ring on the right, or on the left when the right
/// has no room, or inside the bounds over the ring when neither has; for
/// the list alone, to the left of the point it is opened from. Centred on
/// the wheel up and down, as far as the bounds let it, and no taller than
/// they are: the lines show whole at rest (the height is a whole number of
/// lines) and the rest scrolls, under the footer row.
pub fn flyout_place(w: &Wheel, rows: &[FlyoutRow], bounds: Rect, scroll: f32) -> FlyoutPlace {
    let fw = flyout_width(w);
    let list: Vec<&FlyoutRow> = rows
        .iter()
        .filter(|r| !matches!(r, FlyoutRow::Options(_)))
        .collect();
    let footer_h = if list.len() < rows.len() {
        FLYOUT_FOOTER
    } else {
        0.0
    };
    let content: f32 = list.iter().map(|r| flyout_row_height(r)).sum();
    let room = (bounds.h - 2.0 * EDGE_GAP).max(0.0);
    let body_room = (room - 2.0 * FLYOUT_PAD - footer_h).max(0.0);
    let body_h = snapped_height(&list, body_room);
    let h = body_h + 2.0 * FLYOUT_PAD + footer_h;
    let (cx, cy) = w.center;
    let reach = w.extent() * w.scale + FLYOUT_GAP;
    let (lo, hi) = (bounds.x + EDGE_GAP, bounds.right() - EDGE_GAP);
    let x = if w.list_only {
        // Beside what asked for it, ending at its edge: the point is the
        // list's right edge, so the list lies to its left.
        (cx - fw).min(hi - fw).max(lo)
    } else if cx + reach + fw <= hi {
        cx + reach
    } else if cx - reach - fw >= lo {
        cx - reach - fw
    } else {
        (cx + reach).min(hi - fw).max(lo)
    };
    let (top, bottom) = (bounds.y + EDGE_GAP, bounds.bottom() - EDGE_GAP);
    let y = (cy - h / 2.0).min(bottom - h).max(top);
    let rect = Rect::new(x, y, fw, h);
    let body = Rect::new(
        x + FLYOUT_PAD,
        y + FLYOUT_PAD,
        fw - 2.0 * FLYOUT_PAD,
        body_h,
    );
    let footer = (footer_h > 0.0).then(|| Rect::new(body.x, body.bottom(), body.w, footer_h));
    let scroll = scroll.clamp(0.0, (content - body_h).max(0.0));
    let mut at = body.y - scroll;
    let boxes = rows
        .iter()
        .map(|row| match (row, footer) {
            (FlyoutRow::Options(_), Some(f)) => f,
            _ => {
                let rh = flyout_row_height(row);
                let r = Rect::new(body.x, at, body.w, rh);
                at += rh;
                r
            }
        })
        .collect();
    FlyoutPlace {
        rect,
        rows: boxes,
        content,
        scroll,
        body,
        footer,
    }
}

/// Which line of More's list is under `(x, y)`, as a spot: a format or a
/// tool, or the footer row. A heading, or a line scrolled out of view, is
/// nothing; so is a point off the list.
pub fn flyout_hit(place: &FlyoutPlace, rows: &[FlyoutRow], x: f32, y: f32) -> Option<Spot> {
    let v = place.view();
    rows.iter()
        .zip(&place.rows)
        .find(|(row, r)| {
            let on = if matches!(row, FlyoutRow::Options(_)) {
                place.footer.is_some()
            } else {
                v.contains(x, y)
            };
            on && r.contains(x, y)
        })
        .and_then(|(row, _)| row_spot(row))
}

/// The spot a line of More's list stands for.
pub fn row_spot(row: &FlyoutRow) -> Option<Spot> {
    match row {
        FlyoutRow::Heading(_) => None,
        FlyoutRow::Item(spot, _) => Some(*spot),
        FlyoutRow::Options(_) => Some(Spot::Options),
    }
}

/// The spot `delta` lines on from `from` in More's list, skipping
/// headings and wrapping round; the first line from nothing going down,
/// the last going up.
pub fn flyout_step(rows: &[FlyoutRow], from: Option<Spot>, delta: isize) -> Option<Spot> {
    let spots: Vec<Spot> = rows.iter().filter_map(row_spot).collect();
    let at = from.and_then(|f| spots.iter().position(|s| *s == f));
    step(at, delta, spots.len()).map(|i| spots[i])
}

/// Where a suggestion sits on the wheel: its inner segment, or More and
/// its line in the list More opens. Empty when the wheel doesn't offer it.
pub fn marks(w: &Wheel, s: &Suggested) -> Vec<Spot> {
    // A tool's kind comes from the files, the same for every item.
    let is = |it: &Item| match (s, &it.action) {
        (Suggested::Convert(ext), Action::Convert(e)) => e == ext,
        (Suggested::Tool(t), Action::Tool(u, _)) => t == u,
        _ => false,
    };
    if let Some(i) = w.items.iter().position(is) {
        return vec![Spot::Inner(i)];
    }
    match w.more_items().position(is) {
        Some(j) => vec![Spot::Inner(w.items.len()), Spot::Flyout(j)],
        None => vec![],
    }
}

/// Where the keyboard starts on a wheel: on the suggestion, or More when
/// the suggestion is in it, or else the first segment.
pub fn start_spot(w: &Wheel, s: Option<&Suggested>) -> Spot {
    s.and_then(|s| marks(w, s).first().copied())
        .unwrap_or(Spot::Inner(0))
}

/// A format as the wheel names it: "WEBP: Small image for the web". A
/// format Kiln has no description for is just its code. The footer, the
/// options sheet and the screen reader's names for the segments all use
/// this.
pub fn target_label(ext: &str) -> String {
    let code = ext.trim_start_matches('.').to_ascii_uppercase();
    match kiln_core::describe_format(ext) {
        Some(what) => format!("{code}: {what}"),
        None => code,
    }
}

/// [`target_label`] for a target the source becomes by way of its text alone
/// (`text_only`: a PDF made into another kind of document): the format's
/// code and what the conversion keeps, "DOCX (text only)". Any other
/// target reads as [`target_label`] has it.
pub fn target_label_for(ext: &str, text_only: bool) -> String {
    if text_only {
        format!(
            "{} (text only)",
            ext.trim_start_matches('.').to_ascii_uppercase()
        )
    } else {
        target_label(ext)
    }
}

/// The footer on an open wheel while a convert target is lit: one
/// sentence, the code, what the format is and, for one file, its likely
/// size, "JPG. Photo that opens anywhere, about 85 KB." For a target
/// `text_only` says the files become by way of their text it is
/// "DOCX (text only)" (`target_label_for`). A format Kiln has no words for
/// is its code alone. Tools and More have nothing to say, and leave the
/// footer as it was.
pub fn lit_footer(
    item: Option<&Item>,
    text_only: &dyn Fn(&str) -> bool,
    estimate: &dyn Fn(&str) -> Option<u64>,
) -> Option<String> {
    let Action::Convert(ext) = &item?.action else {
        return None;
    };
    if text_only(ext) {
        return Some(target_label_for(ext, true));
    }
    let code = ext.trim_start_matches('.').to_ascii_uppercase();
    let Some(what) = kiln_core::describe_format(ext) else {
        return Some(code);
    };
    Some(
        match estimate(ext).filter(|&b| b >= crate::inspector::MIN_ESTIMATE) {
            Some(bytes) => format!("{code}. {what}, about {}.", kiln_core::about(bytes)),
            None => format!("{code}. {what}."),
        },
    )
}

/// Make smaller's caption on its segment, on two lines as the tools' are,
/// and with the name the app uses everywhere else.
pub const MAKE_SMALLER_CAPTION: &str = "Make\nsmaller";

/// Keyboard movement: +1 is clockwise.
pub fn step(current: Option<usize>, delta: isize, n: usize) -> Option<usize> {
    if n == 0 {
        return None;
    }
    let n = n as isize;
    Some(match current {
        None if delta >= 0 => 0,
        None => (n - 1) as usize,
        Some(i) => (i as isize + delta).rem_euclid(n) as usize,
    })
}

/// What moves on an open wheel, the same on the one over Kiln's window and
/// the drag-wheel's floating one: what is lit, More's list opening, and
/// the wheel's own fade and scale since it opened (look spec, Motion).
/// Every time is passed in, so it is tested without a clock.
#[derive(Debug, Clone, Copy)]
pub struct WheelMotion {
    /// What is lit.
    pub hover: Option<Spot>,
    /// When the wheel opened: it fades in and scales from 0.92 to 1.
    pub opened: Instant,
    /// When More's list opened, while it is open: it fades in.
    pub flyout: Option<Instant>,
    /// The last format lit, on the ring or in the list: what "Options…"
    /// is for.
    pub format: Option<Spot>,
    /// Pointing at More opens its list, as on a wheel under a drag, where
    /// nothing can be clicked; pointing at another segment then closes it.
    /// Otherwise More opens when it is chosen.
    pub hover_opens: bool,
}

impl WheelMotion {
    /// A wheel that opened at `now`, nothing lit and More shut.
    pub fn new(now: Instant) -> Self {
        WheelMotion {
            hover: None,
            opened: now,
            flyout: None,
            format: None,
            hover_opens: false,
        }
    }

    /// A wheel that opened at `now` with `hover` already lit, as one
    /// opened from the keyboard is.
    pub fn starting_on(wheel: &Wheel, hover: Option<Spot>, now: Instant) -> Self {
        let mut m = WheelMotion::new(now);
        m.set_hover(wheel, hover);
        m
    }

    /// Whether More's list is open.
    pub fn flyout_open(&self) -> bool {
        self.flyout.is_some()
    }

    /// Open More's list at `now`, if it isn't.
    pub fn open_flyout(&mut self, now: Instant) {
        self.flyout.get_or_insert(now);
    }

    pub fn close_flyout(&mut self) {
        self.flyout = None;
    }

    /// Light `spot`, or nothing, remembering a format.
    pub fn set_hover(&mut self, wheel: &Wheel, spot: Option<Spot>) {
        self.hover = spot;
        if spot.and_then(|s| wheel.format_at(s)).is_some() {
            self.format = spot;
        }
    }

    /// Point at `spot` on `wheel`: light it, and where pointing opens
    /// More, open or close its list. Returns whether anything changed.
    pub fn point(&mut self, wheel: Option<&Wheel>, spot: Option<Spot>, now: Instant) -> bool {
        if spot == self.hover {
            return false;
        }
        let Some(w) = wheel else {
            self.hover = spot;
            return true;
        };
        if self.hover_opens {
            match spot {
                Some(s) if w.is_more_spot(s) => self.open_flyout(now),
                Some(Spot::Inner(_)) => self.close_flyout(),
                _ => {}
            }
        }
        self.set_hover(w, spot);
        true
    }

    /// How far it has opened at `now`: its opacity and its scale about
    /// its centre.
    pub fn opening(&self, now: Instant, animations: bool) -> (f32, f32) {
        anim::wheel_open(anim::elapsed_ms(self.opened, now), animations)
    }

    /// How far More's list has faded in at `now`, 0 to 1; 0 while shut.
    pub fn flyout_shown(&self, now: Instant, animations: bool) -> f32 {
        self.flyout.map_or(0.0, |t| {
            anim::fluent(fluent_motion::SMALL, anim::elapsed_ms(t, now), animations)
        })
    }

    /// Whether anything on the wheel is moving at `now`.
    pub fn animating(&self, now: Instant, animations: bool) -> bool {
        self.opening(now, animations).0 < 1.0 || {
            let f = self.flyout_shown(now, animations);
            self.flyout.is_some() && f < 1.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wheel reaching further than the ring, as the old More ring did:
    /// `place` takes any reach.
    const BIG: f32 = 208.0;

    fn item(i: usize, kind: Kind) -> Item {
        Item {
            label: format!("F{i}"),
            is_code: true,
            action: Action::Convert(format!("f{i}")),
            kind: Some(kind),
        }
    }

    fn wheel(n: usize) -> Wheel {
        Wheel {
            kind: WheelKind::Convert,
            origin: Origin::Window,
            items: (0..n).map(|i| item(i, Kind::Image)).collect(),
            flyout: vec![],
            center: (200.0, 200.0),
            scale: 1.0,
            source_code: "PNG".into(),
            source_kind: None,
            list_only: false,
            settable: (0..n).map(|i| format!("f{i}")).collect(),
        }
    }

    #[test]
    fn item_zero_is_at_the_top_and_items_go_clockwise() {
        let w = wheel(4);
        let r = (R_INNER + R_OUTER) / 2.0;
        let at = |x, y| hit(&w, x, y);
        assert_eq!(at(200.0, 200.0 - r), Some(Spot::Inner(0)));
        assert_eq!(at(200.0 + r, 200.0), Some(Spot::Inner(1)));
        assert_eq!(at(200.0, 200.0 + r), Some(Spot::Inner(2)));
        assert_eq!(at(200.0 - r, 200.0), Some(Spot::Inner(3)));
    }

    #[test]
    fn centre_and_far_away_select_nothing() {
        let w = wheel(5);
        let at = |x, y| hit(&w, x, y);
        assert_eq!(at(200.0, 200.0), None);
        assert_eq!(at(200.0, 200.0 - R_INNER + 1.0), None);
        assert_eq!(at(200.0, 200.0 - R_OUTER - REACH - 1.0), None);
        assert_eq!(at(200.0, 200.0 - R_OUTER - 10.0), Some(Spot::Inner(0)));
    }

    #[test]
    fn a_single_item_owns_the_whole_ring() {
        let w = wheel(1);
        for (x, y) in [(300.0, 200.0), (100.0, 200.0), (200.0, 300.0)] {
            assert_eq!(hit(&w, x, y), Some(Spot::Inner(0)));
        }
    }

    #[test]
    fn segments_tile_the_circle() {
        for n in 1..=10 {
            let mut total = 0.0;
            for i in 0..n {
                let (a, b) = segment(i, n);
                total += b - a;
                let (lx, ly) = label_point(i, n);
                let w = Wheel {
                    center: (0.0, 0.0),
                    ..wheel(n)
                };
                assert_eq!(hit(&w, lx, ly), Some(Spot::Inner(i)), "n={n} i={i}");
            }
            assert!((total - TAU).abs() < 1e-4);
        }
    }

    #[test]
    fn the_wheel_stays_on_the_area_with_its_shadow_inside() {
        // the room kept on every side is the shadow's 24, from the
        // content layer's edges (here a 720 x 480 layer at the origin).
        let area = Rect::new(0.0, 0.0, 720.0, 480.0);
        assert_eq!(SHADOW_GAP, 24.0);
        assert_eq!(clamp_center(10.0, 10.0, area), (160.0, 160.0));
        assert_eq!(clamp_center(700.0, 470.0, area), (560.0, 320.0));
        assert_eq!(clamp_center(300.0, 200.0, area), (300.0, 200.0));
        // A layer that isn't at the origin keeps its own edges.
        let layer = Rect::new(16.0, 48.0, 688.0, 416.0);
        assert_eq!(clamp_center(0.0, 0.0, layer), (176.0, 208.0));
        assert_eq!(clamp_center(2000.0, 2000.0, layer), (544.0, 304.0));
        // Too small an area shrinks it to fit with that room all round.
        let tiny = Rect::new(0.0, 0.0, 200.0, 200.0);
        assert_eq!(clamp_center(10.0, 10.0, tiny), (100.0, 100.0));
    }

    #[test]
    fn a_wheel_poured_from_the_footer_keeps_its_shadow_inside_the_layer() {
        // from the footer's Convert, at the layer's bottom edge,
        // the ring and the shadow it drops stay inside the content layer.
        for (w, h) in [(1100.0, 720.0), (880.0, 600.0), (440.0, 300.0)] {
            let layer = crate::layout::content_layer(w, h, crate::layout::TITLE_BAR);
            let ((cx, cy), s) = place(w - 100.0, h - 30.0, layer, R_OUTER);
            let r = R_OUTER * s;
            assert!(
                cy + r + SHADOW_GAP <= layer.bottom() + 0.01,
                "{w}x{h}: {cy} {s}"
            );
            assert!(cy - r - SHADOW_GAP >= layer.y - 0.01, "{w}x{h}: {cy} {s}");
            assert!(cx - r - SHADOW_GAP >= layer.x - 0.01, "{w}x{h}: {cx} {s}");
            assert!(
                cx + r + SHADOW_GAP <= layer.right() + 0.01,
                "{w}x{h}: {cx} {s}"
            );
        }
    }

    #[test]
    fn a_wheel_with_more_fits_or_shrinks() {
        let area = Rect::new(0.0, 0.0, 720.0, 480.0);
        let ((cx, cy), s) = place(10.0, 10.0, area, BIG);
        assert_eq!((cx, cy, s), (BIG + SHADOW_GAP, BIG + SHADOW_GAP, 1.0));
        // The smallest window holds the whole wheel, a little smaller.
        let small = Rect::new(0.0, 0.0, 480.0, 360.0);
        let ((cx, cy), s) = place(10.0, 10.0, small, BIG);
        assert!(s < 1.0);
        assert!(cy - BIG * s >= SHADOW_GAP - 0.01, "{cy} {s}");
        assert!(cx - BIG * s >= SHADOW_GAP - 0.01);
    }

    #[test]
    fn a_wheel_poured_beside_a_row_stays_over_the_list() {
        // Poured at a row's right end, where its link is, right by the
        // inspector: the wheel and the ring More pours keep to the list.
        let (w, h) = (1300.0, 800.0);
        let l = crate::layout::layout(w, h, 5);
        let area = Rect::new(0.0, 0.0, w, h);
        let (list, inspector) = (l.list, l.inspector.expect("an inspector"));
        let at = (list.right() - 30.0, list.y + 40.0);
        for extent in [R_OUTER, BIG] {
            let ((cx, cy), s) = place_within(at.0, at.1, area, extent, (list.x, list.right()));
            assert_eq!(s, 1.0, "the list is wide enough for it whole");
            assert!(cx + extent <= list.right() - EDGE_GAP + 0.01, "{cx}");
            assert!(cx + extent < inspector.x);
            assert!(cx - extent >= list.x + EDGE_GAP - 0.01);
            // Up and down, it goes where it would in the window.
            assert_eq!(cy, place(at.0, at.1, area, extent).0 .1);
        }
        // Anywhere well inside the list, it pours where it was asked.
        let mid = (list.x + list.w / 2.0, list.y + list.h / 2.0);
        let (c, _) = place_within(mid.0, mid.1, area, R_OUTER, (list.x, list.right()));
        assert_eq!(c, mid);
        // At the fold point the list is narrower than More's ring: the
        // wheel is made smaller rather than reach over the inspector.
        let (w, h) = (760.0, 600.0);
        let l = crate::layout::layout(w, h, 5);
        let area = Rect::new(0.0, 0.0, w, h);
        let (list, inspector) = (l.list, l.inspector.expect("an inspector at 760"));
        assert!(list.w < 2.0 * (BIG + EDGE_GAP), "{list:?}");
        let ((cx, _), s) = place_within(list.right(), list.y, area, BIG, (list.x, list.right()));
        assert!(s < 1.0);
        assert!(cx + BIG * s <= list.right() + 0.01);
        assert!(cx + BIG * s < inspector.x);
        assert!(cx - BIG * s >= list.x - 0.01);
    }

    #[test]
    fn only_a_wheel_poured_over_the_list_keeps_to_it() {
        // whether a wheel keeps to the list is the way it was asked
        // for, not where the pointer is.
        let (w, h) = (1300.0, 800.0);
        let l = crate::layout::layout(w, h, 5);
        let area = Rect::new(0.0, 0.0, w, h);
        let (list, inspector) = (l.list, l.inspector.expect("an inspector"));
        let across = Some((list.x, list.right()));
        let at = |x, y, pour| place_pour(pour, x, y, area, R_OUTER, across);

        // A drag wheel right by the inspector's rule, within the wheel's
        // reach of it: it opens at the pointer, not moved to the left.
        let (px, py) = (list.right() - 30.0, list.y + 200.0);
        assert!(px + R_OUTER + EDGE_GAP > list.right(), "near the rule");
        let (c, s) = at(px, py, Pour::AtPointer);
        assert_eq!((c, s), ((px, py), 1.0));
        assert_eq!((c, s), place(px, py, area, R_OUTER));
        // The pointer starts in the hole, on no format, so releasing there
        // converts nothing.
        let drag = Wheel {
            center: c,
            scale: s,
            ..wheel(5)
        };
        assert_eq!(hit(&drag, px, py), None);

        // The same point poured from a key or a row keeps to the list.
        let (c, s) = at(px, py, Pour::OverList);
        assert!(c.0 < px, "moved left, off the rule: {c:?}");
        assert!(c.0 + R_OUTER * s <= list.right() - EDGE_GAP + 0.01);
        assert!(c.0 + R_OUTER * s < inspector.x);
        assert_eq!(
            c,
            place_within(px, py, area, R_OUTER, (list.x, list.right())).0
        );

        // Which way it was asked for decides it, not the point: a point
        // past the list, as the footer's Convert is, keeps to the list too
        // when poured over it, and stays put when dragged.
        let (fx, fy) = (inspector.x + 40.0, h - 30.0);
        assert!(!list.contains(fx, fy));
        let (c, s) = at(fx, fy, Pour::OverList);
        assert!(c.0 + R_OUTER * s < inspector.x, "{c:?}");
        assert_eq!(at(fx, fy, Pour::AtPointer).0 .0, fx);

        // Well inside the list, or with no inspector to keep clear of, the
        // two agree.
        let mid = (list.x + list.w / 2.0, list.y + list.h / 2.0);
        assert_eq!(
            at(mid.0, mid.1, Pour::OverList),
            at(mid.0, mid.1, Pour::AtPointer)
        );
        for pour in [Pour::OverList, Pour::AtPointer] {
            assert_eq!(
                place_pour(pour, px, py, area, R_OUTER, None),
                place(px, py, area, R_OUTER)
            );
        }
    }

    #[test]
    fn keyboard_steps_wrap() {
        assert_eq!(step(None, 1, 3), Some(0));
        assert_eq!(step(None, -1, 3), Some(2));
        assert_eq!(step(Some(2), 1, 3), Some(0));
        assert_eq!(step(Some(0), -1, 3), Some(2));
        assert_eq!(step(None, 1, 0), None);
    }

    fn with_more() -> Wheel {
        let kinds = [Kind::Image, Kind::Video, Kind::Audio];
        Wheel {
            items: (0..8).map(|i| item(i, Kind::Image)).collect(),
            flyout: grouped((8..20).map(|i| item(i, kinds[(i - 8) / 4])).collect()),
            settable: (0..20).map(|i| format!("f{i}")).collect(),
            ..wheel(0)
        }
    }

    #[test]
    fn more_is_the_last_segment_and_the_wheel_reaches_only_its_ring() {
        let w = with_more();
        assert_eq!(w.segments(), 9);
        assert!(w.is_more(8) && !w.is_more(0));
        assert!(w.is_more_spot(Spot::Inner(8)) && !w.is_more_spot(Spot::Flyout(0)));
        // No outer ring: More's list goes beside the ring.
        assert_eq!(w.extent(), R_OUTER);
        assert_eq!(wheel(3).extent(), R_OUTER);
        assert_eq!(wheel(3).segments(), 3);
    }

    #[test]
    fn the_ring_is_flat_from_56_to_136_round_a_104_dip_hub() {
        assert_eq!((R_INNER, R_OUTER, 2.0 * R_HUB), (56.0, 136.0, 104.0));
        assert_eq!((GAP, CORNER), (2.0, 4.0));
        const { assert!(R_HUB < R_INNER, "the hub sits a gap inside the ring") };
    }

    #[test]
    fn neighbouring_segments_stand_two_dips_apart() {
        for n in [2, 3, 6, 8, 9] {
            for i in 0..n {
                let a = SegmentShape::of(i, n);
                let b = SegmentShape::of((i + 1) % n, n);
                assert_eq!((a.d, a.rc), (1.0, 4.0), "n={n}: full gap and corners");
                // Along the line between them: in neither within a DIP of
                // it, and in one just past.
                let edge = a.a1;
                let (along, across) = ((edge.cos(), edge.sin()), (-edge.sin(), edge.cos()));
                for r in [R_INNER + 8.0, (R_INNER + R_OUTER) / 2.0, R_OUTER - 8.0] {
                    let at =
                        |off: f32| (along.0 * r + across.0 * off, along.1 * r + across.1 * off);
                    for off in [-0.9, 0.0, 0.9] {
                        let (x, y) = at(off);
                        assert!(!a.contains(x, y) && !b.contains(x, y), "n={n} i={i} r={r}");
                    }
                    let (x, y) = at(-1.1);
                    assert!(a.contains(x, y) && !b.contains(x, y), "n={n} i={i}");
                    let (x, y) = at(1.1);
                    assert!(b.contains(x, y) && !a.contains(x, y), "n={n} i={i}");
                }
            }
        }
        // One item is the whole ring.
        let one = SegmentShape::of(0, 1);
        assert!(one.full && one.contains(0.0, R_INNER + 1.0) && one.contains(R_OUTER - 1.0, 0.0));
    }

    #[test]
    fn a_segment_covers_the_ring_between_its_edges_and_its_kind_arc_sits_inside() {
        let s = SegmentShape::of(0, 6);
        let (lx, ly) = label_point(0, 6);
        assert!(s.contains(lx, ly));
        assert!(!s.contains(0.0, -(R_INNER - 1.0)) && !s.contains(0.0, -(R_OUTER + 1.0)));
        let (a0, a1, r) = s.kind_arc();
        assert_eq!(r, R_OUTER - KIND_ARC_INSET);
        assert!(a0 > s.a0 && a1 < s.a1 && a0 < a1);
        // Clear of the corners: past the side and the corner's radius.
        assert!((a0 - s.a0) * r >= s.d + s.rc);
    }

    #[test]
    fn labels_sit_on_each_segments_middle_a_little_past_the_ring_s_middle() {
        assert_eq!(R_LABEL, 98.0);
        let (x, y) = label_point(0, 4);
        assert!(x.abs() < 1e-4 && (y + R_LABEL).abs() < 1e-4);
        let (x, y) = label_point(1, 4);
        assert!((x - R_LABEL).abs() < 1e-4 && y.abs() < 1e-4);
        assert_eq!(label_point(0, 1), (0.0, -R_LABEL));
    }

    #[test]
    fn hits_go_by_angle_and_radius_at_every_scale() {
        // The window's DPI never reaches the model (all DIPs); a wheel
        // made smaller for a small window is hit at its own size.
        for scale in [1.0, 0.8, 0.5] {
            let w = Wheel { scale, ..wheel(6) };
            for i in 0..6 {
                let (lx, ly) = label_point(i, 6);
                let at = (200.0 + lx * scale, 200.0 + ly * scale);
                assert_eq!(hit(&w, at.0, at.1), Some(Spot::Inner(i)), "{scale}");
                // In the gap at its edge it is still by angle.
                let (a0, _) = segment(i, 6);
                let r = (R_INNER + R_OUTER) / 2.0 * scale;
                let edge = (200.0 + r * (a0 + 1e-3).cos(), 200.0 + r * (a0 + 1e-3).sin());
                assert_eq!(hit(&w, edge.0, edge.1), Some(Spot::Inner(i)));
            }
            assert_eq!(hit(&w, 200.0, 200.0 - (R_INNER - 1.0) * scale), None);
            let past = (R_OUTER + REACH + 1.0) * scale;
            assert_eq!(hit(&w, 200.0, 200.0 - past), None);
        }
    }

    #[test]
    fn hits_follow_the_wheel_scale() {
        let w = Wheel {
            scale: 0.5,
            ..wheel(4)
        };
        let r = (R_INNER + R_OUTER) / 2.0 * 0.5;
        assert_eq!(hit(&w, 200.0 + r, 200.0), Some(Spot::Inner(1)));
        let reach = (R_OUTER + REACH) * 0.5;
        assert_eq!(hit(&w, 200.0 + reach - 1.0, 200.0), Some(Spot::Inner(1)));
        assert_eq!(hit(&w, 200.0 + reach + 1.0, 200.0), None);
        assert_eq!(hit(&w, 200.0 + R_INNER * 0.5 - 1.0, 200.0), None);
    }

    #[test]
    fn picking_a_format_converts_at_once_on_either_wheel() {
        // eight formats, More, and three more in its list.
        let mut w = wheel(8);
        w.flyout = grouped((8..11).map(|i| item(i, Kind::Image)).collect());
        let drag = Wheel {
            origin: Origin::Drag,
            ..w.clone()
        };
        assert_eq!(Origin::default(), Origin::Window);
        for i in 0..8 {
            let (x, y) = label_point(i, w.segments());
            let spot = hit(&w, w.center.0 + x, w.center.1 + y).unwrap();
            assert_eq!(spot, Spot::Inner(i));
            let run = Some(Picked::Run(Action::Convert(format!("f{i}"))));
            assert_eq!(picked(&w, spot), run);
            assert_eq!(picked(&drag, spot), run);
        }
        // More opens its list; a format in it converts too.
        assert_eq!(picked(&w, Spot::Inner(8)), Some(Picked::More));
        for j in 0..3 {
            assert_eq!(
                picked(&w, Spot::Flyout(j)),
                Some(Picked::Run(Action::Convert(format!("f{}", j + 8))))
            );
        }
        // Nothing there, nothing picked; "Options…" is for the last format.
        assert_eq!(picked(&w, Spot::Flyout(9)), None);
        assert_eq!(picked(&wheel(2), Spot::Inner(5)), None);
        assert_eq!(picked(&w, Spot::Options), None);
        // Tab or Options… on a format: its options.
        assert_eq!(
            picked_options(&w, Some(Spot::Inner(2))),
            Some(Picked::Options("f2".into()))
        );
        assert_eq!(
            picked_options(&w, Some(Spot::Flyout(1))),
            Some(Picked::Options("f9".into()))
        );
        assert_eq!(picked_options(&w, Some(Spot::Inner(8))), None);
        assert_eq!(picked_options(&w, None), None);
        // Tools run as they are picked.
        let tool = Item {
            label: "Cut in half".into(),
            is_code: false,
            action: Action::Tool(Tool::CutInHalf, Kind::Audio),
            kind: None,
        };
        assert_eq!(
            pick_item(&tool),
            Picked::Run(Action::Tool(Tool::CutInHalf, Kind::Audio))
        );
        let tools = Wheel {
            kind: WheelKind::Tools,
            items: vec![tool],
            ..wheel(0)
        };
        assert_eq!(picked_options(&tools, Some(Spot::Inner(0))), None);
    }

    fn smaller_item() -> Item {
        Item {
            label: "Make smaller".into(),
            is_code: false,
            action: Action::Tool(Tool::Smaller, Kind::Image),
            kind: None,
        }
    }

    #[test]
    fn the_hub_says_what_is_lit() {
        let mut w = with_more();
        w.source_code = "JPG".into();
        w.items[0] = smaller_item();
        w.items[1].label = "PNG".into();
        w.items[1].action = Action::Convert("png".into());
        let est = |ext: &str| (ext == "png").then_some(1_100_000);
        let none = |_: &str| None;
        let say = |lit, files, e: &dyn Fn(&str) -> Option<u64>| {
            let h = w.hub_lines(lit, files, e);
            (h.pair, h.hint)
        };
        let pair = |a: &str, b: &str| (a.to_owned(), b.to_owned());
        // Nothing lit: the code, and what to do.
        assert_eq!(say(None, 1, &est), pair("JPG", "Pick a format"));
        assert_eq!(say(None, 3, &est), pair("JPG \u{b7} 3", "Pick a format"));
        // A format: where it goes, and its estimate or how many.
        assert_eq!(
            say(Some(Spot::Inner(1)), 1, &est),
            pair("JPG \u{2192} PNG", "about 1.1 MB")
        );
        assert_eq!(
            say(Some(Spot::Inner(1)), 1, &none),
            pair("JPG \u{2192} PNG", "")
        );
        assert_eq!(
            say(Some(Spot::Inner(1)), 3, &est),
            pair("JPG \u{2192} PNG", "3 files")
        );
        // Make smaller keeps the format.
        assert_eq!(
            say(Some(Spot::Inner(0)), 1, &est),
            pair("Make smaller", "Keeps JPG")
        );
        // More.
        assert_eq!(
            say(Some(Spot::Inner(8)), 1, &est),
            pair("More", "Formats, tools")
        );
        // A format in More's list reads as one on the ring.
        let f = w.item(Spot::Flyout(0)).unwrap().label.clone();
        assert_eq!(
            say(Some(Spot::Flyout(0)), 1, &none).0,
            format!("JPG \u{2192} {f}")
        );
        // Nothing there reads as nothing lit.
        assert_eq!(
            say(Some(Spot::Options), 1, &est),
            pair("JPG", "Pick a format")
        );
        // The tools wheel asks for a tool.
        let tools = Wheel {
            kind: WheelKind::Tools,
            ..wheel(2)
        };
        assert_eq!(tools.hub_lines(None, 1, &none).hint, "Pick a tool");
    }

    fn rows_of(w: &Wheel) -> Vec<FlyoutRow<'_>> {
        w.flyout_rows(None)
    }

    #[test]
    fn mores_list_goes_beside_the_ring_where_there_is_room() {
        let bounds = Rect::new(0.0, 48.0, 1100.0, 672.0);
        let place_at = |center: (f32, f32), bounds: Rect| {
            let w = Wheel {
                center,
                ..with_more()
            };
            let rows = rows_of(&w);
            let p = flyout_place(&w, &rows, bounds, 0.0);
            let heights: Vec<f32> = rows.iter().map(flyout_row_height).collect();
            (p, heights)
        };
        let (p, heights) = place_at((330.0, 360.0), bounds);
        // On the right of the ring, centred on it up and down.
        assert_eq!(p.rect.x, 330.0 + R_OUTER + FLYOUT_GAP);
        assert_eq!(p.rect.w, FLYOUT_W);
        assert!((p.rect.y + p.rect.h / 2.0 - 360.0).abs() < 0.01 || p.rect.y == 48.0 + EDGE_GAP);
        // Its lines stack inside its padding, each as tall as it is.
        assert_eq!(p.rows.len(), heights.len());
        assert_eq!(p.rows[0].y, p.rect.y + FLYOUT_PAD);
        for (r, h) in p.rows.iter().zip(&heights) {
            assert_eq!(r.h, *h);
            assert_eq!(r.x, p.rect.x + FLYOUT_PAD);
        }
        // The footer row is not among the lines that scroll.
        let footer = p.footer.expect("a footer row");
        assert_eq!(*p.rows.last().unwrap(), footer);
        assert_eq!(
            p.content,
            p.rows[..p.rows.len() - 1].iter().map(|r| r.h).sum::<f32>()
        );
        // Near the right edge it goes on the left.
        let (p, _) = place_at((900.0, 360.0), bounds);
        assert_eq!(p.rect.right(), 900.0 - R_OUTER - FLYOUT_GAP);
        // Too narrow for either side: inside the bounds.
        let (p, _) = place_at((240.0, 240.0), Rect::new(0.0, 0.0, 480.0, 480.0));
        assert!(p.rect.x >= EDGE_GAP && p.rect.right() <= 480.0 - EDGE_GAP);
        // Near the top it stays below the bounds' top.
        let (p, _) = place_at((330.0, 60.0), bounds);
        assert_eq!(p.rect.y, 48.0 + EDGE_GAP);
    }

    #[test]
    fn the_footer_row_is_always_there_so_the_list_stays_put_as_a_format_is_lit() {
        let w = Wheel {
            center: (330.0, 360.0),
            ..with_outer(3)
        };
        let bounds = Rect::new(0.0, 48.0, 1100.0, 672.0);
        let without = w.flyout_rows(None);
        let with = w.flyout_rows(Some(Spot::Inner(1)));
        // Lit, it is for that format; nothing lit, for the first with settings.
        assert_eq!(with.last(), Some(&FlyoutRow::Options("f1")));
        assert_eq!(without.last(), Some(&FlyoutRow::Options("f0")));
        let (a, b) = (
            flyout_place(&w, &without, bounds, 0.0),
            flyout_place(&w, &with, bounds, 0.0),
        );
        assert_eq!(a, b);
    }

    #[test]
    fn a_list_taller_than_the_window_scrolls() {
        let mut w = with_more();
        w.center = (300.0, 200.0);
        let rows = rows_of(&w);
        let short = Rect::new(0.0, 0.0, 900.0, 300.0);
        let p = flyout_place(&w, &rows, short, 0.0);
        assert!(p.rect.h <= 300.0 - 2.0 * EDGE_GAP);
        assert!(p.max_scroll() > 0.0);
        // Scrolled, the lines move up; past the end it stops.
        let far = flyout_place(&w, &rows, short, 1e6);
        assert_eq!(far.scroll, p.max_scroll());
        assert!(far.rows[0].y < p.rows[0].y);
        // Keeping the last scrolling line in view scrolls to the end; the
        // footer row never scrolls.
        let last = rows.len() - 2;
        assert_eq!(p.scroll_to(last), p.max_scroll());
        assert_eq!(p.scroll_to(rows.len() - 1), p.scroll);
        assert_eq!(far.scroll_to(0), 0.0);
        // A line out of view is not hit.
        let r = p.rows[last];
        assert_eq!(flyout_hit(&p, &rows, r.x + 10.0, r.y + 10.0), None);
        let r = far.rows[last];
        assert_eq!(
            flyout_hit(&far, &rows, r.x + 10.0, r.y + 10.0),
            row_spot(&rows[last])
        );
    }

    #[test]
    fn a_taller_list_shows_whole_lines_at_rest_and_never_ends_on_a_heading() {
        let mut w = with_more();
        w.center = (300.0, 200.0);
        let rows = rows_of(&w);
        for room in [300.0, 333.0, 420.0, 555.0] {
            let p = flyout_place(&w, &rows, Rect::new(0.0, 0.0, 900.0, room), 0.0);
            assert!(p.rect.h <= room - 2.0 * EDGE_GAP);
            assert!(p.max_scroll() > 0.0);
            // The body ends exactly where a line ends, and not on a heading.
            let mut edge = 0.0;
            let mut ends = vec![];
            for r in rows.iter().filter(|r| !matches!(r, FlyoutRow::Options(_))) {
                edge += flyout_row_height(r);
                ends.push((edge, matches!(r, FlyoutRow::Heading(_))));
            }
            let found = ends.iter().find(|(e, _)| (e - p.body.h).abs() < 0.01);
            assert!(
                matches!(found, Some((_, false))),
                "{room}: body {} not at a line's end",
                p.body.h
            );
            // The footer row sits under the body, inside the frame.
            let footer = p.footer.unwrap();
            assert_eq!(footer.y, p.body.bottom());
            assert_eq!(footer.bottom() + FLYOUT_PAD, p.rect.bottom());
            assert_eq!(footer.h, FLYOUT_FOOTER);
        }
    }

    #[test]
    fn a_list_that_fits_is_as_tall_as_its_lines_and_does_not_scroll() {
        let w = Wheel {
            center: (400.0, 400.0),
            ..with_outer(3)
        };
        let rows = rows_of(&w);
        let p = flyout_place(&w, &rows, Rect::new(0.0, 0.0, 1100.0, 1000.0), 0.0);
        assert_eq!(p.max_scroll(), 0.0);
        assert_eq!(p.body.h, p.content);
        assert_eq!(p.rect.h, p.content + 2.0 * FLYOUT_PAD + FLYOUT_FOOTER);
    }

    #[test]
    fn the_list_is_360_wide_and_the_drag_wheels_stays_300() {
        assert_eq!(FLYOUT_W, 360.0);
        let w = with_more();
        let rows = rows_of(&w);
        let bounds = Rect::new(0.0, 0.0, 1100.0, 720.0);
        assert_eq!(flyout_place(&w, &rows, bounds, 0.0).rect.w, 360.0);
        let drag = Wheel {
            origin: Origin::Drag,
            ..with_more()
        };
        let rows = drag.flyout_rows(None);
        assert_eq!(flyout_place(&drag, &rows, bounds, 0.0).rect.w, 300.0);
    }

    #[test]
    fn the_list_alone_lies_to_the_left_of_its_point_and_inside_its_bounds() {
        // Opened beside the side pane, it never goes over it.
        let w = Wheel {
            list_only: true,
            settable: vec![],
            items: vec![],
            center: (800.0, 300.0),
            ..with_more()
        };
        let rows = rows_of(&w);
        let list = Rect::new(0.0, 48.0, 800.0, 600.0);
        let p = flyout_place(&w, &rows, list, 0.0);
        assert_eq!(p.rect.right(), list.right() - EDGE_GAP);
        assert!(p.rect.y >= list.y + EDGE_GAP && p.rect.bottom() <= list.bottom() - EDGE_GAP);
        // Too close to the left edge it keeps inside.
        let near = Wheel {
            center: (200.0, 300.0),
            ..w.clone()
        };
        let p = flyout_place(&near, &rows, list, 0.0);
        assert_eq!(p.rect.x, list.x + EDGE_GAP);
    }

    #[test]
    fn the_list_alone_gathers_every_format_under_its_kinds_heading_and_the_tools_last() {
        let kiln = Kiln::new();
        let (items, flyout) = build(
            WheelKind::Convert,
            &kiln,
            &Availability::all(),
            &paths(&["a.mp4"]),
            &no_subtitles,
        );
        let ring_formats = items
            .iter()
            .filter(|i| matches!(i.action, Action::Convert(_)))
            .count();
        let groups = all_groups(items.clone(), flyout.clone(), Some(Kind::Video));
        let titles: Vec<&str> = groups.iter().map(|g| g.title).collect();
        assert_eq!(titles, ["Video", "Photos", "Sound", "Tools"]);
        // Every item once: the ring's, More's and the tools.
        let total: usize = groups.iter().map(|g| g.items.len()).sum();
        assert_eq!(
            total,
            items.len() + flyout.iter().map(|g| g.items.len()).sum::<usize>()
        );
        assert!(ring_formats > 0);
        // Make smaller, from the ring, is among the tools.
        let tools = groups.last().unwrap();
        assert!(tools
            .items
            .iter()
            .any(|i| matches!(i.action, Action::Tool(Tool::Smaller, _))));
        // Formats of a kind sit together.
        let video = &groups[0];
        assert!(video.items.iter().all(|i| i.kind == Some(Kind::Video)));
    }

    #[test]
    fn remove_location_data_has_the_map_pin_and_no_tool_shares_a_glyph_with_another() {
        assert_eq!(glyph_for_tool(Tool::RemoveLocation), '\u{E707}');
        let all = [
            Tool::Smaller,
            Tool::Resize,
            Tool::RemoveLocation,
            Tool::RemoveEdges,
            Tool::CutInHalf,
            Tool::SaveEachPage,
            Tool::Merge,
        ];
        let glyphs: std::collections::HashSet<char> =
            all.iter().map(|t| glyph_for_tool(*t)).collect();
        assert_eq!(glyphs.len(), all.len());
        // The list's formats use one description source.
        assert_eq!(format_words("png"), kiln_core::describe_format("png"));
    }

    #[test]
    fn the_list_is_hit_by_line_and_its_headings_are_nothing() {
        let mut w = with_more();
        w.center = (300.0, 360.0);
        let rows = w.flyout_rows(Some(Spot::Inner(2)));
        let p = flyout_place(&w, &rows, Rect::new(0.0, 0.0, 1100.0, 720.0), 0.0);
        for (row, r) in rows.iter().zip(&p.rows) {
            let got = flyout_hit(&p, &rows, r.x + r.w / 2.0, r.y + r.h / 2.0);
            match row {
                FlyoutRow::Heading(_) => assert_eq!(got, None),
                FlyoutRow::Item(spot, _) => assert_eq!(got, Some(*spot)),
                FlyoutRow::Options(_) => assert_eq!(got, Some(Spot::Options)),
            }
        }
        assert_eq!(flyout_hit(&p, &rows, p.rect.x - 1.0, p.rect.y + 10.0), None);
    }

    #[test]
    fn up_and_down_in_the_list_skip_headings_and_wrap() {
        let w = with_more();
        let rows = w.flyout_rows(Some(Spot::Inner(0)));
        // Down from nothing: the first item, past the first heading.
        assert_eq!(flyout_step(&rows, None, 1), Some(Spot::Flyout(0)));
        assert_eq!(flyout_step(&rows, None, -1), Some(Spot::Options));
        assert_eq!(
            flyout_step(&rows, Some(Spot::Flyout(0)), 1),
            Some(Spot::Flyout(1))
        );
        // From the last item of a group to the next group's first.
        let n = w.more_len();
        assert_eq!(
            flyout_step(&rows, Some(Spot::Flyout(n - 1)), 1),
            Some(Spot::Options)
        );
        assert_eq!(
            flyout_step(&rows, Some(Spot::Options), 1),
            Some(Spot::Flyout(0))
        );
        assert_eq!(flyout_step(&[], None, 1), None);
    }

    #[test]
    fn tools_have_glyphs_and_say_what_they_do() {
        for t in Tool::ALL {
            let g = glyph_for_tool(t);
            assert!(('\u{E000}'..='\u{F8FF}').contains(&g), "{t:?}");
            for k in [Kind::Image, Kind::Video, Kind::Audio, Kind::Document] {
                assert!(!tool_note(t, k).is_empty());
            }
        }
        assert_eq!(glyph_for_tool(Tool::Smaller), '\u{E73F}');
        assert_eq!(
            tool_note(Tool::RemoveLocation, Kind::Image),
            "Removes where it was taken and the camera details"
        );
        assert_eq!(
            tool_note(Tool::RemoveQuietEnds, Kind::Audio),
            "Cuts silence at the start and end"
        );
    }

    #[test]
    fn files_of_one_kind_are_offered_what_any_of_them_can_become() {
        // UX-06: a JPG and a PNG are offered JPG and PNG; the file already
        // in the format is left alone when it is picked.
        assert_eq!(
            ring_of(&["a.jpg", "b.png"]),
            ["Smaller", "JPG", "PNG", "WEBP", "PDF", "AVIF", "GIF", "TIFF"]
        );
        let video = ring_of(&["a.mp4", "b.mkv"]);
        assert_eq!(&video[..3], ["Smaller", "MP4", "MP3"]);
        assert!(video.contains(&"MKV".to_owned()));
        // Make smaller is for all of them: a HEIC among them leaves it out.
        let photos = ring_of(&["a.jpg", "b.heic"]);
        assert_eq!(photos[0], "JPG");
        assert!(!photos.contains(&"Smaller".to_owned()));
        // A format only one of them can become is offered too.
        let kiln = Kiln::new();
        let one = items_for(
            WheelKind::Convert,
            &kiln,
            &Availability::all(),
            &paths(&["a.mkv", "b.avi"]),
            &|p| p.extension().is_some_and(|e| e == "mkv"),
        );
        assert!(one.iter().any(|i| i.label == "SRT"));
        // Files of several kinds are offered only what every one can
        // become.
        let mixed = items_for(
            WheelKind::Convert,
            &kiln,
            &Availability::all(),
            &paths(&["a.mp4", "b.wav"]),
            &no_subtitles,
        );
        let labels: Vec<&str> = mixed.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"MP3") && !labels.contains(&"MP4"),
            "{labels:?}"
        );
        assert!(mixed.iter().all(|i| i.is_code), "no tools for two kinds");
    }

    #[test]
    fn files_picked_for_a_format_split_into_convert_already_and_cant() {
        let kiln = Kiln::new();
        let all = Availability::all();
        let split = |names: &[&str], ext: &str, available: &Availability| {
            split_for(&kiln, Some(available), &paths(names), ext, &|p: &Path| {
                p.extension().is_some_and(|e| e == "mkv")
            })
        };
        // A DOCX can't become WEBP; the PDF can.
        assert_eq!(
            split(&["a.docx", "b.pdf"], "webp", &all),
            Split {
                convert: paths(&["b.pdf"]),
                already: vec![],
                cant: paths(&["a.docx"]),
                needs: vec![],
            }
        );
        // Subtitles come only from a video that holds a subtitle track.
        assert_eq!(
            split(&["a.mkv", "b.avi"], "srt", &all),
            Split {
                convert: paths(&["a.mkv"]),
                already: vec![],
                cant: paths(&["b.avi"]),
                needs: vec![],
            }
        );
        // A file already in the format is left alone, not counted as one
        // that can't become it; `photo.jpeg` is already JPG.
        assert_eq!(
            split(&["a.jpeg", "b.png", "c.wav"], "jpg", &all),
            Split {
                convert: paths(&["b.png"]),
                already: paths(&["a.jpeg"]),
                cant: paths(&["c.wav"]),
                needs: vec![],
            }
        );
        // The AVI reaches MOV only through FFmpeg; without it, the MP4
        // still does on Kiln's own engine. The AVI waits on FFmpeg, and the
        // WAV can't become MOV with any helper.
        let no_ffmpeg = Availability::all().without(&["ffmpeg"]);
        assert_eq!(
            split(&["a.mp4", "b.avi", "c.wav"], "mov", &no_ffmpeg),
            Split {
                convert: paths(&["a.mp4"]),
                already: vec![],
                cant: paths(&["c.wav"]),
                needs: vec![(PathBuf::from("b.avi"), "ffmpeg".to_owned())],
            }
        );
        // A Markdown file waits on Pandoc for DOCX.
        let no_pandoc = Availability::all().without(&["pandoc"]);
        assert_eq!(
            split(&["a.md"], "docx", &no_pandoc).needs,
            [(PathBuf::from("a.md"), "pandoc".to_owned())]
        );
        // Subtitles a video doesn't hold can't come, whatever is missing.
        assert_eq!(
            split(&["b.avi"], "srt", &no_ffmpeg),
            Split {
                cant: paths(&["b.avi"]),
                ..Split::default()
            }
        );
        // Before the engines are known, nothing is ruled out here: core
        // says so per file, as before.
        assert_eq!(
            split_for(
                &kiln,
                None,
                &paths(&["a.docx", "b.pdf"]),
                "webp",
                &no_subtitles
            ),
            Split {
                convert: paths(&["a.docx", "b.pdf"]),
                already: vec![],
                cant: vec![],
                needs: vec![],
            }
        );
    }

    #[test]
    fn a_file_left_out_says_why_in_the_format_code() {
        assert_eq!(cant_line("webp"), "Can't become WEBP");
        assert_eq!(cant_line(".srt"), "Can't become SRT");
        // The drag-wheel's notice counts what it left out too.
        let split = Split {
            convert: paths(&["b.png"]),
            already: paths(&[r"C:\in\a.jpg"]),
            cant: paths(&[r"C:\in\c.docx"]),
            needs: vec![],
        };
        assert_eq!(
            left_out_lines(&split, "jpg"),
            ["a.jpg is already JPG.", "c.docx can't become JPG."]
        );
        let several = Split {
            convert: vec![],
            already: paths(&["a.jpg", "b.jpg"]),
            cant: paths(&["c.docx", "d.xlsx", "e.pptx"]),
            needs: vec![],
        };
        assert_eq!(
            left_out_lines(&several, "jpg"),
            ["2 files are already JPG.", "3 files can't become JPG."]
        );
        // A file blocked only by a missing helper names it.
        assert_eq!(needs_line("mov", "ffmpeg"), "Needs FFmpeg to become MOV");
        assert_eq!(needs_line(".docx", "pandoc"), "Needs Pandoc to become DOCX");
        let waiting = Split {
            needs: vec![(PathBuf::from(r"C:\in\b.avi"), "ffmpeg".to_owned())],
            ..Split::default()
        };
        assert_eq!(
            left_out_lines(&waiting, "mov"),
            ["b.avi needs FFmpeg to become MOV."]
        );
        let waiting = Split {
            cant: paths(&["c.wav"]),
            needs: vec![
                (PathBuf::from("a.avi"), "ffmpeg".to_owned()),
                (PathBuf::from("b.mkv"), "ffmpeg".to_owned()),
                (PathBuf::from("d.md"), "pandoc".to_owned()),
            ],
            ..Split::default()
        };
        assert_eq!(
            left_out_lines(&waiting, "mov"),
            [
                "c.wav can't become MOV.",
                "2 files need FFmpeg to become MOV.",
                "d.md needs Pandoc to become MOV.",
            ]
        );
        assert!(left_out_lines(&Split::default(), "jpg").is_empty());
    }

    #[test]
    fn convert_offers_formats_every_file_shares() {
        let kiln = Kiln::new();
        let items = items_for(
            WheelKind::Convert,
            &kiln,
            &Availability::all(),
            &["a.png".into()],
            &no_subtitles,
        );
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        // Make smaller, then a picture's ring (spec 3.7), then More.
        assert_eq!(&labels[..5], ["Make smaller", "JPG", "WEBP", "PDF", "AVIF"]);
        assert_eq!(items.iter().filter(|i| i.is_code).count(), 12);

        let both = items_for(
            WheelKind::Convert,
            &kiln,
            &Availability::all(),
            &["a.mp4".into(), "b.mkv".into()],
            &no_subtitles,
        );
        let labels: Vec<&str> = both.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"MP3") && labels.contains(&"WEBM"));
        // One kind: each is offered the other's format (UX-06).
        assert!(labels.contains(&"MP4") && labels.contains(&"MKV"));

        let none = items_for(
            WheelKind::Convert,
            &kiln,
            &Availability::all(),
            &["a.png".into(), "b.mp3".into()],
            &no_subtitles,
        );
        assert!(none.is_empty());
    }

    #[test]
    fn the_offer_says_why_a_wheel_would_be_empty() {
        let kiln = Kiln::new();
        let p = |names: &[&str]| -> Vec<PathBuf> { names.iter().map(PathBuf::from).collect() };
        let offer =
            |names: &[&str]| offer(WheelKind::Convert, &kiln, &Availability::all(), &p(names));
        // One kind.
        assert_eq!(offer(&["a.png", "b.png"]), Offer::Choices);
        // Several kinds with some formats in common.
        assert_eq!(offer(&["a.mp4", "b.mkv"]), Offer::Choices);
        // Nothing selected.
        assert_eq!(offer(&[]), Offer::Nothing);
        // Several kinds with none in common: the kinds present, most first.
        assert_eq!(
            offer(&["a.png", "b.mp3", "c.mp3"]),
            Offer::Mixed(vec![("MP3".into(), 2), ("PNG".into(), 1)])
        );
        // One kind Kiln has nothing for, known or not.
        assert_eq!(offer(&["a.xyz"]), Offer::Unsupported("XYZ".into()));
        assert_eq!(offer(&["noext"]), Offer::Unsupported("Other".into()));
    }

    #[test]
    fn five_hundred_mixed_files_get_a_sentence_not_a_wheel() {
        let kiln = Kiln::new();
        let exts = ["png", "mp3", "docx", "mp4", "pdf", "md"];
        let paths: Vec<PathBuf> = (0..500)
            .map(|i| PathBuf::from(format!("f{i}.{}", exts[i % exts.len()])))
            .collect();
        let o = offer(WheelKind::Convert, &kiln, &Availability::all(), &paths);
        let Offer::Mixed(kinds) = &o else {
            panic!("{o:?}");
        };
        assert_eq!(kinds.iter().map(|k| k.1).sum::<usize>(), 500);
        let s = offer_sentence(WheelKind::Convert, &o).unwrap();
        assert!(
            s.starts_with("These files have no format in common: "),
            "{s}"
        );
        assert!(s.ends_with("Select one kind at a time."), "{s}");
    }

    #[test]
    fn the_sentence_lists_four_kinds_and_counts_the_rest() {
        let kinds: Vec<(String, usize)> = ["A", "B", "C", "D", "E", "F"]
            .iter()
            .map(|c| (c.to_string(), 2))
            .collect();
        assert_eq!(
            offer_sentence(WheelKind::Convert, &Offer::Mixed(kinds)).unwrap(),
            "These files have no format in common: A (2), B (2), C (2), D (2), 2 more. Select one kind at a time."
        );
        assert_eq!(
            offer_sentence(WheelKind::Convert, &Offer::Unsupported("XYZ".into())).unwrap(),
            "Kiln can't convert XYZ files."
        );
        assert_eq!(
            offer_sentence(WheelKind::Convert, &Offer::Nothing).unwrap(),
            "Select a file first."
        );
        assert_eq!(offer_sentence(WheelKind::Convert, &Offer::Choices), None);
        // Waiting on a helper app: Get it, which the row's link offers in
        // a narrow window and the inspector's button in a wide one, where
        // the row has no link.
        assert_eq!(
            offer_sentence(
                WheelKind::Convert,
                &Offer::NeedsHelper("pandoc".into(), "MD".into())
            )
            .unwrap(),
            "MD files need Pandoc, which isn't installed. Use Get Pandoc to download it."
        );
        // FFmpeg comes with Kiln: nothing to get, and nowhere to send anyone.
        assert_eq!(
            offer_sentence(
                WheelKind::Convert,
                &Offer::NeedsHelper("ffmpeg".into(), "MP4".into())
            )
            .unwrap(),
            "MP4 files need FFmpeg, which isn't installed."
        );
        // Windows' Media Feature Pack isn't downloaded: the row's Install
        // it opens Settings.
        assert_eq!(
            offer_sentence(
                WheelKind::Convert,
                &Offer::NeedsHelper("audio-mf".into(), "WMA".into())
            )
            .unwrap(),
            "Needs Windows' Media Feature Pack, which isn't installed."
        );
    }

    #[test]
    fn tools_have_no_empty_wheel_either() {
        let kiln = Kiln::new();
        let paths: Vec<PathBuf> = vec!["a.png".into(), "b.mp3".into()];
        let o = offer(WheelKind::Tools, &kiln, &Availability::all(), &paths);
        if let Offer::Mixed(_) = o {
            assert!(offer_sentence(WheelKind::Tools, &o)
                .unwrap()
                .starts_with("No tool fits all of these files"));
        } else {
            // Some tool applies to both: then the wheel has it.
            assert_eq!(o, Offer::Choices);
        }
    }

    #[test]
    fn video_keeps_its_everyday_targets_on_the_inner_ring() {
        let kiln = Kiln::new();
        let (inner, flyout) = offering(
            WheelKind::Convert,
            &kiln,
            &Availability::all(),
            &["a.mp4".into()],
            &|_| true,
        );
        let labels: Vec<&str> = inner.iter().map(|i| i.label.as_str()).collect();
        // The mock's ring, less MP4 itself, with Make smaller first.
        assert_eq!(
            labels,
            ["Make smaller", "MP3", "GIF", "MOV", "WEBM", "MKV", "M4A"]
        );
        let more: Vec<&Item> = flyout.iter().flat_map(|g| &g.items).collect();
        assert!(more.iter().any(|i| i.label == "SRT"));
        assert!(more.iter().all(|i| !labels.contains(&i.label.as_str())));
    }

    #[test]
    fn tools_offer_what_applies() {
        let kiln = Kiln::new();
        let items = items_for(
            WheelKind::Tools,
            &kiln,
            &Availability::all(),
            &["a.mp3".into(), "b.mp3".into()],
            &no_subtitles,
        );
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(
            labels,
            ["Remove silence at the ends", "Cut in half", "Merge"]
        );
        assert!(items.iter().all(|i| !i.is_code));
        assert!(items
            .iter()
            .all(|i| i.action == Action::Tool(tool_of(i), Kind::Audio)));
        // Each kind names the tools for what they do to it.
        for (file, want) in [
            (
                "a.mp4",
                &[
                    "Make smaller",
                    "Remove black bars",
                    "Remove black at the ends",
                    "Cut in half",
                ][..],
            ),
            (
                "a.png",
                &[
                    "Make smaller",
                    "Resize",
                    "Remove location data",
                    "Remove borders",
                ][..],
            ),
            ("a.pdf", &["Make smaller", "Save each page as a PDF"][..]),
        ] {
            let items = items_for(
                WheelKind::Tools,
                &kiln,
                &Availability::all(),
                &[file.into()],
                &no_subtitles,
            );
            let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
            assert_eq!(labels, want, "{file}");
        }
        // Every tool has a glyph, offered yet or not.
        for tool in Tool::ALL {
            let g = glyph_for_tool(tool);
            assert!(('\u{E700}'..='\u{F8FF}').contains(&g), "{tool:?}");
        }
    }

    // The ring's order per kind (spec 3.7, the mock's rings) and More.

    /// Paths from names.
    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    /// What the ring shows for `names`, with every tool shown, as
    /// Task 19's wheel will: the codes, and "Smaller" for Make smaller.
    fn ring_of(names: &[&str]) -> Vec<String> {
        let kiln = Kiln::new();
        let (ring, _) = build(
            WheelKind::Convert,
            &kiln,
            &Availability::all(),
            &paths(names),
            &no_subtitles,
        );
        ring.iter()
            .map(|i| match &i.action {
                Action::Tool(Tool::Smaller, _) => "Smaller".to_owned(),
                _ => i.label.clone(),
            })
            .collect()
    }

    #[test]
    fn each_kind_has_its_own_ring_without_its_own_format() {
        // The mock's rings, less the source's own format; Make smaller
        // first where R2 allows it.
        assert_eq!(
            ring_of(&["a.jpg"]),
            ["Smaller", "PNG", "WEBP", "PDF", "AVIF", "GIF", "TIFF"]
        );
        assert_eq!(
            ring_of(&["a.png"]),
            ["Smaller", "JPG", "WEBP", "PDF", "AVIF", "GIF", "TIFF"]
        );
        assert_eq!(
            ring_of(&["a.mp4"]),
            ["Smaller", "MP3", "GIF", "MOV", "WEBM", "MKV", "M4A"]
        );
        assert_eq!(
            ring_of(&["a.mkv"]),
            ["Smaller", "MP4", "MP3", "GIF", "MOV", "WEBM", "M4A"]
        );
        assert_eq!(ring_of(&["a.mp3"]), ["M4A", "WAV", "FLAC", "OGG", "AAC"]);
        assert_eq!(ring_of(&["a.wav"]), ["MP3", "M4A", "FLAC", "OGG", "AAC"]);
        assert_eq!(
            ring_of(&["a.docx"]),
            ["PDF", "TXT", "HTML", "ODT", "RTF", "EPUB"]
        );
        // A PDF has a list of its own.
        assert_eq!(
            ring_of(&["a.pdf"]),
            ["Smaller", "DOCX", "PNG", "JPG", "TXT"]
        );
        // Kiln can't write HEIC, so JPG comes first instead.
        assert_eq!(
            ring_of(&["a.heic"]),
            ["JPG", "PNG", "WEBP", "PDF", "AVIF", "GIF", "TIFF"]
        );
    }

    #[test]
    fn make_smaller_is_first_only_for_the_formats_kiln_can_write() {
        for ext in ["mp4", "mov", "m4v", "mkv", "jpg", "png", "webp", "pdf"] {
            assert_eq!(ring_of(&[&format!("a.{ext}")])[0], "Smaller", "{ext}");
        }
        for ext in ["heic", "bmp", "svg", "dng", "avi", "wmv", "mp3", "docx"] {
            let ring = ring_of(&[&format!("a.{ext}")]);
            assert!(!ring.contains(&"Smaller".to_owned()), "{ext}: {ring:?}");
        }
        // It is the tool for the files' kind, and none of the formats.
        let kiln = Kiln::new();
        let (ring, _) = build(
            WheelKind::Convert,
            &kiln,
            &Availability::all(),
            &paths(&["a.mp4"]),
            &no_subtitles,
        );
        assert_eq!(ring[0].action, Action::Tool(Tool::Smaller, Kind::Video));
        assert_eq!(ring[0].label, "Make smaller");
        assert!(!ring[0].is_code && ring[0].kind.is_none());
    }

    #[test]
    fn the_app_shows_make_smaller_first_on_a_photos_ring() {
        // No gate any more: what Kiln offers is what the wheel shows.
        let kiln = Kiln::new();
        let (ring, flyout) = offering(
            WheelKind::Convert,
            &kiln,
            &Availability::all(),
            &paths(&["a.jpg"]),
            &no_subtitles,
        );
        assert_eq!(ring[0].action, Action::Tool(Tool::Smaller, Kind::Image));
        assert_eq!(ring[1].label, "PNG");
        let tools: Vec<Tool> = flyout
            .iter()
            .flat_map(|g| &g.items)
            .filter_map(|i| match i.action {
                Action::Tool(t, _) => Some(t),
                _ => None,
            })
            .collect();
        assert!(tools.contains(&Tool::Resize) && tools.contains(&Tool::RemoveLocation));
    }

    #[test]
    fn more_holds_the_other_formats_by_kind_then_the_tools() {
        let kiln = Kiln::new();
        let more = |names: &[&str]| {
            build(
                WheelKind::Convert,
                &kiln,
                &Availability::all(),
                &paths(names),
                &no_subtitles,
            )
            .1
        };
        let titles = |g: &[Group]| g.iter().map(|g| g.title).collect::<Vec<_>>();
        let labels = |g: &Group| g.items.iter().map(|i| i.label.clone()).collect::<Vec<_>>();

        // A video: its own kind first, then the others, then the tools.
        let video = more(&["a.mp4"]);
        assert_eq!(titles(&video), ["Video", "Photos", "Sound", "Tools"]);
        assert!(labels(&video[0]).starts_with(&["AVI".to_owned(), "WMV".to_owned()]));
        assert!(labels(&video[2]).starts_with(&["WAV".to_owned(), "FLAC".to_owned()]));
        // Make smaller is on the ring, so not among the tools.
        assert_eq!(
            labels(&video[3]),
            [
                "Remove black bars",
                "Remove black at the ends",
                "Cut in half"
            ]
        );
        // Nothing on the ring is in More too.
        let ring = ring_of(&["a.mp4"]);
        for g in &video {
            for l in labels(g) {
                assert!(!ring.contains(&l), "{l} is on the ring and in More");
            }
        }
        // A photo's More: other pictures, as the mock lists them.
        let photo = more(&["a.jpg"]);
        assert_eq!(titles(&photo), ["Photos", "Tools"]);
        assert!(labels(&photo[0]).starts_with(&["BMP".to_owned(), "ICO".to_owned()]));
        assert_eq!(
            labels(&photo[1]),
            ["Resize", "Remove location data", "Remove borders"]
        );
        // A document's: other documents, then pictures of its pages.
        let doc = more(&["a.docx"]);
        assert_eq!(titles(&doc)[0], "Documents");
        // Sound is called Sound, as the mock has it.
        assert_eq!(titles(&more(&["a.mp3"])), ["Sound", "Tools"]);
    }

    #[test]
    fn merge_is_in_more_only_for_two_files_or_more() {
        let kiln = Kiln::new();
        let tools = |names: &[&str]| {
            let (_, flyout) = build(
                WheelKind::Convert,
                &kiln,
                &Availability::all(),
                &paths(names),
                &no_subtitles,
            );
            flyout
                .into_iter()
                .find(|g| g.title == TOOLS_GROUP)
                .map(|g| g.items.into_iter().map(|i| i.label).collect::<Vec<_>>())
                .unwrap_or_default()
        };
        assert!(!tools(&["a.mp3"]).contains(&"Merge".to_owned()));
        assert_eq!(tools(&["a.mp3", "b.mp3"]).last().unwrap(), "Merge");
        assert!(!tools(&["a.pdf"]).contains(&"Merge".to_owned()));
        assert!(tools(&["a.pdf", "b.pdf"]).contains(&"Merge".to_owned()));
    }

    #[test]
    fn subtitles_are_offered_only_for_a_file_with_a_subtitle_track() {
        let kiln = Kiln::new();
        let codes = |subs: &dyn Fn(&Path) -> bool| {
            items_for(
                WheelKind::Convert,
                &kiln,
                &Availability::all(),
                &paths(&["a.mkv"]),
                subs,
            )
            .into_iter()
            .map(|i| i.label)
            .collect::<Vec<_>>()
        };
        let without = codes(&no_subtitles);
        assert!(!without
            .iter()
            .any(|c| SUBTITLES.contains(&c.to_ascii_lowercase().as_str())));
        let with = codes(&|_| true);
        for c in ["SRT", "VTT", "ASS"] {
            assert!(with.contains(&c.to_owned()), "{c}");
        }
        // A subtitle file is offered the other subtitle formats anyway.
        let srt = items_for(
            WheelKind::Convert,
            &kiln,
            &Availability::all(),
            &paths(&["a.srt"]),
            &no_subtitles,
        );
        assert!(srt.iter().any(|i| i.label == "VTT"));
    }

    #[test]
    fn the_subtitle_list_is_the_registrys() {
        let reg = kiln_core::Registry::builtin();
        let mut subs: Vec<&str> = reg
            .extensions()
            // Each format by its own extension, as targets are named.
            .filter(|e| {
                reg.format(e)
                    .is_some_and(|f| f.kind == Kind::Subtitle && f.ext == *e)
            })
            .collect();
        subs.sort_unstable();
        let mut ours = SUBTITLES.to_vec();
        ours.sort_unstable();
        assert_eq!(subs, ours);
    }

    #[test]
    fn mores_list_is_its_groups_then_options_for_a_lit_format() {
        let kiln = Kiln::new();
        let (items, flyout) = build(
            WheelKind::Convert,
            &kiln,
            &Availability::all(),
            &paths(&["a.mp4"]),
            &no_subtitles,
        );
        let w = Wheel {
            items,
            flyout,
            settable: vec!["mp3".into(), "avi".into()],
            ..wheel(0)
        };
        // Nothing lit, or Make smaller or More lit: the footer row is for
        // the first format with settings, and can be used.
        for lit in [None, Some(Spot::Inner(0)), Some(Spot::Inner(w.items.len()))] {
            let rows = w.flyout_rows(lit);
            assert_eq!(rows.last(), Some(&FlyoutRow::Options("mp3")));
            assert_eq!(row_spot(rows.last().unwrap()), Some(Spot::Options));
        }
        let rows = w.flyout_rows(None);
        assert_eq!(rows[0], FlyoutRow::Heading("Video"));
        // Every item once, numbered as Spot::Flyout counts.
        let spots: Vec<Spot> = rows
            .iter()
            .filter_map(|r| match r {
                FlyoutRow::Item(s, item) => {
                    assert_eq!(w.item(*s), Some(*item));
                    Some(*s)
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            spots,
            (0..w.more_len()).map(Spot::Flyout).collect::<Vec<_>>()
        );
        let headings: Vec<&str> = rows
            .iter()
            .filter_map(|r| match r {
                FlyoutRow::Heading(h) => Some(*h),
                _ => None,
            })
            .collect();
        assert_eq!(headings, ["Video", "Photos", "Sound", "Tools"]);
        // A lit format with settings: Options for it, last.
        let rows = w.flyout_rows(Some(Spot::Inner(1)));
        assert_eq!(rows.last(), Some(&FlyoutRow::Options("mp3")));
        let rows = w.flyout_rows(Some(Spot::Flyout(0)));
        assert_eq!(rows.last(), Some(&FlyoutRow::Options("avi")));
        // A lit format with nothing to set: the first that has settings.
        let bare = (0..w.more_len())
            .map(Spot::Flyout)
            .find(|s| {
                w.format_at(*s)
                    .is_some_and(|e| !w.settable.iter().any(|x| x == e))
            })
            .expect("a format with nothing to set");
        let rows = w.flyout_rows(Some(bare));
        assert_eq!(rows.last(), Some(&FlyoutRow::Options("mp3")));
        // A lit tool in More: the same.
        let tool = w.more_len() - 1;
        let rows = w.flyout_rows(Some(Spot::Flyout(tool)));
        assert_eq!(rows.last(), Some(&FlyoutRow::Options("mp3")));
        // It names its format, the code in capitals.
        assert_eq!(options_line("mp3"), "Options for MP3\u{2026}");
        assert_eq!(options_line("jpg"), "Options for JPG\u{2026}");
        // No format has settings: no footer row.
        let none = Wheel {
            settable: vec![],
            ..w.clone()
        };
        assert!(!none
            .flyout_rows(Some(Spot::Flyout(0)))
            .iter()
            .any(|r| matches!(r, FlyoutRow::Options(_))));
        // The drag-wheel's files take no options: no footer there.
        let drag = Wheel {
            origin: Origin::Drag,
            ..w.clone()
        };
        let rows = drag.flyout_rows(Some(Spot::Flyout(0)));
        assert!(!rows.iter().any(|r| matches!(r, FlyoutRow::Options(_))));
    }

    #[test]
    fn the_tools_wheel_has_every_tool_on_its_ring_and_no_more() {
        let kiln = Kiln::new();
        let (ring, flyout) = build(
            WheelKind::Tools,
            &kiln,
            &Availability::all(),
            &paths(&["a.png", "b.png"]),
            &no_subtitles,
        );
        let labels: Vec<&str> = ring.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Make smaller",
                "Resize",
                "Remove location data",
                "Remove borders",
                "Merge"
            ]
        );
        assert!(flyout.is_empty());
    }

    #[test]
    fn the_inspector_and_the_wheel_order_formats_alike() {
        // A video's formats, alphabetical as the registry lists them.
        let mut exts: Vec<String> = ["avi", "gif", "m4a", "mkv", "mov", "mp3", "wav", "webm"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        Order::Kind(Kind::Video).sort(&mut exts);
        // Off the ring, the most popular first.
        assert_eq!(
            exts,
            ["mp3", "gif", "mov", "webm", "mkv", "m4a", "wav", "avi"]
        );
        let mut pdf: Vec<String> = ["txt", "jpg", "png", "docx", "html"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        Order::Pdf.sort(&mut pdf);
        assert_eq!(pdf, ["docx", "png", "jpg", "txt", "html"]);
        let kiln = Kiln::new();
        assert_eq!(Order::of(&kiln, &paths(&["a.pdf", "b.pdf"])), Order::Pdf);
        assert_eq!(
            Order::of(&kiln, &paths(&["a.pdf", "b.docx"])),
            Order::Kind(Kind::Document)
        );
        assert_eq!(Order::of(&kiln, &paths(&["a.png", "b.mp3"])), Order::Any);
    }

    fn tool_of(item: &Item) -> Tool {
        match item.action {
            Action::Tool(t, _) => t,
            _ => panic!("{item:?}"),
        }
    }

    /// The wheel the app opens for `path`, a file with a subtitle track.
    fn real(kind: WheelKind, path: &str) -> Wheel {
        let kiln = Kiln::new();
        let (items, flyout) =
            offering(kind, &kiln, &Availability::all(), &[path.into()], &|_| true);
        Wheel {
            kind,
            origin: Origin::Window,
            items,
            flyout,
            ..wheel(0)
        }
    }

    #[test]
    fn the_dot_goes_on_an_inner_target() {
        let w = real(WheelKind::Convert, "a.png");
        let m = marks(&w, &Suggested::Convert("jpg".into()));
        let Some(&Spot::Inner(i)) = m.first() else {
            panic!("{m:?}");
        };
        assert_eq!(m.len(), 1);
        assert_eq!(w.items[i].action, Action::Convert("jpg".into()));
    }

    #[test]
    fn a_target_in_more_marks_more_and_its_slot() {
        let w = real(WheelKind::Convert, "a.mp4");
        let j = w
            .more_items()
            .position(|i| i.label == "SRT")
            .expect("SRT is in More");
        let m = marks(&w, &Suggested::Convert("srt".into()));
        assert_eq!(m, [Spot::Inner(w.items.len()), Spot::Flyout(j)]);
    }

    #[test]
    fn a_tool_marks_its_segment() {
        let w = real(WheelKind::Tools, "a.mp4");
        let m = marks(&w, &Suggested::Tool(Tool::RemoveEdges));
        assert_eq!(m.len(), 1);
        let Spot::Inner(i) = m[0] else {
            panic!("{m:?}");
        };
        assert_eq!(
            w.items[i].action,
            Action::Tool(Tool::RemoveEdges, Kind::Video)
        );
        assert_eq!(w.items[i].label, "Remove black bars");
    }

    #[test]
    fn a_suggestion_not_on_the_wheel_marks_nothing() {
        let w = real(WheelKind::Convert, "a.png");
        let s = Suggested::Convert("mp3".into());
        assert_eq!(marks(&w, &s), []);
        assert_eq!(start_spot(&w, Some(&s)), Spot::Inner(0));
        assert_eq!(start_spot(&w, None), Spot::Inner(0));
        // A tool is marked only where it is: in More's Tools.
        let borders = Suggested::Tool(Tool::RemoveEdges);
        let j = w.more_items().position(|i| i.label == "Remove borders");
        let j = j.expect("Remove borders is in More");
        assert_eq!(
            marks(&w, &borders),
            [Spot::Inner(w.items.len()), Spot::Flyout(j)]
        );
    }

    #[test]
    fn the_keyboard_starts_on_the_suggestion() {
        let w = real(WheelKind::Convert, "a.png");
        let s = Suggested::Convert("webp".into());
        assert_eq!(start_spot(&w, Some(&s)), marks(&w, &s)[0]);
        let w = real(WheelKind::Convert, "a.mp4");
        let s = Suggested::Convert("srt".into());
        assert_eq!(start_spot(&w, Some(&s)), Spot::Inner(w.items.len()));
    }

    #[test]
    fn a_target_is_named_with_what_it_is() {
        assert_eq!(target_label("webp"), "WEBP: Small image for the web");
        assert_eq!(target_label(".QOI"), "QOI: Fast, lossless image");
        assert_eq!(target_label("wv"), "WV: WavPack, lossless audio");
        assert_eq!(target_label("dbk"), "DBK: DocBook document");
        // Nothing to say about a format Kiln doesn't know: just its code.
        assert_eq!(target_label("xyz"), "XYZ");
    }

    #[test]
    fn a_text_only_target_says_so() {
        // The spec's label: the code, then what the conversion keeps.
        assert_eq!(target_label_for("docx", true), "DOCX (text only)");
        assert_eq!(target_label_for(".MD", true), "MD (text only)");
        assert_eq!(target_label_for("azw3", true), "AZW3 (text only)");
        // Any other target is named as it was.
        assert_eq!(target_label_for("docx", false), target_label("docx"));
        assert_eq!(
            target_label_for("webp", false),
            "WEBP: Small image for the web"
        );
        assert_eq!(target_label_for("xyz", false), "XYZ");
    }

    /// No target is text only.
    fn plain(_: &str) -> bool {
        false
    }

    /// No estimate for any format.
    fn no_estimate(_: &str) -> Option<u64> {
        None
    }

    #[test]
    fn the_footer_is_one_sentence_for_a_lit_convert_target() {
        let w = real(WheelKind::Convert, "a.png");
        let jpg = w.items.iter().find(|i| i.label == "JPG").unwrap();
        // The code, what it is and, for one file, about how big.
        let at_85_kb = |ext: &str| (ext == "jpg").then_some(85_000);
        assert_eq!(
            lit_footer(Some(jpg), &plain, &at_85_kb).as_deref(),
            Some("JPG. Photo that opens anywhere, about 85 KB.")
        );
        // No estimate: the same sentence without the size.
        assert_eq!(
            lit_footer(Some(jpg), &plain, &no_estimate).as_deref(),
            Some("JPG. Photo that opens anywhere.")
        );
        // Nothing lit, so the old hint stays.
        assert_eq!(lit_footer(None, &plain, &no_estimate), None);
        // A tool has no format to name.
        let tools = real(WheelKind::Tools, "a.mp4");
        assert_eq!(lit_footer(tools.items.first(), &plain, &no_estimate), None);
        // Every format on every wheel has something to say; the tools in
        // More, as on the tools wheel, have none. No "Suggested:" anywhere.
        for item in w.items.iter().chain(w.more_items()) {
            let Some(said) = lit_footer(Some(item), &plain, &no_estimate) else {
                assert!(!item.is_code, "{}", item.label);
                continue;
            };
            assert!(said.starts_with(&format!("{}. ", item.label)), "{said}");
            assert!(said.ends_with('.') && !said.contains("Suggested"), "{said}");
        }
    }

    #[test]
    fn the_footer_says_a_text_only_target_is() {
        // A PDF's wheel, with the rule the registry gives: the targets a
        // PDF becomes by way of its text read "(text only)", as the
        // sheet's target line does, and the others as the footer has them.
        let kiln = Kiln::new();
        let reg = kiln.registry();
        let pdf = reg.format_of(Path::new("a.pdf")).unwrap();
        let text_only = |ext: &str| reg.format(ext).is_some_and(|to| reg.text_only(pdf, to));
        let w = real(WheelKind::Convert, "a.pdf");
        let docx = w
            .items
            .iter()
            .chain(w.more_items())
            .find(|i| i.label == "DOCX");
        let docx = docx.expect("a PDF is offered DOCX");
        assert_eq!(
            lit_footer(Some(docx), &text_only, &no_estimate).as_deref(),
            Some("DOCX (text only)")
        );
        let mut said_text_only = 0;
        // Every format; More's tools have no format to name.
        for item in w.items.iter().chain(w.more_items()).filter(|i| i.is_code) {
            let said = lit_footer(Some(item), &text_only, &no_estimate).unwrap();
            let Action::Convert(ext) = &item.action else {
                panic!("{}", item.label);
            };
            if text_only(ext) {
                said_text_only += 1;
                assert_eq!(said, format!("{} (text only)", item.label));
            } else {
                assert!(said.starts_with(&format!("{}. ", item.label)), "{said}");
            }
        }
        assert!(
            said_text_only >= 4,
            "only {said_text_only} text-only targets"
        );
    }

    #[test]
    fn make_smaller_is_named_in_full_on_two_lines() {
        // the wheel says what the pane and More say.
        assert_eq!(MAKE_SMALLER_CAPTION, "Make\nsmaller");
        assert_eq!(
            MAKE_SMALLER_CAPTION.replace('\n', " "),
            crate::inspector::SMALLER
        );
    }

    #[test]
    fn source_codes() {
        assert_eq!(source_code(&["a.png".into(), "b.PNG".into()]), "PNG");
        assert_eq!(source_code(&["a.png".into(), "b.jpg".into()]), "MIXED");
        assert_eq!(source_code(&[]), "");
    }

    // The motion both wheels share.

    use std::time::Duration;

    fn with_outer(n: usize) -> Wheel {
        let mut w = wheel(n);
        w.flyout = grouped(vec![item(90, Kind::Image), item(91, Kind::Image)]);
        w.settable.extend(["f90".to_owned(), "f91".to_owned()]);
        w
    }

    fn after(t: Instant, ms: u64) -> Instant {
        t + Duration::from_millis(ms)
    }

    #[test]
    fn under_a_drag_pointing_at_more_opens_its_list_and_another_segment_shuts_it() {
        let (t, w) = (Instant::now(), with_outer(3));
        let mut m = WheelMotion::new(t);
        m.hover_opens = true;
        assert!(!m.flyout_open());
        assert!(m.point(Some(&w), Some(Spot::Inner(3)), t));
        assert_eq!(m.hover, Some(Spot::Inner(3)));
        assert!(m.flyout_open());
        // Into the list, it stays open.
        assert!(m.point(Some(&w), Some(Spot::Flyout(1)), t));
        assert!(m.flyout_open());
        // Off everything, too: the pointer may be on its way to it.
        assert!(m.point(Some(&w), None, t));
        assert!(m.flyout_open());
        // Another segment of the ring shuts it.
        assert!(m.point(Some(&w), Some(Spot::Inner(1)), t));
        assert!(!m.flyout_open());
    }

    #[test]
    fn over_the_window_pointing_at_more_only_lights_it() {
        let (t, w) = (Instant::now(), with_outer(3));
        let mut m = WheelMotion::new(t);
        assert!(m.point(Some(&w), Some(Spot::Inner(3)), t));
        assert!(!m.flyout_open(), "a click opens it");
        m.open_flyout(t);
        assert!(m.point(Some(&w), Some(Spot::Inner(0)), t));
        assert!(m.flyout_open(), "it stays until it is closed");
        m.close_flyout();
        assert!(!m.flyout_open());
    }

    #[test]
    fn the_same_spot_again_changes_nothing() {
        let (t, w) = (Instant::now(), with_outer(3));
        let mut m = WheelMotion::new(t);
        assert!(m.point(Some(&w), Some(Spot::Inner(1)), t));
        assert!(!m.point(Some(&w), Some(Spot::Inner(1)), t));
    }

    #[test]
    fn the_last_format_lit_is_kept_for_options() {
        let (t, mut w) = (Instant::now(), with_outer(3));
        w.items[0] = Item {
            label: "Cut in half".into(),
            is_code: false,
            action: Action::Tool(Tool::CutInHalf, Kind::Image),
            kind: None,
        };
        let mut m = WheelMotion::new(t);
        m.point(Some(&w), Some(Spot::Inner(1)), t);
        assert_eq!(m.format, Some(Spot::Inner(1)));
        // A tool, More and nothing leave it.
        for spot in [
            Some(Spot::Inner(0)),
            Some(Spot::Inner(3)),
            None,
            Some(Spot::Options),
        ] {
            m.point(Some(&w), spot, t);
            assert_eq!(m.format, Some(Spot::Inner(1)));
        }
        m.point(Some(&w), Some(Spot::Flyout(1)), t);
        assert_eq!(m.format, Some(Spot::Flyout(1)));
    }

    #[test]
    fn it_opens_with_a_fade_and_a_scale_over_167_ms() {
        let (t, w) = (Instant::now(), wheel(4));
        let m = WheelMotion::new(t);
        assert_eq!(m.opening(t, true), (0.0, 0.92));
        assert!(m.animating(after(t, 100), true));
        assert_eq!(m.opening(after(t, 167), true), (1.0, 1.0));
        assert!(!m.animating(after(t, 167), true));
        // Animation effects off: open at once, nothing moving.
        assert_eq!(m.opening(t, false), (1.0, 1.0));
        assert!(!m.animating(t, false));
        let _ = w;
    }

    #[test]
    fn mores_list_fades_in_over_167_ms() {
        let t = Instant::now();
        let mut m = WheelMotion::new(t);
        let settled = after(t, 5000);
        assert_eq!(m.flyout_shown(settled, true), 0.0);
        m.open_flyout(settled);
        assert!(m.animating(after(settled, 80), true));
        assert_eq!(m.flyout_shown(after(settled, 167), true), 1.0);
        assert!(!m.animating(after(settled, 167), true));
        assert_eq!(m.flyout_shown(settled, false), 1.0);
    }

    #[test]
    fn a_wheel_opened_on_a_spot_has_it_lit() {
        let (t, w) = (Instant::now(), wheel(4));
        let m = WheelMotion::starting_on(&w, Some(Spot::Inner(2)), t);
        assert_eq!(m.hover, Some(Spot::Inner(2)));
        assert_eq!(m.format, Some(Spot::Inner(2)));
        assert_eq!(m.opened, t);
        let none = WheelMotion::starting_on(&w, None, t);
        assert_eq!(none.hover, None);
    }
}
