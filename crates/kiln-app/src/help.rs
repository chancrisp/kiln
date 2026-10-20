//! The help overlay's words: the ways in, how held results work, and every
//! keyboard shortcut Kiln has. Pure, so the table can be checked against
//! the keys the window really handles, and where each line goes can be
//! tested without a window.

use crate::settings::KeyMap;
use crate::ways_in::MenuItem;
use crate::wheel::WheelKind;

/// The overlay's title, and the Help button's name.
pub const TITLE: &str = "Help";

/// Tooltips for the controls drawn as icons, and for the buttons with a
/// shortcut, each naming its shortcut.
pub const SETTINGS_TIP: &str = "Settings (Ctrl+,)";
pub const HELP_TIP: &str = "Help (F1)";
pub const STOP_TIP: &str = "Stop";
pub const EXPORT_TIP: &str = "Export (Ctrl+S)";
pub const ADD_FILES_TIP: &str = "Add files (Ctrl+O)";
pub const EXPORT_ALL_TIP: &str = "Export all (Ctrl+Shift+S)";

/// One part of the overlay, under its heading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpSection {
    pub title: &'static str,
    pub lines: Vec<HelpLine>,
}

/// One line: the keys or gesture on the left, what it does on the right.
/// A line with no keys is a note that runs the overlay's width.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpLine {
    pub keys: String,
    pub what: String,
}

fn line(keys: impl Into<String>, what: impl Into<String>) -> HelpLine {
    HelpLine {
        keys: keys.into(),
        what: what.into(),
    }
}

/// Everything the overlay says, with the wheel keys as they are set now.
pub fn help_content(keys: &KeyMap) -> Vec<HelpSection> {
    let enter = |w: WheelKind| MenuItem::Wheel(w).parts(keys).1;
    let held = |w: WheelKind| format!("Hold {} and drop", keys.key_for(w).name());
    vec![
        HelpSection {
            title: "Ways in",
            lines: vec![
                line("Drop", "Drop files anywhere on the window to add them."),
                line("Ctrl+O", "Add files: choose them in a window."),
                line("Ctrl+V", "Paste files copied in File Explorer."),
                line(
                    "Right-click",
                    "Right-click a file in the list to convert it, use a tool on it or act on it.",
                ),
                line(held(WheelKind::Convert), "Choose a format to convert to."),
                line(held(WheelKind::Tools), "Choose a tool."),
                line(held(WheelKind::Actions), "Choose a file action."),
            ],
        },
        HelpSection {
            title: "Results",
            lines: vec![
                line(
                    "",
                    "Results wait in Kiln until you export them. Nothing is saved until you do.",
                ),
                line("Ctrl+S", "Export the selected results."),
                line("Ctrl+Shift+S", "Export all: every result Kiln holds."),
                line("Double-click", "Open a result to look at it."),
            ],
        },
        HelpSection {
            title: "Keyboard shortcuts",
            lines: vec![
                line(enter(WheelKind::Convert), "Convert the selected files."),
                line(enter(WheelKind::Tools), "Tools for the selected files."),
                line(
                    enter(WheelKind::Actions),
                    "File actions for the selected files.",
                ),
                line("Menu key or Shift+F10", "The right-click menu."),
                line("Up, Down, Home, End", "Move through the files."),
                line("Shift+Up, Shift+Down", "Select more files."),
                line("Shift+Home, Shift+End", "Select to the first or last file."),
                line("Ctrl+A", "Select every file."),
                line("Escape", "Clear the selection."),
                line(
                    "Space",
                    "Export, show details or open the folder, as the file's link says, or stop a running file.",
                ),
                line(
                    "Delete or Backspace",
                    "Remove the selected files from the list.",
                ),
                line(
                    "Tab, Shift+Tab",
                    "Move between the list and the buttons. Enter or Space presses a button.",
                ),
                line("Ctrl+,", "Settings."),
                line("F1", "This help."),
            ],
        },
        HelpSection {
            title: "In a wheel",
            lines: vec![
                line("Left, Right, Up, Down", "Move between choices."),
                line("Tab", "Options for the lit format, where it has them."),
                line("Enter or Space", "Choose."),
                line("Escape", "Close the wheel, or step back from More."),
            ],
        },
        HelpSection {
            title: "In panels",
            lines: vec![
                line("Tab, Shift+Tab", "Move between controls."),
                line("Left, Right", "Change the value under focus."),
                line("Space", "Press the control under focus."),
                line(
                    "Enter",
                    "Convert from the options sheet, close the details, or press the control under focus in Settings.",
                ),
                line("Page Up, Page Down", "Scroll long text."),
                line("Ctrl+C", "Copy the details."),
                line("Escape", "Close the panel."),
            ],
        },
    ]
}

