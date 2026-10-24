//! The ways into Kiln besides a drop: the right-click menu, the footer's
//! idle hint, the empty window's lines and the wheel's title. Pure, so the
//! copy and the rules for what shows can be tested anywhere.

use std::path::Path;
use std::time::Duration;

use crate::model::{Entry, JobState};
use crate::settings::{KeyMap, Settings};
use crate::wheel::Action;
use crate::wheel::WheelKind;

/// A row as the right-click menu sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowState {
    /// Nothing to export, show or stop: idle, failed or cancelled, a
    /// running Move, or a held result that's being saved or has gone.
    Plain,
    /// Converting, or running a tool, which can be stopped.
    Running,
    /// Holds results that can be exported or opened.
    Held,
    /// Its results were exported to a folder.
    Saved,
    /// Cancelled, or failed in a way that trying again can fix.
    Retryable,
}

impl Entry {
    /// What the right-click menu offers for this row. `saving` says an
    /// export of it is under way; `exists` whether a path is still on disk.
    pub fn row_state(&self, saving: bool, exists: impl Fn(&Path) -> bool) -> RowState {
        if self.retryable() {
            return RowState::Retryable;
        }
        if let JobState::Running { action, .. } = &self.job {
            // A Move runs on its own and can't be stopped.
            return if matches!(action, Action::File(_)) {
                RowState::Plain
            } else {
                RowState::Running
            };
        }
        if self.held().is_some() {
            if saving || self.is_gone(exists) {
                RowState::Plain
            } else {
                RowState::Held
            }
        } else if self.saved().is_some() && !saving {
            RowState::Saved
        } else {
            RowState::Plain
        }
    }
}

/// One line of a right-click menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuItem {
    /// Opens that wheel at the row, for the selection.
    Wheel(WheelKind),
    Export,
    /// Save one held result under a name of its own.
    ExportAs,
    Open,
    /// Opens Explorer on a saved result, or else on the file's own folder.
    ShowInFolder,
    /// Puts the selected files' paths on the clipboard.
    CopyPath,
    /// Cancels a running job and keeps its row.
    Stop,
    /// Starts the same job again on a cancelled or failed row.
    Retry,
    Remove,
    AddFiles,
    Paste,
    Separator,
}

impl MenuItem {
    /// The item's text and its shortcut, apart: ("Tools…", "Ctrl+Enter").
    /// The shortcut is empty where there isn't one. A wheel's shortcut
    /// follows the wheel keys: plain Enter always opens Convert, and a
    /// wheel's own key with Enter opens it.
    pub fn parts(self, keys: &KeyMap) -> (&'static str, String) {
        let enter = |w: WheelKind| match w {
            WheelKind::Convert => "Enter".to_owned(),
            _ => format!("{}+Enter", keys.key_for(w).name()),
        };
        match self {
            MenuItem::Wheel(w) => (
                match w {
                    WheelKind::Convert => "Convert…",
                    WheelKind::Tools => "Tools…",
                    WheelKind::Actions => "File actions…",
                },
                enter(w),
            ),
            MenuItem::Export => ("Export…", "Ctrl+S".to_owned()),
            MenuItem::ExportAs => ("Export as…", String::new()),
            MenuItem::Open => ("Open", String::new()),
            MenuItem::ShowInFolder => ("Show in folder", String::new()),
            MenuItem::CopyPath => ("Copy path", String::new()),
            MenuItem::Stop => ("Cancel", String::new()),
            MenuItem::Retry => ("Retry", String::new()),
            MenuItem::Remove => ("Remove", "Delete".to_owned()),
            MenuItem::AddFiles => ("Add files…", "Ctrl+O".to_owned()),
            MenuItem::Paste => ("Paste", "Ctrl+V".to_owned()),
            MenuItem::Separator => ("", String::new()),
        }
    }

    /// The text a Windows menu takes: the label, then a tab and the
    /// shortcut, which Windows lines up on the right.
    pub fn text(self, keys: &KeyMap) -> String {
        match self.parts(keys) {
            (label, key) if key.is_empty() => label.to_owned(),
            (label, key) => format!("{label}\t{key}"),
        }
    }
}

