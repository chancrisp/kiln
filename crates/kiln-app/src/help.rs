//! Help's words, as the mock has them: Convert files, Shortcuts, then
//! Show first-run help. Pure, so the shortcut list can be checked against
//! the keys the window handles and the tooltips that name them, and where
//! each line goes can be tested without a window.

use crate::ways_in::MenuItem;
use crate::wheel::WheelKind;

/// The dialog's title, and the Help button's name.
pub const TITLE: &str = "Help";

/// Its two sections' headings.
pub const CONVERT_FILES: &str = "Convert files";
pub const SHORTCUTS: &str = "Shortcuts";

/// Tooltips for the controls drawn as icons, and for the buttons with a
/// shortcut, each naming its shortcut.
pub const SETTINGS_TIP: &str = "Settings (Ctrl+,)";
pub const HELP_TIP: &str = "Help (F1)";
pub const STOP_TIP: &str = "Cancel";
pub const ADD_FILES_TIP: &str = "Add files (Ctrl+O)";
pub const CONVERT_TIP: &str = "Convert the selected files (Enter)";

/// One part of help, under its heading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpSection {
    pub title: &'static str,
    pub lines: Vec<HelpLine>,
}

/// One line: the keys or gesture on the left, what it does on the right.
/// A line with no keys is a note that runs the dialog's width.
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

/// Convert files, when the new file is saved next to the original: the
/// mock's words.
const CONVERT_BESIDE: &str = "Drop files on the window, or choose Add files, then pick a format. The new file is saved next to the original.";

/// Convert files, when the new file is saved in the folder chosen in
/// Settings.
const CONVERT_CHOSEN: &str = "Drop files on the window, or choose Add files, then pick a format. The new file is saved in the folder chosen in Settings.";

/// Where results go, as Help explains it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Results {
    /// Each is saved next to its original as soon as it's ready.
    Beside,
    /// Each is saved in the folder chosen in Settings as soon as it's
    /// ready (or written there, without a holding folder).
    Chosen,
    /// This run has no holding folder, so they are saved next to each file.
    NoHolding,
}

impl Results {
    /// Where results go, given whether this run has a holding folder and
    /// `settings`: the folder chosen in Settings, or next to the original.
    pub fn of(holding: bool, settings: &crate::settings::Settings) -> Self {
        match (holding, settings.save_to.is_some()) {
            (_, true) => Results::Chosen,
            (true, false) => Results::Beside,
            (false, false) => Results::NoHolding,
        }
    }
}

/// Everything help says. `results` says where results go: Convert files
/// says the same.
pub fn help_content(results: Results) -> Vec<HelpSection> {
    let tools_key = MenuItem::Wheel(WheelKind::Tools).parts().1;
    let shortcuts = [
        Some(line("Enter", "Convert the selected files")),
        Some(line("Delete", "Remove from the list")),
        Some(line(tools_key, "Tools for the selected files")),
        Some(line("Ctrl+O", "Add files")),
        Some(line("Ctrl+Z", "Undo remove")),
        Some(line("Ctrl+, (comma)", "Settings")),
        Some(line("F1", "Help")),
    ];
    vec![
        HelpSection {
            title: CONVERT_FILES,
            lines: vec![line(
                "",
                match results {
                    Results::Chosen => CONVERT_CHOSEN,
                    Results::Beside | Results::NoHolding => CONVERT_BESIDE,
                },
            )],
        },
        HelpSection {
            title: SHORTCUTS,
            lines: shortcuts.into_iter().flatten().collect(),
        },
    ]
}

/// The width of the keys column: the widest keys in Body
/// (`render::help`'s test measures them).
pub const KEYS_WIDTH: f32 = 152.0;
/// The gap between the keys and what they do (the mock's grid gap).
pub const KEYS_GAP: f32 = 16.0;
/// A section heading's height (Body Strong), and the space under it.
pub const HEADING: f32 = 20.0;
pub const HEADING_GAP: f32 = 6.0;
/// The space under a section, before the next one or the link.
pub const SECTION_GAP: f32 = 12.0;
/// A line's least height, and the space between lines.
pub const LINE: f32 = 20.0;
pub const LINE_GAP: f32 = 4.0;
/// How far Show first-run help's focus ring stands off its words: the
/// body scrolls on past the link by this much, so the ring shows whole.
pub const LINK_RING: f32 = crate::layout::kit::FOCUS_OFF + 2.0;

