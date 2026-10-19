//! The glaze wheel: which items it offers and where they sit. Drawing and
//! input live in `win`; this is the platform-independent geometry.

use std::f32::consts::{PI, TAU};
use std::path::{Path, PathBuf};

use kiln_core::{Kiln, Kind, Tool};

/// Outer and inner radius of the ring, in DIPs.
pub const R_OUTER: f32 = 136.0;
pub const R_INNER: f32 = 56.0;
/// How far past the outer edge the pointer still counts as on an item.
pub const REACH: f32 = 48.0;
/// Space kept between the wheel and the window edge.
pub const EDGE_GAP: f32 = 8.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelKind {
    /// Shift: convert to another format.
    Convert,
    /// Ctrl: compress, crop, trim, split, merge.
    Tools,
}

impl WheelKind {
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
    Tool(Tool),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// Format code ("JPG") or tool name ("Compress").
    pub label: String,
    /// Whether `label` is a format code, drawn in monospace.
    pub is_code: bool,
    pub icon: &'static str,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Wheel {
    pub kind: WheelKind,
    pub items: Vec<Item>,
    /// Centre in window DIPs.
    pub center: (f32, f32),
    /// The source type, shown muted under the title: "PNG", or "MIXED".
    pub source_code: String,
}

pub fn icon_for_kind(kind: Kind) -> &'static str {
    match kind {
        Kind::Image => "image",
        Kind::Audio => "audio",
        Kind::Video => "video",
        Kind::Document => "document",
    }
}

pub fn icon_for_tool(tool: Tool) -> &'static str {
    tool.id()
}

/// What the wheel shows for these files: for Convert, the formats every
/// file can become; for Tools, the tools that apply to all of them.
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
            common
                .unwrap_or_default()
                .into_iter()
                .filter_map(|ext| {
                    let f = kiln.registry().format(&ext)?;
                    Some(Item {
                        label: f.code(),
                        is_code: true,
                        icon: icon_for_kind(f.kind),
                        action: Action::Convert(ext),
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
            })
            .collect(),
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

/// Keep the whole wheel inside a `w` x `h` window.
pub fn clamp_center(x: f32, y: f32, w: f32, h: f32) -> (f32, f32) {
    let r = R_OUTER + EDGE_GAP;
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
    (cx, cy)
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

/// Which item is under a point (window DIPs). The centre selects nothing.
pub fn hit(w: &Wheel, x: f32, y: f32) -> Option<usize> {
    let n = w.items.len();
    if n == 0 {
        return None;
    }
    let (dx, dy) = (x - w.center.0, y - w.center.1);
    let d = (dx * dx + dy * dy).sqrt();
    if !(R_INNER..=R_OUTER + REACH).contains(&d) {
        return None;
    }
    // Angle measured from the top, clockwise, in 0..TAU, shifted so item 0
    // spans the top.
    let from_top = (dy.atan2(dx) + PI / 2.0).rem_euclid(TAU);
    let span = TAU / n as f32;
    Some(((from_top + span / 2.0) / span).floor() as usize % n)
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

    fn wheel(n: usize) -> Wheel {
        Wheel {
            kind: WheelKind::Convert,
            items: (0..n)
                .map(|i| Item {
                    label: format!("F{i}"),
                    is_code: true,
                    icon: "image",
                    action: Action::Convert(format!("f{i}")),
                })
                .collect(),
            center: (200.0, 200.0),
            source_code: "PNG".into(),
        }
    }

    #[test]
    fn item_zero_is_at_the_top_and_items_go_clockwise() {
        let w = wheel(4);
        let r = (R_INNER + R_OUTER) / 2.0;
        assert_eq!(hit(&w, 200.0, 200.0 - r), Some(0));
        assert_eq!(hit(&w, 200.0 + r, 200.0), Some(1));
        assert_eq!(hit(&w, 200.0, 200.0 + r), Some(2));
        assert_eq!(hit(&w, 200.0 - r, 200.0), Some(3));
    }

    #[test]
    fn centre_and_far_away_select_nothing() {
        let w = wheel(5);
        assert_eq!(hit(&w, 200.0, 200.0), None);
        assert_eq!(hit(&w, 200.0, 200.0 - R_INNER + 1.0), None);
        assert_eq!(hit(&w, 200.0, 200.0 - R_OUTER - REACH - 1.0), None);
        assert_eq!(hit(&w, 200.0, 200.0 - R_OUTER - 10.0), Some(0));
    }

    #[test]
    fn a_single_item_owns_the_whole_ring() {
        let w = wheel(1);
        for (x, y) in [(300.0, 200.0), (100.0, 200.0), (200.0, 300.0)] {
            assert_eq!(hit(&w, x, y), Some(0));
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
                assert_eq!(hit(&w, lx, ly), Some(i), "n={n} i={i}");
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
    fn keyboard_steps_wrap() {
        assert_eq!(step(None, 1, 3), Some(0));
        assert_eq!(step(None, -1, 3), Some(2));
        assert_eq!(step(Some(2), 1, 3), Some(0));
        assert_eq!(step(Some(0), -1, 3), Some(2));
        assert_eq!(step(None, 1, 0), None);
    }

    #[test]
    fn convert_offers_formats_every_file_shares() {
        let kiln = Kiln::new();
        let items = items_for(WheelKind::Convert, &kiln, &["a.png".into()]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "JPG");
        assert_eq!(items[0].icon, "image");

        let both = items_for(WheelKind::Convert, &kiln, &["a.mp4".into(), "b.mkv".into()]);
        let labels: Vec<&str> = both.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"MP3") && labels.contains(&"WEBM"));
        assert!(!labels.contains(&"MP4") && !labels.contains(&"MKV"));

        let none = items_for(WheelKind::Convert, &kiln, &["a.png".into(), "b.mp3".into()]);
        assert!(none.is_empty());
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
    fn source_codes() {
        assert_eq!(source_code(&["a.png".into(), "b.PNG".into()]), "PNG");
        assert_eq!(source_code(&["a.png".into(), "b.jpg".into()]), "MIXED");
        assert_eq!(source_code(&[]), "");
    }
}