/// The menu for the rows `selection` (one state per selected row), given
/// the wheels that have anything for them. Items that don't apply are
/// left out, never greyed: the wheels, then Export and Open for a held
/// result, Cancel for a running one or Retry for a cancelled or failed
/// one, Show in folder for one row and Copy path for any, then
/// Remove, with a separator between groups that are there. Empty without a
/// selection.
pub fn menu_items(selection: &[RowState], wheels: &[WheelKind]) -> Vec<MenuItem> {
    if selection.is_empty() {
        return vec![];
    }
    let one = |s: RowState| selection.len() == 1 && selection[0] == s;
    let open: Vec<MenuItem> = WheelKind::ALL
        .into_iter()
        .filter(|w| wheels.contains(w))
        .map(MenuItem::Wheel)
        .collect();
    let mut results = vec![];
    if selection.contains(&RowState::Held) {
        results.push(MenuItem::Export);
    }
    if one(RowState::Held) {
        results.push(MenuItem::ExportAs);
        results.push(MenuItem::Open);
    }
    if selection.contains(&RowState::Running) {
        results.push(MenuItem::Stop);
    }
    if one(RowState::Retryable) {
        results.push(MenuItem::Retry);
    }
    // Where the file is, for any selection: Show in folder for one row (its
    // saved result, if it has one), Copy path for any number.
    if selection.len() == 1 {
        results.push(MenuItem::ShowInFolder);
    }
    results.push(MenuItem::CopyPath);
    let mut items = vec![];
    for group in [open, results, vec![MenuItem::Remove]] {
        if group.is_empty() {
            continue;
        }
        if !items.is_empty() {
            items.push(MenuItem::Separator);
        }
        items.extend(group);
    }
    items
}

/// The menu for an empty part of the list: Add files, and Paste while the
/// clipboard holds files.
pub fn list_menu(can_paste: bool) -> Vec<MenuItem> {
    let mut items = vec![MenuItem::AddFiles];
    if can_paste {
        items.push(MenuItem::Paste);
    }
    items
}

/// What the footer says when files are listed and nothing more pressing
/// is going on.
pub const IDLE_HINT: &str = "Right-click a file, or select it and press Enter, to convert it.";

/// The idle hint where the full one doesn't fit the footer.
pub const IDLE_HINT_SHORT: &str = "Right-click a file to convert it.";

/// The idle hint for a status area `available` DIPs wide, given the full
/// hint measures `full` there: the full one when it fits, else the short.
pub fn fit_hint(available: f32, full: f32) -> &'static str {
    if full <= available {
        IDLE_HINT
    } else {
        IDLE_HINT_SHORT
    }
}

/// The one-time tip after a plain drop, naming the keys that choose what
/// to do with a drop as the settings have them.
pub fn drop_tip(keys: &crate::settings::KeyMap) -> String {
    use crate::wheel::WheelKind::*;
    format!(
        "Tip: hold {}, {} or {} as you drop.",
        keys.key_for(Convert).name(),
        keys.key_for(Tools).name(),
        keys.key_for(Actions).name()
    )
}

/// Whether a drop that `added` files, with no key held, shows the tip:
/// once, and only when it brought files in. `shown` says it has been.
pub fn show_drop_tip(added: usize, shown: bool) -> bool {
    added > 0 && !shown
}

/// How long "Added 2 files." stays before the idle hint takes its place.
pub const ADDED_FOR: Duration = Duration::from_secs(2);

/// Whether an add's line gives way to the idle hint after [`ADDED_FOR`]:
/// only the plain "Added 2 files.". A line that also says files were
/// already listed or skipped stays until something else replaces it.
pub fn hint_follows(added: usize, duplicates: usize, skipped: usize) -> bool {
    added > 0 && duplicates == 0 && skipped == 0
}

/// How much longer to wait before the idle hint can show, `elapsed` after
/// the add. A timer can fire a little early, so it's asked again then.
pub fn hint_wait(elapsed: Duration) -> Option<Duration> {
    ADDED_FOR
        .checked_sub(elapsed)
        .filter(|rest| !rest.is_zero())
}

/// What Ctrl+V says when the clipboard holds no files.
pub const NOTHING_TO_PASTE: &str = "There are no files to paste.";

/// The footer's idle hint, when it should take the place of `status`:
/// `listed` files are listed, nothing is `running`, and the status says
/// no more than the plain count (`plain`), or is what adding files said
/// (`added` is how long ago, set only for a plain "Added 2 files.") and
/// has been up for two seconds.
pub fn idle_hint(
    listed: usize,
    running: bool,
    status: &str,
    plain: &str,
    added: Option<Duration>,
) -> Option<&'static str> {
    let quiet = status.is_empty() || status == plain || added.is_some_and(|t| t >= ADDED_FOR);
    (listed > 0 && !running && quiet).then_some(IDLE_HINT)
}

