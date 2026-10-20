//! The glaze wheel: which items it offers and where they sit. Drawing and
//! input live in `win`; this is the platform-independent geometry.

use std::f32::consts::{PI, TAU};
use std::path::{Path, PathBuf};

use kiln_core::{Kiln, Kind, Suggested, Tool};

use crate::actions::FileAction;

/// Outer and inner radius of the ring, in DIPs.
pub const R_OUTER: f32 = 136.0;
pub const R_INNER: f32 = 56.0;
/// How far past the outer edge the pointer still counts as on an item.
pub const REACH: f32 = 48.0;
/// Space kept between the wheel and the window edge.
pub const EDGE_GAP: f32 = 8.0;
/// The band More pours open around the wheel, for the formats that
/// didn't fit on the inner ring.
pub const R_MORE_INNER: f32 = 144.0;
pub const R_MORE_OUTER: f32 = 208.0;
/// Most formats the inner ring shows when the rest go to More.
pub const COMMON: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelKind {
    /// Shift: convert to another format.
    Convert,
    /// Ctrl: compress, crop, trim, split, merge.
    Tools,
    /// Alt: copy path, move, open in Windows Terminal.
    Actions,
}

impl WheelKind {
    pub const ALL: [WheelKind; 3] = [WheelKind::Convert, WheelKind::Tools, WheelKind::Actions];

    /// The label in the wheel's centre.
    pub fn title(self) -> &'static str {
        match self {
            WheelKind::Convert => "Convert",
            WheelKind::Tools => "Tools",
            WheelKind::Actions => "Actions",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Convert to this extension.
    Convert(String),
    Tool(Tool),
    File(FileAction),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// Format code ("JPG") or tool name ("Compress").
    pub label: String,
    /// Whether `label` is a format code, drawn in monospace.
    pub is_code: bool,
    pub icon: &'static str,
    pub action: Action,
    /// The target's kind, which groups the outer ring. `None` for tools
    /// and file actions.
    pub kind: Option<Kind>,
}

/// A place on the wheel the pointer or the keyboard can be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spot {
    /// A segment of the inner ring. One past the last item is More.
    Inner(usize),
    /// An item on the outer ring that More opens.
    Outer(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Wheel {
    pub kind: WheelKind,
    /// The inner ring.
    pub items: Vec<Item>,
    /// What More holds, grouped by kind. Empty when everything fits on the
    /// inner ring, and then there is no More segment.
    pub more: Vec<Item>,
    /// Centre in window DIPs.
    pub center: (f32, f32),
    /// Below 1 only when the window is too small for the whole wheel.
    pub scale: f32,
    /// The source type, shown muted under the title: "PNG", or "MIXED".
    pub source_code: String,
}

impl Wheel {
    /// Segments on the inner ring: the items, and More if there is one.
    pub fn segments(&self) -> usize {
        self.items.len() + usize::from(!self.more.is_empty())
    }

    /// Whether inner segment `i` is More.
    pub fn is_more(&self, i: usize) -> bool {
        !self.more.is_empty() && i == self.items.len()
    }

    pub fn item(&self, spot: Spot) -> Option<&Item> {
        match spot {
            Spot::Inner(i) => self.items.get(i),
            Spot::Outer(i) => self.more.get(i),
        }
    }

    /// How far the wheel reaches from its centre at full size.
    pub fn extent(&self) -> f32 {
        if self.more.is_empty() {
            R_OUTER
        } else {
            R_MORE_OUTER
        }
    }
}

pub fn icon_for_kind(kind: Kind) -> &'static str {
    match kind {
        Kind::Image => "image",
        Kind::Audio => "audio",
        Kind::Video => "video",
        Kind::Document => "document",
        Kind::Spreadsheet => "sheet",
        Kind::Presentation => "slides",
        Kind::Subtitle => "subtitle",
    }
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

/// Targets people reach for most, first. The inner ring shows a file's
/// highest-ranked targets in this order, so a format keeps its place from
/// one wheel to the next.
const POPULAR: &[&str] = &[
    "jpg", "png", "pdf", "mp4", "mp3", "docx", "webp", "gif", "wav", "txt", "html", "md", "mov",
    "mkv", "webm", "m4a", "flac", "xlsx", "csv", "pptx", "epub", "avif", "ogg", "aac", "opus",
    "srt", "odt", "rtf", "tiff", "bmp", "ico", "jxl", "mobi", "azw3", "ods", "odp", "vtt", "avi",
    "wmv", "doc", "xls", "ppt", "ass", "ts", "m4v", "flv", "3gp", "mpg", "ogv", "apng", "aiff",
    "wma", "ac3", "wv", "tga", "qoi", "ppm", "tex", "rst", "org", "adoc", "typ", "wiki", "textile",
    "dbk", "fb2", "ipynb",
];

fn rank(ext: &str) -> usize {
    POPULAR
        .iter()
        .position(|p| *p == ext)
        .unwrap_or(POPULAR.len())
}

pub fn icon_for_tool(tool: Tool) -> &'static str {
    tool.id()
}

/// What the wheel shows for these files: for Convert, the formats every
/// file can become, most popular first; for Tools, the tools that apply
/// to all of them.
pub fn items_for(kind: WheelKind, kiln: &Kiln, paths: &[PathBuf]) -> Vec<Item> {
    let refs: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
    match kind {
        WheelKind::Convert => {
            let mut common: Option<Vec<String>> = None;
            for p in &refs {
                let targets: Vec<String> =
                    kiln.targets_for(p).iter().map(|f| f.ext.clone()).collect();
                common = Some(match common {
                    None => targets,
                    Some(c) => c.into_iter().filter(|e| targets.contains(e)).collect(),
                });
            }
            let mut exts = common.unwrap_or_default();
            exts.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.cmp(b)));
            exts.into_iter()
                .filter_map(|ext| {
                    let f = kiln.registry().format(&ext)?;
                    Some(Item {
                        label: f.code(),
                        is_code: true,
                        icon: icon_for_kind(f.kind),
                        action: Action::Convert(ext),
                        kind: Some(f.kind),
                    })
                })
                .collect()
        }
        WheelKind::Tools => kiln
            .tools_for(&refs)
            .into_iter()
            .map(|t| Item {
                label: t.label().to_owned(),
                is_code: false,
                icon: icon_for_tool(t),
                action: Action::Tool(t),
                kind: None,
            })
            .collect(),
        WheelKind::Actions => FileAction::ALL
            .into_iter()
            .map(|a| Item {
                label: a.label().to_owned(),
                is_code: false,
                icon: a.icon(),
                action: Action::File(a),
                kind: None,
            })
            .collect(),
    }
}

