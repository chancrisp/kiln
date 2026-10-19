//! The glaze wheel: which items it offers and where they sit. Drawing and
//! input live in `win`; this is the platform-independent geometry.

use std::f32::consts::{PI, TAU};
use std::path::{Path, PathBuf};

use kiln_core::{Kiln, Kind, Tool};

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

/// One slot of the outer ring: an item, or the kind icon that starts a
/// group.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OuterSlot {
    pub a0: f32,
    pub a1: f32,
    /// Index into `Wheel::more`; `None` for a group's icon.
    pub item: Option<usize>,
    pub icon: &'static str,
}

/// The outer ring's slots, clockwise from the top: each group's kind
/// icon, then its formats.
pub fn outer_slots(more: &[Item]) -> Vec<OuterSlot> {
    let mut plan: Vec<(Option<usize>, &'static str)> = vec![];
    let mut group = None;
    for (i, item) in more.iter().enumerate() {
        let g = kind_order(item.kind);
        if group != Some(g) {
            group = Some(g);
            plan.push((None, item.icon));
        }
        plan.push((Some(i), item.icon));
    }
    let n = plan.len();
    plan.into_iter()
        .enumerate()
        .map(|(k, (item, icon))| {
            let (a0, a1) = segment(k, n);
            OuterSlot { a0, a1, item, icon }
        })
        .collect()
}

/// Where an outer slot's label is centred, relative to the wheel.
pub fn outer_point(slot: &OuterSlot) -> (f32, f32) {
    let mid = (slot.a0 + slot.a1) / 2.0;
    let r = (R_MORE_INNER + R_MORE_OUTER) / 2.0;
    (r * mid.cos(), r * mid.sin())
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
        return slots[slot_at(from_top, slots.len())].item.map(Spot::Outer);
    }
    None
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

    #[test]
    fn source_codes() {
        assert_eq!(source_code(&["a.png".into(), "b.PNG".into()]), "PNG");
        assert_eq!(source_code(&["a.png".into(), "b.jpg".into()]), "MIXED");
        assert_eq!(source_code(&[]), "");
    }
}