/// What the footer says, given the `line` it would say otherwise: while
/// several rows are selected, their count (`selected`), unless a job or
/// export is `running` or the line is an `alert`, something that went
/// wrong or didn't happen. A summary gives way to the count and comes
/// back once the selection is one row or none.
pub fn footer_line<'a>(
    line: &'a str,
    selected: Option<&'a str>,
    running: bool,
    alert: bool,
) -> &'a str {
    match selected {
        Some(count) if !running && !alert => count,
        _ => line,
    }
}

/// What the footer adds to the line about the work under way while the
/// selection holds a file that is converting.
pub const CANCEL_HINT: &str = "Space cancels the selected file.";

/// The footer's `status`, with [`CANCEL_HINT`] after it while it is still
/// `working`, the line last said about the work under way, and the
/// selection holds a file that is converting (`selected_running`). The
/// hint follows the selection: it comes and goes as rows are selected, and
/// never stays on a line about something else. The selection is looked
/// through only while the status is about the work.
pub fn with_cancel_hint(
    status: &str,
    working: Option<&str>,
    selected_running: impl FnOnce() -> bool,
) -> String {
    if working == Some(status) && selected_running() {
        format!("{status} {CANCEL_HINT}")
    } else {
        status.to_owned()
    }
}

/// The empty window's lines, top to bottom, with the Add files button
/// between the instruction and the key hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmptyLines {
    pub headline: &'static str,
    pub instruction: &'static str,
    pub button: &'static str,
    /// Which keys to hold while dropping, as they are set now.
    pub keys: String,
    pub privacy: &'static str,
}

/// What Kiln does, in one line: the last thing Help says.
pub const TAGLINE: &str = "Fire raw files into finished ones.";

pub fn empty_lines(settings: &Settings) -> EmptyLines {
    EmptyLines {
        headline: "Drop files here",
        instruction: "Or add them with the button below, or Ctrl+O.",
        button: ADD_FILES,
        keys: settings.keys.hint(),
        privacy: "Everything converts on this PC. Nothing is uploaded.",
    }
}

/// The Add files button, in the empty window and the footer.
pub const ADD_FILES: &str = "Add files";

/// The title of the Open dialog Add files shows.
pub const ADD_FILES_TITLE: &str = "Add files";