/// The width of the keys column.
pub const KEYS_WIDTH: f32 = 152.0;
/// The gap between the keys and what they do.
pub const KEYS_GAP: f32 = 8.0;
/// A section heading's height, and the space above every heading but the
/// first.
pub const HEADING: f32 = 24.0;
pub const SECTION_GAP: f32 = 16.0;
/// A line's least height, and the space under it.
pub const LINE: f32 = 20.0;
pub const LINE_GAP: f32 = 4.0;

/// Where a piece of the overlay's text goes, in the body's coordinates
/// before scrolling.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Placed<'a> {
    Heading(&'a str, crate::layout::Rect),
    Keys(&'a str, crate::layout::Rect),
    What(&'a str, crate::layout::Rect),
}

/// Lay the sections out in a body `width` wide. `height` says how tall a
/// piece of text runs when wrapped to a width. Returns the pieces and the
/// whole height, for scrolling.
pub fn place<'a>(
    sections: &'a [HelpSection],
    width: f32,
    height: impl Fn(&str, f32) -> f32,
) -> (Vec<Placed<'a>>, f32) {
    use crate::layout::Rect;
    let mut out = vec![];
    let mut y = 0.0;
    for (k, s) in sections.iter().enumerate() {
        if k > 0 {
            y += SECTION_GAP;
        }
        out.push(Placed::Heading(s.title, Rect::new(0.0, y, width, HEADING)));
        y += HEADING;
        for l in &s.lines {
            let (x, w) = if l.keys.is_empty() {
                (0.0, width)
            } else {
                let x = KEYS_WIDTH + KEYS_GAP;
                (x, (width - x).max(0.0))
            };
            let h = height(&l.what, w).ceil().max(LINE);
            if !l.keys.is_empty() {
                out.push(Placed::Keys(&l.keys, Rect::new(0.0, y, KEYS_WIDTH, LINE)));
            }
            out.push(Placed::What(&l.what, Rect::new(x, y, w, h)));
            y += h + LINE_GAP;
        }
    }
    (out, y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Modifier;

    fn find<'a>(sections: &'a [HelpSection], title: &str) -> &'a HelpSection {
        sections.iter().find(|s| s.title == title).unwrap()
    }

    fn keys_of(s: &HelpSection) -> Vec<&str> {
        s.lines.iter().map(|l| l.keys.as_str()).collect()
    }

    #[test]
    fn help_has_the_ways_in_results_and_shortcuts() {
        let h = help_content(&KeyMap::default());
        let titles: Vec<_> = h.iter().map(|s| s.title).collect();
        assert_eq!(
            titles,
            [
                "Ways in",
                "Results",
                "Keyboard shortcuts",
                "In a wheel",
                "In panels"
            ]
        );
        assert_eq!(
            keys_of(find(&h, "Ways in")),
            [
                "Drop",
                "Ctrl+O",
                "Ctrl+V",
                "Right-click",
                "Hold Shift and drop",
                "Hold Ctrl and drop",
                "Hold Alt and drop"
            ]
        );
        let results = find(&h, "Results");
        assert_eq!(
            results.lines[0].what,
            "Results wait in Kiln until you export them. Nothing is saved until you do."
        );
        assert_eq!(
            keys_of(results),
            ["", "Ctrl+S", "Ctrl+Shift+S", "Double-click"]
        );
        assert!(results.lines[2].what.starts_with("Export all"));
    }

    #[test]
    fn the_table_lists_the_keys_the_window_handles() {
        let h = help_content(&KeyMap::default());
        // The table's content, pinned. Nothing here ties it to the
        // handlers: each entry was checked by hand against `App::key`,
        // `WM_CONTEXTMENU` and `WM_LBUTTONDBLCLK`, and a key added there
        // needs a line here and in `help_content`.
        assert_eq!(
            keys_of(find(&h, "Keyboard shortcuts")),
            [
                "Enter",
                "Ctrl+Enter",
                "Alt+Enter",
                "Menu key or Shift+F10",
                "Up, Down, Home, End",
                "Shift+Up, Shift+Down",
                "Shift+Home, Shift+End",
                "Ctrl+A",
                "Escape",
                "Space",
                "Delete or Backspace",
                "Tab, Shift+Tab",
                "Ctrl+,",
                "F1"
            ]
        );
        assert_eq!(
            keys_of(find(&h, "In a wheel")),
            ["Left, Right, Up, Down", "Tab", "Enter or Space", "Escape"]
        );
        // Space presses in every panel; Enter does something different in
        // each, so it has a line of its own.
        assert_eq!(
            keys_of(find(&h, "In panels")),
            [
                "Tab, Shift+Tab",
                "Left, Right",
                "Space",
                "Enter",
                "Page Up, Page Down",
                "Ctrl+C",
                "Escape"
            ]
        );
    }

    #[test]
    fn help_follows_the_wheel_keys() {
        let mut keys = KeyMap::default();
        keys.assign(WheelKind::Tools, Modifier::Shift);
        let h = help_content(&keys);
        let ways = keys_of(find(&h, "Ways in"));
        assert_eq!(
            ways[4..],
            [
                "Hold Ctrl and drop",
                "Hold Shift and drop",
                "Hold Alt and drop"
            ]
        );
        let table = keys_of(find(&h, "Keyboard shortcuts"));
        assert_eq!(table[..3], ["Enter", "Shift+Enter", "Alt+Enter"]);
    }

    #[test]
    fn tooltips_name_their_shortcuts() {
        assert_eq!(SETTINGS_TIP, "Settings (Ctrl+,)");
        assert_eq!(HELP_TIP, "Help (F1)");
        assert_eq!(STOP_TIP, "Stop");
        assert_eq!(EXPORT_TIP, "Export (Ctrl+S)");
        assert_eq!(ADD_FILES_TIP, "Add files (Ctrl+O)");
        assert_eq!(EXPORT_ALL_TIP, "Export all (Ctrl+Shift+S)");
        // Each shortcut a tip names is one the help table lists.
        let h = help_content(&KeyMap::default());
        let listed = |k: &str| h.iter().flat_map(|s| &s.lines).any(|l| l.keys == k);
        for k in ["Ctrl+,", "F1", "Ctrl+S", "Ctrl+O", "Ctrl+Shift+S"] {
            assert!(listed(k), "{k}");
        }
    }

    #[test]
    fn help_reads_calmly() {
        for s in help_content(&KeyMap::default()) {
            for l in &s.lines {
                assert!(l.what.ends_with('.'), "{}", l.what);
                assert!(!l.what.contains('!'), "{}", l.what);
                assert!(l.what.chars().next().unwrap().is_uppercase());
            }
        }
    }

    #[test]
    fn the_keys_fit_their_column() {
        // DirectWrite widths of the longest keys in 14 px Segoe UI.
        const WIDEST: [(&str, f32); 5] = [
            ("Shift+Home, Shift+End", 142.5),
            ("Menu key or Shift+F10", 138.5),
            ("Shift+Up, Shift+Down", 135.7),
            ("Up, Down, Home, End", 135.3),
            ("Left, Right, Up, Down", 130.2),
        ];
        let h = help_content(&KeyMap::default());
        for (keys, w) in WIDEST {
            assert!(h.iter().flat_map(|s| &s.lines).any(|l| l.keys == keys));
            assert!(w <= KEYS_WIDTH, "{keys}");
        }
        // The rest are shorter, by length.
        for l in h.iter().flat_map(|s| &s.lines) {
            assert!(
                l.keys.chars().count() <= "Shift+Home, Shift+End".chars().count(),
                "{}",
                l.keys
            );
        }
    }

    #[test]
    fn lines_stack_and_notes_run_the_width() {
        let h = help_content(&KeyMap::default());
        // A fake measure: 20 per line at 7 DIPs a character.
        let height = |s: &str, w: f32| {
            let per = (w / 7.0).floor().max(1.0) as usize;
            20.0 * s.chars().count().div_ceil(per) as f32
        };
        for width in [384.0, 528.0] {
            let (placed, total) = place(&h, width, height);
            let mut bottom = 0.0f32;
            for p in &placed {
                let r = match p {
                    Placed::Heading(_, r) | Placed::What(_, r) => *r,
                    Placed::Keys(_, r) => *r,
                };
                assert!(r.x >= 0.0 && r.right() <= width, "{p:?}");
                assert!(r.h >= LINE);
                bottom = bottom.max(r.bottom());
            }
            assert!(total >= bottom);
            let note = placed
                .iter()
                .find_map(|p| match p {
                    Placed::What(s, r) if s.starts_with("Results wait") => Some(*r),
                    _ => None,
                })
                .unwrap();
            assert_eq!((note.x, note.w), (0.0, width));
            // Each piece starts below the one before it, keys beside what.
            let ys: Vec<f32> = placed
                .iter()
                .filter_map(|p| match p {
                    Placed::Heading(_, r) | Placed::What(_, r) => Some(r.y),
                    _ => None,
                })
                .collect();
            assert!(ys.windows(2).all(|w| w[0] < w[1]));
        }
        // The smallest window's body can't hold it all: it scrolls.
        let body = crate::layout::help(480.0, 360.0).body;
        let (_, total) = place(&h, body.w, height);
        assert!(total > body.h);
    }
}
