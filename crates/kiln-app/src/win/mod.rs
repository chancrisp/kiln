//! The Win32 window: creation, the message loop, input and animation.

mod drop;
mod handoff;
mod inspect;
mod render;
mod system;
mod text;
mod updates;

use std::cell::OnceCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows::core::{w, Result, BOOL, HSTRING, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWINDOWATTRIBUTE};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, CreateSolidBrush, InvalidateRect, ScreenToClient, UpdateWindow, ValidateRect,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::{
    IDropTarget, OleInitialize, OleUninitialize, RegisterDragDrop, RevokeDragDrop,
};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::{
    GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use kiln_core::{
    suggest, Availability, Facts, Format, Habits, Holding, JobError, JobEvent, JobId, JobKind,
    Kiln, Knob, MediaInfo, Offered, Options, Progress, Quality, Runner, Suggestion, Tool,
};

use crate::actions::{self, FileAction};
use crate::layout::{
    self, Control, SettingsLayout, SettingsShape, SettingsTab, SheetControl, SheetLayout,
    ROW_HEIGHT,
};
use crate::model::{
    self, batch_summary, leave_prompt, status_line, FileList, JobState, Leave, PromptButton, Tone,
};
use crate::quality::{self as q, Value};
use crate::settings::{self, Engine, Settings};
use crate::theme::{self, motion, Palette};
use crate::update::{Reply, UpdateState};
use crate::ways_in::{self, MenuItem, RowState};
use crate::wheel::{self, Action, Spot, Wheel, WheelKind};
use drop::{DragEvent, DropTarget, DRAG_ENTER, DRAG_LEAVE, DRAG_OVER, WM_KILN_DRAG, WM_KILN_DROP};
use inspect::Inspector;
use render::{Renderer, RowAnim, SettingsView, SheetView, View, WheelView};

const ANIM_TIMER: usize = 1;
/// Fires once, when "Added 2 files." has been up long enough for the
/// footer's idle hint to take its place.
const HINT_TIMER: usize = 2;
/// Added to the hint timer's wait, so it fires after the two seconds.
const HINT_SLACK: Duration = Duration::from_millis(50);
const FRAME_MS: u32 = 16;
/// Frame time when only progress lines are glowing: slow enough to stay
/// cheap, fast enough to look smooth.
const HEAT_FRAME_MS: u32 = 50;

/// Posted by the worker threads; lparam is a `Box<JobEvent>`.
const WM_KILN_JOB: u32 = WM_APP + 3;
/// Posted when a move finishes; lparam is a `Box<Moved>`.
const WM_KILN_MOVED: u32 = WM_APP + 4;
/// Posted when the update check is answered; lparam is a `Box<Reply>`.
const WM_KILN_UPDATE: u32 = WM_APP + 5;
/// Posted when the options sheet's files have been read; lparam is a
/// `Box<Probed>`.
const WM_KILN_PROBE: u32 = WM_APP + 6;
/// Posted when the inspect thread has looked inside a listed file; lparam
/// is a `Box<Inspected>`.
const WM_KILN_FACTS: u32 = WM_APP + 7;
/// Posted when an export thread has finished; lparam is a `Box<Exported>`.
const WM_KILN_EXPORTED: u32 = WM_APP + 8;
/// How long a row's second line takes to cross over to what Kiln found.
const FACTS_MS: u32 = 160;
/// Most files the options sheet reads for its estimate.
const PROBE_LIMIT: usize = 64;

/// Drag modifier flags (MK_SHIFT, MK_CONTROL, MK_ALT).
const MK_SHIFT_FLAG: u32 = 0x0004;
const MK_CONTROL_FLAG: u32 = 0x0008;
const MK_ALT_FLAG: u32 = 0x0020;

/// How long the wheel takes to pour open.
const OPEN_MS: u32 = 160;
/// The pulse on a chosen item before its job starts.
const PULSE_MS: u32 = motion::SETTLE_MS;

/// A value that eases toward a target over a fixed duration.
struct Tween {
    from: f32,
    to: f32,
    start: Instant,
    ms: u32,
}

impl Tween {
    fn new(v: f32) -> Self {
        Self {
            from: v,
            to: v,
            start: Instant::now(),
            ms: motion::QUICK_MS,
        }
    }

    fn value(&self) -> f32 {
        let t = self.start.elapsed().as_secs_f32() * 1000.0 / self.ms as f32;
        self.from + (self.to - self.from) * motion::ease_out(t)
    }

    fn set(&mut self, to: f32, ms: u32) {
        if (self.to - to).abs() < f32::EPSILON {
            return;
        }
        self.from = self.value();
        self.to = to;
        self.ms = ms;
        self.start = Instant::now();
    }

    fn running(&self) -> bool {
        self.start.elapsed() < Duration::from_millis(self.ms as u64)
    }
}

/// Eased progress, 0 to 1, of something that started at `t`.
fn eased(t: Instant, ms: u32) -> f32 {
    motion::ease_out(t.elapsed().as_secs_f32() * 1000.0 / ms as f32)
}

fn within(t: Instant, ms: u32) -> bool {
    t.elapsed() < Duration::from_millis(ms as u64)
}

/// The heat of running progress lines now. Every line shares one clock so
/// they breathe together.
fn heat_now() -> f32 {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let ms = EPOCH.get_or_init(Instant::now).elapsed().as_secs_f32() * 1000.0;
    motion::heat(ms)
}

/// An HWND the worker threads can post to.
#[derive(Clone, Copy)]
struct PostTarget(usize);

impl PostTarget {
    /// Post `value` boxed in lparam; the handler for `msg` takes the box.
    fn post<T>(self, msg: u32, value: T) {
        let raw = Box::into_raw(Box::new(value));
        let hwnd = HWND(self.0 as *mut core::ffi::c_void);
        let posted = unsafe { PostMessageW(Some(hwnd), msg, WPARAM(0), LPARAM(raw as isize)) };
        if posted.is_err() {
            // The window is gone; nobody will free the event but us.
            drop(unsafe { Box::from_raw(raw) });
        }
    }
}

/// The glaze wheel while it is on screen.
struct WheelUi {
    wheel: Wheel,
    /// The files it acts on.
    paths: Vec<PathBuf>,
    hover: Option<Spot>,
    /// The outer ring More pours open: 0 shut, 1 open.
    more: Tween,
    opened: Instant,
    closing: Option<Instant>,
    /// The chosen item and when it was chosen; its job starts once the
    /// pulse has played.
    pulse: Option<(Spot, Instant)>,
    /// Opened from the keyboard rather than during a drag.
    keyboard: bool,
    /// What Kiln suggests on this wheel, once every file's facts are in.
    suggestion: Option<Suggestion>,
    /// When the suggestion arrived while the wheel was open, so its dot
    /// fades in. `None` shows it fully.
    dot_at: Option<Instant>,
}

impl WheelUi {
    fn is_open(&self) -> bool {
        self.closing.is_none() && self.pulse.is_none()
    }

    fn more_open(&self) -> bool {
        self.more.to > 0.5
    }

    /// Where the suggestion's dots go.
    fn marks(&self) -> Vec<Spot> {
        self.suggestion
            .as_ref()
            .map_or_else(Vec::new, |s| wheel::marks(&self.wheel, &s.action))
    }

    /// Point at `spot`. Pointing at More pours its ring; pointing at any
    /// other inner segment lets it go. Returns whether anything changed.
    fn point(&mut self, spot: Option<Spot>) -> bool {
        if spot == self.hover {
            return false;
        }
        match spot {
            Some(Spot::Inner(i)) if self.wheel.is_more(i) => self.more.set(1.0, OPEN_MS),
            Some(Spot::Inner(_)) => self.more.set(0.0, motion::QUICK_MS),
            _ => {}
        }
        self.hover = spot;
        true
    }
}

/// The options sheet: one conversion's quality, chosen before it starts.
struct SheetUi {
    id: u64,
    paths: Vec<PathBuf>,
    /// The first file's format; the knobs follow it.
    from: Format,
    to: Format,
    knobs: Vec<Knob>,
    quality: Quality,
    /// What the files hold, once read; `None` while reading.
    infos: Option<Vec<MediaInfo>>,
    names: Vec<String>,
    bytes: u64,
    hover: Option<usize>,
    focus: Option<usize>,
    pressed: Option<usize>,
    opened: Instant,
    closing: Option<Instant>,
}

impl SheetUi {
    /// What the knobs can be set to. With one file read, sizes it already
    /// fits under are left out.
    fn values(&self, knob: Knob) -> Vec<Value> {
        let info = match self.infos.as_deref() {
            Some([one]) => Some(one),
            _ => None,
        };
        q::values(knob, &self.from, &self.to, info)
    }

    fn shown(&self, knob: Knob) -> Value {
        q::shown(&self.quality, knob, &self.values(knob))
    }

    /// Step a knob along its values, wrapping at the ends.
    fn step(&mut self, row: usize, delta: isize) {
        let Some(&knob) = self.knobs.get(row) else {
            return;
        };
        let list = self.values(knob);
        let v = q::stepped(&list, self.shown(knob), delta, true);
        q::set(&mut self.quality, knob, v);
    }

    fn is_open(&self) -> bool {
        self.closing.is_none()
    }
}

/// The options sheet's files, read on a worker thread.
struct Probed {
    id: u64,
    infos: Vec<MediaInfo>,
}

/// What the inspect thread found in one listed file.
struct Inspected {
    path: PathBuf,
    facts: Facts,
}

/// The result of moving files, from the thread that moved them.
struct Moved {
    dest: PathBuf,
    results: Vec<(PathBuf, std::result::Result<PathBuf, String>)>,
}

/// What an export thread did, for WM_KILN_EXPORTED.
struct Exported {
    id: u64,
    folder: PathBuf,
    /// Each held file and where it went, or `None` where it couldn't be
    /// saved.
    results: Vec<(PathBuf, Option<PathBuf>)>,
}

/// An export under way on its own thread.
struct Export {
    /// The held files it is moving.
    files: Vec<PathBuf>,
    /// An Export all, whose result the footer sums up. A single row's
    /// export speaks through its row.
    all: bool,
}

/// Where an export sends its files.
enum Dest {
    /// Into a folder, never over a file that's there.
    Into(PathBuf),
    /// To the name the Save dialog chose; it asked before replacing.
    As(PathBuf),
}

/// The rows a leave covers, by file, so they still mean the same rows if
/// the list changes while an export runs.
enum Rows {
    All,
    Files(Vec<PathBuf>),
}

/// What a leave does once it goes ahead.
enum Then {
    Close,
    Clear,
    Remove(PathBuf),
    /// Start a new job on rows, which replaces what they hold.
    Start(Action, Vec<PathBuf>, Option<Quality>),
}

/// A leave whose prompt chose Export all, waiting for the export.
struct Leaving {
    export: u64,
    rows: Rows,
    folder: PathBuf,
    then: Then,
}

/// The settings panel while it is open.
struct SettingsUi {
    tab: SettingsTab,
    /// Where the tab marker sits, in tabs from the left; it slides.
    tab_marker: Tween,
    /// When the page last changed, for the crossfade.
    switched: Instant,
    hover: Option<usize>,
    focus: Option<usize>,
    output_line: (String, bool),
    engine_lines: [(String, bool); 4],
}

struct RowTimes {
    added: Instant,
    /// When the row's last job finished.
    settled: Option<Instant>,
}

struct JobInfo {
    paths: Vec<PathBuf>,
    action: Action,
    batch: u64,
}

/// Jobs started by one choice on the wheel, for the status line.
struct Batch {
    action: Action,
    files: usize,
    ok: usize,
    failed: usize,
    cancelled: usize,
    pending: usize,
    /// Result files the batch left held, for the footer's export hint.
    held: usize,
}

/// Which engines are installed, looked for once and kept until settings
/// are saved or read: looking searches PATH and the usual install
/// folders, too slow to do for every file's facts or every wheel.
#[derive(Default)]
struct Engines(OnceCell<Availability>);

impl Engines {
    /// The engines found, running `search` only when nothing is kept.
    fn get(&self, search: impl FnOnce() -> Availability) -> &Availability {
        self.0.get_or_init(search)
    }

    /// Look again next time.
    fn forget(&mut self) {
        self.0.take();
    }
}

struct App {
    hwnd: HWND,
    kiln: Arc<Kiln>,
    runner: Runner,
    /// Looks inside listed files for suggestions, one at a time.
    inspector: Inspector,
    /// The engines suggestions are worked out with.
    engines: Engines,
    jobs: HashMap<JobId, JobInfo>,
    batches: HashMap<u64, Batch>,
    next_batch: u64,
    palette: &'static Palette,
    settings: Settings,
    /// Where settings.toml is, and whether it could be read.
    settings_store: settings::Store,
    system_light: bool,
    settings_ui: Option<SettingsUi>,
    settings_hover: bool,
    /// Where the pointer last was, to ignore the moves Windows reports
    /// when a window appears under a still pointer.
    last_mouse: Option<(i32, i32)>,
    renderer: Renderer,
    files: FileList,
    rows: Vec<RowTimes>,
    scroll: f32,
    hover_row: Option<usize>,
    hover_cancel: bool,
    hover_link: bool,
    drag: Tween,
    /// The files in the current drag, read when it entered the window.
    drag_paths: Vec<PathBuf>,
    wheel: Option<WheelUi>,
    sheet: Option<SheetUi>,
    next_sheet: u64,
    /// Engine output shown in the details dialog.
    details: Option<String>,
    dialog_close_hover: bool,
    status: String,
    clear_hover: bool,
    clear_pressed: bool,
    /// The settings control the pointer went down on; it acts on release.
    settings_pressed: Option<usize>,
    dialog_close_pressed: bool,
    update: UpdateState,
    tracking_mouse: bool,
    dpi: f32,
    /// This run's private folder for finished results. `None` if it
    /// couldn't be made; results then go next to each file, as before.
    holding: Option<Holding>,
    /// The folder the last export in this run went to.
    last_export: Option<PathBuf>,
    /// Lines that stand in for a row's own second line, by the row's file:
    /// an export that failed, or a held result that's gone.
    notes: HashMap<PathBuf, (String, Tone)>,
    export_hover: bool,
    export_pressed: bool,
    /// How many held results Windows was last told about, to explain why
    /// it shouldn't shut down. 0 when it hasn't been told.
    shutdown_told: usize,
    /// Exports under way, by id.
    exports: HashMap<u64, Export>,
    next_export: u64,
    /// A close, clear, remove or new job waiting on its Export all.
    leaving: Option<Leaving>,
    /// Close was asked for while an export ran: close once it's done.
    close_when_saved: bool,
    /// What the footer says whenever it has nothing more pressing: why
    /// results are saved next to each file, when there's no holding folder.
    sticky: Option<&'static str>,
    /// The Add files button, in the empty window or the footer.
    add_hover: bool,
    add_pressed: bool,
    /// The folder files were last added from in this run, where Add files
    /// opens.
    last_add: Option<PathBuf>,
    /// What adding files last said, and when, so the footer's idle hint
    /// can take its place once it has been read.
    added: Option<(String, Instant)>,
}

impl App {
    fn dips(&self, x: i32, y: i32) -> (f32, f32) {
        let k = 96.0 / self.dpi;
        (x as f32 * k, y as f32 * k)
    }

    /// A screen point in window DIPs.
    fn screen_dips(&self, x: i32, y: i32) -> (f32, f32) {
        let mut pt = POINT { x, y };
        unsafe {
            let _ = ScreenToClient(self.hwnd, &mut pt);
        }
        self.dips(pt.x, pt.y)
    }

    fn layout(&self) -> layout::Layout {
        let (w, h) = self.renderer.size_dips();
        layout::layout(w, h, self.files.len())
    }

    /// The footer's Export all button, while results are held.
    fn export_button(&self) -> Option<layout::Rect> {
        let clear = self.layout().clear_button?;
        (self.files.held_count() > 0).then(|| layout::export_all_button(&clear))
    }

    fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn animate(&self) {
        unsafe {
            SetTimer(Some(self.hwnd), ANIM_TIMER, FRAME_MS, None);
        }
        self.invalidate();
    }

    fn animating(&self) -> bool {
        self.moving() || self.burning()
    }

    /// Whether any job is running, so its progress line glows.
    fn burning(&self) -> bool {
        self.files
            .entries()
            .iter()
            .any(|e| matches!(e.job, JobState::Running { .. }))
    }

    /// Whether anything is in motion that needs full frame rate.
    fn moving(&self) -> bool {
        let rows = self.rows.iter().any(|r| {
            within(r.added, motion::SETTLE_MS)
                || r.settled.is_some_and(|t| within(t, motion::CONFIRM_MS))
        }) || self
            .files
            .entries()
            .iter()
            .any(|e| e.facts_at.is_some_and(|t| within(t, FACTS_MS)));
        let wheel = self.wheel.as_ref().is_some_and(|w| {
            within(w.opened, OPEN_MS)
                || w.closing.is_some()
                || w.pulse.is_some()
                || w.more.running()
                || w.dot_at.is_some_and(|t| within(t, FACTS_MS))
        });
        let sheet = self
            .sheet
            .as_ref()
            .is_some_and(|s| within(s.opened, OPEN_MS) || s.closing.is_some());
        let settings = self
            .settings_ui
            .as_ref()
            .is_some_and(|ui| ui.tab_marker.running() || within(ui.switched, motion::QUICK_MS));
        self.drag.running() || rows || wheel || sheet || settings
    }

    fn row_anim(&self, i: usize) -> RowAnim {
        let facts = self
            .files
            .entries()
            .get(i)
            .and_then(|e| e.facts_at)
            .map_or(1.0, |t| eased(t, FACTS_MS));
        match self.rows.get(i) {
            Some(r) => RowAnim {
                appear: eased(r.added, motion::SETTLE_MS),
                settle: r.settled.map_or(1.0, |t| eased(t, motion::CONFIRM_MS)),
                facts,
            },
            None => RowAnim {
                appear: 1.0,
                settle: 1.0,
                facts,
            },
        }
    }

    fn add(&mut self, paths: Vec<PathBuf>) {
        let r = self.files.add(&self.kiln, paths);
        let now = Instant::now();
        while self.rows.len() < self.files.len() {
            self.rows.push(RowTimes {
                added: now,
                settled: None,
            });
        }
        self.status = status_line(self.files.len(), r.added, r.duplicates, r.skipped);
        // Only the plain "Added 2 files." gives way to the idle hint; a line
        // about duplicates or skipped items stays until something replaces
        // it.
        self.added = ways_in::hint_follows(r.added, r.duplicates, r.skipped)
            .then(|| (self.status.clone(), Instant::now()));
        if self.added.is_some() {
            self.arm_hint(ways_in::ADDED_FOR);
        }
        // New rows go on the end of the list.
        let fresh: Vec<PathBuf> = self.files.entries()[self.files.len() - r.added..]
            .iter()
            .map(|e| e.path.clone())
            .collect();
        self.inspect(fresh);
        if r.added > 0 {
            self.last_add = self.files.entries()[self.files.len() - 1]
                .path
                .parent()
                .map(Path::to_path_buf);
            self.files.select(Some(self.files.len() - 1));
            self.scroll_to_selection();
        }
        self.animate();
    }

    /// Wake in `wait` to show the idle hint, with a little slack, as a
    /// timer can fire a few milliseconds early.
    fn arm_hint(&self, wait: Duration) {
        let ms = (wait + HINT_SLACK).as_millis().min(u32::MAX as u128) as u32;
        unsafe {
            SetTimer(Some(self.hwnd), HINT_TIMER, ms, None);
        }
    }

    /// How much longer "Added 2 files." has to stay, if it's still what the
    /// footer says and its two seconds aren't up.
    fn hint_wait(&self) -> Option<Duration> {
        self.added
            .as_ref()
            .filter(|(said, _)| *said == self.status)
            .and_then(|(_, at)| ways_in::hint_wait(at.elapsed()))
    }

    /// Ask for files with the Windows Open dialog, and add them as a drop
    /// would.
    fn add_files(&mut self) {
        let picked = system::pick_files(
            self.hwnd,
            ways_in::ADD_FILES_TITLE,
            self.last_add.as_deref(),
        );
        if !picked.is_empty() {
            self.add(picked);
        }
    }

    /// Add the files copied in File Explorer, as a drop would.
    fn paste(&mut self) {
        let files = system::clipboard_files(self.hwnd);
        if files.is_empty() {
            self.status = ways_in::NOTHING_TO_PASTE.to_owned();
            self.invalidate();
        } else {
            self.add(files);
        }
    }

    /// The Add files button: in the middle of the empty window, or in the
    /// footer left of the other buttons.
    fn add_button(&self) -> Option<layout::Rect> {
        let l = self.layout();
        if self.files.is_empty() {
            return Some(layout::empty_state(&l.panel).button);
        }
        let clear = l.clear_button?;
        Some(layout::add_files_button(
            &clear,
            self.files.held_count() > 0,
        ))
    }

    /// Cancel every job that involves `path`.
    fn cancel_jobs_for(&self, path: &std::path::Path) {
        for (id, job) in &self.jobs {
            if job.paths.iter().any(|p| p == path) {
                self.runner.cancel(*id);
            }
        }
    }

    fn remove_selected(&mut self) {
        let Some(i) = self.files.selected() else {
            return;
        };
        let path = self.files.entries()[i].path.clone();
        self.leave(
            Leave::Remove,
            Rows::Files(vec![path.clone()]),
            Then::Remove(path),
        );
    }

    /// Take `path`'s row out of the list, stopping its job.
    fn remove_row(&mut self, path: &Path) {
        // The prompt can take a while; the list may have moved meanwhile.
        let Some(i) = self.files.index_of(path) else {
            return;
        };
        let path = self.files.entries()[i].path.clone();
        self.cancel_jobs_for(&path);
        self.inspector.cancel(&path);
        self.files.select(Some(i));
        self.files.remove_selected();
        self.rows.remove(i);
        self.notes.remove(&path);
        self.status = status_line(self.files.len(), 0, 0, 0);
        self.held_changed();
        self.clamp_scroll();
        self.invalidate();
    }

    fn clear(&mut self) {
        self.leave(Leave::Clear, Rows::All, Then::Clear);
    }

    /// Empty the list.
    fn clear_rows(&mut self) {
        // Stop what's running first, and forget it: results for files
        // that are no longer listed have nowhere to go.
        self.runner.cancel_all();
        self.inspector.cancel_all();
        self.jobs.clear();
        self.batches.clear();
        self.files.clear();
        self.rows.clear();
        self.notes.clear();
        self.scroll = 0.0;
        self.status.clear();
        self.held_changed();
        self.invalidate();
    }

    // Held results.

    /// Which rows of the list as it is now `rows` names.
    fn mask(&self, rows: &Rows) -> Vec<bool> {
        let mut mask = vec![matches!(rows, Rows::All); self.files.len()];
        if let Rows::Files(files) = rows {
            for f in files {
                if let Some(i) = self.files.index_of(f) {
                    mask[i] = true;
                }
            }
        }
        mask
    }

    /// The held files that only the rows `rows` names hold, less any an
    /// export is already moving.
    fn held_by(&self, rows: &Rows) -> Vec<PathBuf> {
        let mask = self.mask(rows);
        let saving = self.saving_files();
        self.files
            .held_only_by(|i| mask.get(i).copied().unwrap_or(false))
            .into_iter()
            .filter(|f| !saving.contains(f))
            .collect()
    }

    /// The held files exports are moving now.
    fn saving_files(&self) -> HashSet<PathBuf> {
        self.exports
            .values()
            .flat_map(|x| x.files.iter().cloned())
            .collect()
    }

    /// The rows, by file, whose results an export is moving now.
    fn saving_rows(&self) -> HashSet<PathBuf> {
        let saving = self.saving_files();
        self.files
            .entries()
            .iter()
            .filter(|e| {
                e.held()
                    .is_some_and(|h| h.iter().any(|p| saving.contains(p)))
            })
            .map(|e| e.path.clone())
            .collect()
    }

    fn row_saving(&self, i: usize) -> bool {
        self.files
            .entries()
            .get(i)
            .is_some_and(|e| self.saving_rows().contains(&e.path))
    }

    /// Leave the rows `rows` names by `verb`, then do `then`. When that
    /// would lose held results or stop running jobs, ask first. Export all
    /// saves them on a worker thread, and the leave goes ahead once every
    /// one is saved.
    fn leave(&mut self, verb: Leave, rows: Rows, then: Then) {
        // An export under way finishes first.
        if !self.exports.is_empty() {
            if matches!(then, Then::Close) {
                self.close_when_saved = true;
                self.status = "Saving… Kiln will close when it's done.".to_owned();
                self.invalidate();
                return;
            }
            let mask = self.mask(&rows);
            let saving = self.saving_rows();
            let busy = self
                .files
                .entries()
                .iter()
                .zip(&mask)
                .any(|(e, &picked)| picked && saving.contains(&e.path));
            if busy || self.leaving.is_some() {
                self.status = model::SAVING.to_owned();
                self.invalidate();
                return;
            }
        }
        let mask = self.mask(&rows);
        let held = self.held_by(&rows).len();
        // A new job on a row only replaces what it holds.
        let running = match then {
            Then::Start(..) => 0,
            _ => self
                .files
                .entries()
                .iter()
                .zip(&mask)
                .filter(|(e, &picked)| picked && matches!(e.job, JobState::Running { .. }))
                .count(),
        };
        let Some(prompt) = leave_prompt(held, running, verb) else {
            self.proceed(then);
            return;
        };
        match system::ask(self.hwnd, &prompt) {
            PromptButton::Cancel => {}
            PromptButton::ExportAll => self.export_then(rows, then),
            PromptButton::Discard | PromptButton::Stop(_) => {
                // Results that finished while the prompt was open go too.
                let held = self.held_by(&rows);
                self.discard(&held);
                self.proceed(then);
            }
        }
    }

    /// Export all, from a leave prompt: pick the folder, export on a
    /// worker thread, and go ahead with `then` once everything is saved.
    /// Cancelling the picker cancels the leave and leaves running jobs be.
    fn export_then(&mut self, rows: Rows, then: Then) {
        let held = self.held_by(&rows);
        if held.is_empty() {
            self.proceed(then);
            return;
        }
        let Some(dir) = self.pick_export_folder(&held) else {
            return;
        };
        // Results that finished while the picker was open go too.
        let held = self.held_by(&rows);
        if let Some(export) = self.export(held, Dest::Into(dir.clone()), true) {
            self.leaving = Some(Leaving {
                export,
                rows,
                folder: dir,
                then,
            });
        }
    }

    /// Go ahead with a leave. Only now are its rows' jobs stopped.
    fn proceed(&mut self, then: Then) {
        match then {
            Then::Close => {
                self.close_when_saved = false;
                self.runner.cancel_all();
                unsafe {
                    let _ = DestroyWindow(self.hwnd);
                }
            }
            Then::Clear => self.clear_rows(),
            Then::Remove(path) => self.remove_row(&path),
            Then::Start(action, paths, quality) => self.launch(action, paths, quality),
        }
    }

    /// Throw held results away. Only files in this run's own folder.
    fn discard(&self, files: &[PathBuf]) {
        let Some(h) = &self.holding else {
            return;
        };
        for f in files.iter().filter(|f| f.parent() == Some(h.dir())) {
            let _ = std::fs::remove_file(f);
        }
    }

    /// Keep Windows told, while results are held, why it shouldn't shut
    /// down: "3 results aren't saved yet."
    fn held_changed(&mut self) {
        let n = self.files.held_count();
        if n == self.shutdown_told {
            return;
        }
        self.shutdown_told = n;
        let reason = leave_prompt(n, 0, Leave::Close).map(|p| p.title);
        system::shutdown_reason(self.hwnd, reason.as_deref());
    }

    /// Say on each row whose held result was removed by hand that it's
    /// gone, and stop saying it once it's back. A row being exported is
    /// left be: its files are on their way out.
    fn check_gone(&mut self) {
        let saving = self.saving_rows();
        let rows: Vec<(PathBuf, bool)> = self
            .files
            .entries()
            .iter()
            .filter(|e| e.held().is_some() && !saving.contains(&e.path))
            .map(|e| (e.path.clone(), e.is_gone(|p| p.exists())))
            .collect();
        for (path, gone) in rows {
            let noted = self.notes.get(&path).is_some_and(|(l, _)| l == model::GONE);
            if gone && !noted {
                self.notes
                    .insert(path, (model::GONE.to_owned(), Tone::Crack));
                self.invalidate();
            } else if !gone && noted {
                self.notes.remove(&path);
                self.invalidate();
            }
        }
    }

    /// Where an export dialog for `source`'s results opens.
    fn export_start(&self, source: &Path) -> PathBuf {
        model::export_start(
            self.last_export.as_deref(),
            self.settings.output_folder.as_deref(),
            source,
        )
    }

    /// The row holding `file`.
    fn source_of(&self, file: &Path) -> Option<PathBuf> {
        self.files
            .entries()
            .iter()
            .find(|e| e.held().is_some_and(|h| h.iter().any(|p| p == file)))
            .map(|e| e.path.clone())
    }

    /// Ask for the folder to export `files` into.
    fn pick_export_folder(&mut self, files: &[PathBuf]) -> Option<PathBuf> {
        let source = files.first().and_then(|f| self.source_of(f))?;
        let start = self.export_start(&source);
        let n = files.len();
        let title = format!("Export {n} file{} to", if n == 1 { "" } else { "s" });
        system::pick_folder_in(self.hwnd, &title, &start)
    }

    /// Whether `path` is inside this run's own folder, which is no place
    /// to export to: it's emptied when Kiln closes.
    fn in_holding(&self, path: &Path) -> bool {
        self.holding
            .as_ref()
            .is_some_and(|h| path.starts_with(h.dir()))
    }

    /// Of `files`, those the row for `source` still holds and no export is
    /// moving: a dialog can be open a while.
    fn still_held(&self, source: &Path, files: &[PathBuf]) -> Vec<PathBuf> {
        let saving = self.saving_files();
        let held = self
            .files
            .index_of(source)
            .and_then(|i| self.files.entries()[i].held())
            .unwrap_or(&[]);
        files
            .iter()
            .filter(|f| held.contains(f) && !saving.contains(*f))
            .cloned()
            .collect()
    }

    /// Export a row's results: the Save dialog for one result, a folder
    /// picker for several. A row already being exported is left be.
    fn export_row(&mut self, i: usize) {
        if self.leaving.is_some() {
            self.status = model::SAVING.to_owned();
            self.invalidate();
            return;
        }
        if self.row_saving(i) {
            return;
        }
        let Some(e) = self.files.entries().get(i) else {
            return;
        };
        let Some(held) = e.held().map(<[PathBuf]>::to_vec) else {
            return;
        };
        let source = e.path.clone();
        self.check_gone();
        if self
            .notes
            .get(&source)
            .is_some_and(|(l, _)| l == model::GONE)
        {
            return;
        }
        let [one] = held.as_slice() else {
            if let Some(dir) = self.pick_export_folder(&held) {
                let held = self.still_held(&source, &held);
                if !held.is_empty() {
                    self.export(held, Dest::Into(dir), false);
                }
            }
            return;
        };
        let start = self.export_start(&source);
        let name = one.file_name().unwrap_or_default().to_string_lossy();
        let ext = one.extension().unwrap_or_default().to_string_lossy();
        let Some(dest) = system::save_as(self.hwnd, "Export", &start, &name, &ext) else {
            return;
        };
        let held = self.still_held(&source, std::slice::from_ref(one));
        if !held.is_empty() {
            self.export(held, Dest::As(dest), false);
        }
    }

    /// Every held result, into one folder.
    fn export_everything(&mut self) {
        if self.leaving.is_some() {
            self.status = model::SAVING.to_owned();
            self.invalidate();
            return;
        }
        let held = self.held_by(&Rows::All);
        if held.is_empty() {
            return;
        }
        let Some(dir) = self.pick_export_folder(&held) else {
            return;
        };
        let held = self.held_by(&Rows::All);
        if !held.is_empty() {
            self.export(held, Dest::Into(dir), true);
        }
    }

    /// Export `files` to `dest` on a worker thread, one at a time; it
    /// reports back with WM_KILN_EXPORTED. `all` is an Export all. Returns
    /// the export's id, or `None` when it was settled on the spot.
    fn export(&mut self, files: Vec<PathBuf>, dest: Dest, all: bool) -> Option<u64> {
        let folder = match &dest {
            Dest::Into(dir) => dir.clone(),
            Dest::As(path) => path.parent().map(Path::to_path_buf).unwrap_or_default(),
        };
        if self.in_holding(&folder) {
            let results = files.into_iter().map(|f| (f, None)).collect();
            self.exported(&folder, results, all);
            return None;
        }
        let id = self.next_export;
        self.next_export += 1;
        self.exports.insert(
            id,
            Export {
                files: files.clone(),
                all,
            },
        );
        // Across drives a move is a copy, which can take a while: keep it
        // off the UI thread.
        let target = PostTarget(self.hwnd.0 as usize);
        std::thread::spawn(move || {
            let results = files
                .into_iter()
                .map(|f| {
                    let to = match &dest {
                        Dest::Into(dir) => kiln_core::export_into(&f, dir),
                        Dest::As(path) => kiln_core::export_as(&f, path),
                    };
                    (f, to.ok())
                })
                .collect();
            target.post(
                WM_KILN_EXPORTED,
                Exported {
                    id,
                    folder,
                    results,
                },
            );
        });
        self.invalidate();
        Some(id)
    }

    /// An export thread is done. A leave waiting on it goes ahead if
    /// everything was saved, exporting anything that finished meanwhile
    /// first; otherwise it's off, and the footer says what wasn't saved.
    fn on_exported(&mut self, ex: Exported) {
        let Some(export) = self.exports.remove(&ex.id) else {
            return;
        };
        let saved = self.exported(&ex.folder, ex.results, export.all);
        if !saved {
            self.close_when_saved = false;
        }
        if self.leaving.as_ref().is_some_and(|l| l.export == ex.id) {
            let Some(l) = self.leaving.take() else {
                return;
            };
            if saved {
                let late = self.held_by(&l.rows);
                if late.is_empty() {
                    self.proceed(l.then);
                } else if let Some(export) = self.export(late, Dest::Into(l.folder.clone()), true) {
                    self.leaving = Some(Leaving { export, ..l });
                }
            }
        }
        if self.close_when_saved && self.exports.is_empty() {
            // Close as asked; anything still at stake asks first.
            self.close_when_saved = false;
            unsafe {
                let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
    }

    /// Record what an export did: where each held file went, or `None`
    /// where it couldn't be saved. Returns whether every one was saved.
    /// An Export all sums up in the footer; a row's export speaks only
    /// through its row.
    fn exported(
        &mut self,
        folder: &Path,
        results: Vec<(PathBuf, Option<PathBuf>)>,
        all: bool,
    ) -> bool {
        let moved: Vec<(PathBuf, PathBuf)> = results
            .iter()
            .filter_map(|(f, to)| Some((f.clone(), to.clone()?)))
            .collect();
        let failed: Vec<&PathBuf> = results
            .iter()
            .filter(|(_, to)| to.is_none())
            .map(|(f, _)| f)
            .collect();
        if !moved.is_empty() {
            self.last_export = Some(folder.to_path_buf());
        }
        self.files.exported(&moved, folder);
        // A move across drives can leave the held copy behind after a good
        // copy; the export still counts, and the copy goes with the run.
        for e in self.files.entries() {
            match e.held() {
                Some(h) if h.iter().any(|p| failed.contains(&p)) => {
                    let note = if e.is_gone(|p| p.exists()) {
                        model::GONE.to_owned()
                    } else {
                        model::save_failed_line(folder)
                    };
                    self.notes.insert(e.path.clone(), (note, Tone::Crack));
                }
                None if e.saved().is_some() => {
                    self.notes.remove(&e.path);
                }
                _ => {}
            }
        }
        self.status = if all {
            model::export_summary(moved.len(), results.len())
        } else {
            status_line(self.files.len(), 0, 0, 0)
        };
        self.held_changed();
        self.invalidate();
        failed.is_empty()
    }

    /// Open Explorer on an exported row's results.
    fn show_saved(&mut self, i: usize) {
        let Some((folder, files)) = self.files.entries().get(i).and_then(|e| e.saved()) else {
            return;
        };
        if system::show_in_folder(folder, files).is_err() {
            self.status = format!("Couldn't open {}.", folder.display());
            self.invalidate();
        }
    }

    /// Double-click on a held row: open its result in its usual app, so it
    /// can be checked before it's kept. False if `(x, y)` isn't on one.
    fn open_held_at(&mut self, x: f32, y: f32) -> bool {
        if self.wheel.is_some()
            || self.sheet.is_some()
            || self.settings_ui.is_some()
            || self.details.is_some()
        {
            return false;
        }
        let (Some(i), false, false) = self.hit_row(x, y) else {
            return false;
        };
        self.open_held(i)
    }

    /// Open a held row's result in its usual app. False if the row holds
    /// nothing that can be opened now.
    fn open_held(&mut self, i: usize) -> bool {
        if self.row_saving(i) {
            return false;
        }
        let Some(file) = self
            .files
            .entries()
            .get(i)
            .and_then(|e| e.held())
            .and_then(|h| h.first())
            .cloned()
        else {
            return false;
        };
        if !system::open_file(self.hwnd, &file) {
            let name = file.file_name().unwrap_or_default().to_string_lossy();
            self.status = format!("Couldn't open {name}.");
            self.check_gone();
            self.invalidate();
        }
        true
    }

    /// Follow a row's link: a failed row's details, a held row's export, or
    /// an exported row's folder.
    fn follow_link(&mut self, i: usize) {
        if self.row_saving(i) {
            return;
        }
        let Some(e) = self.files.entries().get(i) else {
            return;
        };
        if e.held().is_some() {
            self.export_row(i);
        } else if e.saved().is_some() {
            self.show_saved(i);
        } else {
            self.show_details(i);
        }
    }

    // Suggestions.

    /// Queue `paths` for inspecting, while suggestions are on. Each job
    /// carries the options a conversion between files of its type would
    /// run with, so the inspect thread never reads the app's state.
    fn inspect(&mut self, paths: Vec<PathBuf>) {
        if !self.settings.suggestions {
            return;
        }
        for path in paths {
            let opts = self.inspect_options(&path);
            self.inspector.queue(path, opts);
        }
    }

    fn inspect_options(&self, path: &Path) -> Options {
        match self.kiln.registry().format_of(path) {
            Some(f) => self.settings.options_for(f, f),
            None => self.settings.options(),
        }
    }

    /// Which engines are installed, as last looked for.
    fn availability(&self) -> &Availability {
        self.engines
            .get(|| self.kiln.availability(&self.settings.options()))
    }

    /// What Kiln puts forward for one file, given what it offers for it:
    /// its conversion, or else its tool.
    fn suggestion_for(&self, facts: &Facts, offered: &Offered) -> Option<Suggestion> {
        let s = suggest(std::slice::from_ref(facts), offered, &self.settings.habits);
        s.convert.or(s.tool)
    }

    fn on_inspected(&mut self, found: Inspected) {
        // Posted before Suggestions was turned off.
        if !self.settings.suggestions {
            return;
        }
        let offered = self
            .kiln
            .offered_with(&[found.path.as_path()], self.availability());
        let suggestion = self.suggestion_for(&found.facts, &offered);
        if self.files.set_facts(&found.path, found.facts, suggestion) {
            self.animate();
        }
        // An open wheel waiting on this file gets its suggestion now, and
        // its dot fades in. Nothing else on it changes.
        let waiting = self
            .wheel
            .as_ref()
            .filter(|w| w.is_open() && w.suggestion.is_none() && w.paths.contains(&found.path))
            .map(|w| (w.wheel.kind, w.paths.clone()));
        let Some((kind, paths)) = waiting else {
            return;
        };
        if let Some(s) = self.wheel_suggestion(kind, &paths) {
            if let Some(w) = self.wheel.as_mut() {
                w.suggestion = Some(s);
                w.dot_at = Some(Instant::now());
            }
            self.animate();
        }
    }

    /// What a wheel of `kind` suggests for `paths`, once every file's
    /// facts are in: the conversion on Convert, the tool on Tools, nothing
    /// on Actions.
    fn wheel_suggestion(&self, kind: WheelKind, paths: &[PathBuf]) -> Option<Suggestion> {
        if !self.settings.suggestions || kind == WheelKind::Actions {
            return None;
        }
        let facts = self.files.facts_for(paths)?;
        let refs: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
        let offered = self.kiln.offered_with(&refs, self.availability());
        let s = suggest(&facts, &offered, &self.settings.habits);
        match kind {
            WheelKind::Convert => s.convert,
            WheelKind::Tools => s.tool,
            WheelKind::Actions => None,
        }
    }

    /// Habits or engines changed: every row's suggestion follows from its
    /// facts, with no new fade. Engines are looked for at most once, and
    /// what Kiln offers is worked out once per type.
    fn habits_changed(&mut self) {
        let available = self.availability();
        let mut offers: HashMap<String, Offered> = HashMap::new();
        let fresh: Vec<Option<Option<Suggestion>>> = self
            .files
            .entries()
            .iter()
            .map(|e| {
                let f = e.facts.as_ref()?;
                let offered = offers
                    .entry(f.ext.clone())
                    .or_insert_with(|| self.kiln.offered_with(&[e.path.as_path()], available));
                Some(self.suggestion_for(f, offered))
            })
            .collect();
        for (e, s) in self.files.entries_mut().iter_mut().zip(fresh) {
            if let Some(s) = s {
                e.suggestion = s;
            }
        }
        self.invalidate();
    }

    fn clamp_scroll(&mut self) {
        let l = self.layout();
        self.scroll = self
            .scroll
            .clamp(0.0, layout::max_scroll(self.files.len(), l.panel.h));
    }

    fn scroll_to_selection(&mut self) {
        if let Some(i) = self.files.selected() {
            let l = self.layout();
            self.scroll = layout::scroll_to_show(i, self.scroll, l.panel.h);
            self.clamp_scroll();
        }
    }

    // The wheel.

    fn open_wheel(&mut self, kind: WheelKind, paths: Vec<PathBuf>, at: (f32, f32), keyboard: bool) {
        let (w, h) = self.renderer.size_dips();
        let (items, more) = wheel::split_common(wheel::items_for(kind, &self.kiln, &paths));
        let mut wheel = Wheel {
            kind,
            items,
            more,
            center: (0.0, 0.0),
            scale: 1.0,
            source_code: wheel::source_code(&paths),
        };
        // Placed so the outer ring fits too, should More be opened.
        (wheel.center, wheel.scale) = wheel::place(at.0, at.1, w, h, wheel.extent());
        let suggestion = self.wheel_suggestion(kind, &paths);
        // From the keyboard it starts on the suggestion, or on More when
        // the suggestion is in it.
        let start = wheel::start_spot(&wheel, suggestion.as_ref().map(|s| &s.action));
        let hover = (keyboard && wheel.segments() > 0).then_some(start);
        self.wheel = Some(WheelUi {
            wheel,
            paths,
            hover,
            more: Tween::new(0.0),
            opened: Instant::now(),
            closing: None,
            pulse: None,
            keyboard,
            suggestion,
            dot_at: None,
        });
        self.animate();
    }

    /// Open a wheel for the selected file, from the keyboard.
    fn open_wheel_for_selection(&mut self, kind: WheelKind) {
        let Some(i) = self.files.selected() else {
            return;
        };
        // From the keyboard there's no pointer to pour from, so the wheel
        // opens in the middle of the list.
        let l = self.layout();
        let at = (l.panel.x + l.panel.w / 2.0, l.panel.y + l.panel.h / 2.0);
        let paths = vec![self.files.entries()[i].path.clone()];
        self.open_wheel(kind, paths, at, true);
    }

    // The right-click menu.

    /// The middle of row `i` as it shows now, where a wheel opened from
    /// the menu pours.
    fn row_centre(&self, i: usize) -> (f32, f32) {
        let r = self.layout().rows[i];
        (r.x + r.w / 2.0, r.y - self.scroll + r.h / 2.0)
    }

    /// A point in window DIPs, in screen pixels.
    fn to_screen(&self, x: f32, y: f32) -> (i32, i32) {
        let k = self.dpi / 96.0;
        let mut pt = POINT {
            x: (x * k).round() as i32,
            y: (y * k).round() as i32,
        };
        unsafe {
            let _ = ClientToScreen(self.hwnd, &mut pt);
        }
        (pt.x, pt.y)
    }

    /// What the menu offers for row `i`.
    fn row_state(&self, i: usize) -> RowState {
        self.files.entries()[i].row_state(self.row_saving(i), |p| p.exists())
    }

    /// The right-click menu. `at` is where it was asked for, in screen
    /// pixels, or `None` from the keyboard (the Menu key or Shift+F10),
    /// which opens it at the selected row. A row gets the row menu; an
    /// empty part of the list gets Add files and Paste.
    fn context_menu(&mut self, at: Option<(i32, i32)>) {
        let row = match at {
            Some((x, y)) => {
                let (x, y) = self.screen_dips(x, y);
                if !self.layout().panel.contains(x, y) {
                    return;
                }
                // Right-clicking a row selects it first, and an empty part
                // of the list clears the selection, as File Explorer does.
                let (row, _, _) = self.hit_row(x, y);
                self.files.select(row);
                row
            }
            None => self.files.selected(),
        };
        let items = match row {
            Some(i) => {
                let paths = vec![self.files.entries()[i].path.clone()];
                let wheels: Vec<WheelKind> = WheelKind::ALL
                    .into_iter()
                    .filter(|&k| !wheel::items_for(k, &self.kiln, &paths).is_empty())
                    .collect();
                ways_in::menu_items(&[self.row_state(i)], &wheels)
            }
            None => ways_in::list_menu(system::clipboard_has_files()),
        };
        let texts: Vec<Option<String>> = items
            .iter()
            .map(|&item| (item != MenuItem::Separator).then(|| item.text(&self.settings.keys)))
            .collect();
        let (x, y) = match (at, row) {
            (Some(p), _) => p,
            // From the keyboard: at the row's bottom left, scrolled into
            // view, or the list's top left with nothing selected.
            (None, Some(i)) => {
                self.scroll_to_selection();
                let r = self.layout().rows[i];
                self.to_screen(r.x, r.bottom() - self.scroll)
            }
            (None, None) => {
                let panel = self.layout().panel;
                self.to_screen(panel.x + 16.0, panel.y + 16.0)
            }
        };
        self.invalidate();
        // The row by its file: the list can change while the menu is up.
        let file = row.map(|i| self.files.entries()[i].path.clone());
        let Some(k) = system::popup_menu(self.hwnd, &texts, x, y) else {
            return;
        };
        let row = match &file {
            Some(f) => match self.files.index_of(f) {
                Some(i) => Some(i),
                None => return,
            },
            None => None,
        };
        self.menu_chosen(items[k], row, at.is_none());
    }

    /// Do what the menu item `item` says, for `row`.
    fn menu_chosen(&mut self, item: MenuItem, row: Option<usize>, keyboard: bool) {
        match (item, row) {
            (MenuItem::AddFiles, _) => self.add_files(),
            (MenuItem::Paste, _) => self.paste(),
            (MenuItem::Wheel(kind), Some(i)) => {
                let paths = vec![self.files.entries()[i].path.clone()];
                let at = self.row_centre(i);
                self.open_wheel(kind, paths, at, keyboard);
            }
            (MenuItem::Export, Some(i)) => self.export_row(i),
            (MenuItem::Open, Some(i)) => {
                self.open_held(i);
            }
            (MenuItem::ShowInFolder, Some(i)) => self.show_saved(i),
            (MenuItem::Remove, Some(i)) => {
                self.files.select(Some(i));
                self.remove_selected();
            }
            _ => {}
        }
        self.invalidate();
    }

    fn close_wheel(&mut self) {
        if let Some(w) = &mut self.wheel {
            if w.closing.is_none() {
                w.closing = Some(Instant::now());
                self.animate();
            }
        }
    }

    /// Choose `spot`: More pours its ring and moves in to the suggestion
    /// when it is there, or else to its first format; anything else
    /// pulses, and its job starts when the pulse ends.
    fn choose(&mut self, spot: Spot) {
        let Some(w) = self.wheel.as_mut().filter(|w| w.is_open()) else {
            return;
        };
        match spot {
            Spot::Inner(i) if w.wheel.is_more(i) => {
                // The keyboard can start on More with its ring still shut,
                // so pour it here rather than through `point`.
                w.more.set(1.0, OPEN_MS);
                let first = w
                    .marks()
                    .into_iter()
                    .find(|m| matches!(m, Spot::Outer(_)))
                    .unwrap_or(Spot::Outer(0));
                w.hover = Some(first);
            }
            _ if w.wheel.item(spot).is_some() => {
                w.pulse = Some((spot, Instant::now()));
                w.hover = Some(spot);
            }
            _ => return,
        }
        self.animate();
    }

    /// Point the open wheel at whatever is under `(x, y)`.
    fn wheel_point(&mut self, x: f32, y: f32) {
        if let Some(w) = self.wheel.as_mut().filter(|w| w.is_open()) {
            let spot = wheel::hit(&w.wheel, x, y, w.more_open());
            if w.point(spot) {
                self.animate();
            }
        }
    }

    /// The convert target at `spot`, if the options sheet has anything to
    /// offer for it.
    fn sheet_target(&self, spot: Option<Spot>) -> Option<(Format, Format, Vec<Knob>)> {
        let w = self.wheel.as_ref()?;
        let Action::Convert(ext) = &w.wheel.item(spot?)?.action else {
            return None;
        };
        let reg = self.kiln.registry();
        let from = reg.format_of(w.paths.first()?)?.clone();
        let to = reg.format(ext)?.clone();
        let knobs = kiln_core::quality::knobs(&from, &to);
        (!knobs.is_empty()).then_some((from, to, knobs))
    }

    /// The footer's hint on the open wheel: the suggestion's reason while
    /// nothing else is lit, or how to reach a lit format's options.
    fn wheel_hint(&self) -> Option<String> {
        let w = self.wheel.as_ref().filter(|w| w.is_open())?;
        if let Some(s) = &w.suggestion {
            let marks = w.marks();
            if wheel::shows_suggestion(w.hover, &marks) {
                // The label as the wheel shows it: the item itself, which
                // is the last mark (its outer slot when it is in More).
                if let Some(item) = marks.last().and_then(|&m| w.wheel.item(m)) {
                    return Some(wheel::suggestion_footer(&item.label, &s.reason));
                }
            }
        }
        let (_, to, _) = self.sheet_target(w.hover)?;
        Some(format!("Tab or right-click for {} options.", to.code()))
    }

    /// Advance the animations: start a pulsed job, drop a faded wheel or
    /// sheet.
    fn tick(&mut self) {
        if self
            .sheet
            .as_ref()
            .is_some_and(|s| s.closing.is_some_and(|t| !within(t, motion::QUICK_MS)))
        {
            self.sheet = None;
        }
        let Some(w) = &mut self.wheel else {
            return;
        };
        if let Some((spot, t)) = w.pulse {
            if !within(t, PULSE_MS) {
                w.pulse = None;
                w.closing = Some(Instant::now());
                let Some(item) = w.wheel.item(spot) else {
                    return;
                };
                let action = item.action.clone();
                let paths = w.paths.clone();
                self.start(action, paths, None);
                return;
            }
        }
        if w.closing.is_some_and(|t| !within(t, motion::QUICK_MS)) {
            self.wheel = None;
        }
    }

    fn wheel_view<'a>(w: Option<&'a WheelUi>, keys: &settings::KeyMap) -> Option<WheelView<'a>> {
        let w = w?;
        Some(WheelView {
            wheel: &w.wheel,
            title: ways_in::wheel_title(w.wheel.kind, keys),
            hover: w.hover,
            more: w.more.value().clamp(0.0, 1.0),
            open: eased(w.opened, OPEN_MS),
            fade: w.closing.map_or(1.0, |t| 1.0 - eased(t, motion::QUICK_MS)),
            pulse: w
                .pulse
                .map(|(s, t)| (s, t.elapsed().as_secs_f32() * 1000.0 / PULSE_MS as f32)),
            marks: w.marks(),
            dot: w.dot_at.map_or(1.0, |t| eased(t, FACTS_MS)),
        })
    }

    // The options sheet.

    /// Open the options sheet for the format at `spot` on the wheel. The
    /// wheel fades as the sheet rises.
    fn open_sheet(&mut self, spot: Option<Spot>) -> bool {
        let Some((from, to, knobs)) = self.sheet_target(spot) else {
            return false;
        };
        let Some(w) = &self.wheel else {
            return false;
        };
        let paths = w.paths.clone();
        let keyboard = w.keyboard;
        self.close_wheel();
        let names = paths
            .iter()
            .map(|p| {
                p.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        let bytes = paths
            .iter()
            .filter_map(|p| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .sum();
        let id = self.next_sheet;
        self.next_sheet += 1;
        let quality = self.settings.quality.quality_for(&from, &to);
        let (sw, sh) = self.renderer.size_dips();
        let convert = layout::sheet(sw, sh, &knobs).controls.len() - 1;
        self.sheet = Some(SheetUi {
            id,
            paths: paths.clone(),
            from,
            to,
            knobs,
            quality,
            infos: None,
            names,
            bytes,
            hover: None,
            // From the keyboard, Convert is ready for Enter.
            focus: keyboard.then_some(convert),
            pressed: None,
            opened: Instant::now(),
            closing: None,
        });
        // Reading media takes a moment; the sheet shows meanwhile.
        let kiln = self.kiln.clone();
        let opts = self.settings.options();
        let target = PostTarget(self.hwnd.0 as usize);
        std::thread::spawn(move || {
            let infos = paths
                .iter()
                .take(PROBE_LIMIT)
                .map(|p| kiln.probe(p, &opts).unwrap_or_default())
                .collect();
            target.post(WM_KILN_PROBE, Probed { id, infos });
        });
        self.animate();
        true
    }

    fn on_probed(&mut self, p: Probed) {
        if let Some(s) = self.sheet.as_mut().filter(|s| s.id == p.id) {
            // Estimates only count when every file was read.
            s.infos = Some(if p.infos.len() == s.paths.len() {
                p.infos
            } else {
                vec![]
            });
            self.invalidate();
        }
    }

    fn sheet_layout(&self) -> Option<SheetLayout> {
        let s = self.sheet.as_ref()?;
        let (w, h) = self.renderer.size_dips();
        Some(layout::sheet(w, h, &s.knobs))
    }

    fn close_sheet(&mut self, convert: bool) {
        let Some(s) = self.sheet.as_mut().filter(|s| s.is_open()) else {
            return;
        };
        s.closing = Some(Instant::now());
        s.hover = None;
        if convert {
            let action = Action::Convert(s.to.ext.clone());
            let (paths, quality) = (s.paths.clone(), s.quality);
            self.start(action, paths, Some(quality));
        }
        self.animate();
    }

    /// Press a sheet control. `delta` steps a value back or forward.
    fn sheet_activate(&mut self, c: SheetControl, delta: isize) {
        match c {
            SheetControl::Cancel => return self.close_sheet(false),
            SheetControl::Convert => return self.close_sheet(true),
            SheetControl::Preset(_, preset) => {
                if let Some(s) = &mut self.sheet {
                    s.quality.preset = preset;
                }
            }
            SheetControl::Step(row) => {
                if let Some(s) = &mut self.sheet {
                    s.step(row, delta);
                }
            }
        }
        self.invalidate();
    }

    fn sheet_hit(&self, x: f32, y: f32) -> Option<usize> {
        self.sheet_layout()?
            .controls
            .iter()
            .position(|(_, r)| r.contains(x, y))
    }

    /// Tab and the up and down arrows move focus; left and right change
    /// the focused value; Enter converts unless Cancel has focus; Space
    /// presses; Escape cancels.
    fn sheet_key(&mut self, key: VIRTUAL_KEY, shift: bool) {
        let Some(l) = self.sheet_layout() else {
            return;
        };
        let n = l.controls.len();
        let Some(s) = self.sheet.as_mut().filter(|s| s.is_open()) else {
            return;
        };
        let focused = s.focus.map(|i| l.controls[i].0);
        match key {
            VK_ESCAPE => return self.close_sheet(false),
            VK_TAB | VK_DOWN | VK_UP => {
                let back = (key == VK_TAB && shift) || key == VK_UP;
                // Up and down skip between rows, not between segments.
                let mut f = s.focus;
                loop {
                    f = wheel::step(f, if back { -1 } else { 1 }, n);
                    let next = f.map(|i| l.controls[i].0);
                    let same_row = key != VK_TAB
                        && matches!((next, focused), (Some(SheetControl::Preset(a, _)), Some(SheetControl::Preset(b, _))) if a == b);
                    if !same_row || f == s.focus {
                        break;
                    }
                }
                s.focus = f;
            }
            VK_LEFT | VK_RIGHT => {
                let delta = if key == VK_LEFT { -1 } else { 1 };
                match focused {
                    Some(SheetControl::Step(row)) => s.step(row, delta),
                    Some(SheetControl::Preset(row, _)) => {
                        let i = s.focus.unwrap_or(0) as isize + delta;
                        if let Some((SheetControl::Preset(r, p), _)) =
                            l.controls.get(i.max(0) as usize)
                        {
                            if *r == row {
                                s.focus = Some(i as usize);
                                s.quality.preset = *p;
                            }
                        }
                    }
                    _ => {
                        s.focus = wheel::step(s.focus, delta, n);
                    }
                }
            }
            VK_RETURN => {
                let convert = focused != Some(SheetControl::Cancel);
                return self.close_sheet(convert);
            }
            VK_SPACE => {
                if let Some(c) = focused {
                    return self.sheet_activate(c, 1);
                }
            }
            _ => return,
        }
        self.invalidate();
    }

    // Jobs.

    /// Start `action` on `paths`. A convert runs at `quality`, or at the
    /// defaults from settings for its kind of file.
    fn start(&mut self, action: Action, paths: Vec<PathBuf>, quality: Option<Quality>) {
        // Only files that made it into the list.
        let paths: Vec<PathBuf> = paths
            .into_iter()
            .filter(|p| self.files.index_of(p).is_some())
            .collect();
        if paths.is_empty() {
            return;
        }
        // A new job or a move replaces what a row holds: ask first.
        if replaces(&action) {
            let rows = Rows::Files(paths.clone());
            self.leave(Leave::Remove, rows, Then::Start(action, paths, quality));
        } else {
            self.launch(action, paths, quality);
        }
    }

    /// Start `action` on `paths`, once nothing they hold is at stake.
    fn launch(&mut self, action: Action, paths: Vec<PathBuf>, quality: Option<Quality>) {
        // An export it waited on may have taken a while.
        let paths: Vec<PathBuf> = paths
            .into_iter()
            .filter(|p| self.files.index_of(p).is_some())
            .collect();
        if paths.is_empty() {
            return;
        }
        if replaces(&action) {
            for p in &paths {
                self.notes.remove(p);
            }
        }
        self.record_habit(&action, &paths);
        if let Action::File(a) = action {
            self.file_action(a, paths);
            return;
        }
        let batch = self.next_batch;
        self.next_batch += 1;
        let groups: Vec<Vec<PathBuf>> = match action {
            Action::Tool(Tool::Merge) => vec![paths.clone()],
            _ => paths.iter().map(|p| vec![p.clone()]).collect(),
        };
        self.batches.insert(
            batch,
            Batch {
                action: action.clone(),
                files: paths.len(),
                ok: 0,
                failed: 0,
                cancelled: 0,
                pending: groups.len(),
                held: 0,
            },
        );
        for group in groups {
            let kind = match &action {
                Action::Convert(ext) => JobKind::Convert {
                    input: group[0].clone(),
                    target: ext.clone(),
                },
                Action::Tool(tool) => JobKind::Tool {
                    tool: *tool,
                    inputs: group.clone(),
                },
                Action::File(_) => unreachable!("file actions return above"),
            };
            let mut options = match (&action, quality) {
                (_, Some(q)) => self.settings.options_with(q),
                (Action::Convert(ext), None) => {
                    let reg = self.kiln.registry();
                    match (reg.format_of(&group[0]), reg.format(ext)) {
                        (Some(from), Some(to)) => self.settings.options_for(from, to),
                        _ => self.settings.options(),
                    }
                }
                _ => self.settings.options(),
            };
            // Results wait in this run's folder until they're exported.
            options.out_dir = self.holding.as_ref().map(|h| h.dir().to_path_buf());
            let id = self.runner.submit(kind, options);
            for p in &group {
                if let Some(i) = self.files.index_of(p) {
                    self.files.entries_mut()[i].job = JobState::Running {
                        action: action.clone(),
                        fraction: None,
                    };
                    self.rows[i].settled = None;
                }
            }
            self.jobs.insert(
                id,
                JobInfo {
                    paths: group,
                    action: action.clone(),
                    batch,
                },
            );
        }
        let n = paths.len();
        self.status = format!("Working on {n} file{}.", if n == 1 { "" } else { "s" });
        self.held_changed();
        self.animate();
    }

    /// Count a conversion's picks, once per source type, and save them.
    /// Tools and file actions are not counted.
    fn record_habit(&mut self, action: &Action, paths: &[PathBuf]) {
        let reg = self.kiln.registry();
        // Aliases count as their format, as the suggestions read them:
        // a .jpeg file is a jpg.
        let canon = |e: String| reg.format(&e).map_or(e, |f| f.ext.clone());
        let picks: BTreeSet<(String, String)> = habit_picks(action, paths)
            .into_iter()
            .map(|(from, to)| (canon(from), canon(to)))
            .collect();
        if picks.is_empty() {
            return;
        }
        let before = favourites(&self.settings.habits);
        for (from, to) in &picks {
            self.settings.habits.record(from, to);
        }
        let changed = favourites(&self.settings.habits) != before;
        // Picks can't change the engines, so the cached search stands.
        // Not over a settings.toml that couldn't be read: see Store.
        if let Err(e) = self.settings_store.save_picks(&self.settings) {
            self.status = e;
        }
        // Suggestions follow favourites only: no new favourite, nothing to
        // work out again.
        if changed {
            self.habits_changed();
        }
    }

    fn on_job(&mut self, ev: JobEvent) {
        match ev {
            JobEvent::Progress(id, p) => {
                let Some(job) = self.jobs.get(&id) else {
                    return;
                };
                let f = match p {
                    Progress::Fraction(f) => Some(f),
                    Progress::Indeterminate => None,
                };
                for path in &job.paths {
                    if let Some(i) = self.files.index_of(path) {
                        if let JobState::Running { fraction, .. } =
                            &mut self.files.entries_mut()[i].job
                        {
                            *fraction = f;
                        }
                    }
                }
            }
            JobEvent::Finished(id, result) => {
                let Some(job) = self.jobs.remove(&id) else {
                    return;
                };
                let now = Instant::now();
                let state = match &result {
                    Ok(outputs) => JobState::Done {
                        action: job.action.clone(),
                        outputs: outputs.clone(),
                        // Without a holding folder, results were written
                        // next to their files: they're already saved.
                        saved: match &self.holding {
                            Some(_) => None,
                            None => outputs
                                .first()
                                .and_then(|p| p.parent())
                                .map(Path::to_path_buf),
                        },
                    },
                    Err(e) if e.cancelled => JobState::Cancelled,
                    Err(e) => JobState::Failed {
                        // One plain line in the row; the rest is in the
                        // details.
                        message: self.no_room(e).unwrap_or_else(|| {
                            e.message.lines().next().unwrap_or_default().to_owned()
                        }),
                        details: e.details.clone(),
                    },
                };
                for path in &job.paths {
                    if let Some(i) = self.files.index_of(path) {
                        self.files.entries_mut()[i].job = state.clone();
                        self.rows[i].settled = Some(now);
                    }
                }
                if let Some(b) = self.batches.get_mut(&job.batch) {
                    match &result {
                        Ok(outputs) => {
                            b.ok += 1;
                            if self.holding.is_some() {
                                b.held += outputs.len();
                            }
                        }
                        Err(e) if e.cancelled => b.cancelled += 1,
                        Err(_) => b.failed += 1,
                    }
                    b.pending -= 1;
                    if b.pending == 0 {
                        let summary =
                            batch_summary(&b.action, b.files, b.ok, b.failed, b.cancelled);
                        self.status = model::with_export_hint(summary, b.held);
                        self.batches.remove(&job.batch);
                    }
                }
                self.held_changed();
                self.animate();
            }
        }
        self.invalidate();
    }

    /// "There isn't room on C: to hold the result.", when a job failed for
    /// want of room on the holding folder's drive.
    fn no_room(&self, e: &JobError) -> Option<String> {
        model::no_room_line(e.no_room.as_deref()?, self.holding.as_ref()?.dir())
    }

    // File actions.

    fn file_action(&mut self, a: FileAction, paths: Vec<PathBuf>) {
        match a {
            FileAction::CopyPath => {
                let text = actions::clipboard_text(&paths);
                self.status = match system::set_clipboard(self.hwnd, &text) {
                    Ok(()) => actions::copied_summary(paths.len()),
                    Err(e) => format!("Couldn't copy the path: {}", e.message()),
                };
            }
            FileAction::OpenTerminal => {
                let Some(folder) = actions::terminal_folder(&paths) else {
                    return;
                };
                self.status = match actions::terminal_command(&folder).spawn() {
                    Ok(_) => format!("Opened Terminal in {}.", actions::folder_name(&folder)),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        "Windows Terminal isn't installed. Get it from the Microsoft Store."
                            .to_owned()
                    }
                    Err(e) => format!("Couldn't open Terminal: {e}"),
                };
            }
            FileAction::Move => self.move_files(paths),
        }
        self.invalidate();
    }

    fn move_files(&mut self, paths: Vec<PathBuf>) {
        let busy = paths.iter().any(|p| {
            self.files
                .index_of(p)
                .is_some_and(|i| matches!(self.files.entries()[i].job, JobState::Running { .. }))
        });
        if busy {
            self.status = "Kiln can't move a file while a job is using it.".to_owned();
            return;
        }
        let title = if paths.len() == 1 {
            "Move the file to"
        } else {
            "Move the files to"
        };
        let Some(dest) = system::pick_folder(self.hwnd, title) else {
            return;
        };
        let action = Action::File(FileAction::Move);
        for p in &paths {
            if let Some(i) = self.files.index_of(p) {
                self.files.entries_mut()[i].job = JobState::Running {
                    action: action.clone(),
                    fraction: None,
                };
                self.rows[i].settled = None;
            }
        }
        self.animate();
        // Across drives a move is a copy, which can take a while: keep it
        // off the UI thread.
        let target = PostTarget(self.hwnd.0 as usize);
        std::thread::spawn(move || {
            let results = paths
                .into_iter()
                .map(|p| {
                    let r = actions::move_to(&p, &dest);
                    (p, r)
                })
                .collect();
            target.post(WM_KILN_MOVED, Moved { dest, results });
        });
    }

    fn on_moved(&mut self, m: Moved) {
        let now = Instant::now();
        let (mut ok, mut failed) = (0, 0);
        for (old, result) in m.results {
            self.notes.remove(&old);
            let index = match &result {
                Ok(new) => {
                    let i = self.files.relocate(&self.kiln, &old, new.clone());
                    // An inspect still under way read the old path.
                    if i.is_some_and(|i| self.files.entries()[i].facts.is_none()) {
                        self.inspector.cancel(&old);
                        self.inspect(vec![new.clone()]);
                    }
                    i
                }
                Err(_) => self.files.index_of(&old),
            };
            let state = match result {
                Ok(new) => {
                    ok += 1;
                    JobState::Done {
                        action: Action::File(FileAction::Move),
                        outputs: vec![new],
                        saved: None,
                    }
                }
                Err(message) => {
                    failed += 1;
                    JobState::Failed {
                        message,
                        details: String::new(),
                    }
                }
            };
            if let Some(i) = index {
                self.files.entries_mut()[i].job = state;
                self.rows[i].settled = Some(now);
            }
        }
        self.status = actions::moved_summary(ok, failed, &m.dest);
        self.held_changed();
        self.animate();
    }

    // Settings.

    /// Read settings.toml. Called once the window is on screen.
    fn load_settings(&mut self) {
        let (s, err) = self.settings_store.load();
        self.settings = s;
        self.engines.forget();
        if let Some(e) = err {
            self.status = format!("{e}. Using the defaults.");
            log(&self.status);
        }
        self.apply_theme();
    }

    /// Save after an explicit change in Settings.
    fn save_settings(&mut self) {
        // The engines may be others now.
        self.engines.forget();
        if let Err(e) = self.settings_store.save(&self.settings) {
            self.status = e;
        }
    }

    fn apply_theme(&mut self) {
        let p = if self.settings.theme.is_light(self.system_light) {
            &theme::LIGHT
        } else {
            &theme::DARK
        };
        if !std::ptr::eq(p, self.palette) {
            self.palette = p;
            style_title_bar(self.hwnd, p);
        }
        self.invalidate();
    }

    fn settings_layout(&self) -> SettingsLayout {
        let (w, h) = self.renderer.size_dips();
        let e = &self.settings.engines;
        layout::settings(
            w,
            h,
            SettingsShape {
                tab: self
                    .settings_ui
                    .as_ref()
                    .map_or_else(SettingsTab::default, |ui| ui.tab),
                output_custom: self.settings.output_folder.is_some(),
                engine_custom: Engine::ALL.map(|k| e.get(k).is_some()),
            },
        )
    }

    /// The text beside each chooser, refreshed whenever settings change.
    fn settings_lines(&self) -> ((String, bool), [(String, bool); 4]) {
        let output = match &self.settings.output_folder {
            Some(p) => (p.display().to_string(), false),
            None => ("Last export folder".to_owned(), true),
        };
        let path_var = std::env::var_os("PATH").unwrap_or_default();
        let line = |e: Engine| {
            let st = settings::engine_status(&self.settings, e, &path_var);
            let muted = !matches!(st, settings::EngineStatus::Chosen { .. });
            (st.describe(), muted)
        };
        (output, Engine::ALL.map(line))
    }

    fn open_settings(&mut self) {
        self.close_wheel();
        let (output_line, engine_lines) = self.settings_lines();
        self.settings_ui = Some(SettingsUi {
            tab: SettingsTab::General,
            tab_marker: Tween::new(0.0),
            switched: Instant::now() - Duration::from_secs(1),
            hover: None,
            focus: None,
            output_line,
            engine_lines,
        });
        self.settings_hover = false;
        self.invalidate();
    }

    /// The one network call, and only on request. Once a newer version
    /// is known the same button opens its page instead.
    fn check_updates(&mut self) {
        match self.update {
            UpdateState::Checking => {}
            UpdateState::Available(_) => updates::open_releases_page(),
            _ => {
                self.update = UpdateState::Checking;
                let target = PostTarget(self.hwnd.0 as usize);
                std::thread::spawn(move || target.post(WM_KILN_UPDATE, updates::fetch()));
            }
        }
        self.invalidate();
    }

    /// Press a settings control. `delta` steps a value back or forward.
    fn activate(&mut self, c: Control, delta: isize) {
        let hwnd = self.hwnd;
        match c {
            Control::Tab(t) => {
                if let Some(ui) = &mut self.settings_ui {
                    if ui.tab != t {
                        ui.tab = t;
                        ui.tab_marker.set(t.index() as f32, OPEN_MS);
                        ui.switched = Instant::now();
                        ui.hover = None;
                        // The tabs come first, so focus stays put.
                    }
                }
                self.animate();
                return;
            }
            Control::Preset(s, preset) => self.settings.quality.set(s, Value::Preset(preset)),
            Control::Step(s) => self.settings.quality.step(s, delta),
            Control::Close => {
                self.settings_ui = None;
                self.invalidate();
                return;
            }
            Control::OutputChoose => match system::pick_folder(hwnd, "Start exports in") {
                Some(p) => self.settings.output_folder = Some(p),
                None => return,
            },
            Control::OutputReset => self.settings.output_folder = None,
            Control::Theme(t) => {
                self.settings.theme = t;
                self.apply_theme();
            }
            Control::Key(w) => self.settings.keys.cycle(w),
            Control::Suggestions(on) => {
                if self.settings.suggestions == on {
                    return;
                }
                self.settings.suggestions = on;
                self.save_settings();
                self.suggestions_changed();
                self.invalidate();
                return;
            }
            Control::ForgetPicks => {
                if self.settings.habits.is_empty() {
                    return;
                }
                self.settings.habits.clear();
                self.habits_changed();
            }
            Control::EngineChoose(e) => {
                let title = format!("Choose {}.exe", e.program());
                match system::pick_program(hwnd, &title) {
                    Some(p) => self.settings.engines.set(e, Some(p)),
                    None => return,
                }
            }
            Control::EngineReset(e) => self.settings.engines.set(e, None),
            Control::CheckUpdates => {
                self.check_updates();
                return;
            }
        }
        self.save_settings();
        // What Kiln offers follows the engines, and so do the rows'
        // suggestions.
        if matches!(c, Control::EngineChoose(_) | Control::EngineReset(_)) {
            self.habits_changed();
        }
        let (output_line, engine_lines) = self.settings_lines();
        // Reset buttons come and go. Keep keyboard focus on the same
        // control, or on its Choose button when a Reset went away.
        let had_focus = self
            .settings_ui
            .as_ref()
            .is_some_and(|ui| ui.focus.is_some());
        let l = self.settings_layout();
        let find = |c: Control| l.controls.iter().position(|(k, _)| *k == c);
        let refocus = had_focus.then(|| {
            find(c)
                .filter(|_| c != Control::ForgetPicks)
                .or_else(|| match c {
                    Control::OutputReset => find(Control::OutputChoose),
                    Control::EngineReset(e) => find(Control::EngineChoose(e)),
                    // Forget picks is now disabled: focus goes to the
                    // chosen On or Off segment.
                    Control::ForgetPicks => find(Control::Suggestions(self.settings.suggestions)),
                    _ => None,
                })
                .unwrap_or(0)
        });
        if let Some(ui) = &mut self.settings_ui {
            ui.output_line = output_line;
            ui.engine_lines = engine_lines;
            ui.focus = refocus;
            ui.hover = None;
        }
        self.invalidate();
    }

    /// Suggestions were turned on or off; the setting is already saved.
    /// Off stops inspecting and forgets what was found; on looks inside
    /// every row.
    fn suggestions_changed(&mut self) {
        if self.settings.suggestions {
            let all = self
                .files
                .entries()
                .iter()
                .map(|e| e.path.clone())
                .collect();
            self.inspect(all);
        } else {
            self.inspector.cancel_all();
            self.files.forget_facts();
        }
    }

    /// Whether a settings control can be used now. Forget picks is
    /// disabled while there are no picks.
    fn control_enabled(&self, c: Control) -> bool {
        c != Control::ForgetPicks || !self.settings.habits.is_empty()
    }

    /// The enabled control under the pointer, so a disabled one shows no
    /// hover or press and a click on it does nothing.
    fn settings_hit(&self, x: f32, y: f32) -> Option<usize> {
        self.settings_layout()
            .controls
            .iter()
            .position(|(c, r)| r.contains(x, y) && self.control_enabled(*c))
    }

    /// Tab, Shift+Tab and the up and down arrows move focus. Left and
    /// right change the focused value: a step, a segment, a page. Enter or
    /// Space presses; Escape closes.
    fn settings_key(&mut self, key: VIRTUAL_KEY, shift: bool) {
        let l = self.settings_layout();
        let n = l.controls.len();
        let enabled: Vec<bool> = l
            .controls
            .iter()
            .map(|(c, _)| self.control_enabled(*c))
            .collect();
        let Some(ui) = &mut self.settings_ui else {
            return;
        };
        let focused = ui.focus.map(|i| l.controls[i].0);
        match key {
            VK_ESCAPE => self.settings_ui = None,
            VK_LEFT | VK_RIGHT if focused.is_some_and(segmented_or_step) => {
                let delta = if key == VK_LEFT { -1 } else { 1 };
                let c = focused.expect("checked above");
                if let Control::Step(_) = c {
                    return self.activate(c, delta);
                }
                // Segments: the neighbour in the same group takes focus
                // and is chosen.
                let i = ui.focus.unwrap_or(0) as isize + delta;
                if let Some((next, _)) = l.controls.get(i.max(0) as usize) {
                    if i >= 0 && same_group(c, *next) {
                        ui.focus = Some(i as usize);
                        return self.activate(*next, 1);
                    }
                }
            }
            VK_TAB | VK_DOWN | VK_RIGHT | VK_UP | VK_LEFT => {
                let back = (key == VK_TAB && shift) || key == VK_UP || key == VK_LEFT;
                // Disabled controls are skipped.
                let mut f = ui.focus;
                for _ in 0..n {
                    f = wheel::step(f, if back { -1 } else { 1 }, n);
                    if f.is_some_and(|i| enabled[i]) {
                        break;
                    }
                }
                ui.focus = f;
            }
            VK_RETURN | VK_SPACE => {
                if let Some(c) = focused {
                    self.activate(c, 1);
                }
            }
            _ => return,
        }
        self.invalidate();
    }

    fn cancel_row(&self, i: usize) {
        if let Some(e) = self.files.entries().get(i) {
            self.cancel_jobs_for(&e.path);
        }
    }

    fn show_details(&mut self, i: usize) {
        if let Some(JobState::Failed { details, .. }) = self.files.entries().get(i).map(|e| &e.job)
        {
            if !details.is_empty() {
                self.details = Some(details.clone());
                self.dialog_close_hover = false;
                self.invalidate();
            }
        }
    }

    /// Whether `(x, y)` is on a row's link, with nothing over the list.
    fn on_link(&self, x: f32, y: f32) -> bool {
        self.wheel.is_none()
            && self.sheet.is_none()
            && self.settings_ui.is_none()
            && self.details.is_none()
            && self.hit_row(x, y).2
    }

    // Input.

    /// The row under a point and whether the point is on its cancel cross
    /// or its link: "Show details", "Export", or an exported row's
    /// "Saved to" line.
    fn hit_row(&self, x: f32, y: f32) -> (Option<usize>, bool, bool) {
        let l = self.layout();
        let Some(i) = layout::row_at(&l, x, y, self.scroll) else {
            return (None, false, false);
        };
        let mut r = l.rows[i];
        r.y -= self.scroll;
        let e = &self.files.entries()[i];
        let cancel =
            matches!(e.job, JobState::Running { .. }) && layout::row_cancel(&r).contains(x, y);
        // A row being exported has no link until it's done.
        let link = !self.row_saving(i)
            && match e.link() {
                Some(_) => layout::row_link(&r).contains(x, y),
                None => e.saved().is_some() && layout::row_line(&r).contains(x, y),
            };
        (Some(i), cancel, link)
    }

    fn mouse_move(&mut self, x: f32, y: f32) {
        if self.sheet.is_some() {
            let hover = self.sheet_hit(x, y);
            if let Some(s) = self.sheet.as_mut().filter(|s| s.is_open()) {
                if s.hover != hover {
                    s.hover = hover;
                    self.invalidate();
                }
            }
            return;
        }
        if self.settings_ui.is_some() {
            let hover = self.settings_hit(x, y);
            if let Some(ui) = &mut self.settings_ui {
                if ui.hover != hover {
                    ui.hover = hover;
                    self.invalidate();
                }
            }
            return;
        }
        if self.details.is_some() {
            let (w, h) = self.renderer.size_dips();
            let hot = layout::dialog(w, h).close.contains(x, y);
            if hot != self.dialog_close_hover {
                self.dialog_close_hover = hot;
                self.invalidate();
            }
            return;
        }
        if self.wheel.is_some() {
            self.wheel_point(x, y);
            return;
        }
        let (row, cancel, link) = self.hit_row(x, y);
        let l = self.layout();
        let clear = l.clear_button.is_some_and(|b| b.contains(x, y));
        let export = self.export_button().is_some_and(|b| b.contains(x, y));
        if export != self.export_hover {
            self.export_hover = export;
            self.invalidate();
        }
        let add = self.add_button().is_some_and(|b| b.contains(x, y));
        if add != self.add_hover {
            self.add_hover = add;
            self.invalidate();
        }
        let gear = l.settings_button.contains(x, y);
        if gear != self.settings_hover {
            self.settings_hover = gear;
            self.invalidate();
        }
        if (row, cancel, link, clear)
            != (
                self.hover_row,
                self.hover_cancel,
                self.hover_link,
                self.clear_hover,
            )
        {
            self.hover_row = row;
            self.hover_cancel = cancel;
            self.hover_link = link;
            self.clear_hover = clear;
            self.invalidate();
        }
    }

    /// A pressed button acts when the pointer is released over it, so a
    /// press can be taken back by sliding off.
    fn mouse_up(&mut self, x: f32, y: f32) {
        let clear = std::mem::take(&mut self.clear_pressed);
        let export = std::mem::take(&mut self.export_pressed);
        let add = std::mem::take(&mut self.add_pressed);
        let setting = self.settings_pressed.take();
        let close = std::mem::take(&mut self.dialog_close_pressed);
        let sheet = self.sheet.as_mut().and_then(|s| s.pressed.take());
        unsafe {
            let _ = ReleaseCapture();
        }
        if let Some(i) = sheet {
            if let (Some(l), true) = (self.sheet_layout(), self.sheet_hit(x, y) == Some(i)) {
                let (c, r) = l.controls[i];
                self.sheet_activate(c, step_delta(&r, x));
            }
            self.invalidate();
            return;
        }
        if !clear && !export && !add && setting.is_none() && !close {
            return;
        }
        if add && self.add_button().is_some_and(|b| b.contains(x, y)) {
            self.add_files();
        }
        if export && self.export_button().is_some_and(|b| b.contains(x, y)) {
            self.export_everything();
        }
        if clear && self.layout().clear_button.is_some_and(|b| b.contains(x, y)) {
            self.clear();
        }
        if let Some(i) = setting {
            if self.settings_ui.is_some() && self.settings_hit(x, y) == Some(i) {
                let (c, r) = self.settings_layout().controls[i];
                self.activate(c, step_delta(&r, x));
            }
        }
        if close && self.details.is_some() {
            let (w, h) = self.renderer.size_dips();
            if layout::dialog(w, h).close.contains(x, y) {
                self.details = None;
            }
        }
        self.invalidate();
    }

    fn mouse_down(&mut self, x: f32, y: f32) {
        if self.sheet.is_some() {
            let hit = self.sheet_hit(x, y);
            let inside = self.sheet_layout().is_some_and(|l| l.panel.contains(x, y));
            match (self.sheet.as_mut().filter(|s| s.is_open()), hit) {
                (Some(s), Some(i)) => {
                    s.pressed = Some(i);
                    unsafe { SetCapture(self.hwnd) };
                }
                // A click on the scrim cancels.
                (Some(_), None) if !inside => self.close_sheet(false),
                _ => {}
            }
            self.invalidate();
            return;
        }
        if self.settings_ui.is_some() {
            if let Some(i) = self.settings_hit(x, y) {
                self.settings_pressed = Some(i);
                unsafe { SetCapture(self.hwnd) };
            } else if !self.settings_layout().panel.contains(x, y) {
                // A click on the scrim closes the panel.
                self.settings_ui = None;
            }
            self.invalidate();
            return;
        }
        if self.details.is_some() {
            let (w, h) = self.renderer.size_dips();
            if layout::dialog(w, h).close.contains(x, y) {
                self.dialog_close_pressed = true;
                unsafe { SetCapture(self.hwnd) };
            }
            self.invalidate();
            return;
        }
        if let Some(w) = &self.wheel {
            if w.is_open() {
                match wheel::hit(&w.wheel, x, y, w.more_open()) {
                    Some(spot) => self.choose(spot),
                    None => self.close_wheel(),
                }
            }
            return;
        }
        let l = self.layout();
        if l.settings_button.contains(x, y) {
            self.open_settings();
        } else if l.clear_button.is_some_and(|b| b.contains(x, y)) {
            self.clear_pressed = true;
            unsafe { SetCapture(self.hwnd) };
        } else if self.export_button().is_some_and(|b| b.contains(x, y)) {
            self.export_pressed = true;
            unsafe { SetCapture(self.hwnd) };
        } else if self.add_button().is_some_and(|b| b.contains(x, y)) {
            self.add_pressed = true;
            unsafe { SetCapture(self.hwnd) };
        } else {
            let (row, cancel, link) = self.hit_row(x, y);
            self.files.select(row);
            if let Some(i) = row {
                if cancel {
                    self.cancel_row(i);
                } else if link {
                    self.follow_link(i);
                }
            }
        }
        self.invalidate();
    }

    fn key(&mut self, key: VIRTUAL_KEY) -> bool {
        let down = |k: VIRTUAL_KEY| unsafe { GetKeyState(k.0 as i32) } < 0;
        let (shift, ctrl, alt) = (down(VK_SHIFT), down(VK_CONTROL), down(VK_MENU));
        if self.sheet.is_some() {
            self.sheet_key(key, shift);
            return true;
        }
        if self.settings_ui.is_some() {
            self.settings_key(key, shift);
            return true;
        }
        if self.details.is_some() {
            if matches!(key, VK_ESCAPE | VK_RETURN | VK_SPACE) {
                self.details = None;
                self.invalidate();
            }
            return true;
        }
        // A fading wheel no longer takes keys; a pulsing one is busy.
        if self.wheel.as_ref().is_some_and(|w| w.pulse.is_some()) {
            return true;
        }
        if self.wheel.as_ref().is_some_and(|w| w.is_open()) {
            let hover = self.wheel.as_ref().and_then(|w| w.hover);
            if key == VK_TAB && self.open_sheet(hover) {
                return true;
            }
            let Some(w) = self.wheel.as_mut() else {
                return true;
            };
            let (inner, outer) = (w.wheel.segments(), w.wheel.more.len());
            let more = w.wheel.items.len();
            let delta = match key {
                VK_RIGHT | VK_DOWN | VK_TAB => 1,
                VK_LEFT | VK_UP => -1,
                _ => 0,
            };
            match (key, w.hover) {
                (_, Some(Spot::Outer(j))) if delta != 0 => {
                    w.hover = wheel::step(Some(j), delta, outer).map(Spot::Outer);
                }
                (_, h) if delta != 0 => {
                    let i = match h {
                        Some(Spot::Inner(i)) => Some(i),
                        _ => None,
                    };
                    let next = wheel::step(i, delta, inner).map(Spot::Inner);
                    w.point(next);
                }
                // Escape on the outer ring steps back to More.
                (VK_ESCAPE, Some(Spot::Outer(_))) => {
                    w.hover = Some(Spot::Inner(more));
                }
                (VK_ESCAPE, _) => self.close_wheel(),
                (VK_RETURN | VK_SPACE, Some(spot)) => self.choose(spot),
                _ => return true,
            }
            self.animate();
            return true;
        }
        match key {
            VK_UP => self.files.step(-1),
            VK_DOWN => self.files.step(1),
            VK_HOME => self.files.select(Some(0)),
            VK_END => self.files.select(self.files.len().checked_sub(1)),
            VK_DELETE | VK_BACK => self.remove_selected(),
            VK_ESCAPE => self.files.select(None),
            // Enter opens the convert wheel; with a modifier, that
            // modifier's wheel, following the key settings.
            VK_RETURN => {
                let kind = self
                    .settings
                    .keys
                    .wheel_for(shift, ctrl, alt)
                    .unwrap_or(WheelKind::Convert);
                self.open_wheel_for_selection(kind);
            }
            VK_OEM_COMMA if ctrl => self.open_settings(),
            VK_O if ctrl => self.add_files(),
            VK_V if ctrl => self.paste(),
            // Ctrl+S exports the selected row, Ctrl+Shift+S everything.
            VK_S if ctrl && shift => self.export_everything(),
            VK_S if ctrl => {
                if let Some(i) = self.files.selected() {
                    self.export_row(i);
                }
            }
            // Space follows the selected row's link.
            VK_SPACE => {
                if let Some(i) = self.files.selected() {
                    self.follow_link(i);
                }
            }
            _ => return false,
        }
        self.scroll_to_selection();
        self.invalidate();
        true
    }

    /// While a drag is over the window, a held modifier pours its wheel
    /// (Shift convert, Ctrl tools, Alt actions, unless settings say
    /// otherwise); letting go of the key lets it fade.
    fn drag_over(&mut self, ev: &DragEvent) {
        if self.settings_ui.is_some() || self.details.is_some() || self.sheet.is_some() {
            return;
        }
        let at = self.screen_dips(ev.x, ev.y);
        let keys = ev.keys.0;
        let want = self.settings.keys.wheel_for(
            keys & MK_SHIFT_FLAG != 0,
            keys & MK_CONTROL_FLAG != 0,
            keys & MK_ALT_FLAG != 0,
        );
        let showing = self
            .wheel
            .as_ref()
            .filter(|w| w.is_open() && !w.keyboard)
            .map(|w| w.wheel.kind);
        match (showing, want) {
            (s, Some(k)) if s != Some(k) => {
                if self.drag_paths.is_empty() {
                    return;
                }
                self.open_wheel(k, self.drag_paths.clone(), at, false);
            }
            (Some(_), None) => self.close_wheel(),
            _ => {}
        }
        self.wheel_point(at.0, at.1);
    }

    fn dropped(&mut self, ev: &DragEvent) {
        self.drag.set(0.0, motion::QUICK_MS);
        let chosen = self
            .wheel
            .as_mut()
            .filter(|w| w.is_open() && !w.keyboard)
            .and_then(|w| {
                // The drop is what counts: use its own paths.
                w.paths = ev.paths.clone();
                w.hover
            });
        self.add(ev.paths.clone());
        match chosen {
            Some(spot) => self.choose(spot),
            None => self.close_wheel(),
        }
        self.drag_paths.clear();
    }

    fn paint(&mut self) {
        let settings_layout = self.settings_layout();
        let anims: Vec<RowAnim> = (0..self.files.len()).map(|i| self.row_anim(i)).collect();
        let anim = |i: usize| {
            anims.get(i).copied().unwrap_or(RowAnim {
                appear: 1.0,
                settle: 1.0,
                facts: 1.0,
            })
        };
        let empty = ways_in::empty_lines(&self.settings);
        let update_note = self.update.note();
        let wheel_hint = self.wheel_hint();
        let sheet_layout = self.sheet_layout();
        let sheet_rows: Vec<(String, String)> = self
            .sheet
            .as_ref()
            .map(|s| {
                let unit = q::size_unit(&s.to);
                s.knobs
                    .iter()
                    .map(|&k| {
                        (
                            q::knob_name(k, &s.to).to_owned(),
                            q::label(k, s.shown(k), unit),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let sheet_lines = self.sheet.as_ref().map(|s| {
            let title = match s.paths.len() {
                1 => format!("Convert to {}", s.to.code()),
                n => format!("Convert {n} files to {}", s.to.code()),
            };
            let infos = s.infos.as_deref();
            let one = infos.and_then(|i| (i.len() == 1).then(|| &i[0]));
            let context = q::source_line(&s.names, s.bytes, one);
            let result = match infos {
                None => "Reading the file…".to_owned(),
                Some(i) => q::result_line(&s.from, &s.to, &s.quality, i),
            };
            (title, context, result)
        });
        let held = self.files.held_count();
        let export_all = (held > 0).then(|| model::export_all_label(held));
        let saving = self.saving_rows();
        let plain = status_line(self.files.len(), 0, 0, 0);
        let added = self
            .added
            .as_ref()
            .filter(|(said, _)| *said == self.status)
            .map(|(_, at)| at.elapsed());
        let running = self.burning() || !self.exports.is_empty();
        // Once the status has nothing more to say, a warning that stands
        // comes first, then the hint on what to do next.
        let status =
            match ways_in::idle_hint(self.files.len(), running, &self.status, &plain, added) {
                Some(hint) => self.sticky.unwrap_or(hint),
                None => model::footer_status(&self.status, &plain, self.sticky),
            };
        let view = View {
            palette: self.palette,
            entries: self.files.entries(),
            notes: &self.notes,
            saving: &saving,
            export_all: export_all.as_deref(),
            export_hover: self.export_hover,
            export_pressed: self.export_pressed,
            selected: self.files.selected(),
            hover_row: self.hover_row,
            hover_cancel: self.hover_cancel,
            hover_link: self.hover_link,
            scroll: self.scroll,
            drag: self.drag.value().clamp(0.0, 1.0),
            heat: heat_now(),
            row_anim: &anim,
            status: wheel_hint.as_deref().unwrap_or(status),
            empty: &empty,
            add_hover: self.add_hover,
            add_pressed: self.add_pressed,
            clear_hover: self.clear_hover,
            clear_pressed: self.clear_pressed,
            dialog_close_pressed: self.dialog_close_pressed,
            wheel: Self::wheel_view(self.wheel.as_ref(), &self.settings.keys),
            details: self.details.as_deref(),
            dialog_close_hover: self.dialog_close_hover,
            settings_hover: self.settings_hover,
            sheet: match (&self.sheet, &sheet_layout, &sheet_lines) {
                (Some(s), Some(l), Some((title, context, result))) => Some(SheetView {
                    layout: l,
                    title,
                    context,
                    result,
                    rows: &sheet_rows,
                    preset: s.quality.preset,
                    hover: s.hover,
                    pressed: s.pressed,
                    focus: s.focus,
                    open: eased(s.opened, OPEN_MS),
                    fade: s.closing.map_or(1.0, |t| 1.0 - eased(t, motion::QUICK_MS)),
                }),
                _ => None,
            },
            settings: self.settings_ui.as_ref().map(|ui| SettingsView {
                layout: &settings_layout,
                tab_marker: ui.tab_marker.value(),
                page: eased(ui.switched, motion::QUICK_MS),
                quality: &self.settings.quality,
                output: (ui.output_line.0.as_str(), ui.output_line.1),
                theme: self.settings.theme,
                suggestions: self.settings.suggestions,
                has_habits: !self.settings.habits.is_empty(),
                keys: self.settings.keys,
                engines: ui.engine_lines.each_ref().map(|(s, m)| (s.as_str(), *m)),
                hover: ui.hover,
                pressed: self.settings_pressed,
                update: (update_note.0.as_str(), update_note.1),
                update_button: self.update.button(),
                focus: ui.focus,
            }),
        };
        if let Err(e) = self.renderer.draw(&view) {
            log(&format!("draw failed: {e}"));
        }
    }
}

/// A click on the left end of a stepping control steps back; anywhere
/// else steps forward.
fn step_delta(r: &layout::Rect, x: f32) -> isize {
    if x < r.x + layout::CHEVRON_ZONE {
        -1
    } else {
        1
    }
}

/// Each source type's favourite target, to tell whether a pick changed
/// any.
fn favourites(h: &Habits) -> BTreeMap<String, String> {
    h.favourites()
        .into_iter()
        .map(|(from, to)| (from.to_owned(), to.to_owned()))
        .collect()
}

/// The picks a conversion of `paths` counts: one (source type, target)
/// pair per source type, by lower-case extension. Tools and file actions
/// count nothing.
fn habit_picks(action: &Action, paths: &[PathBuf]) -> Vec<(String, String)> {
    let Action::Convert(to) = action else {
        return Vec::new();
    };
    let to = to.trim_start_matches('.').to_ascii_lowercase();
    let from: BTreeSet<String> = paths
        .iter()
        .filter_map(|p| p.extension()?.to_str())
        .map(str::to_ascii_lowercase)
        .collect();
    from.into_iter().map(|f| (f, to.clone())).collect()
}

/// Controls whose value the left and right arrows change.
fn segmented_or_step(c: Control) -> bool {
    matches!(
        c,
        Control::Tab(_)
            | Control::Theme(_)
            | Control::Suggestions(_)
            | Control::Preset(..)
            | Control::Step(_)
    )
}

/// Whether two controls are segments of one group.
fn same_group(a: Control, b: Control) -> bool {
    match (a, b) {
        (Control::Tab(_), Control::Tab(_))
        | (Control::Theme(_), Control::Theme(_))
        | (Control::Suggestions(_), Control::Suggestions(_)) => true,
        (Control::Preset(x, _), Control::Preset(y, _)) => x == y,
        _ => false,
    }
}

fn log(msg: &str) {
    let line = HSTRING::from(format!("kiln-app: {msg}\n"));
    unsafe { windows::Win32::System::Diagnostics::Debug::OutputDebugStringW(&line) };
}

/// Show a plain error dialog for a failure that stops the app.
pub fn fatal(e: &windows::core::Error) {
    let text = HSTRING::from(format!(
        "Kiln couldn't start: {}\n\nError code {:#010X}.",
        e.message(),
        e.code().0
    ));
    unsafe {
        MessageBoxW(None, &text, w!("Kiln"), MB_OK | MB_ICONERROR);
    }
}

pub fn run() -> Result<()> {
    unsafe {
        // The manifest already asks for per-monitor v2; this covers runs
        // without it (for example under a debugger that strips resources).
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    // Files dropped on the app icon arrive as arguments. With Kiln already
    // open they go to that window instead.
    let files: Vec<PathBuf> = std::env::args_os()
        .skip(1)
        .map(|a| std::path::absolute(&a).unwrap_or_else(|_| PathBuf::from(a)))
        .collect();
    let Some(_first) = handoff::claim(&files) else {
        return Ok(());
    };
    unsafe {
        OleInitialize(None)?;
    }
    let result = run_window(files);
    unsafe { OleUninitialize() };
    result
}

fn run_window(files: Vec<PathBuf>) -> Result<()> {
    // The first frame follows the Windows app mode; settings.toml, read
    // once the window is up, can override it.
    let system_light = system::system_light();
    let palette = if system_light {
        &theme::LIGHT
    } else {
        &theme::DARK
    };
    let instance: HINSTANCE = unsafe { GetModuleHandleW(None)? }.into();
    let class = handoff::CLASS;
    let icon = unsafe { LoadIconW(Some(instance), PCWSTR(1 as _)) }.unwrap_or_default();
    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
        lpfnWndProc: Some(wndproc),
        hInstance: instance,
        hIcon: icon,
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
        // Soot, so the first frame never flashes white.
        hbrBackground: unsafe { CreateSolidBrush(COLORREF(palette.ground.colorref())) },
        lpszClassName: class,
        ..Default::default()
    };
    if unsafe { RegisterClassW(&wc) } == 0 {
        return Err(windows::core::Error::from_thread());
    }

    let (w, h) = layout::DEFAULT_WINDOW;
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            w!("Kiln"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            w as i32,
            h as i32,
            None,
            None,
            Some(instance),
            None,
        )?
    };

    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96) as f32;
    style_title_bar(hwnd, palette);

    let holding = open_holding();
    // Without a holding folder results go next to each file, as before,
    // and the footer keeps saying so.
    let sticky = holding.is_none().then_some(model::NO_HOLDING);

    let kiln = Arc::new(Kiln::new());
    let target = PostTarget(hwnd.0 as usize);
    let runner = Runner::new(kiln.clone(), Runner::default_workers(), move |ev| {
        target.post(WM_KILN_JOB, ev)
    });
    // The inspect thread has its own Kiln and never touches the app's.
    let inspect_kiln = Kiln::new();
    let inspector = Inspector::new(
        move |path, opts, cancel| kiln_core::inspect(&inspect_kiln, path, opts, cancel),
        move |path, facts| {
            if let Some(facts) = facts {
                target.post(WM_KILN_FACTS, Inspected { path, facts });
            }
        },
    );

    let app = Box::new(App {
        hwnd,
        kiln,
        runner,
        inspector,
        engines: Engines::default(),
        jobs: HashMap::new(),
        batches: HashMap::new(),
        next_batch: 1,
        palette,
        settings: Settings::default(),
        settings_store: settings::Store::new(settings::default_path()),
        system_light,
        settings_ui: None,
        settings_hover: false,
        last_mouse: None,
        renderer: Renderer::new(hwnd, dpi)?,
        files: FileList::default(),
        rows: vec![],
        scroll: 0.0,
        hover_row: None,
        hover_cancel: false,
        hover_link: false,
        drag: Tween::new(0.0),
        drag_paths: vec![],
        wheel: None,
        sheet: None,
        next_sheet: 1,
        details: None,
        dialog_close_hover: false,
        status: String::new(),
        clear_hover: false,
        clear_pressed: false,
        settings_pressed: None,
        dialog_close_pressed: false,
        update: UpdateState::default(),
        tracking_mouse: false,
        dpi,
        holding,
        last_export: None,
        notes: HashMap::new(),
        export_hover: false,
        export_pressed: false,
        shutdown_told: 0,
        exports: HashMap::new(),
        next_export: 1,
        leaving: None,
        close_when_saved: false,
        sticky,
        add_hover: false,
        add_pressed: false,
        last_add: None,
        added: None,
    });
    let app = Box::into_raw(app);
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, app as isize) };

    // Size the window for its monitor's scale, centred where Windows put it.
    resize_for_dpi(hwnd, dpi);

    if !app_ref(app).renderer.display_font_bundled() {
        log("bundled serif unavailable; using the fallback");
    }

    let target: IDropTarget = DropTarget::new(hwnd).into();
    unsafe { RegisterDragDrop(hwnd, &target)? };

    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = UpdateWindow(hwnd);
        (*app).load_settings();
    }

    if !files.is_empty() {
        unsafe { (*app).add(files) };
    }

    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        // Dropping the app stops the workers; events they post now find
        // no window and are freed on their side.
        let mut app = Box::from_raw(app);
        let holding = app.holding.take();
        drop(app);
        // The prompt has been answered: nothing held is kept.
        if let Some(h) = holding {
            if let Err(e) = h.discard() {
                log(&format!("couldn't empty the holding folder: {e}"));
            }
        }
    }
    Ok(())
}

/// Clear what crashed runs left, then make this run's holding folder.
fn open_holding() -> Option<Holding> {
    let Some(root) = settings::held_root() else {
        log("LOCALAPPDATA isn't set");
        return None;
    };
    for dir in kiln_core::sweep_stale(&root) {
        log(&format!("cleared {}", dir.display()));
    }
    match Holding::open(&root) {
        Ok(h) => Some(h),
        Err(e) => {
            log(&format!("couldn't open {}: {e}", root.display()));
            None
        }
    }
}

/// Whether `action` replaces what its rows hold: anything but copying a
/// path or opening a terminal.
fn replaces(action: &Action) -> bool {
    !matches!(
        action,
        Action::File(FileAction::CopyPath | FileAction::OpenTerminal)
    )
}

fn app_ref<'a>(app: *mut App) -> &'a App {
    unsafe { &*app }
}

fn style_title_bar(hwnd: HWND, p: &Palette) {
    // Dark title bar (Windows 10 20H1+), and on Windows 11 the caption in
    // the palette's own ground, text and border colours. Errors are fine:
    // older systems keep the default frame.
    const USE_IMMERSIVE_DARK_MODE: i32 = 20;
    const BORDER_COLOR: i32 = 34;
    const CAPTION_COLOR: i32 = 35;
    const TEXT_COLOR: i32 = 36;
    let dark: BOOL = (p.name == "dark").into();
    unsafe {
        let set = |attr: i32, value: *const core::ffi::c_void, size: usize| {
            let _ = DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(attr), value, size as u32);
        };
        set(USE_IMMERSIVE_DARK_MODE, &dark as *const _ as _, 4);
        for (attr, c) in [
            (CAPTION_COLOR, p.ground),
            (TEXT_COLOR, p.text),
            (BORDER_COLOR, p.line),
        ] {
            let v = c.colorref();
            set(attr, &v as *const u32 as _, 4);
        }
    }
}

fn resize_for_dpi(hwnd: HWND, dpi: f32) {
    let (w, h) = layout::DEFAULT_WINDOW;
    let scale = dpi / 96.0;
    let mut rc = RECT {
        left: 0,
        top: 0,
        right: (w * scale).round() as i32,
        bottom: (h * scale).round() as i32,
    };
    unsafe {
        let _ = windows::Win32::UI::HiDpi::AdjustWindowRectExForDpi(
            &mut rc,
            WS_OVERLAPPEDWINDOW,
            false,
            WINDOW_EX_STYLE::default(),
            dpi as u32,
        );
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            rc.right - rc.left,
            rc.bottom - rc.top,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

fn lo(l: LPARAM) -> i32 {
    (l.0 & 0xFFFF) as i16 as i32
}

fn hi(l: LPARAM) -> i32 {
    ((l.0 >> 16) & 0xFFFF) as i16 as i32
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut App;
    if ptr.is_null() {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    let app = unsafe { &mut *ptr };
    match handle(app, msg, wparam, lparam) {
        Some(r) => r,
        None => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn handle(app: &mut App, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    let hwnd = app.hwnd;
    match msg {
        WM_PAINT => {
            app.paint();
            unsafe {
                let _ = ValidateRect(Some(hwnd), None);
            }
            Some(LRESULT(0))
        }
        WM_ERASEBKGND => Some(LRESULT(1)),
        // Files from a second launch, such as a drop on the app icon.
        WM_COPYDATA => {
            let files = unsafe { handoff::received(lparam) }?;
            if !files.is_empty() {
                app.add(files);
            }
            Some(LRESULT(1))
        }
        WM_SIZE => {
            app.renderer.resize();
            app.clamp_scroll();
            app.invalidate();
            Some(LRESULT(0))
        }
        WM_DPICHANGED => {
            app.dpi = (wparam.0 & 0xFFFF) as f32;
            app.renderer.set_dpi(app.dpi);
            let r = unsafe { &*(lparam.0 as *const RECT) };
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            app.invalidate();
            Some(LRESULT(0))
        }
        WM_GETMINMAXINFO => {
            let info = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
            let scale = app.dpi / 96.0;
            let (w, h) = layout::MIN_WINDOW;
            info.ptMinTrackSize = POINT {
                x: (w * scale) as i32,
                y: (h * scale) as i32,
            };
            Some(LRESULT(0))
        }
        // "Added 2 files." has been read: the idle hint can show. A timer
        // that fires early waits out the rest instead.
        WM_TIMER if wparam.0 == HINT_TIMER => {
            match app.hint_wait() {
                Some(rest) => app.arm_hint(rest),
                None => {
                    unsafe {
                        let _ = KillTimer(Some(hwnd), HINT_TIMER);
                    }
                    app.invalidate();
                }
            }
            Some(LRESULT(0))
        }
        WM_TIMER if wparam.0 == ANIM_TIMER => {
            app.tick();
            if !app.animating() {
                unsafe {
                    let _ = KillTimer(Some(hwnd), ANIM_TIMER);
                }
            } else if !app.moving() {
                unsafe {
                    SetTimer(Some(hwnd), ANIM_TIMER, HEAT_FRAME_MS, None);
                }
            }
            app.invalidate();
            Some(LRESULT(0))
        }
        WM_MOUSEMOVE => {
            if !app.tracking_mouse {
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                unsafe {
                    let _ = TrackMouseEvent(&mut tme);
                }
                app.tracking_mouse = true;
            }
            let at = (lo(lparam), hi(lparam));
            if app.last_mouse != Some(at) {
                app.last_mouse = Some(at);
                let (x, y) = app.dips(at.0, at.1);
                app.mouse_move(x, y);
            }
            Some(LRESULT(0))
        }
        WM_MOUSELEAVE => {
            app.tracking_mouse = false;
            app.last_mouse = None;
            app.hover_row = None;
            app.hover_cancel = false;
            app.hover_link = false;
            app.clear_hover = false;
            app.clear_pressed = false;
            app.export_hover = false;
            app.export_pressed = false;
            app.add_hover = false;
            app.add_pressed = false;
            app.invalidate();
            Some(LRESULT(0))
        }
        WM_LBUTTONDOWN => {
            unsafe {
                let _ = SetFocus(Some(hwnd));
            }
            let (x, y) = app.dips(lo(lparam), hi(lparam));
            app.mouse_down(x, y);
            Some(LRESULT(0))
        }
        // The second click of a double-click: on a held row it opens the
        // result; anywhere else it's a click like any other.
        WM_LBUTTONDBLCLK => {
            unsafe {
                let _ = SetFocus(Some(hwnd));
            }
            let (x, y) = app.dips(lo(lparam), hi(lparam));
            // A link acted on the first click; a second would open it twice.
            if !app.open_held_at(x, y) && !app.on_link(x, y) {
                app.mouse_down(x, y);
            }
            Some(LRESULT(0))
        }
        WM_LBUTTONUP => {
            let (x, y) = app.dips(lo(lparam), hi(lparam));
            app.mouse_up(x, y);
            Some(LRESULT(0))
        }
        WM_MOUSEWHEEL => {
            if app.wheel.is_none() && app.details.is_none() && app.sheet.is_none() {
                let delta = ((wparam.0 >> 16) & 0xFFFF) as i16 as f32 / WHEEL_DELTA as f32;
                app.scroll -= delta * ROW_HEIGHT;
                app.clamp_scroll();
                app.invalidate();
            }
            Some(LRESULT(0))
        }
        // The Apps key, Shift+F10 or a right click: the right-click menu.
        // On an open wheel, the options for the format under the pointer.
        WM_CONTEXTMENU => {
            if app.wheel.as_ref().is_some_and(|w| w.is_open()) {
                let spot = if lparam.0 == -1 {
                    app.wheel.as_ref().and_then(|w| w.hover)
                } else {
                    let at = app.screen_dips(lo(lparam), hi(lparam));
                    let w = app.wheel.as_ref().expect("checked above");
                    wheel::hit(&w.wheel, at.0, at.1, w.more_open())
                };
                app.open_sheet(spot);
                return Some(LRESULT(0));
            }
            if app.wheel.is_some()
                || app.details.is_some()
                || app.settings_ui.is_some()
                || app.sheet.is_some()
            {
                return Some(LRESULT(0));
            }
            let at = (lparam.0 != -1).then(|| (lo(lparam), hi(lparam)));
            app.context_menu(at);
            Some(LRESULT(0))
        }
        WM_KEYDOWN => app.key(VIRTUAL_KEY(wparam.0 as u16)).then_some(LRESULT(0)),
        // Alt+Enter arrives as a system key.
        WM_SYSKEYDOWN if wparam.0 == VK_RETURN.0 as usize => {
            app.key(VK_RETURN);
            Some(LRESULT(0))
        }
        // Its character would otherwise go looking for a menu mnemonic,
        // which beeps.
        WM_SYSCHAR if wparam.0 == '\r' as usize => Some(LRESULT(0)),
        // There's no menu bar, so a lone Alt shouldn't put the window into
        // menu mode and swallow the next key.
        WM_SYSCOMMAND if (wparam.0 & 0xFFF0) == SC_KEYMENU as usize && lparam.0 == 0 => {
            Some(LRESULT(0))
        }
        WM_SETTINGCHANGE => {
            let name = lparam.0 as *const u16;
            if !name.is_null()
                && unsafe { PCWSTR(name).to_string() }.ok().as_deref() == Some("ImmersiveColorSet")
            {
                app.system_light = system::system_light();
                app.apply_theme();
            }
            None
        }
        WM_KILN_UPDATE => {
            let reply = unsafe { Box::from_raw(lparam.0 as *mut Reply) };
            app.update = UpdateState::from_reply(&reply);
            app.invalidate();
            Some(LRESULT(0))
        }
        WM_KILN_PROBE => {
            let p = unsafe { Box::from_raw(lparam.0 as *mut Probed) };
            app.on_probed(*p);
            Some(LRESULT(0))
        }
        WM_KILN_FACTS => {
            let found = unsafe { Box::from_raw(lparam.0 as *mut Inspected) };
            app.on_inspected(*found);
            Some(LRESULT(0))
        }
        WM_KILN_MOVED => {
            let m = unsafe { Box::from_raw(lparam.0 as *mut Moved) };
            app.on_moved(*m);
            Some(LRESULT(0))
        }
        WM_KILN_EXPORTED => {
            let ex = unsafe { Box::from_raw(lparam.0 as *mut Exported) };
            app.on_exported(*ex);
            Some(LRESULT(0))
        }
        WM_KILN_DRAG => {
            let ev = unsafe { &*(lparam.0 as *const DragEvent) };
            match wparam.0 {
                DRAG_ENTER => {
                    app.drag_paths = ev.paths.clone();
                    app.drag.set(1.0, motion::QUICK_MS);
                    app.animate();
                    app.drag_over(ev);
                }
                DRAG_OVER => app.drag_over(ev),
                DRAG_LEAVE => {
                    app.drag.set(0.0, motion::QUICK_MS);
                    app.drag_paths.clear();
                    app.close_wheel();
                    app.animate();
                }
                _ => {}
            }
            Some(LRESULT(0))
        }
        WM_KILN_DROP => {
            let ev = unsafe { &*(lparam.0 as *const DragEvent) };
            app.dropped(ev);
            unsafe {
                let _ = SetForegroundWindow(hwnd);
            }
            Some(LRESULT(0))
        }
        WM_KILN_JOB => {
            let ev = unsafe { Box::from_raw(lparam.0 as *mut JobEvent) };
            app.on_job(*ev);
            Some(LRESULT(0))
        }
        // Coming back to Kiln: a held result may have been removed by hand
        // meanwhile.
        WM_ACTIVATE => {
            if (wparam.0 & 0xFFFF) as u32 != WA_INACTIVE {
                app.check_gone();
            }
            None
        }
        // Closing with results held or jobs running asks first. Cancel, or
        // cancelling the folder picker, keeps the window open; so does an
        // export that didn't save everything.
        WM_CLOSE => {
            app.leave(Leave::Close, Rows::All, Then::Close);
            Some(LRESULT(0))
        }
        // While results are held, ask Windows to wait: the shutdown block
        // reason says why, and the person decides.
        WM_QUERYENDSESSION => Some(LRESULT((app.files.held_count() == 0) as isize)),
        WM_DESTROY => {
            // Stop the inspect thread with the window, without waiting
            // for a decode under way: nothing is reported after this.
            app.inspector.shutdown();
            unsafe {
                let _ = RevokeDragDrop(hwnd);
                PostQuitMessage(0);
            }
            Some(LRESULT(0))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_copying_a_path_or_a_terminal_leaves_results_be() {
        assert!(!replaces(&Action::File(FileAction::CopyPath)));
        assert!(!replaces(&Action::File(FileAction::OpenTerminal)));
        assert!(replaces(&Action::File(FileAction::Move)));
        assert!(replaces(&Action::Convert("jpg".to_owned())));
        assert!(replaces(&Action::Tool(Tool::Compress)));
    }

    #[test]
    fn a_conversion_counts_once_per_source_type() {
        let paths: Vec<PathBuf> = ["a.png", "b.png", "c.PNG", "d.jpg"]
            .into_iter()
            .map(PathBuf::from)
            .collect();
        assert_eq!(
            habit_picks(&Action::Convert("webp".into()), &paths),
            [
                ("jpg".to_owned(), "webp".to_owned()),
                ("png".to_owned(), "webp".to_owned())
            ]
        );
        assert_eq!(habit_picks(&Action::Tool(Tool::Merge), &paths), []);
        assert_eq!(habit_picks(&Action::File(FileAction::ALL[0]), &paths), []);
    }

    #[test]
    fn engines_are_looked_for_once_until_settings_change() {
        let searches = std::cell::Cell::new(0);
        let search = || {
            searches.set(searches.get() + 1);
            Availability::default()
        };
        let mut engines = Engines::default();
        engines.get(search);
        engines.get(search);
        assert_eq!(searches.get(), 1);
        engines.forget();
        engines.get(search);
        engines.get(search);
        assert_eq!(searches.get(), 2);
    }
}
