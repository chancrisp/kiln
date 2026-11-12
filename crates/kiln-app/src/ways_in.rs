//! The ways into Kiln besides a drop: the right-click menu, the footer's
//! idle hint, the empty window's lines and the wheel's title. Pure, so the
//! copy and the rules for what shows can be tested anywhere.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::model::{Entry, JobState};
use crate::wheel::{Offer, WheelKind};

/// A row as the right-click menu sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowState {
    /// Nothing to show or stop: idle, failed or cancelled, a running Move,
    /// or a result still on its way to its folder.
    Plain,
    /// Converting, or running a tool, which can be stopped.
    Running,
    /// Its results were saved to a folder.
    Saved,
    /// Cancelled, or failed in a way that trying again can fix.
    Retryable,
}

impl Entry {
    /// What the right-click menu offers for this row. `saving` says its
    /// result is on its way to its folder.
    pub fn row_state(&self, saving: bool) -> RowState {
        if self.retryable() {
            return RowState::Retryable;
        }
        if let JobState::Running { .. } = &self.job {
            return RowState::Running;
        }
        if self.saved().is_some() && !saving {
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
    /// Makes the selection smaller at once, keeping its format.
    Smaller,
    /// Opens Explorer on a saved result, or else on the file's own folder.
    ShowInFolder,
    /// Puts the selected files' paths on the clipboard.
    CopyPath,
    /// Cancels a running job and keeps its row.
    Stop,
    /// Starts the same job again on a cancelled or failed row.
    Retry,
    /// A finished drag-wheel conversion's options sheet, to redo it.
    Options,
    Remove,
    AddFiles,
    Paste,
    Separator,
}

impl MenuItem {
    /// The item's text and its shortcut, apart: ("Tools…", "Ctrl+Enter").
    /// The shortcut is empty where there isn't one. Plain Enter converts
    /// to the suggested format, so Convert to… has none; Ctrl+Enter opens
    /// the tools.
    pub fn parts(self) -> (&'static str, String) {
        let enter = |w: WheelKind| match w {
            WheelKind::Convert => String::new(),
            WheelKind::Tools => "Ctrl+Enter".to_owned(),
        };
        match self {
            MenuItem::Wheel(w) => (
                match w {
                    WheelKind::Convert => "Convert to…",
                    WheelKind::Tools => "Tools…",
                },
                enter(w),
            ),
            MenuItem::Smaller => ("Make smaller", String::new()),

            MenuItem::ShowInFolder => ("Show in folder", String::new()),
            MenuItem::CopyPath => ("Copy path", String::new()),
            MenuItem::Stop => ("Cancel", String::new()),
            MenuItem::Retry => ("Retry", String::new()),
            MenuItem::Options => ("Options", String::new()),
            MenuItem::Remove => ("Remove from list", "Del".to_owned()),
            MenuItem::AddFiles => ("Add files…", "Ctrl+O".to_owned()),
            MenuItem::Paste => ("Paste", "Ctrl+V".to_owned()),
            MenuItem::Separator => ("", String::new()),
        }
    }

    /// The glyph in front of the item, from Segoe Fluent Icons, as a Fluent
    /// menu has them. Tools…, Cancel, Options, Paste and the separator have
    /// none; the column is there for all of them while any has one.
    pub fn glyph(self) -> Option<char> {
        use crate::theme::fluent::glyph;
        match self {
            MenuItem::Wheel(WheelKind::Convert) => Some(glyph::CONVERT),
            MenuItem::Smaller => Some(glyph::MAKE_SMALLER),
            MenuItem::ShowInFolder => Some(glyph::SHOW_IN_FOLDER),
            MenuItem::CopyPath => Some(glyph::COPY_PATH),
            MenuItem::Retry => Some(glyph::RETRY),
            MenuItem::Remove => Some(glyph::REMOVE),
            MenuItem::AddFiles => Some(glyph::ADD_FILES),
            MenuItem::Wheel(WheelKind::Tools)
            | MenuItem::Stop
            | MenuItem::Options
            | MenuItem::Paste
            | MenuItem::Separator => None,
        }
    }

    /// The text a Windows menu takes: the label, then a tab and the
    /// shortcut, which Windows lines up on the right.
    pub fn text(self) -> String {
        match self.parts() {
            (label, key) if key.is_empty() => label.to_owned(),
            (label, key) => format!("{label}\t{key}"),
        }
    }
}

/// The menu for the rows `selection` (one state per selected row), given
/// the wheels that have anything for them. Items that don't apply are
/// left out, never greyed: the wheels, then Cancel for a running one or
/// Retry for a cancelled or failed
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

/// `items` for one row whose finished drag-wheel conversion offers its
/// Options (`options`): Options joins the row's own actions, before Show
/// in folder (which one row always has), or Copy path; never after
/// Remove.
pub fn with_options(mut items: Vec<MenuItem>, options: bool) -> Vec<MenuItem> {
    if options {
        let at = [MenuItem::ShowInFolder, MenuItem::CopyPath, MenuItem::Remove]
            .iter()
            .find_map(|m| items.iter().position(|i| i == m))
            .unwrap_or(items.len());
        items.insert(at, MenuItem::Options);
    }
    items
}

/// `items` with Make smaller where it applies to the selection
/// (`smaller`), as the mock's menu has it: in Tools…' place, or after
/// Convert to…, or first. The tools stay on the wheel.
pub fn with_smaller(mut items: Vec<MenuItem>, smaller: bool) -> Vec<MenuItem> {
    if !smaller {
        return items;
    }
    let tools = MenuItem::Wheel(WheelKind::Tools);
    if let Some(at) = items.iter().position(|&i| i == tools) {
        items[at] = MenuItem::Smaller;
    } else if let Some(at) = items
        .iter()
        .position(|&i| i == MenuItem::Wheel(WheelKind::Convert))
    {
        items.insert(at + 1, MenuItem::Smaller);
    } else {
        items.splice(0..0, [MenuItem::Smaller, MenuItem::Separator]);
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
/// has been up for two seconds. While the side pane shows (`pane`) it is
/// the instruction, and the footer has no hint.
pub fn idle_hint(
    listed: usize,
    running: bool,
    status: &str,
    plain: &str,
    added: Option<Duration>,
    pane: bool,
) -> Option<&'static str> {
    let quiet = status.is_empty() || status == plain || added.is_some_and(|t| t >= ADDED_FOR);
    (listed > 0 && !running && !pane && quiet).then_some(IDLE_HINT)
}

/// How long a result or an alert stays in the footer once it has been
/// shown, if the selection doesn't change first.
pub const STATUS_FOR: Duration = Duration::from_secs(5);

/// Whether a result or alert that has been up for `shown` has had its time,
/// or the selection has changed (`selection_changed`) since it was first
/// shown: the footer goes back to its count and hint.
pub fn status_stale(shown: Duration, selection_changed: bool) -> bool {
    selection_changed || shown >= STATUS_FOR
}

/// How much longer a result or alert that has been up for `shown` stays.
pub fn status_wait(shown: Duration) -> Option<Duration> {
    STATUS_FOR.checked_sub(shown).filter(|rest| !rest.is_zero())
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

/// The empty window's lines, top to bottom (mock screen 1): the K, then
/// these, then the Add files button.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmptyLines {
    /// "Drop files to convert", in Subtitle.
    pub headline: &'static str,
    /// What Kiln reads, in Body and the secondary colour.
    pub body: &'static str,
    pub button: &'static str,
}

/// The empty window's lines.
pub fn empty_lines() -> EmptyLines {
    EmptyLines {
        headline: "Drop files to convert",
        body: "Photos, videos, sound, documents and e-books.",
        button: ADD_FILES,
    }
}

/// The first-run help under the empty window's Add files, until it is
/// dismissed (spec 2.6): a heading, three numbered steps, the
/// right-click line (decision 5) and Dismiss.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirstRun {
    pub title: &'static str,
    pub steps: [&'static str; 3],
    /// Under the steps, in Caption and the secondary colour.
    pub more: &'static str,
    /// The Dismiss button's name; it is drawn as its glyph alone.
    pub dismiss: &'static str,
    /// The buttons the lines name, set in Body Strong where they appear
    /// (`strong_runs`), as the mock's bold words.
    pub strong: &'static [&'static str],
}

/// What the first-run card sets in Body Strong: the buttons and the
/// command it names.
pub const STRONG: &[&str] = &["Add files", "Make smaller", "Show in folder", "Open with"];

/// Where in `s` each of `strong` appears, as byte ranges in order, every
/// time it does: the runs a line sets in Body Strong.
pub fn strong_runs(s: &str, strong: &[&str]) -> Vec<std::ops::Range<usize>> {
    let mut runs: Vec<std::ops::Range<usize>> = strong
        .iter()
        .filter(|w| !w.is_empty())
        .flat_map(|w| s.match_indices(w).map(|(at, w)| at..at + w.len()))
        .collect();
    runs.sort_by_key(|r| r.start);
    // One word inside another's run adds nothing.
    runs.dedup_by(|b, a| b.start < a.end);
    runs
}

/// The first-run card's heading.
pub const FIRST_RUN_TITLE: &str = "How it works";

/// The first-run card's Dismiss.
pub const DISMISS: &str = "Dismiss";

/// The first-run card's words. `results` says where the window's results
/// go, as Help says it: the third step says the same.
pub fn first_run(results: crate::help::Results) -> FirstRun {
    use crate::help::Results;
    FirstRun {
        title: FIRST_RUN_TITLE,
        steps: [
            "Drop files here, or choose Add files.",
            "Pick a format. To keep the format and shrink the file, pick Make smaller.",
            match results {

                Results::Chosen => "The new file is saved in the folder chosen in Settings. Show in folder takes you to it.",
                Results::Beside | Results::NoHolding => "The new file is saved next to the original. Show in folder takes you to it.",
            },
        ],
        more: "You can also right-click files in File Explorer and choose Open with, then Kiln.",
        dismiss: DISMISS,
        strong: STRONG,
    }
}

/// Whether the empty window shows the first-run card: until it is
/// dismissed, or again after Help's Show first-run help.
pub fn first_run_shows(settings: &crate::settings::Settings) -> bool {
    !settings.first_run_dismissed
}

/// The first-run card was dismissed: it stays away. True when settings
/// changed and should be saved.
pub fn dismiss_first_run(settings: &mut crate::settings::Settings) -> bool {
    !std::mem::replace(&mut settings.first_run_dismissed, true)
}

/// Help's Show first-run help: the card shows again in the empty window.
/// True when settings changed and should be saved.
pub fn show_first_run(settings: &mut crate::settings::Settings) -> bool {
    std::mem::replace(&mut settings.first_run_dismissed, false)
}

/// Help's link that brings the first-run card back.
pub const SHOW_FIRST_RUN: &str = "Show first-run help";

/// The Add files button, in the empty window and the footer.
pub const ADD_FILES: &str = "Add files";

/// The title of the Open dialog Add files shows.
pub const ADD_FILES_TITLE: &str = "Add files";

/// Whether a plain drop opens the format wheel at the drop point (spec
/// 2.1): the files are all of one kind (`kinds`, how many kinds there
/// are) and the convert wheel has something for them (`offer`). Mixed
/// kinds, or nothing to offer, are just listed.
pub fn drop_opens_wheel(offer: &Offer, kinds: usize) -> bool {
    kinds == 1 && *offer == Offer::Choices
}

/// What the InfoBar at the top of the list says when `n` files of more
/// than one kind arrive together: its title and message.
pub fn mixed_drop_info(n: usize) -> (String, String) {
    (
        format!("{n} files added."),
        "They don't share a format, so pick one file or one kind at a time.".to_owned(),
    )
}

/// How long files arriving by Open with, Send to, a second launch or Add
/// files must stop arriving before the format choice opens for them all
pub const ARRIVALS_SETTLE: Duration = Duration::from_millis(250);

/// Files arriving by Open with, Send to, a second launch or Add files,
/// gathered until none has come for [`ARRIVALS_SETTLE`].
#[derive(Debug, Default)]
pub struct Arrivals {
    last: Option<Instant>,
    paths: Vec<PathBuf>,
    /// `paths`, to find one again at once however many arrive.
    seen: std::collections::HashSet<PathBuf>,
}

impl Arrivals {
    /// `paths` arrived at `now`.
    pub fn arrived(&mut self, paths: &[PathBuf], now: Instant) {
        self.last = Some(now);
        for p in paths {
            if self.seen.insert(p.clone()) {
                self.paths.push(p.clone());
            }
        }
    }

    /// A job took `taken`: the arrivals they are, or a folder holding
    /// one, ask nothing more. The others still wait.
    pub fn forget(&mut self, taken: &[PathBuf]) {
        let gone = |a: &PathBuf| taken.iter().any(|t| t == a || t.starts_with(a));
        self.paths.retain(|a| !gone(a));
        self.seen.retain(|a| !gone(a));
        if self.paths.is_empty() {
            self.last = None;
        }
    }

    /// How long until they are due, while any wait.
    pub fn wait(&self, now: Instant) -> Option<Duration> {
        let last = self.last?;
        Some(ARRIVALS_SETTLE.saturating_sub(now.saturating_duration_since(last)))
    }

    /// Forget what arrived: the files were acted on another way first.
    pub fn clear(&mut self) {
        self.last = None;
        self.paths.clear();
        self.seen.clear();
    }

    /// Everything that arrived, once none has for [`ARRIVALS_SETTLE`]:
    /// then they are taken and the next arrival starts afresh.
    pub fn due(&mut self, now: Instant) -> Option<Vec<PathBuf>> {
        if self.wait(now)? > Duration::ZERO {
            return None;
        }
        self.last = None;
        self.seen.clear();
        Some(std::mem::take(&mut self.paths))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::help::Results;
    use crate::model::JobState;
    use crate::wheel::Action;
    use std::path::PathBuf;

    use MenuItem::*;
    use RowState::*;
    use WheelKind::{Convert, Tools};

    const ALL: [WheelKind; 2] = [Convert, Tools];

    #[test]
    fn a_drop_of_one_kind_with_formats_opens_the_wheel() {
        // One kind the wheel has something for: it opens.
        assert!(drop_opens_wheel(&Offer::Choices, 1));
        // Mixed kinds are listed, even with formats in common.
        assert!(!drop_opens_wheel(&Offer::Choices, 2));
        let mixed = Offer::Mixed(vec![("PNG".into(), 1), ("MP3".into(), 1)]);
        assert!(!drop_opens_wheel(&mixed, 2));
        // One kind with nothing to offer is listed, whatever the reason.
        assert!(!drop_opens_wheel(&Offer::Unsupported("XYZ".into()), 1));
        let helper = Offer::NeedsHelper("pandoc".into(), "MD".into());
        assert!(!drop_opens_wheel(&helper, 1));
        // Nothing dropped, nothing to open.
        assert!(!drop_opens_wheel(&Offer::Nothing, 0));
        assert!(!drop_opens_wheel(&Offer::Choices, 0));
    }

    #[test]
    fn the_rule_meets_the_wheels_own_offer() {
        use crate::wheel::{kinds_in, offer};
        use kiln_core::{Availability, Kiln};
        let kiln = Kiln::new();
        let opens = |names: &[&str]| {
            let paths: Vec<PathBuf> = names.iter().map(PathBuf::from).collect();
            let o = offer(Convert, &kiln, &Availability::all(), &paths);
            drop_opens_wheel(&o, kinds_in(&kiln, &paths))
        };
        assert!(opens(&["a.jpg", "b.jpg"]));
        assert!(opens(&["a.jpg", "b.png"]));
        assert!(opens(&["a.mp4"]));
        // A JPG and an MP4: listed (mock "infobar").
        assert!(!opens(&["a.jpg", "b.mp4"]));
        // A type Kiln has nothing for.
        assert!(!opens(&["a.xyz"]));
        assert!(!opens(&["a.xyz", "b.xyz"]));
    }

    #[test]
    fn a_plain_row_offers_its_wheels_and_remove() {
        assert_eq!(
            menu_items(&[Plain], &ALL),
            [
                Wheel(Convert),
                Wheel(Tools),
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
            menu_items(&[Plain], &[Tools, Convert]),
            [
                Wheel(Convert),
                Wheel(Tools),
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
    fn a_saved_row_offers_show_in_folder() {
        assert_eq!(
            menu_items(&[Saved], &[Tools]),
            [
                Wheel(Tools),
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
        for state in [Plain, Saved, Retryable, Running] {
            let items = menu_items(&[state], &[]);
            assert!(items.contains(&ShowInFolder), "{state:?}");
            assert!(items.contains(&CopyPath), "{state:?}");
        }
        // Several rows: only Copy path, which takes every path.
        let items = menu_items(&[Plain, Plain], &[]);
        assert_eq!(items, [CopyPath, Separator, Remove]);
    }

    #[test]
    fn several_rows_offer_copy_path_and_remove() {
        assert_eq!(
            menu_items(&[Saved, Plain], &[Convert]),
            [Wheel(Convert), Separator, CopyPath, Separator, Remove]
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
            menu_items(&[Running], &[Tools]),
            [
                Wheel(Tools),
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
            menu_items(&[Saved, Running], &[]),
            [Stop, CopyPath, Separator, Remove]
        );
    }

    #[test]
    fn arrivals_100_ms_apart_are_due_together_250_ms_after_the_last() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let (a, b, c) = (
            PathBuf::from("a.jpg"),
            PathBuf::from("b.jpg"),
            PathBuf::from("c.jpg"),
        );
        let mut arr = Arrivals::default();
        assert!(arr.due(t0).is_none() && arr.wait(t0).is_none());
        arr.arrived(std::slice::from_ref(&a), ms(0));
        arr.arrived(std::slice::from_ref(&b), ms(100));
        // Still coming in: nothing is due until 250 ms after the last.
        assert!(arr.due(ms(300)).is_none());
        arr.arrived(&[c.clone(), a.clone()], ms(200));
        assert_eq!(arr.wait(ms(300)), Some(Duration::from_millis(150)));
        assert!(arr.due(ms(449)).is_none());
        assert_eq!(arr.due(ms(450)), Some(vec![a.clone(), b, c]));
        // Once taken, nothing more is due.
        assert!(arr.due(ms(2000)).is_none());
        // Cleared, nothing waits.
        arr.arrived(std::slice::from_ref(&a), ms(3000));
        arr.clear();
        assert!(arr.wait(ms(3000)).is_none() && arr.due(ms(4000)).is_none());
    }

    #[test]
    fn a_job_forgets_only_the_arrivals_it_takes() {
        let t0 = Instant::now();
        let (a, b) = (PathBuf::from(r"C:\in\a.jpg"), PathBuf::from(r"C:\in\b.jpg"));
        let folder = PathBuf::from(r"C:\Photos");
        let mut arr = Arrivals::default();
        arr.arrived(&[a.clone(), b.clone(), folder.clone()], t0);
        // A job on `a` and on a file of the folder.
        arr.forget(&[a.clone(), folder.join("c.jpg")]);
        let later = t0 + ARRIVALS_SETTLE;
        assert_eq!(arr.due(later), Some(vec![b.clone()]));
        // Once nothing is left, nothing waits.
        arr.arrived(std::slice::from_ref(&a), later);
        arr.forget(std::slice::from_ref(&a));
        assert!(arr.wait(later).is_none());
    }

    #[test]
    fn many_arrivals_one_at_a_time_are_kept_once_each() {
        let t0 = Instant::now();
        let mut arr = Arrivals::default();
        let files: Vec<PathBuf> = (0..5000)
            .map(|i| PathBuf::from(format!("{i}.jpg")))
            .collect();
        let started = Instant::now();
        for f in files.iter().chain(&files) {
            arr.arrived(std::slice::from_ref(f), t0);
        }
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(arr.due(t0 + ARRIVALS_SETTLE), Some(files));
    }

    #[test]
    fn make_smaller_takes_the_place_of_tools_when_it_applies() {
        // As the mock's menu: Convert to…, then Make smaller.
        let plain = menu_items(&[Plain], &ALL);
        assert_eq!(
            with_smaller(plain.clone(), true),
            [
                Wheel(Convert),
                Smaller,
                Separator,
                ShowInFolder,
                CopyPath,
                Separator,
                Remove
            ]
        );
        assert_eq!(with_smaller(plain.clone(), false), plain);
        // With no Tools wheel, it comes after Convert to….
        let convert = menu_items(&[Plain], &[Convert]);
        assert_eq!(with_smaller(convert, true)[..2], [Wheel(Convert), Smaller]);
        // With neither wheel, it opens the menu's first group.
        let none = with_smaller(menu_items(&[Plain], &[]), true);
        assert_eq!(none[..2], [Smaller, Separator]);
        assert_eq!(Smaller.text(), "Make smaller");
    }

    #[test]
    fn a_finished_drop_offers_its_options_in_the_menu() {
        let wheels = [Convert];
        let held = menu_items(&[RowState::Saved], &wheels);
        let with = with_options(held.clone(), true);
        // Options sits among the row's own actions, before Show in folder.
        let at = with.iter().position(|&i| i == Options).unwrap();
        assert_eq!(with[at + 1], ShowInFolder);
        assert_eq!(with.len(), held.len() + 1);
        assert_eq!(with_options(held.clone(), false), held);
        assert_eq!(Options.text(), "Options");
        // Whatever the row's state, and whatever wheels it offers, one
        // row has Show in folder, so Options lands among its own actions:
        // before Show in folder, and never after Remove.
        for state in [
            RowState::Plain,
            RowState::Running,
            RowState::Saved,
            RowState::Retryable,
        ] {
            for wheels in [&[][..], &[Convert][..]] {
                let items = with_options(menu_items(&[state], wheels), true);
                let at = |m: MenuItem| items.iter().position(|&i| i == m).unwrap();
                assert_eq!(at(ShowInFolder), at(Options) + 1, "{state:?}");
                assert!(at(Options) < at(Remove), "{state:?}");
            }
        }
        // Without Show in folder, before Copy path; without either, before
        // Remove.
        assert_eq!(
            with_options(vec![CopyPath, Separator, Remove], true)[0],
            Options
        );
        assert_eq!(
            with_options(vec![Stop, Separator, Remove], true)[2],
            Options
        );
    }

    #[test]
    fn the_empty_list_menu_pastes_only_with_files_to_paste() {
        assert_eq!(list_menu(true), [AddFiles, Paste]);
        assert_eq!(list_menu(false), [AddFiles]);
    }

    #[test]
    fn menu_text_carries_the_shortcut() {
        let text: Vec<String> = [
            Wheel(Convert),
            Wheel(Tools),
            ShowInFolder,
            Stop,
            Remove,
            AddFiles,
            Paste,
        ]
        .into_iter()
        .map(|i| i.text())
        .collect();
        assert_eq!(
            text,
            [
                "Convert to…",
                "Tools…\tCtrl+Enter",
                "Show in folder",
                "Cancel",
                "Remove from list\tDel",
                "Add files…\tCtrl+O",
                "Paste\tCtrl+V",
            ]
        );
    }

    #[test]
    fn wheel_shortcuts_name_the_wheel_keys() {
        // Plain Enter converts to the suggested format, so only the tools
        // carry a shortcut here.
        assert_eq!(Wheel(Convert).text(), "Convert to…");
        assert_eq!(Wheel(Tools).text(), "Tools…	Ctrl+Enter");
    }

    #[test]
    fn the_idle_hint_waits_for_added_to_be_read() {
        let plain = "2 files.";
        let added = "Added 2 files.";
        let hint = Some(IDLE_HINT);
        let ms = Duration::from_millis;
        assert_eq!(idle_hint(2, false, plain, plain, None, false), hint);
        assert_eq!(idle_hint(2, false, "", plain, None, false), hint);
        assert_eq!(
            idle_hint(2, false, added, plain, Some(ms(500)), false),
            None
        );
        assert_eq!(
            idle_hint(2, false, added, plain, Some(ms(2000)), false),
            hint
        );
        // Nothing listed, something running, or a status worth keeping.
        assert_eq!(idle_hint(0, false, "", "0 files.", None, false), None);
        assert_eq!(idle_hint(2, true, plain, plain, None, false), None);
        let done = "Converted 2 files to JPG. Save them to keep them.";
        assert_eq!(idle_hint(2, false, done, plain, None, false), None);
    }

    #[test]
    fn the_pane_is_the_instruction_so_it_has_no_hint() {
        // with the side pane showing, nothing says "Right-click a file".
        let plain = "2 files.";
        assert_eq!(idle_hint(2, false, plain, plain, None, true), None);
        assert_eq!(idle_hint(2, false, "", plain, None, true), None);
        // Folded, the hint stays.
        assert_eq!(
            idle_hint(2, false, plain, plain, None, false),
            Some(IDLE_HINT)
        );
    }

    #[test]
    fn a_result_or_alert_goes_when_the_selection_changes_or_after_five_seconds() {
        let s = Duration::from_secs;
        assert!(!status_stale(s(0), false));
        assert!(!status_stale(s(4), false));
        assert!(status_stale(s(5), false));
        assert!(status_stale(s(1), true));
        assert_eq!(status_wait(s(2)), Some(s(3)));
        assert_eq!(status_wait(s(5)), None);
        assert_eq!(status_wait(s(9)), None);
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
            assert_eq!(
                idle_hint(3, false, &said, &plain, None, false),
                None,
                "{said}"
            );
        }
    }

    #[test]
    fn a_selection_count_stands_over_all_but_work_and_trouble() {
        let count = Some("3 selected");
        let done = "Converted 3 files to JPG. Save them to keep them.";
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
        let l = empty_lines();
        assert_eq!(l.headline, "Drop files to convert");
        assert_eq!(l.body, "Photos, videos, sound, documents and e-books.");
        assert_eq!(l.button, "Add files");
    }

    #[test]
    fn the_first_run_card_says_three_steps_and_the_right_click_line() {
        let c = first_run(Results::Beside);
        assert_eq!(c.title, "How it works");
        assert_eq!(
            c.steps,
            [
                "Drop files here, or choose Add files.",
                "Pick a format. To keep the format and shrink the file, pick Make smaller.",
                "The new file is saved next to the original. Show in folder takes you to it.",
            ]
        );
        // Decision 5's words.
        assert_eq!(
            c.more,
            "You can also right-click files in File Explorer and choose Open with, then Kiln."
        );
        assert_eq!(c.dismiss, "Dismiss");
        // Saved into the folder chosen in Settings.
        let chosen = first_run(Results::Chosen);
        assert_eq!(chosen.steps[..2], c.steps[..2]);
        assert_eq!(
            chosen.steps[2],
            "The new file is saved in the folder chosen in Settings. Show in folder takes you to it."
        );
        assert_eq!(first_run(Results::NoHolding).steps, c.steps);
        for s in c.steps.iter().chain(&chosen.steps).chain([&c.more]) {
            assert!(s.ends_with('.') && !s.contains('!'), "{s}");
        }
    }

    #[test]
    fn the_first_run_card_sets_the_buttons_it_names_in_strong() {
        let words = |s: &'static str| -> Vec<&str> {
            strong_runs(s, STRONG).into_iter().map(|r| &s[r]).collect()
        };
        let c = first_run(Results::Beside);
        assert_eq!(words(c.steps[0]), ["Add files"]);
        assert_eq!(words(c.steps[1]), ["Make smaller"]);
        assert_eq!(words(c.steps[2]), ["Show in folder"]);
        assert_eq!(words(c.more), ["Open with"]);
        assert_eq!(
            words(first_run(Results::Chosen).steps[2]),
            ["Show in folder"]
        );
        // Each run is where its words are, in order, once each.
        let s = "Pick Add files, then Add files and Open with.";
        let runs = strong_runs(s, STRONG);
        assert_eq!(runs, [5..14, 21..30, 35..44]);
    }

    #[test]
    fn the_first_run_card_shows_until_dismissed_and_after_helps_link() {
        let mut s = crate::settings::Settings::default();
        assert!(first_run_shows(&s));
        // Dismiss is remembered once; a second changes nothing.
        assert!(dismiss_first_run(&mut s));
        assert!(s.first_run_dismissed && !first_run_shows(&s));
        assert!(!dismiss_first_run(&mut s));
        // Help's link brings it back, and Dismiss works again after it.
        assert!(show_first_run(&mut s));
        assert!(first_run_shows(&s));
        assert!(!show_first_run(&mut s));
        assert!(dismiss_first_run(&mut s));
        assert!(!first_run_shows(&s));
        assert_eq!(SHOW_FIRST_RUN, "Show first-run help");
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
            from_drag: false,
            check: kiln_core::FileCheck::Ok,
            missing: None,
            save_failed: false,
            job_note: None,
            result_bytes: None,
            waits_on: None,
            ffmpeg_found: true,
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
        // A result on its way to its folder has nothing to show yet.
        assert_eq!(held.row_state(false), Plain);
        assert_eq!(held.row_state(true), Plain);
        let saved = entry(JobState::Done {
            action: Action::Convert("jpg".into()),
            outputs: vec![out],
            saved: Some(crate::model::Saved::Folder(PathBuf::from(r"D:\out"))),
        });
        assert_eq!(saved.row_state(false), Saved);
        assert_eq!(saved.row_state(true), Plain);
        assert_eq!(entry(JobState::Idle).row_state(false), Plain);
        let converting = entry(JobState::Running {
            action: Action::Convert("jpg".into()),
            fraction: Some(0.5),
            started: None,
        });
        assert_eq!(converting.row_state(false), Running);
    }
}