/// Split items between the inner ring and More. One more than `COMMON`
/// still fits, since More would take that segment anyway. Past that the
/// first `COMMON` stay and the rest go to More, grouped by kind and then
/// alphabetical.
pub fn split_common(mut items: Vec<Item>) -> (Vec<Item>, Vec<Item>) {
    if items.len() <= COMMON + 1 {
        return (items, vec![]);
    }
    let mut more = items.split_off(COMMON);
    more.sort_by(|a, b| {
        kind_order(a.kind)
            .cmp(&kind_order(b.kind))
            .then_with(|| a.label.cmp(&b.label))
    });
    (items, more)
}

/// How much of the outer ring a group's heading takes, against one for a
/// format. The heading holds an icon with its name under it, which a
/// format's slot is too narrow for once the ring is full.
pub const HEADING_WEIGHT: f32 = 2.5;

/// One slot of the outer ring: an item, or the heading that starts a
/// group.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OuterSlot {
    pub a0: f32,
    pub a1: f32,
    /// Index into `Wheel::more`; `None` for a group's heading.
    pub item: Option<usize>,
    pub icon: &'static str,
    /// The group's name, on a heading.
    pub heading: Option<&'static str>,
}

/// The plain name of a kind of format, muted beside its icon on the outer
/// ring.
pub fn kind_label(kind: Kind) -> &'static str {
    match kind {
        Kind::Image => "Images",
        Kind::Audio => "Audio",
        Kind::Video => "Video",
        Kind::Document => "Documents",
        Kind::Spreadsheet => "Sheets",
        Kind::Presentation => "Slides",
        Kind::Subtitle => "Subtitles",
    }
}

/// The outer ring's slots, clockwise from the top: each group's heading
/// (its kind icon and name), then its formats. The first slot is centred
/// at the top.
pub fn outer_slots(more: &[Item]) -> Vec<OuterSlot> {
    type Plan = (Option<usize>, &'static str, Option<&'static str>);
    let mut plan: Vec<Plan> = vec![];
    let mut group = None;
    for (i, item) in more.iter().enumerate() {
        let g = kind_order(item.kind);
        if group != Some(g) {
            group = Some(g);
            plan.push((None, item.icon, item.kind.map(kind_label)));
        }
        plan.push((Some(i), item.icon, None));
    }
    let weight = |item: Option<usize>| if item.is_none() { HEADING_WEIGHT } else { 1.0 };
    let total: f32 = plan.iter().map(|p| weight(p.0)).sum();
    let unit = TAU / total;
    let mut a = -PI / 2.0 - unit * weight(plan.first().and_then(|p| p.0)) / 2.0;
    plan.into_iter()
        .map(|(item, icon, heading)| {
            let a0 = a;
            a += unit * weight(item);
            OuterSlot {
                a0,
                a1: a,
                item,
                icon,
                heading,
            }
        })
        .collect()
}

/// Which outer slot an angle (clockwise from the positive x axis) falls in.
fn outer_slot_at(slots: &[OuterSlot], angle: f32) -> Option<&OuterSlot> {
    let first = slots.first()?;
    let rel = (angle - first.a0).rem_euclid(TAU);
    slots
        .iter()
        .find(|s| rel < s.a1 - first.a0)
        .or(slots.last())
}

/// Where an outer slot's label is centred, relative to the wheel.
pub fn outer_point(slot: &OuterSlot) -> (f32, f32) {
    let mid = (slot.a0 + slot.a1) / 2.0;
    let r = (R_MORE_INNER + R_MORE_OUTER) / 2.0;
    (r * mid.cos(), r * mid.sin())
}

/// Side of a heading's icon, in DIPs (the 24 unit icon at 0.75).
pub const HEADING_ICON: f32 = 18.0;
/// Height of a heading's name line.
pub const HEADING_LINE: f32 = 14.0;
/// Space between a heading's icon and its name.
pub const HEADING_GAP: f32 = 2.0;

/// The width a heading's name needs, in DIPs: DirectWrite's measure of the
/// text at the caption size (12 DIPs) in Segoe UI and in Segoe UI Variable
/// Text, which agree, plus about 2 DIPs of headroom so the caption's
/// ellipsis trimming never bites.
///
/// | name      | measured | here |
/// |-----------|----------|------|
/// | Images    | 38.1     | 40   |
/// | Audio     | 31.5     | 34   |
/// | Video     | 30.7     | 33   |
/// | Documents | 60.3     | 62   |
/// | Sheets    | 34.9     | 37   |
/// | Slides    | 30.6     | 33   |
/// | Subtitles | 45.5     | 48   |
///
/// A name not in the table gets 7 DIPs a character.
pub fn heading_width(name: &str) -> f32 {
    match name {
        "Images" => 40.0,
        "Audio" => 34.0,
        "Video" => 33.0,
        "Documents" => 62.0,
        "Sheets" => 37.0,
        "Slides" => 33.0,
        "Subtitles" => 48.0,
        _ => name.chars().count() as f32 * 7.0,
    }
}