/// The wheel's title with the key that opens it: "Convert · Shift".
pub fn wheel_title(kind: WheelKind, keys: &KeyMap) -> String {
    format!("{} · {}", kind.title(), keys.key_for(kind).name())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::FileAction;
    use crate::model::JobState;
    use crate::settings::Modifier;
    use crate::wheel::Action;
    use std::path::PathBuf;

    use MenuItem::*;
    use RowState::*;
    use WheelKind::{Actions, Convert, Tools};

    const ALL: [WheelKind; 3] = [Convert, Tools, Actions];

    #[test]
    fn a_plain_row_offers_its_wheels_and_remove() {
        assert_eq!(
            menu_items(&[Plain], &ALL),
            [
                Wheel(Convert),
                Wheel(Tools),
                Wheel(Actions),
                Separator,
                ShowInFolder,
                CopyPath,
                Separator,
                Remove
            ]
        );
    }

    #[test]
    fn a_wheel_with_nothing_for_the_row_is_left_out() {
        assert_eq!(
            menu_items(&[Plain], &[Actions, Convert]),
            [
                Wheel(Convert),
                Wheel(Actions),
                Separator,
                ShowInFolder,
                CopyPath,
                Separator,
                Remove
            ]
        );
        assert_eq!(
            menu_items(&[Plain], &[]),
            [ShowInFolder, CopyPath, Separator, Remove]
        );
    }

    #[test]
    fn a_held_row_offers_export_and_open() {
        assert_eq!(
            menu_items(&[Held], &[Convert, Actions]),
            [
                Wheel(Convert),
                Wheel(Actions),
                Separator,
                Export,
                ExportAs,
                Open,
                ShowInFolder,
                CopyPath,
                Separator,
                Remove
            ]
        );
    }

    #[test]
    fn a_saved_row_offers_show_in_folder() {
        assert_eq!(
            menu_items(&[Saved], &[Actions]),
            [
                Wheel(Actions),
                Separator,
                ShowInFolder,
                CopyPath,
                Separator,
                Remove
            ]
        );
        assert_eq!(
            menu_items(&[Saved], &[]),
            [ShowInFolder, CopyPath, Separator, Remove]
        );
    }

    #[test]
    fn any_file_can_be_shown_or_have_its_path_copied() {
        // One row of any state: Show in folder and Copy path.
        for state in [Plain, Held, Saved, Retryable, Running] {
            let items = menu_items(&[state], &[]);
            assert!(items.contains(&ShowInFolder), "{state:?}");
            assert!(items.contains(&CopyPath), "{state:?}");
        }
        // Several rows: only Copy path, which takes every path.
        let items = menu_items(&[Plain, Plain], &[]);
        assert_eq!(items, [CopyPath, Separator, Remove]);
    }

    #[test]
    fn several_rows_export_together_but_open_one_at_a_time() {
        assert_eq!(
            menu_items(&[Held, Plain], &[Convert]),
            [
                Wheel(Convert),
                Separator,
                Export,
                CopyPath,
                Separator,
                Remove
            ]
        );
        assert_eq!(
            menu_items(&[Saved, Saved], &[]),
            [CopyPath, Separator, Remove]
        );
        assert!(menu_items(&[], &ALL).is_empty());
        // A cancelled row, or one that stopped, offers Retry; two rows don't.
        assert_eq!(
            menu_items(&[Retryable], &[Convert]),
            [
                Wheel(Convert),
                Separator,
                Retry,
                ShowInFolder,
                CopyPath,
                Separator,
                Remove
            ]
        );
        assert_eq!(
            menu_items(&[Retryable, Retryable], &[]),
            [CopyPath, Separator, Remove]
        );
    }

    #[test]
    fn a_running_row_offers_stop_above_remove() {
        assert_eq!(
            menu_items(&[Running], &[Actions]),
            [
                Wheel(Actions),
                Separator,
                Stop,
                ShowInFolder,
                CopyPath,
                Separator,
                Remove
            ]
        );
        assert_eq!(
            menu_items(&[Running], &[]),
            [Stop, ShowInFolder, CopyPath, Separator, Remove]
        );
        // Any running row in the selection can be stopped.
        assert_eq!(
            menu_items(&[Held, Running], &[]),
            [Export, Stop, CopyPath, Separator, Remove]
        );
    }

    #[test]
    fn the_empty_list_menu_pastes_only_with_files_to_paste() {
        assert_eq!(list_menu(true), [AddFiles, Paste]);
        assert_eq!(list_menu(false), [AddFiles]);
    }

    #[test]
    fn menu_text_carries_the_shortcut() {
        let keys = KeyMap::default();
        let text: Vec<String> = [
            Wheel(Convert),
            Wheel(Tools),
            Wheel(Actions),
            Export,
            Open,
            ShowInFolder,
            Stop,
            Remove,
            AddFiles,
            Paste,
        ]
        .into_iter()
        .map(|i| i.text(&keys))
        .collect();
        assert_eq!(
            text,
            [
                "Convert…\tEnter",
                "Tools…\tCtrl+Enter",
                "File actions…\tAlt+Enter",
                "Export…\tCtrl+S",
                "Open",
                "Show in folder",
                "Cancel",
                "Remove\tDelete",
                "Add files…\tCtrl+O",
                "Paste\tCtrl+V",
            ]
        );
    }

    #[test]
    fn wheel_shortcuts_follow_the_wheel_keys() {
        let mut keys = KeyMap::default();
        keys.assign(Tools, Modifier::Shift);
        assert_eq!(keys.convert, Modifier::Ctrl);
        assert_eq!(Wheel(Convert).text(&keys), "Convert…\tEnter");
        assert_eq!(Wheel(Tools).text(&keys), "Tools…\tShift+Enter");
        assert_eq!(Wheel(Actions).text(&keys), "File actions…\tAlt+Enter");
        // The shortcut opens the wheel it names.
        for w in [Tools, Actions] {
            let m = keys.key_for(w);
            let held = (
                m == Modifier::Shift,
                m == Modifier::Ctrl,
                m == Modifier::Alt,
            );
            assert_eq!(keys.wheel_for(held.0, held.1, held.2), Some(w));
        }
    }

    #[test]
    fn the_idle_hint_waits_for_added_to_be_read() {
        let plain = "2 files.";
        let added = "Added 2 files.";
        let hint = Some(IDLE_HINT);
        let ms = Duration::from_millis;
        assert_eq!(idle_hint(2, false, plain, plain, None), hint);
        assert_eq!(idle_hint(2, false, "", plain, None), hint);
        assert_eq!(idle_hint(2, false, added, plain, Some(ms(500))), None);
        assert_eq!(idle_hint(2, false, added, plain, Some(ms(2000))), hint);
        // Nothing listed, something running, or a status worth keeping.
        assert_eq!(idle_hint(0, false, "", "0 files.", None), None);
        assert_eq!(idle_hint(2, true, plain, plain, None), None);
        let done = "Converted 2 files to JPG. Export to keep them.";
        assert_eq!(idle_hint(2, false, done, plain, None), None);
    }

    #[test]
    fn only_a_plain_add_gives_way_to_the_hint() {
        use crate::model::status_line;
        assert!(hint_follows(2, 0, 0));
        assert!(!hint_follows(2, 1, 0));
        assert!(!hint_follows(2, 0, 1));
        assert!(!hint_follows(0, 1, 0));
        // Lines that say more than the count stay: the app only times an
        // add that gives way, so these come with no `added`.
        let plain = status_line(3, 0, 0, 0);
        for (a, d, s) in [(2, 1, 0), (2, 0, 1), (0, 1, 0), (0, 0, 1)] {
            assert!(!hint_follows(a, d, s));
            let said = status_line(3, a, d, s);
            assert_eq!(idle_hint(3, false, &said, &plain, None), None, "{said}");
        }
    }

    #[test]
    fn a_selection_count_stands_over_all_but_work_and_trouble() {
        let count = Some("3 selected");
        let done = "Converted 3 files to JPG. Export to keep them.";
        // Summaries, the hint and the standing warning give way.
        for line in [done, "Saved 3 files.", IDLE_HINT, crate::model::NO_HOLDING] {
            assert_eq!(footer_line(line, count, false, false), "3 selected");
            // And come back with one row selected or none.
            assert_eq!(footer_line(line, None, false, false), line);
        }
        // Work under way and lines about trouble stand.
        assert_eq!(
            footer_line("Working on 3 files.", count, true, false),
            "Working on 3 files."
        );
        assert_eq!(footer_line("Saving…", count, true, false), "Saving…");
        let failed = "Saved 2 of 3. 1 couldn't be saved.";
        assert_eq!(footer_line(failed, count, false, true), failed);
        assert_eq!(
            footer_line(NOTHING_TO_PASTE, count, false, true),
            NOTHING_TO_PASTE
        );
    }

    #[test]
    fn the_cancel_hint_follows_the_selection() {
        let working = "3 of 50 done.";
        // A converting file is selected while the footer says the work.
        assert_eq!(
            with_cancel_hint(working, Some(working), || true),
            "3 of 50 done. Space cancels the selected file."
        );
        // The selection moved off it, or was cleared: the hint goes.
        assert_eq!(with_cancel_hint(working, Some(working), || false), working);
        // Something else has been said since: the hint isn't added to it,
        // and the selection isn't looked through to find that out.
        let copied = "Copied the path.";
        let unasked = || -> bool { panic!("the selection was looked through") };
        assert_eq!(with_cancel_hint(copied, Some(working), unasked), copied);
        assert_eq!(with_cancel_hint(working, None, unasked), working);
        // A job's finish said with the count still counts as the work.
        let ready = "photo.jpg is ready. 13 of 50 done.";
        assert_eq!(
            with_cancel_hint(ready, Some(ready), || true),
            "photo.jpg is ready. 13 of 50 done. Space cancels the selected file."
        );
    }

    #[test]
    fn an_early_timer_waits_out_the_rest() {
        let ms = Duration::from_millis;
        assert_eq!(hint_wait(ms(0)), Some(ADDED_FOR));
        assert_eq!(hint_wait(ms(1990)), Some(ms(10)));
        assert_eq!(hint_wait(ms(2000)), None);
        assert_eq!(hint_wait(ms(2500)), None);
    }

    #[test]
    fn the_drop_tip_comes_once_and_names_the_keys() {
        let keys = crate::settings::KeyMap::default();
        assert_eq!(drop_tip(&keys), "Tip: hold Shift, Ctrl or Alt as you drop.");
        assert!(show_drop_tip(2, false));
        assert!(!show_drop_tip(0, false));
        assert!(!show_drop_tip(2, true));
    }

    #[test]
    fn the_full_hint_shows_only_where_it_fits() {
        // The caption widths DirectWrite measures for the two hints.
        let full = 303.0;
        assert_eq!(fit_hint(400.0, full), IDLE_HINT);
        assert_eq!(fit_hint(303.0, full), IDLE_HINT);
        assert_eq!(fit_hint(302.0, full), IDLE_HINT_SHORT);
        assert_eq!(fit_hint(0.0, full), IDLE_HINT_SHORT);
    }

    #[test]
    fn the_idle_hint_is_a_calm_sentence() {
        assert_eq!(
            IDLE_HINT,
            "Right-click a file, or select it and press Enter, to convert it."
        );
        assert_eq!(IDLE_HINT_SHORT, "Right-click a file to convert it.");
        assert_eq!(NOTHING_TO_PASTE, "There are no files to paste.");
        for s in [IDLE_HINT, IDLE_HINT_SHORT, NOTHING_TO_PASTE] {
            assert!(s.ends_with('.') && !s.contains('!'));
        }
    }

    #[test]
    fn the_empty_window_reads_in_order() {
        let mut settings = Settings::default();
        let l = empty_lines(&settings);
        assert_eq!(l.headline, "Drop files here");
        assert_eq!(
            l.instruction,
            "Or add them with the button below, or Ctrl+O."
        );
        assert_eq!(l.button, "Add files");
        assert_eq!(
            l.keys,
            "Hold Shift while dropping to convert, Ctrl for tools, or Alt for file actions."
        );
        settings.keys.assign(Convert, Modifier::Alt);
        assert_eq!(
            empty_lines(&settings).keys,
            "Hold Alt while dropping to convert, Ctrl for tools, or Shift for file actions."
        );
    }

    #[test]
    fn wheel_titles_name_their_key() {
        let mut keys = KeyMap::default();
        assert_eq!(wheel_title(Convert, &keys), "Convert · Shift");
        assert_eq!(wheel_title(Tools, &keys), "Tools · Ctrl");
        assert_eq!(wheel_title(Actions, &keys), "Actions · Alt");
        keys.assign(Actions, Modifier::Shift);
        assert_eq!(wheel_title(Actions, &keys), "Actions · Shift");
        assert_eq!(wheel_title(Convert, &keys), "Convert · Alt");
    }

    fn entry(job: JobState) -> Entry {
        Entry {
            path: PathBuf::from(r"C:\in\a.png"),
            name: "a.png".into(),
            folder: r"C:\in".into(),
            code: "PNG".into(),
            size: 1,
            modified: None,
            targets: vec![],
            job,
            facts: None,
            suggestion: None,
            facts_at: None,
            last: None,
            check: kiln_core::FileCheck::Ok,
            missing: None,
            save_failed: false,
        }
    }

    #[test]
    fn row_states_follow_the_job() {
        let out = PathBuf::from(r"C:\held\a.jpg");
        let held = entry(JobState::Done {
            action: Action::Convert("jpg".into()),
            outputs: vec![out.clone()],
            saved: None,
        });
        let there = |_: &Path| true;
        let gone = |_: &Path| false;
        assert_eq!(held.row_state(false, there), Held);
        assert_eq!(held.row_state(true, there), Plain);
        assert_eq!(held.row_state(false, gone), Plain);
        let saved = entry(JobState::Done {
            action: Action::Convert("jpg".into()),
            outputs: vec![out],
            saved: Some(crate::model::Saved::Folder(PathBuf::from(r"D:\out"))),
        });
        assert_eq!(saved.row_state(false, there), Saved);
        // A moved file isn't a result to show.
        let moved = entry(JobState::Done {
            action: Action::File(FileAction::Move),
            outputs: vec![],
            saved: None,
        });
        assert_eq!(moved.row_state(false, there), Plain);
        assert_eq!(entry(JobState::Idle).row_state(false, there), Plain);
        let converting = entry(JobState::Running {
            action: Action::Convert("jpg".into()),
            fraction: Some(0.5),
            started: None,
        });
        assert_eq!(converting.row_state(false, there), Running);
        // A Move can't be stopped, so it offers no Stop.
        let moving = entry(JobState::Running {
            action: Action::File(FileAction::Move),
            fraction: None,
            started: None,
        });
        assert_eq!(moving.row_state(false, there), Plain);
    }
}