/// Where a piece of help goes, in the content's coordinates before
/// scrolling.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Placed<'a> {
    Heading(&'a str, crate::layout::Rect),
    Keys(&'a str, crate::layout::Rect),
    What(&'a str, crate::layout::Rect),
    /// A paragraph with no keys beside it, across the content: the body
    /// text, in the primary colour.
    Note(&'a str, crate::layout::Rect),
    /// Show first-run help, after the last section.
    Link(crate::layout::Rect),
}

/// Lay the sections out in content `width` wide, then Show first-run help,
/// its words `link_w` wide. `height` says how tall a piece of text runs
/// when wrapped to a width. Returns the pieces and the whole height, for
/// scrolling: it runs on past the link as far as its focus ring.
pub fn place<'a>(
    sections: &'a [HelpSection],
    width: f32,
    link_w: f32,
    height: impl Fn(&str, f32) -> f32,
) -> (Vec<Placed<'a>>, f32) {
    use crate::layout::Rect;
    let mut out = vec![];
    let mut y = 0.0;
    for s in sections {
        out.push(Placed::Heading(s.title, Rect::new(0.0, y, width, HEADING)));
        y += HEADING + HEADING_GAP;
        for (k, l) in s.lines.iter().enumerate() {
            if k > 0 {
                y += LINE_GAP;
            }
            let (x, w) = if l.keys.is_empty() {
                (0.0, width)
            } else {
                let x = KEYS_WIDTH + KEYS_GAP;
                (x, (width - x).max(0.0))
            };
            let h = height(&l.what, w).ceil().max(LINE);
            if !l.keys.is_empty() {
                out.push(Placed::Keys(
                    &l.keys,
                    Rect::new(0.0, y, KEYS_WIDTH.min(width), LINE),
                ));
            }
            let at = Rect::new(x, y, w, h);
            out.push(if l.keys.is_empty() {
                Placed::Note(&l.what, at)
            } else {
                Placed::What(&l.what, at)
            });
            y += h;
        }
        y += SECTION_GAP;
    }
    out.push(Placed::Link(Rect::new(0.0, y, link_w.min(width), LINE)));
    (out, y + LINE + LINK_RING)
}

/// Show first-run help's place in the content, before scrolling.
pub fn link_place(placed: &[Placed]) -> Option<crate::layout::Rect> {
    placed.iter().find_map(|p| match p {
        Placed::Link(r) => Some(*r),
        _ => None,
    })
}

/// Where each line of help sits, as (section, line, rectangle in the
/// content before scrolling), for telling assistive tools which lines
/// show.
pub fn line_places(
    sections: &[HelpSection],
    width: f32,
    height: impl Fn(&str, f32) -> f32,
) -> Vec<(usize, usize, crate::layout::Rect)> {
    let (placed, _) = place(sections, width, 0.0, height);
    let mut out = vec![];
    let (mut section, mut line) = (None::<usize>, 0);
    for p in &placed {
        match p {
            Placed::Heading(..) => {
                section = Some(section.map_or(0, |k| k + 1));
                line = 0;
            }
            Placed::Keys(..) | Placed::Link(_) => {}
            Placed::What(_, r) | Placed::Note(_, r) => {
                if let Some(k) = section {
                    out.push((k, line, *r));
                }
                line += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find<'a>(sections: &'a [HelpSection], title: &str) -> &'a HelpSection {
        sections
            .iter()
            .find(|s| s.title == title)
            .unwrap_or_else(|| panic!("no section {title}"))
    }

    fn keys_of(s: &HelpSection) -> Vec<&str> {
        s.lines.iter().map(|l| l.keys.as_str()).collect()
    }

    #[test]
    fn help_is_convert_files_then_shortcuts_as_the_mock_has_it() {
        let h = help_content(Results::Beside);
        let titles: Vec<_> = h.iter().map(|s| s.title).collect();
        assert_eq!(titles, ["Convert files", "Shortcuts"]);
        assert_eq!(
            find(&h, CONVERT_FILES).lines,
            [line(
                "",
                "Drop files on the window, or choose Add files, then pick a format. The new file is saved next to the original."
            )]
        );
        // The shortcuts start with Enter and Delete.
        let shortcuts = find(&h, SHORTCUTS);
        assert_eq!(
            shortcuts.lines[..2],
            [
                line("Enter", "Convert the selected files"),
                line("Delete", "Remove from the list"),
            ]
        );
    }

    #[test]
    fn the_shortcuts_list_every_key_a_tooltip_names_and_settings() {
        let h = help_content(Results::Beside);
        assert_eq!(
            keys_of(find(&h, SHORTCUTS)),
            [
                "Enter",
                "Delete",
                "Ctrl+Enter",
                "Ctrl+O",
                "Ctrl+Z",
                "Ctrl+, (comma)",
                "F1",
            ]
        );
        let what = |k: &str| {
            find(&h, SHORTCUTS)
                .lines
                .iter()
                .find(|l| l.keys == k)
                .map(|l| l.what.clone())
                .unwrap()
        };
        assert_eq!(what("Ctrl+Enter"), "Tools for the selected files");
        assert_eq!(what("Ctrl+O"), "Add files");
        assert_eq!(what("Ctrl+Z"), "Undo remove");
        assert_eq!(what("Ctrl+, (comma)"), "Settings");
        assert_eq!(what("F1"), "Help");
    }

    #[test]
    fn help_never_lists_save() {
        // Every result is saved as it is ready: no Save lines.
        for results in [Results::Beside, Results::NoHolding, Results::Chosen] {
            let h = help_content(results);
            assert_ne!(find(&h, CONVERT_FILES).lines[0].what, "");
            let keys = keys_of(find(&h, SHORTCUTS));
            assert!(!keys.contains(&"Ctrl+S") && !keys.contains(&"Ctrl+Shift+S"));
            assert!(keys.contains(&"Ctrl+, (comma)") && keys.contains(&"F1"));
        }
    }

    #[test]
    fn tooltips_name_their_shortcuts() {
        assert_eq!(SETTINGS_TIP, "Settings (Ctrl+,)");
        assert_eq!(HELP_TIP, "Help (F1)");
        assert_eq!(STOP_TIP, "Cancel");
        assert_eq!(ADD_FILES_TIP, "Add files (Ctrl+O)");
        assert_eq!(CONVERT_TIP, "Convert the selected files (Enter)");
        // Each shortcut a tip names is one help lists.
        let h = help_content(Results::Beside);
        let listed = |k: &str| h.iter().flat_map(|s| &s.lines).any(|l| l.keys == k);
        // The comma is spelled out in Help, so it isn't read as a typo.
        for k in ["Ctrl+, (comma)", "F1", "Ctrl+O", "Enter"] {
            assert!(listed(k), "{k}");
        }
    }

    #[test]
    fn results_go_where_the_setting_says() {
        use crate::settings::Settings;
        // A new install, and an old file that held results: next to the
        // original.
        assert_eq!(Results::of(true, &Settings::default()), Results::Beside);
        let old = Settings::parse("save_next_to_original = false\n").unwrap();
        assert_eq!(Results::of(true, &old), Results::Beside);
        // A folder chosen in Settings: saved there.
        let chosen = Settings {
            save_to: Some(std::path::PathBuf::from(r"D:\Converted")),
            ..Settings::default()
        };
        assert_eq!(Results::of(true, &chosen), Results::Chosen);
        assert_eq!(Results::of(false, &chosen), Results::Chosen);
        assert!(help_content(Results::Chosen)[0].lines[0]
            .what
            .ends_with("The new file is saved in the folder chosen in Settings."));
        // No holding folder: saved next to each file.
        assert_eq!(Results::of(false, &Settings::default()), Results::NoHolding);
    }
    #[test]
    fn help_reads_calmly() {
        for results in [Results::Beside, Results::Chosen, Results::NoHolding] {
            for s in help_content(results) {
                for l in &s.lines {
                    // The mock's shortcut lines are phrases; a note is a
                    // sentence.
                    assert_eq!(l.what.ends_with('.'), l.keys.is_empty(), "{}", l.what);
                    assert!(!l.what.contains('!'), "{}", l.what);
                    assert!(!l.what.contains("  ") && !l.keys.contains("  "));
                    assert!(l.what.chars().next().unwrap().is_uppercase());
                }
            }
        }
    }

    #[test]
    fn no_key_is_longer_than_the_widest_measured() {
        // `render::help`'s test measures every key in Body against
        // KEYS_WIDTH; here, none is longer than the longest it holds.
        for results in [Results::Chosen, Results::Beside] {
            for l in help_content(results).iter().flat_map(|s| &s.lines) {
                assert!(l.keys.chars().count() <= "Ctrl+, (comma)".len());
            }
        }
    }

    /// A fake measure: 20 per line at 7 DIPs a character.
    fn fake(s: &str, w: f32) -> f32 {
        let per = (w / 7.0).floor().max(1.0) as usize;
        20.0 * s.chars().count().div_ceil(per) as f32
    }

    #[test]
    fn headings_then_lines_then_the_link_stack_down_the_content() {
        let h = help_content(Results::Beside);
        for width in [300.0, 432.0] {
            let (placed, total) = place(&h, width, 120.0, fake);
            // Heading, the note; heading, then keys beside what for each
            // shortcut; then the link.
            let shortcuts = h[1].lines.len();
            assert_eq!(placed.len(), 2 + 1 + 2 * shortcuts + 1);
            assert!(matches!(placed[0], Placed::Heading("Convert files", _)));
            assert!(matches!(placed[2], Placed::Heading("Shortcuts", _)));
            let Some(Placed::Link(link)) = placed.last() else {
                panic!("the link comes last")
            };
            assert_eq!((link.x, link.w, link.h), (0.0, 120.0, LINE));
            // The body runs on past the link as far as its focus ring.
            assert_eq!(total, link.bottom() + LINK_RING);
            let mut bottom = 0.0f32;
            for p in &placed {
                let r = match p {
                    Placed::Heading(_, r)
                    | Placed::Keys(_, r)
                    | Placed::What(_, r)
                    | Placed::Note(_, r) => *r,
                    Placed::Link(r) => *r,
                };
                assert!(r.x >= 0.0 && r.right() <= width, "{p:?}");
                assert!(r.y >= bottom - LINE - 1.0, "{p:?}");
                bottom = bottom.max(r.bottom());
            }
            // The note runs the width; keys sit left of what they do.
            let Placed::Note(_, note) = placed[1] else {
                panic!()
            };
            assert_eq!((note.x, note.w), (0.0, width));
            assert_eq!(note.y, HEADING + HEADING_GAP);
            let (Placed::Keys(_, k), Placed::What(_, w)) = (placed[3], placed[4]) else {
                panic!()
            };
            assert_eq!(k.y, w.y);
            assert_eq!((k.x, k.w), (0.0, KEYS_WIDTH));
            assert_eq!(w.x, KEYS_WIDTH + KEYS_GAP);
            assert_eq!(w.right(), width);
            // A section's gap before the next heading and the link.
            let Placed::Heading(_, second) = placed[2] else {
                panic!()
            };
            assert_eq!(second.y, note.bottom() + SECTION_GAP);
        }
        // A link wider than the content is cut to it.
        let (placed, _) = place(&h, 100.0, 300.0, fake);
        assert_eq!(link_place(&placed).map(|r| r.w), Some(100.0));
    }

    #[test]
    fn every_line_of_help_has_a_place_in_order() {
        let h = help_content(Results::Beside);
        let places = line_places(&h, 400.0, |_, _| 18.0);
        let lines: usize = h.iter().map(|s| s.lines.len()).sum();
        assert_eq!(places.len(), lines);
        assert_eq!(places[0].0, 0);
        assert_eq!(places.last().map(|p| p.0), Some(h.len() - 1));
        assert!(places.windows(2).all(|w| w[1].2.y >= w[0].2.y + w[0].2.h));
    }
}