/// A box in wheel space: left, top, width, height.
pub type Box4 = (f32, f32, f32, f32);

/// Whether a box in wheel space lies wholly inside the outer band, with a
/// DIP to spare. The band's inner edge curves into the box, so that bound
/// uses the box's nearest point to the centre, not just its corners; the
/// outer bound uses the farthest corner.
pub fn box_in_band(b: Box4) -> bool {
    let (x, y, w, h) = b;
    let near = (x.max(0.0).max(-(x + w))).hypot(y.max(0.0).max(-(y + h)));
    let far = [(x, y), (x + w, y), (x, y + h), (x + w, y + h)]
        .into_iter()
        .map(|(px, py)| px.hypot(py))
        .fold(0.0, f32::max);
    near >= R_MORE_INNER + 1.0 && far <= R_MORE_OUTER - 1.0
}

/// Where a heading draws, relative to the wheel's centre: the icon's box
/// and the name's, stacked and centred on the slot. The name's box is as
/// wide as its measured text. Beside the ring's left and right the name is
/// nearly as wide as the band, so the stack shifts a little, as little as
/// it can, to keep each box inside the band and its own slot.
pub fn heading_boxes(slot: &OuterSlot) -> Option<(Box4, Box4)> {
    let name = slot.heading?;
    let mid = (slot.a0 + slot.a1) / 2.0;
    let w = heading_width(name);
    let tall = HEADING_ICON + HEADING_GAP + HEADING_LINE;
    // The stack centred `dr` along the slot's middle line and `dt` across
    // it from the middle of the band.
    let at = |dr: f32, dt: f32| -> (Box4, Box4) {
        let r = (R_MORE_INNER + R_MORE_OUTER) / 2.0 + dr;
        let (x, y) = (
            r * mid.cos() - dt * mid.sin(),
            r * mid.sin() + dt * mid.cos(),
        );
        let top = y - tall / 2.0;
        (
            (x - HEADING_ICON / 2.0, top, HEADING_ICON, HEADING_ICON),
            (
                x - w / 2.0,
                top + HEADING_ICON + HEADING_GAP,
                w,
                HEADING_LINE,
            ),
        )
    };
    let in_wedge = |b: Box4| {
        let (x, y, bw, bh) = b;
        [(x, y), (x + bw, y), (x, y + bh), (x + bw, y + bh)]
            .into_iter()
            .all(|(px, py)| (py.atan2(px) - slot.a0).rem_euclid(TAU) < slot.a1 - slot.a0)
    };
    let fits = |(icon, name): (Box4, Box4)| {
        [icon, name]
            .into_iter()
            .all(|b| box_in_band(b) && in_wedge(b))
    };
    // The smallest move that fits, trying rings of 1 DIP steps outward.
    for k in 0..=48_i32 {
        for dr in -k..=k {
            for dt in -k..=k {
                if dr.abs().max(dt.abs()) == k {
                    let boxes = at(dr as f32, dt as f32);
                    if fits(boxes) {
                        return Some(boxes);
                    }
                }
            }
        }
    }
    Some(at(0.0, 0.0))
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

/// Where a wheel reaching `extent` DIPs from its centre goes in a `w` x
/// `h` window, and its scale: as near the point as it fits whole, and
/// smaller only when the window can't hold it at full size.
pub fn place(x: f32, y: f32, w: f32, h: f32, extent: f32) -> ((f32, f32), f32) {
    let room = (w.min(h) / 2.0 - EDGE_GAP).max(1.0);
    let scale = (room / extent).min(1.0);
    let r = extent * scale + EDGE_GAP;
    let cx = if w >= 2.0 * r {
        x.clamp(r, w - r)
    } else {
        w / 2.0
    };
    let cy = if h >= 2.0 * r {
        y.clamp(r, h - r)
    } else {
        h / 2.0
    };
    ((cx, cy), scale)
}

/// Keep a plain wheel inside a `w` x `h` window.
pub fn clamp_center(x: f32, y: f32, w: f32, h: f32) -> (f32, f32) {
    place(x, y, w, h, R_OUTER).0
}

/// Start and end angle of item `i` of `n`, in radians, clockwise from the
/// positive x axis (screen coordinates, y down). Item 0 is centred at the
/// top and items go clockwise.
pub fn segment(i: usize, n: usize) -> (f32, f32) {
    let span = TAU / n as f32;
    let mid = -PI / 2.0 + i as f32 * span;
    (mid - span / 2.0, mid + span / 2.0)
}

/// Where item `i`'s icon and label are centred, relative to the wheel.
pub fn label_point(i: usize, n: usize) -> (f32, f32) {
    let (a0, a1) = segment(i, n);
    let mid = (a0 + a1) / 2.0;
    let r = (R_OUTER + R_INNER) / 2.0;
    if n == 1 {
        return (0.0, -r);
    }
    (r * mid.cos(), r * mid.sin())
}

/// Which of `n` equal slots an angle falls in, measured clockwise from
/// the top with slot 0 centred there.
fn slot_at(from_top: f32, n: usize) -> usize {
    let span = TAU / n as f32;
    ((from_top + span / 2.0) / span).floor() as usize % n
}

/// Whether a point (window DIPs) is on the outer band that More opened,
/// heading or format, as against the inner ring or empty space.
pub fn on_outer_band(w: &Wheel, x: f32, y: f32, more_open: bool) -> bool {
    if !more_open || w.more.is_empty() {
        return false;
    }
    let s = w.scale.max(0.01);
    let (dx, dy) = ((x - w.center.0) / s, (y - w.center.1) / s);
    let d = dx.hypot(dy);
    d > (R_OUTER + R_MORE_INNER) / 2.0 && d <= R_MORE_OUTER + REACH
}

/// What is under a point (window DIPs). The centre selects nothing, and
/// neither does a group's icon on the outer ring. With `more_open`, the
/// outer ring takes the space past the wheel's edge.
pub fn hit(w: &Wheel, x: f32, y: f32, more_open: bool) -> Option<Spot> {
    let n = w.segments();
    if n == 0 {
        return None;
    }
    let s = w.scale.max(0.01);
    let (dx, dy) = ((x - w.center.0) / s, (y - w.center.1) / s);
    let d = (dx * dx + dy * dy).sqrt();
    let from_top = (dy.atan2(dx) + PI / 2.0).rem_euclid(TAU);
    let outer = more_open && !w.more.is_empty();
    let edge = if outer {
        (R_OUTER + R_MORE_INNER) / 2.0
    } else {
        R_OUTER + REACH
    };
    if (R_INNER..=edge).contains(&d) {
        return Some(Spot::Inner(slot_at(from_top, n)));
    }
    if outer && d > edge && d <= R_MORE_OUTER + REACH {
        let slots = outer_slots(&w.more);
        let angle = dy.atan2(dx);
        return outer_slot_at(&slots, angle)?.item.map(Spot::Outer);
    }
    None
}

/// Where a suggestion sits on the wheel: its inner segment, or More and
/// its slot on the outer ring. Empty when the wheel doesn't offer it.
pub fn marks(w: &Wheel, s: &Suggested) -> Vec<Spot> {
    let action = match s {
        Suggested::Convert(ext) => Action::Convert(ext.clone()),
        Suggested::Tool(t) => Action::Tool(*t),
    };
    if let Some(i) = w.items.iter().position(|it| it.action == action) {
        return vec![Spot::Inner(i)];
    }
    match w.more.iter().position(|it| it.action == action) {
        Some(j) => vec![Spot::Inner(w.items.len()), Spot::Outer(j)],
        None => vec![],
    }
}

/// Where the keyboard starts on a wheel: on the suggestion, or More when
/// the suggestion is in it, or else the first segment.
pub fn start_spot(w: &Wheel, s: Option<&Suggested>) -> Spot {
    s.and_then(|s| marks(w, s).first().copied())
        .unwrap_or(Spot::Inner(0))
}

/// The footer while the suggestion shows: "Suggested: JPG. " and the
/// reason.
pub fn suggestion_footer(label: &str, reason: &str) -> String {
    format!("Suggested: {label}. {reason}")
}

/// A format as the wheel names it: "WEBP: a small image for the web". A
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

/// The footer while a convert target is lit: what the format is. Tools,
/// file actions and More have nothing to say, and leave the footer as it
/// was.
pub fn lit_footer(item: Option<&Item>) -> Option<String> {
    match &item?.action {
        Action::Convert(ext) => Some(target_label(ext)),
        _ => None,
    }
}

/// The footer on an open wheel, from what is lit and the suggestion line
/// (`Some` only while the suggestion shows, see `shows_suggestion`).
/// - A lit convert target says what it is: "JPG: a photo that opens
///   anywhere". When that target is also the suggested one, the suggestion
///   follows after a dot.
/// - With nothing lit, or More lit, the suggestion stands alone.
/// - Otherwise `None`, and the footer says what it did before.
pub fn wheel_footer(lit: Option<&Item>, suggestion: Option<String>) -> Option<String> {
    match (lit_footer(lit), suggestion) {
        (Some(what), Some(why)) => Some(format!("{what} · {why}")),
        (Some(what), None) => Some(what),
        (None, why) => why,
    }
}

/// Whether the footer gives the suggestion's reason: while nothing is lit,
/// or while a marked spot is.
pub fn shows_suggestion(hover: Option<Spot>, marks: &[Spot]) -> bool {
    !marks.is_empty() && hover.is_none_or(|h| marks.contains(&h))
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn item(i: usize, kind: Kind) -> Item {
        Item {
            label: format!("F{i}"),
            is_code: true,
            icon: icon_for_kind(kind),
            action: Action::Convert(format!("f{i}")),
            kind: Some(kind),
        }
    }

    fn wheel(n: usize) -> Wheel {
        Wheel {
            kind: WheelKind::Convert,
            items: (0..n).map(|i| item(i, Kind::Image)).collect(),
            more: vec![],
            center: (200.0, 200.0),
            scale: 1.0,
            source_code: "PNG".into(),
        }
    }

    #[test]
    fn item_zero_is_at_the_top_and_items_go_clockwise() {
        let w = wheel(4);
        let r = (R_INNER + R_OUTER) / 2.0;
        let at = |x, y| hit(&w, x, y, false);
        assert_eq!(at(200.0, 200.0 - r), Some(Spot::Inner(0)));
        assert_eq!(at(200.0 + r, 200.0), Some(Spot::Inner(1)));
        assert_eq!(at(200.0, 200.0 + r), Some(Spot::Inner(2)));
        assert_eq!(at(200.0 - r, 200.0), Some(Spot::Inner(3)));
    }

    #[test]
    fn centre_and_far_away_select_nothing() {
        let w = wheel(5);
        let at = |x, y| hit(&w, x, y, false);
        assert_eq!(at(200.0, 200.0), None);
        assert_eq!(at(200.0, 200.0 - R_INNER + 1.0), None);
        assert_eq!(at(200.0, 200.0 - R_OUTER - REACH - 1.0), None);
        assert_eq!(at(200.0, 200.0 - R_OUTER - 10.0), Some(Spot::Inner(0)));
    }

    #[test]
    fn a_single_item_owns_the_whole_ring() {
        let w = wheel(1);
        for (x, y) in [(300.0, 200.0), (100.0, 200.0), (200.0, 300.0)] {
            assert_eq!(hit(&w, x, y, false), Some(Spot::Inner(0)));
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
                assert_eq!(hit(&w, lx, ly, false), Some(Spot::Inner(i)), "n={n} i={i}");
            }
            assert!((total - TAU).abs() < 1e-4);
        }
    }

    #[test]
    fn the_wheel_stays_inside_the_window() {
        assert_eq!(clamp_center(10.0, 10.0, 720.0, 480.0), (144.0, 144.0));
        assert_eq!(clamp_center(700.0, 470.0, 720.0, 480.0), (576.0, 336.0));
        assert_eq!(clamp_center(300.0, 200.0, 720.0, 480.0), (300.0, 200.0));
        assert_eq!(clamp_center(10.0, 10.0, 200.0, 200.0), (100.0, 100.0));
    }

    #[test]
    fn a_wheel_with_more_fits_or_shrinks() {
        let ((cx, cy), s) = place(10.0, 10.0, 720.0, 480.0, R_MORE_OUTER);
        assert_eq!((cx, cy, s), (216.0, 216.0, 1.0));
        // The smallest window holds the whole wheel, a little smaller.
        let ((cx, cy), s) = place(10.0, 10.0, 480.0, 360.0, R_MORE_OUTER);
        assert!(s < 1.0);
        assert!(cy - R_MORE_OUTER * s >= EDGE_GAP - 0.01, "{cy} {s}");
        assert!(cx - R_MORE_OUTER * s >= EDGE_GAP - 0.01);
    }

    #[test]
    fn keyboard_steps_wrap() {
        assert_eq!(step(None, 1, 3), Some(0));
        assert_eq!(step(None, -1, 3), Some(2));
        assert_eq!(step(Some(2), 1, 3), Some(0));
        assert_eq!(step(Some(0), -1, 3), Some(2));
        assert_eq!(step(None, 1, 0), None);
    }

    #[test]
    fn up_to_nine_fit_and_the_rest_go_to_more_by_kind() {
        let few: Vec<Item> = (0..9).map(|i| item(i, Kind::Image)).collect();
        let (inner, more) = split_common(few);
        assert_eq!((inner.len(), more.len()), (9, 0));

        let kinds = [Kind::Audio, Kind::Image, Kind::Video];
        let many: Vec<Item> = (0..14).map(|i| item(i, kinds[i % 3])).collect();
        let (inner, more) = split_common(many);
        assert_eq!(inner.len(), COMMON);
        assert_eq!(more.len(), 6);
        let order: Vec<usize> = more.iter().map(|i| kind_order(i.kind)).collect();
        assert!(order.windows(2).all(|w| w[0] <= w[1]), "{order:?}");
    }

    fn with_more() -> Wheel {
        let kinds = [Kind::Audio, Kind::Image, Kind::Video];
        let (items, more) = split_common((0..20).map(|i| item(i, kinds[i % 3])).collect());
        Wheel {
            items,
            more,
            ..wheel(0)
        }
    }

    #[test]
    fn more_is_the_last_inner_segment() {
        let w = with_more();
        assert_eq!(w.segments(), COMMON + 1);
        assert!(w.is_more(COMMON) && !w.is_more(0));
        assert_eq!(w.extent(), R_MORE_OUTER);
        assert_eq!(wheel(3).extent(), R_OUTER);
        assert_eq!(wheel(3).segments(), 3);
    }

    #[test]
    fn the_outer_ring_groups_formats_under_their_kind() {
        let w = with_more();
        let slots = outer_slots(&w.more);
        // One icon slot per kind, then every format once, in order.
        assert_eq!(slots.len(), w.more.len() + 3);
        assert_eq!(slots[0].item, None);
        let items: Vec<usize> = slots.iter().filter_map(|s| s.item).collect();
        assert_eq!(items, (0..w.more.len()).collect::<Vec<_>>());
        for s in &slots {
            if let Some(i) = s.item {
                let (x, y) = outer_point(s);
                let at = (w.center.0 + x, w.center.1 + y);
                assert_eq!(hit(&w, at.0, at.1, true), Some(Spot::Outer(i)));
                // Closed, the same point still reaches the inner ring.
                assert!(matches!(hit(&w, at.0, at.1, false), Some(Spot::Inner(_))));
            }
        }
        let (x, y) = outer_point(&slots[0]);
        assert_eq!(hit(&w, w.center.0 + x, w.center.1 + y, true), None);
    }

    #[test]
    fn hits_follow_the_wheel_scale() {
        let w = Wheel {
            scale: 0.5,
            ..wheel(4)
        };
        let r = (R_INNER + R_OUTER) / 2.0 * 0.5;
        assert_eq!(hit(&w, 200.0 + r, 200.0, false), Some(Spot::Inner(1)));
        let reach = (R_OUTER + REACH) * 0.5;
        assert_eq!(
            hit(&w, 200.0 + reach - 1.0, 200.0, false),
            Some(Spot::Inner(1))
        );
        assert_eq!(hit(&w, 200.0 + reach + 1.0, 200.0, false), None);
        assert_eq!(hit(&w, 200.0 + R_INNER * 0.5 - 1.0, 200.0, false), None);
    }

    #[test]
    fn convert_offers_formats_every_file_shares() {
        let kiln = Kiln::new();
        let items = items_for(WheelKind::Convert, &kiln, &["a.png".into()]);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        // Most popular first.
        assert_eq!(&labels[..4], ["JPG", "PDF", "WEBP", "GIF"]);
        assert_eq!(items.len(), 12);
        assert!(items
            .iter()
            .all(|i| crate::icons::ALL.iter().any(|ic| ic.name == i.icon)));

        let both = items_for(WheelKind::Convert, &kiln, &["a.mp4".into(), "b.mkv".into()]);
        let labels: Vec<&str> = both.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"MP3") && labels.contains(&"WEBM"));
        assert!(!labels.contains(&"MP4") && !labels.contains(&"MKV"));

        let none = items_for(WheelKind::Convert, &kiln, &["a.png".into(), "b.mp3".into()]);
        assert!(none.is_empty());
    }

    #[test]
    fn video_keeps_its_everyday_targets_on_the_inner_ring() {
        let kiln = Kiln::new();
        let (inner, more) = split_common(items_for(WheelKind::Convert, &kiln, &["a.mp4".into()]));
        let labels: Vec<&str> = inner.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(
            labels,
            ["MP3", "WEBP", "GIF", "WAV", "MOV", "MKV", "WEBM", "M4A"]
        );
        assert!(more.iter().any(|i| i.label == "SRT"));
        assert!(more.iter().all(|i| !labels.contains(&i.label.as_str())));
    }

    #[test]
    fn tools_offer_what_applies() {
        let kiln = Kiln::new();
        let items = items_for(WheelKind::Tools, &kiln, &["a.mp3".into(), "b.mp3".into()]);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, ["Compress", "Trim", "Split", "Merge"]);
        assert!(items.iter().all(|i| !i.is_code));
        assert!(items
            .iter()
            .all(|i| crate::icons::ALL.iter().any(|ic| ic.name == i.icon)));
    }

    #[test]
    fn actions_are_offered_for_any_files() {
        let kiln = Kiln::new();
        let items = items_for(WheelKind::Actions, &kiln, &["a.xyz".into(), "b.png".into()]);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, ["Copy path", "Move", "Open in Terminal"]);
        assert!(items
            .iter()
            .all(|i| crate::icons::ALL.iter().any(|ic| ic.name == i.icon)));
    }

    fn real(kind: WheelKind, path: &str) -> Wheel {
        let kiln = Kiln::new();
        let (items, more) = split_common(items_for(kind, &kiln, &[path.into()]));
        Wheel {
            kind,
            items,
            more,
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
            .more
            .iter()
            .position(|i| i.label == "SRT")
            .expect("SRT is in More");
        let m = marks(&w, &Suggested::Convert("srt".into()));
        assert_eq!(m, [Spot::Inner(w.items.len()), Spot::Outer(j)]);
    }

    #[test]
    fn a_tool_marks_its_segment() {
        let w = real(WheelKind::Tools, "a.mp4");
        let m = marks(&w, &Suggested::Tool(Tool::Crop));
        assert_eq!(m.len(), 1);
        let Spot::Inner(i) = m[0] else {
            panic!("{m:?}");
        };
        assert_eq!(w.items[i].action, Action::Tool(Tool::Crop));
    }

    #[test]
    fn a_suggestion_not_on_the_wheel_marks_nothing() {
        let w = real(WheelKind::Convert, "a.png");
        let s = Suggested::Convert("mp3".into());
        assert_eq!(marks(&w, &s), []);
        assert_eq!(start_spot(&w, Some(&s)), Spot::Inner(0));
        assert_eq!(start_spot(&w, None), Spot::Inner(0));
        // The file actions wheel never marks a tool.
        let w = real(WheelKind::Actions, "a.png");
        assert_eq!(marks(&w, &Suggested::Tool(Tool::Crop)), []);
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
    fn the_footer_gives_the_reason() {
        assert_eq!(
            suggestion_footer("JPG", "A photo. JPG keeps the look at about 2 MB."),
            "Suggested: JPG. A photo. JPG keeps the look at about 2 MB."
        );
    }

    #[test]
    fn the_reason_shows_while_nothing_else_is_lit() {
        use Spot::{Inner, Outer};
        assert!(shows_suggestion(None, &[Inner(2)]));
        assert!(shows_suggestion(Some(Inner(2)), &[Inner(2)]));
        assert!(shows_suggestion(Some(Outer(5)), &[Inner(8), Outer(5)]));
        assert!(!shows_suggestion(Some(Inner(1)), &[Inner(2)]));
        assert!(!shows_suggestion(None, &[]));
    }

    #[test]
    fn a_target_is_named_with_what_it_is() {
        assert_eq!(target_label("webp"), "WEBP: a small image for the web");
        assert_eq!(target_label(".QOI"), "QOI: a fast, lossless image");
        assert_eq!(target_label("wv"), "WV: WavPack, lossless audio");
        assert_eq!(target_label("dbk"), "DBK: a DocBook document");
        // Nothing to say about a format Kiln doesn't know: just its code.
        assert_eq!(target_label("xyz"), "XYZ");
    }

    #[test]
    fn the_footer_names_a_lit_convert_target() {
        let w = real(WheelKind::Convert, "a.png");
        let jpg = w.items.iter().find(|i| i.label == "JPG").unwrap();
        assert_eq!(
            lit_footer(Some(jpg)).as_deref(),
            Some("JPG: a photo that opens anywhere")
        );
        // Nothing lit, so the old hint stays.
        assert_eq!(lit_footer(None), None);
        // A tool has no format to name.
        let tools = real(WheelKind::Tools, "a.mp4");
        assert_eq!(lit_footer(tools.items.first()), None);
        // Every format on every wheel has something to say.
        for item in w.items.iter().chain(&w.more) {
            let said = lit_footer(Some(item)).unwrap();
            assert!(said.starts_with(&format!("{}: ", item.label)), "{said}");
        }
    }

    #[test]
    fn each_group_is_named_for_its_kind() {
        assert_eq!(kind_label(Kind::Image), "Images");
        assert_eq!(kind_label(Kind::Audio), "Audio");
        assert_eq!(kind_label(Kind::Video), "Video");
        assert_eq!(kind_label(Kind::Document), "Documents");
        let w = with_more();
        let slots = outer_slots(&w.more);
        for s in &slots {
            assert_eq!(s.heading.is_some(), s.item.is_none());
        }
        let names: Vec<_> = slots.iter().filter_map(|s| s.heading).collect();
        assert_eq!(names, ["Images", "Video", "Audio"]);
    }

    #[test]
    fn the_outer_slots_tile_the_ring() {
        let w = with_more();
        let slots = outer_slots(&w.more);
        assert!((slots[0].a0 + slots[0].a1 - 2.0 * (-PI / 2.0)).abs() < 1e-4);
        for pair in slots.windows(2) {
            assert!((pair[0].a1 - pair[1].a0).abs() < 1e-5);
        }
        assert!((slots.last().unwrap().a1 - slots[0].a0 - TAU).abs() < 1e-4);
        // A heading is wider than a format.
        assert!(slots[0].a1 - slots[0].a0 > slots[1].a1 - slots[1].a0);
    }

    /// Whether a box in wheel space lies wholly in the slot's piece of the
    /// outer ring.
    fn box_in_slot(b: Box4, slot: &OuterSlot) -> bool {
        let (x, y, w, h) = b;
        box_in_band(b)
            && [(x, y), (x + w, y), (x, y + h), (x + w, y + h)]
                .into_iter()
                .all(|(px, py)| {
                    let rel = (py.atan2(px) - slot.a0).rem_euclid(TAU);
                    rel <= slot.a1 - slot.a0 - 0.01
                })
    }

    #[test]
    fn a_heading_stays_in_its_slot_for_every_source_format() {
        let reg = kiln_core::Registry::builtin();
        let mut checked = 0;
        for ext in reg.extensions() {
            let w = real(WheelKind::Convert, &format!("a.{ext}"));
            for slot in outer_slots(&w.more) {
                let Some((icon, name)) = heading_boxes(&slot) else {
                    continue;
                };
                checked += 1;
                assert!(box_in_slot(icon, &slot), "{ext}: the icon leaves its slot");
                assert!(
                    box_in_slot(name, &slot),
                    "{ext}: {:?} leaves its slot {:?} {:?} n={}",
                    slot.heading,
                    slot,
                    name,
                    outer_slots(&w.more).len()
                );
                // The name sits under the icon, not over it.
                assert!(name.1 >= icon.1 + icon.3);
            }
        }
        assert!(checked > 20, "only {checked} headings checked");
    }

    #[test]
    fn headings_clear_the_window_edge_at_the_smallest_window() {
        let (ww, wh) = crate::layout::MIN_WINDOW;
        let reg = kiln_core::Registry::builtin();
        for ext in reg.extensions() {
            let mut w = real(WheelKind::Convert, &format!("a.{ext}"));
            if w.more.is_empty() {
                continue;
            }
            // Opened at each corner and the middle, the way a wheel opens
            // anywhere in the window.
            for (x, y) in [
                (0.0, 0.0),
                (ww, 0.0),
                (0.0, wh),
                (ww, wh),
                (ww / 2.0, wh / 2.0),
            ] {
                let (c, scale) = place(x, y, ww, wh, w.extent());
                w.center = c;
                w.scale = scale;
                for slot in outer_slots(&w.more) {
                    let Some((icon, name)) = heading_boxes(&slot) else {
                        continue;
                    };
                    for (bx, by, bw, bh) in [icon, name] {
                        let (l, t) = (c.0 + bx * scale, c.1 + by * scale);
                        let (r, b) = (l + bw * scale, t + bh * scale);
                        assert!(l >= 0.0 && t >= 0.0 && r <= ww && b <= wh, "{ext}");
                    }
                }
            }
        }
    }

    #[test]
    fn hits_on_the_outer_ring_follow_the_weighted_slots() {
        let w = Wheel {
            center: (300.0, 300.0),
            ..with_more()
        };
        // Just inside each slot's edges lands on that slot; a heading
        // selects nothing.
        for slot in outer_slots(&w.more) {
            for f in [0.1, 0.5, 0.9] {
                let a = slot.a0 + (slot.a1 - slot.a0) * f;
                let r = (R_MORE_INNER + R_MORE_OUTER) / 2.0;
                let got = hit(&w, 300.0 + r * a.cos(), 300.0 + r * a.sin(), true);
                assert_eq!(got, slot.item.map(Spot::Outer));
            }
        }
    }

    #[test]
    fn heading_names_fit_their_measured_width() {
        // Measured with DirectWrite at 12 DIPs; see `heading_width`.
        for (name, measured) in [
            ("Images", 38.1),
            ("Audio", 31.5),
            ("Video", 30.7),
            ("Documents", 60.3),
            ("Sheets", 34.9),
            ("Slides", 30.6),
            ("Subtitles", 45.5),
        ] {
            let w = heading_width(name);
            assert!(w >= measured + 1.5 && w <= measured + 3.0, "{name}: {w}");
        }
        for kind in [
            Kind::Image,
            Kind::Audio,
            Kind::Video,
            Kind::Document,
            Kind::Spreadsheet,
            Kind::Presentation,
            Kind::Subtitle,
        ] {
            let name = kind_label(kind);
            let guess = name.chars().count() as f32 * 7.0;
            assert_ne!(heading_width(name), guess, "{name} is not in the table");
        }
    }

    #[test]
    fn a_click_on_a_heading_is_on_the_outer_band_but_hits_nothing() {
        let w = Wheel {
            center: (300.0, 300.0),
            ..with_more()
        };
        let slots = outer_slots(&w.more);
        let head = slots.iter().find(|s| s.item.is_none()).unwrap();
        let (x, y) = outer_point(head);
        let at = (300.0 + x, 300.0 + y);
        assert_eq!(hit(&w, at.0, at.1, true), None);
        assert!(on_outer_band(&w, at.0, at.1, true));
        // Closed, the band isn't there; the inner ring and the empty
        // space beyond are not the band either.
        assert!(!on_outer_band(&w, at.0, at.1, false));
        assert!(!on_outer_band(&w, 300.0, 300.0, true));
        assert!(!on_outer_band(&w, 300.0 + R_INNER + 10.0, 300.0, true));
        let far = 300.0 + R_MORE_OUTER + REACH + 5.0;
        assert!(!on_outer_band(&w, far, 300.0, true));
        // A wheel without More has no band at all.
        let plain = Wheel {
            center: (300.0, 300.0),
            ..wheel(4)
        };
        assert!(!on_outer_band(&plain, 300.0 + 150.0, 300.0, true));
    }

    #[test]
    fn the_band_check_sees_a_box_dipping_toward_the_centre() {
        // Every corner is outside the inner edge, but the middle of the
        // bottom edge is not.
        let dipping = (-60.0, -150.0, 120.0, 5.5);
        let (x, y, w, h) = dipping;
        for (px, py) in [(x, y), (x + w, y), (x, y + h), (x + w, y + h)] {
            assert!(f32::hypot(px, py) >= R_MORE_INNER + 1.0);
        }
        assert!(!box_in_band(dipping));
        // A box inside the band is in; one over either edge is out.
        assert!(box_in_band((-30.0, -176.0, 60.0, 14.0)));
        assert!(!box_in_band((-30.0, -R_MORE_INNER, 60.0, 10.0)));
        assert!(!box_in_band((-30.0, -R_MORE_OUTER, 60.0, 10.0)));
    }

    fn jpg_item() -> Item {
        real(WheelKind::Convert, "a.png")
            .items
            .into_iter()
            .find(|i| i.label == "JPG")
            .unwrap()
    }

    #[test]
    fn the_footer_with_nothing_lit_is_the_suggestion_or_the_old_hint() {
        let why = suggestion_footer("JPG", "A photo. JPG keeps the look at about 2 MB.");
        assert_eq!(wheel_footer(None, Some(why.clone())), Some(why));
        assert_eq!(wheel_footer(None, None), None);
    }

    #[test]
    fn the_suggested_target_names_itself_then_gives_the_reason() {
        let why = suggestion_footer("JPG", "A photo. JPG keeps the look at about 2 MB.");
        assert_eq!(
            wheel_footer(Some(&jpg_item()), Some(why)).as_deref(),
            Some(
                "JPG: a photo that opens anywhere \u{b7} Suggested: JPG. \
                 A photo. JPG keeps the look at about 2 MB."
            )
        );
    }

    #[test]
    fn another_target_gives_only_its_name() {
        let w = real(WheelKind::Convert, "a.png");
        let webp = w.items.iter().find(|i| i.label == "WEBP").unwrap();
        // The suggestion doesn't show while an unmarked spot is lit.
        assert_eq!(
            wheel_footer(Some(webp), None).as_deref(),
            Some("WEBP: a small image for the web")
        );
    }

    #[test]
    fn more_lit_with_the_suggestion_in_more_keeps_the_suggestion() {
        // More has no item of its own, so only the suggestion is left.
        let why = suggestion_footer("QOI", "Fast.");
        assert_eq!(
            wheel_footer(None, Some(why.clone())).as_deref(),
            Some(why.as_str())
        );
        // The suggested target itself, lit in the ring, names itself first.
        let w = real(WheelKind::Convert, "a.png");
        let qoi = w.more.iter().find(|i| i.label == "QOI").unwrap();
        assert_eq!(
            wheel_footer(Some(qoi), Some(why)).as_deref(),
            Some("QOI: a fast, lossless image \u{b7} Suggested: QOI. Fast.")
        );
    }

    #[test]
    fn a_lit_tool_gives_the_suggestion_or_nothing() {
        let tools = real(WheelKind::Tools, "a.mp4");
        let tool = tools.items.first();
        assert_eq!(wheel_footer(tool, None), None);
        let why = suggestion_footer("Compress", "Big file.");
        assert_eq!(wheel_footer(tool, Some(why.clone())), Some(why));
    }

    #[test]
    fn source_codes() {
        assert_eq!(source_code(&["a.png".into(), "b.PNG".into()]), "PNG");
        assert_eq!(source_code(&["a.png".into(), "b.jpg".into()]), "MIXED");
        assert_eq!(source_code(&[]), "");
    }
}
