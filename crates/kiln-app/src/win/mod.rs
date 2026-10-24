//! The Win32 window: creation, the message loop, input and animation.

mod drag_out;
mod drop;
mod handoff;
mod inspect;
mod motion;
mod render;
mod system;
mod text;
mod thumbs;
mod tips;
mod uia;
mod updates;

use std::cell::{Cell, OnceCell};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows::core::{w, Result, BOOL, HSTRING, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWINDOWATTRIBUTE};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, CreateSolidBrush, DeleteObject, FillRect, GetDC, InvalidateRect, ReleaseDC,
    ScreenToClient, UpdateWindow, ValidateRect,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::{
    IDropTarget, OleInitialize, OleUninitialize, RegisterDragDrop, RevokeDragDrop,
};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::{
    GetDpiForWindow, GetSystemMetricsForDpi, SetProcessDpiAwarenessContext,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use kiln_core::{
    suggest, Availability, Facts, Format, Habits, Holding, JobError, JobEvent, JobId, JobKind,
    Kiln, Knob, MediaInfo, Offered, Options, Progress, Quality, Runner, Suggestion, Tool,
};

use crate::a11y;
use crate::actions::{self, FileAction};
use crate::anim::{self, Tween};
use crate::help::{self, HelpSection};
use crate::inspector::{self as insp, InspectorControl};
use crate::layout::{
    self, Control, DialogButton, SettingsLayout, SettingsShape, SettingsTab, SheetControl,
    SheetLayout, TextScroll, ROW_HEIGHT,
};
use crate::model::{
    self, batch_summary, leave_prompt, status_line, Due, FileList, FocusButton, JobState, Leave,
    Next, PromptButton, Tone,
};
use crate::previews::{self, Preview, PreviewCache, PreviewKey, PreviewSize, CACHE_CAP};
use crate::quality::{self as q, Value};
use crate::settings::{self, Engine, Settings};
use crate::theme::{self, Palette};
use crate::update::{Reply, UpdateState};
use crate::ways_in::{self, MenuItem, RowState};
use crate::wheel::{self, Action, Pour, Spot, Wheel, WheelKind};
use drop::{DragEvent, DropTarget, DRAG_ENTER, DRAG_LEAVE, DRAG_OVER, WM_KILN_DRAG, WM_KILN_DROP};
use inspect::Inspector;
use motion::{new_content, same_page, sheet_rise, ButtonId, Fading, Motion, Shows};
use render::{
    DetailsView, HelpView, InspectorPage, InspectorView, Renderer, SettingsView, SheetView, View,
    WheelView,
};
use thumbs::Thumbs;
use tips::Tips;

const ANIM_TIMER: usize = 1;
/// Tooltip ids. Rows take three each from `TIP_ROWS`: the row itself, its
/// cancel cross and its Export link.
const TIP_SETTINGS: usize = 1;
const TIP_HELP: usize = 2;
const TIP_ADD: usize = 3;
const TIP_EXPORT_ALL: usize = 4;
const TIP_CONVERT: usize = 5;
const TIP_ROWS: usize = 100;
/// Fires once, when "Added 2 files." has been up long enough for the
/// footer's idle hint to take its place.
const HINT_TIMER: usize = 2;
/// Fires once, when the details dialog's "Copied" has been up long
/// enough to read "Copy" again.
const COPIED_TIMER: usize = 3;
/// Fires once, when UI Automation's tree is due to catch up with an
/// animation or a job's progress.
const A11Y_TIMER: usize = 4;
/// Fires every second while the inspector shows a file converting, so its
/// time left keeps up with the row's own line between progress reports.
const CLOCK_TIMER: usize = 5;
const CLOCK_MS: u32 = 1000;
/// Added to the hint timer's wait, so it fires after the two seconds.
const HINT_SLACK: Duration = Duration::from_millis(50);

/// Posted by the runner's thread; lparam is a `Box<JobNews>`: the job's
/// event and, for a finished job, its results' sizes.
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
/// Posted from a thread pool thread when Windows' Text size changes.
const WM_KILN_TEXT_SCALE: u32 = WM_APP + 9;
/// The notification area's message for Kiln's finish notice: lparam's low
/// word says what happened to it.
const WM_KILN_NOTIFY: u32 = WM_APP + 11;
/// Posted by the thumbnail worker; lparam is a `Box<(PreviewKey, Preview)>`.
const WM_KILN_PREVIEW: u32 = WM_APP + 12;
/// The notice was clicked, timed out, or was hidden.
const NIN_BALLOONHIDE: u32 = 0x403;
const NIN_BALLOONTIMEOUT: u32 = 0x404;
const NIN_BALLOONUSERCLICK: u32 = 0x405;
/// How long a row's second line takes to cross over to what Kiln found.
const FACTS_MS: u32 = 160;
/// Most files the options sheet reads for its estimate.
const PROBE_LIMIT: usize = 64;

/// Drag modifier flags (MK_SHIFT, MK_CONTROL, MK_ALT).
const MK_SHIFT_FLAG: u32 = 0x0004;
const MK_CONTROL_FLAG: u32 = 0x0008;
const MK_ALT_FLAG: u32 = 0x0020;
/// The left button is down (a mouse message's MK_LBUTTON).
const MK_LBUTTON_FLAG: u32 = 0x0001;

/// How long More's outer ring takes to pour open.
const OPEN_MS: u32 = 160;
/// The pulse on a chosen item before its job starts.
const PULSE_MS: u32 = theme::motion::SETTLE_MS;

/// The heat of running progress lines now. Every line shares one clock so
/// they breathe together; with Animation effects off they hold still.
fn heat_now() -> f32 {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let ms = EPOCH.get_or_init(Instant::now).elapsed().as_secs_f32() * 1000.0;
    theme::motion::breath(ms, theme::motion::animations())
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
    /// Whether the pointer is on the centre's "Options… (Tab)".
    options_hover: bool,
}

impl WheelUi {
    fn is_open(&self) -> bool {
        self.closing.is_none() && self.pulse.is_none()
    }

    fn more_open(&self) -> bool {
        self.more.target() > 0.5
    }

    /// Whether it is still blooming open at `now`: the disc, or the last
    /// of its segments, which follow it one after another.
    fn blooming(&self, now: Instant, animations: bool) -> bool {
        let ms = anim::elapsed_ms(self.opened, now);
        let last = self.wheel.segments().saturating_sub(1);
        anim::bloom(ms, animations).0 < 1.0 || anim::segment(ms, last, animations).0 < 1.0
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
            Some(Spot::Inner(i)) if self.wheel.is_more(i) => {
                self.more
                    .set(1.0, OPEN_MS, Instant::now(), theme::motion::animations())
            }
            Some(Spot::Inner(_)) => self.more.set(
                0.0,
                theme::motion::QUICK_MS,
                Instant::now(),
                theme::motion::animations(),
            ),
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

/// Move each of `files` with `move_one`, which says where it went or
/// `None` where it couldn't be saved. If it panics, the files it hadn't
/// reported on count as not saved, so every row leaves "Saving…" and keeps
/// its result; a file it had already moved still counts as moved.
fn export_each(
    files: Vec<PathBuf>,
    mut move_one: impl FnMut(&Path) -> Option<PathBuf>,
) -> Vec<(PathBuf, Option<PathBuf>)> {
    let mut results = Vec::with_capacity(files.len());
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        for f in &files {
            let to = move_one(f);
            results.push((f.clone(), to));
        }
    }));
    let reported = results.len();
    results.extend(files[reported..].iter().map(|f| (f.clone(), None)));
    results
}

/// Whether `path` is `dir` or inside it, as Windows compares names: case
/// doesn't matter.
fn inside(path: &Path, dir: &Path) -> bool {
    let lower = |p: &Path| PathBuf::from(p.to_string_lossy().to_lowercase());
    lower(path).starts_with(lower(dir))
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
    /// A Clear that waited for an export: only the rows listed when its
    /// prompt was answered go, and rows added since stay.
    ClearOnly(Vec<PathBuf>),
    Remove(Vec<PathBuf>),
    /// Start a new job on rows, which replaces what they hold.
    Start(Action, Vec<PathBuf>, Option<Quality>),
}

/// Pin a leave to the rows listed when its prompt chose Export all. A
/// Clear then covers only those rows, by file: rows added while the export
/// runs are neither exported nor cleared.
fn pin(rows: Rows, then: Then, listed: Vec<PathBuf>) -> (Rows, Then) {
    match then {
        Then::Clear => (Rows::Files(listed.clone()), Then::ClearOnly(listed)),
        then => (rows, then),
    }
}

/// A leave waiting on exports: its own Export all, or exports already
/// moving its rows' results.
struct Leaving {
    /// The exports it waits on. It goes ahead once they're all done.
    waits: HashSet<u64>,
    rows: Rows,
    /// Where its Export all went, for results that finish meanwhile.
    /// `None` when it only waited: those results then ask for a folder.
    folder: Option<PathBuf>,
    then: Then,
}

/// The settings panel while it is open.
struct SettingsUi {
    tab: SettingsTab,
    /// Where the tab marker sits, in tabs from the left; it slides.
    tab_marker: Tween,
    /// When the page last changed, for the crossfade.
    switched: Instant,
    /// When it opened: it rises into place (`anim::SHEET_MS`).
    opened: Instant,
    hover: Option<usize>,
    focus: Option<usize>,
    output_line: (String, bool),
    engine_lines: [(String, bool); 4],
    /// Engines Kiln can't find, which get a Get it link.
    engine_missing: [bool; 4],
    /// A sentence the panel says in its note line, over the update
    /// check's, such as that a download page didn't open.
    note: Option<String>,
}

/// The text beside a settings chooser, and whether it's muted.
type ChooserLine = (String, bool);

/// The details dialog: what a failed job's engine said.
struct DetailsUi {
    /// "What FFmpeg said".
    title: String,
    text: String,
    /// How far the text is scrolled, in DIPs from its top.
    scroll: f32,
    /// The button with keyboard focus. It starts on Copy.
    focus: DialogButton,
    hover: Option<DialogButton>,
    /// The button the pointer went down on; it acts on release.
    pressed: Option<DialogButton>,
    /// Whether the last copy worked and when it was, for the button's
    /// "Copied" or "Couldn't copy".
    last_copy: Option<(bool, Instant)>,
}

/// Whether a held file a drag carried is still there, as the checks after
/// a drag see it: on disk, or being moved by an export (`exporting`, from
/// `saving_files`), which records where it went once it reports.
fn still_there(exporting: &HashSet<PathBuf>, p: &Path) -> bool {
    p.exists() || exporting.contains(p)
}

/// What releasing a list press without a drag does to the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Release {
    /// The press already did all it does.
    Nothing,
    /// A plain press inside a selection of two or more rows kept it: the
    /// release is the click.
    Click,
    /// A Ctrl-press on a selected row kept it: the release is the
    /// Ctrl-click, which takes the row out.
    CtrlClick,
}

/// A press on a list row, until the button is released or a drag starts.
#[derive(Debug, Clone, Copy)]
struct ListPress {
    row: usize,
    /// Where the pointer went down, in layout units.
    point: (f32, f32),
    /// What the release without a drag does.
    release: Release,
    /// Moving far enough with the button down drags the selection's held
    /// results out of Kiln.
    drag: bool,
}

/// What sits over the file list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Over {
    Nothing,
    /// A wheel, open or fading.
    Wheel,
    /// A sheet, Settings, the details dialog or help. It covers a wheel
    /// too.
    Panel,
}

impl Over {
    fn of(wheel: bool, sheet: bool, settings: bool, details: bool, help: bool) -> Self {
        if sheet || settings || details || help {
            Over::Panel
        } else if wheel {
            Over::Wheel
        } else {
            Over::Nothing
        }
    }

    /// Whether the list beneath takes clicks, double-clicks, links, the
    /// right-click menu, scrolling and tooltips.
    fn list_live(self) -> bool {
        self == Over::Nothing
    }

    /// Whether a drag may pour a wheel: not under a panel.
    fn wheel_may_pour(self) -> bool {
        self != Over::Panel
    }
}

/// What the tooltips' rectangles depend on, so they are measured again
/// only when it changes, not on every animation frame.
#[derive(PartialEq)]
struct TipKey {
    size: (f32, f32),
    scroll: f32,
    over: Over,
    add: Option<layout::Rect>,
    export: Option<layout::Rect>,
    /// Each row in view: its index, rectangle, chip code and shown line,
    /// whether it's being exported, its job's kind and whether it holds a
    /// result.
    rows: Vec<TipRow>,
}

type TipRow = (
    usize,
    layout::Rect,
    String,
    String,
    bool,
    std::mem::Discriminant<JobState>,
    bool,
);

/// The help overlay while it is open. Keys go to it, and its Close has
/// the focus, until it closes.
struct HelpUi {
    sections: Vec<HelpSection>,
    /// How far the text is scrolled, in DIPs from its top.
    scroll: f32,
    hover: bool,
    /// Close went down under the pointer; it acts on release.
    pressed: bool,
    /// When it opened: it rises into place (`anim::SHEET_MS`).
    opened: Instant,
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
    /// How many jobs the batch started, and how many are still to finish.
    groups: usize,
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

/// A picture painted in a frame: its row and size, its pixels, its key's
/// id and how far it has faded in.
type Painted = (usize, PreviewSize, Arc<previews::Pixels>, u64, f32);

/// The kind of file `e` is, by its type; `None` for a type Kiln has no
/// kind for.
fn kind_of(kiln: &Kiln, e: &model::Entry) -> Option<kiln_core::Kind> {
    kiln.registry().format(&e.code).map(|f| f.kind)
}

/// The name of the one file the inspector shows, which its buttons are
/// named with: empty for several files or none.
fn panel_file<'a>(model: &insp::Inspector, entries: &'a [model::Entry]) -> &'a str {
    match model {
        insp::Inspector::Waiting { row, .. }
        | insp::Inspector::Converting { row, .. }
        | insp::Inspector::Done { row, .. }
        | insp::Inspector::Failed { row, .. } => entries.get(*row).map_or("", |e| e.name.as_str()),
        insp::Inspector::Nothing | insp::Inspector::Several { .. } => "",
    }
}

/// The inspector as last built for the selection, while it shows or is
/// folding away.
struct Panel {
    /// Where it was laid out: its whole place at the right of the list's
    /// panel, as it is once it shows in full. The fold slides it across
    /// from there (`App::inspector_shift`).
    rect: layout::Rect,
    /// What it says.
    model: insp::Inspector,
    /// What Nothing says under its title as it was built with: the key
    /// hint, then the privacy line.
    nothing: (String, &'static str),
    layout: layout::InspectorLayout,
    /// Its controls that are placed, in reading order, with where each is.
    /// The pointer, Tab, the drawing and the tree all go by these.
    controls: Vec<(InspectorControl, layout::Rect)>,
    /// Each format's glaze, or each kind's, as its rows show them.
    glazes: Vec<theme::Glaze>,
    /// What it shows, by the files themselves: whether new content is
    /// another page (`new_content`), and whether its rows still hold them.
    shows: Shows,
}

impl Panel {
    /// Whether the rows it draws by their place in the list still hold the
    /// files it shows in `entries`. Once rows came or went above them, it
    /// would show other files.
    fn still_listed(&self, entries: &[model::Entry]) -> bool {
        self.shows.still_at(self.pictured(), |i| {
            entries.get(i).map(|e| e.path.as_path())
        })
    }

    /// The rows it pictures at its own size: the one file, or the first
    /// three of several.
    fn pictured(&self) -> &[usize] {
        match &self.model {
            insp::Inspector::Nothing => &[],
            insp::Inspector::Waiting { row, .. }
            | insp::Inspector::Converting { row, .. }
            | insp::Inspector::Done { row, .. }
            | insp::Inspector::Failed { row, .. } => std::slice::from_ref(row),
            insp::Inspector::Several { rows, .. } => &rows[..rows.len().min(3)],
        }
    }

    /// Where control `c` is, if it is placed.
    fn rect_of(&self, c: InspectorControl) -> Option<layout::Rect> {
        self.controls.iter().find(|(k, _)| *k == c).map(|(_, r)| *r)
    }

    /// The control under a point.
    fn control_at(&self, x: f32, y: f32) -> Option<InspectorControl> {
        self.controls
            .iter()
            .find(|(_, r)| r.contains(x, y))
            .map(|(c, _)| *c)
    }
}

/// How `p` shows in a frame: at `alpha`, still `rise` DIPs from its
/// place, its controls lit by the pointer only while `live`.
fn page(p: &Panel, alpha: f32, rise: f32, live: bool) -> InspectorPage<'_> {
    InspectorPage {
        laid: p.rect,
        model: &p.model,
        layout: &p.layout,
        controls: &p.controls,
        glazes: &p.glazes,
        nothing: [&p.nothing.0, p.nothing.1],
        alpha,
        rise,
        live,
    }
}

/// What the previews asked for depend on: see `App::ask_previews`.
#[derive(Debug, Clone, PartialEq)]
struct PreviewAsk {
    /// The rows whose pictures the inspector shows, at its own size: none
    /// while it doesn't show.
    pictured: Vec<usize>,
    /// The rows in view, which scrolling and the window's size move.
    visible: std::ops::Range<usize>,
    /// How many rows there are, which adding files changes.
    total: usize,
    /// Device pixels per layout DIP, which the screen and Text size set.
    px_per_dip: f32,
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
    /// Kiln's dark or light palette, or one from a contrast theme.
    palette: Palette,
    /// The contrast theme's colours, while one is on.
    contrast: Option<theme::SysColors>,
    /// Windows' Text size, and where it's read from.
    text_scale: f32,
    text_scale_source: system::TextScale,
    /// DIPs per layout unit: the Text size, or less where the screen is
    /// too small for it (`layout::text_zoom`).
    zoom: f32,
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
    /// What moves in the window, and each row's times: see `Motion`.
    motion: Motion,
    scroll: f32,
    hover_row: Option<usize>,
    hover_cancel: bool,
    hover_link: bool,
    hover_retry: bool,
    drag: Tween,
    /// The files in the current drag, read when it entered the window.
    drag_paths: Vec<PathBuf>,
    wheel: Option<WheelUi>,
    sheet: Option<SheetUi>,
    next_sheet: u64,
    /// The details dialog, while it's open.
    details: Option<DetailsUi>,
    /// The help overlay, while it's open.
    help: Option<HelpUi>,
    help_hover: bool,
    /// The native tooltips over the drawn controls, if Windows made them.
    tips: Option<Tips>,
    /// The tooltips' rectangles as last measured, and what they were
    /// measured for.
    tip_cache: Option<(TipKey, Vec<(usize, layout::Rect)>)>,
    status: String,
    clear_hover: bool,
    clear_pressed: bool,
    /// The settings control the pointer went down on; it acts on release.
    settings_pressed: Option<usize>,
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
    notes: model::Notes,
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
    /// The window is gone: nothing more goes ahead.
    destroyed: bool,
    /// What the footer says whenever it has nothing more pressing: why
    /// results are saved next to each file, when there's no holding folder.
    sticky: Option<&'static str>,
    /// Whether the first frame has been drawn.
    painted_once: bool,
    /// Whether this run has said that a helper app is missing, once, so
    /// it isn't said for every file.
    helper_notice: bool,
    /// The taskbar button's progress, made the first time work starts.
    taskbar: Option<system::Taskbar>,
    /// Rows just removed, with where they were, so Undo can bring them
    /// back, and what the footer said then (Undo shows while it still does).
    undo: Option<(Vec<(usize, model::Entry)>, String)>,
    /// Results that were ready before the one-time question about saving
    /// next to the original was answered: the files they came from and the
    /// held results.
    autosave_waiting: Vec<(Vec<PathBuf>, Vec<PathBuf>)>,
    /// What the taskbar shows now, so a change of under a part in a
    /// thousand isn't sent again.
    taskbar_shown: Option<(u8, u32)>,
    /// How work since Kiln was last idle went: ready, failed, cancelled,
    /// and the first result's name, for the finish notice.
    tally: (usize, usize, usize, Option<String>),
    /// The row a clicked finish notice selects.
    notice_row: Option<PathBuf>,
    /// The Add files button, in the empty window or the footer.
    add_hover: bool,
    add_pressed: bool,
    /// The footer's Convert button.
    convert_hover: bool,
    convert_pressed: bool,
    /// The folder files were last added from in this run, where Add files
    /// opens.
    last_add: Option<PathBuf>,
    /// What adding files last said, and when, so the footer's idle hint
    /// can take its place once it has been read.
    added: Option<(String, Instant)>,
    /// The line last said about the work under way, so the footer can add
    /// how to cancel the selected file while it still says it.
    working: Option<String>,
    /// The last status that reported something going wrong or not
    /// happening. While the status still says it, it stands over the
    /// selection count.
    alert: Option<String>,
    /// What UI Automation clients are told the window holds.
    uia: uia::Host,
    /// Each running row's progress as UI Automation was last told it.
    progress: a11y::ProgressGate,
    /// When UI Automation's tree was last built.
    a11y_built: Option<Instant>,
    /// When A11Y_TIMER is due to build it again, for a change held back.
    a11y_due: Option<Instant>,
    /// What to tell UI Automation clients once this message is done.
    a11y_events: Vec<uia::Event>,
    /// Each status set during this message, in order: see `say`.
    status_sets: Vec<String>,
    /// Which status sets are announced.
    teller: a11y::StatusTeller,
    /// A UI Automation client moved the focus: the window takes the
    /// keyboard focus once the message is done.
    take_focus: bool,
    /// The header or footer button Tab moved the keyboard to. `None` is
    /// the list. See `focused_button` for whether it's still there.
    button_focus: Option<FocusButton>,
    /// Files dropped or sent from Explorer while a panel or dialog was up,
    /// added once the list is in view again.
    queued: Vec<PathBuf>,
    /// Set while Kiln runs a drag of its own results, shared with the drop
    /// target, which then ignores it.
    own_drag: Rc<Cell<bool>>,
    /// The press on a list row the button is still down on.
    list_press: Option<ListPress>,
    /// Held files dragged out in this run that are still there: one that
    /// goes later was moved by the drop, not lost.
    dragged: HashSet<PathBuf>,
    /// The listed files' previews from Windows' thumbnail service, for
    /// painting.
    previews: PreviewCache,
    /// Fetches previews on a worker thread; the UI thread never calls the
    /// Shell.
    thumbs: Thumbs,
    /// What the previews were last asked for. `None` asks again after the
    /// next message, whatever else changed.
    previews_asked: Option<PreviewAsk>,
    /// The pictures painted in the last frame. Kept so each frame refills
    /// it rather than making another.
    painted_previews: Vec<Painted>,
    /// The inspector beside the list, as last built: see `refresh_panel`.
    panel: Option<Panel>,
    /// What the inspector showed before its content last changed, and how
    /// far it had faded in then. It fades out over half the cross-fade as
    /// the new content rises in (`Motion::inspector_fading`), then goes;
    /// at once if rows come or go under it (`Panel::still_listed`).
    inspector_prev: Option<(Panel, f32)>,
    /// Whether `CLOCK_TIMER` is running: while the inspector shows a file
    /// converting.
    clock: bool,
    /// The inspector control under the pointer, and the one pressed, which
    /// acts when the pointer is released over it.
    panel_hover: Option<InspectorControl>,
    panel_pressed: Option<InspectorControl>,
    /// What the inspector was last built from, so a message that changed
    /// none of it builds nothing: see `refresh_panel`.
    panel_built: Option<PanelInputs>,
    /// Moves when something the inspector is built from changes outside
    /// the list: Settings, the habits, a result's size, the Text size.
    panel_rev: u64,
    /// Finished results' sizes, for the inspector's before and after. The
    /// runner's thread reads them as each job finishes, so the window
    /// never waits on the disk for them; a result not here has no size to
    /// show yet.
    result_sizes: HashMap<PathBuf, u64>,
}

/// What the inspector is built from: the list's revision, the window's
/// own (`App::panel_rev`), the rows' notes' and the window's size in DIPs.
type PanelInputs = (u64, u64, u64, (f32, f32));

/// A job's news as the runner's thread posts it: the event and, for a
/// finished job, its results' sizes, read there and then.
struct JobNews {
    event: JobEvent,
    sizes: Vec<(PathBuf, u64)>,
}

impl JobNews {
    /// What the runner's thread posts for `event`.
    fn of(event: JobEvent) -> Self {
        let sizes = match &event {
            JobEvent::Finished(_, Ok(outputs)) => outputs
                .iter()
                .filter_map(|p| Some((p.clone(), std::fs::metadata(p).ok()?.len())))
                .collect(),
            _ => vec![],
        };
        JobNews { event, sizes }
    }
}

impl App {
    /// The DPI layout units are drawn at: the monitor's, zoomed by Text
    /// size.
    fn unit_dpi(&self) -> f32 {
        layout::px_per_unit(self.dpi, self.zoom) * 96.0
    }

    /// A client point in layout units.
    fn dips(&self, x: i32, y: i32) -> (f32, f32) {
        let k = 1.0 / layout::px_per_unit(self.dpi, self.zoom);
        (x as f32 * k, y as f32 * k)
    }

    /// The client area in DIPs, before the Text size zoom.
    fn client_dips(&self) -> (f32, f32) {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
        }
        let k = 96.0 / self.dpi;
        (
            (rc.right - rc.left) as f32 * k,
            (rc.bottom - rc.top) as f32 * k,
        )
    }

    /// Work out the zoom again after a resize, a DPI change or a new Text
    /// size, and draw at it.
    fn rezoom(&mut self) {
        let zoom = layout::text_zoom(self.text_scale, self.client_dips());
        if (zoom - self.zoom).abs() < 1e-4 {
            return;
        }
        self.zoom = zoom;
        self.panel_stale();
        let dpi = self.unit_dpi();
        self.renderer.set_dpi(dpi);
        if let Some(t) = &self.tips {
            t.set_dpi(dpi);
        }
        self.tip_cache = None;
        self.clamp_scroll();
        self.invalidate();
    }

    /// Windows' Text size changed: zoom to it, growing the window if it's
    /// now under its smallest size.
    fn text_scale_changed(&mut self) {
        let scale = self.text_scale_source.factor();
        if (scale - self.text_scale).abs() < 1e-4 {
            return;
        }
        self.text_scale = scale;
        self.panel_stale();
        let (w, h) = self.client_dips();
        let (mw, mh) = layout::min_window(scale);
        if w < mw || h < mh {
            grow_client(self.hwnd, self.dpi, (w.max(mw), h.max(mh)));
        }
        self.rezoom();
        self.invalidate();
    }

    /// Read the contrast theme and Animation effects again; repaint in
    /// whatever changed.
    fn display_settings_changed(&mut self) {
        theme::motion::set_animations(system::animations_on());
        // Animation effects off: whatever was easing is where it was going.
        if !theme::motion::animations() {
            self.motion.settle(Instant::now());
            self.inspector_prev = None;
        }
        self.contrast = system::high_contrast();
        self.apply_theme();
        if self.animating() {
            self.animate();
        } else {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), ANIM_TIMER);
            }
        }
        self.invalidate();
    }

    /// A screen point in window DIPs.
    fn screen_dips(&self, x: i32, y: i32) -> (f32, f32) {
        let mut pt = POINT { x, y };
        unsafe {
            let _ = ScreenToClient(self.hwnd, &mut pt);
        }
        self.dips(pt.x, pt.y)
    }

    /// The window as it is drawn now, with as much of the inspector as the
    /// fold shows for this message or frame, read once for all of it
    /// (`Motion::held_fold`). Drawing and the pointer go by it; it reads
    /// no clock.
    fn layout(&self) -> layout::Layout {
        let (w, h) = self.renderer.size_dips();
        layout::layout_with(w, h, self.files.len(), self.motion.held_fold())
    }

    /// The window as it is once the fold has got where it is going: what
    /// UI Automation is told, so its tree doesn't move with each frame.
    fn settled_layout(&self) -> layout::Layout {
        let (w, h) = self.renderer.size_dips();
        layout::layout_with(w, h, self.files.len(), self.motion.fold_target())
    }

    /// The footer's Export all button, while results are held that no
    /// export is moving yet.
    fn export_button(&self) -> Option<layout::Rect> {
        let clear = self.layout().clear_button?;
        (self.export_all_count() > 0).then(|| layout::export_all_button(&clear))
    }

    /// Where the Undo link is now, while the footer offers it.
    fn undo_link_rect(&self) -> Option<layout::Rect> {
        self.undo_offer()?;
        let l = self.layout();
        let (footer, clear) = (l.footer?, l.clear_button?);
        let convert = layout::convert_button(&footer, &clear, self.export_all_count() > 0);
        layout::undo_link(&layout::footer_status(&footer, &convert))
    }

    /// The footer's Convert button, while files are listed.
    fn convert_rect(&self) -> Option<layout::Rect> {
        let l = self.layout();
        let (footer, clear) = (l.footer?, l.clear_button?);
        Some(layout::convert_button(
            &footer,
            &clear,
            self.export_all_count() > 0,
        ))
    }

    /// The selected rows that are converting now.
    fn selected_running(&self) -> Vec<usize> {
        self.files
            .selection()
            .into_iter()
            .filter(|&i| {
                self.files
                    .entries()
                    .get(i)
                    .is_some_and(|e| matches!(e.job, JobState::Running { .. }))
            })
            .collect()
    }

    /// Whether the footer's button reads Cancel: the selection holds a
    /// file that is converting. It then stops those files, from the
    /// pointer, the keyboard or an assistive tool.
    fn footer_cancel(&self) -> bool {
        !self.selected_running().is_empty()
    }

    /// Why the footer's button can't be used on the selection now, if it
    /// can't. Cancel can always be used.
    fn convert_block(&self) -> Option<&'static str> {
        if self.footer_cancel() {
            return None;
        }
        model::convert_block(self.files.selection().len(), 0)
    }

    /// How many held results Export all would take now: those a row says
    /// are gone and those an export is already moving aren't counted, and
    /// Export all skips them.
    fn export_all_count(&self) -> usize {
        let entries = self.files.entries();
        self.files
            .held_count_except(|i| !self.notes.gone(&entries[i].path), &self.saving_files())
    }

    fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    /// Something began to move, or changed: draw it, with frames at the
    /// full rate while anything moves (`frames`).
    fn animate(&self) {
        self.frames();
        self.invalidate();
    }

    /// Run the frame timer at the full rate, if anything needs frames. The
    /// timer slows to the heat's rate, or stops, once it fires and finds
    /// less to do (`anim::frame_ms`). With nothing moving and no line
    /// breathing, as with Animation effects off, no timer is set at all.
    fn frames(&self) {
        if self.animating() {
            unsafe {
                SetTimer(Some(self.hwnd), ANIM_TIMER, anim::FRAME_MS, None);
            }
        }
    }

    /// Whether a frame timer is needed: something moves, or a running
    /// line breathes (not with Animation effects off).
    fn animating(&self) -> bool {
        anim::frame_ms(self.moving(), self.burning(), theme::motion::animations()).is_some()
    }

    /// Whether any job is running, so its progress line glows.
    fn burning(&self) -> bool {
        self.files
            .entries()
            .iter()
            .any(|e| matches!(e.job, JobState::Running { .. }))
    }

    /// Whether anything is in motion that needs full frame rate: the
    /// window's own motion (`Motion::moving_at`), a row's facts crossing
    /// over, the wheel, a sheet, Settings, help or a drag.
    fn moving(&self) -> bool {
        let (now, animations) = (Instant::now(), theme::motion::animations());
        let facts = self.files.entries().iter().any(|e| {
            e.facts_at
                .is_some_and(|t| anim::within(t, FACTS_MS, now, animations))
        });
        let wheel = self.wheel.as_ref().is_some_and(|w| {
            w.blooming(now, animations)
                || w.closing.is_some()
                || w.pulse.is_some()
                || w.more.running(now)
                || w.dot_at
                    .is_some_and(|t| anim::within(t, FACTS_MS, now, animations))
        });
        let rising = |t: Instant| anim::within(t, anim::SHEET_MS, now, animations);
        let sheet = self
            .sheet
            .as_ref()
            .is_some_and(|s| rising(s.opened) || s.closing.is_some());
        let settings = self.settings_ui.as_ref().is_some_and(|ui| {
            rising(ui.opened)
                || ui.tab_marker.running(now)
                || anim::within(ui.switched, theme::motion::QUICK_MS, now, animations)
        });
        let help = self.help.as_ref().is_some_and(|ui| rising(ui.opened));
        self.motion.moving_at(now, animations)
            || self.drag.running(now)
            || facts
            || wheel
            || sheet
            || settings
            || help
    }

    /// Where button `b` is now, if it's there. An inspector control is
    /// where it is drawn, as the fold and a cross-fade move it.
    fn focus_button_rect(&self, b: FocusButton) -> Option<layout::Rect> {
        let l = self.layout();
        match b {
            FocusButton::Convert => self
                .convert_rect()
                .filter(|_| self.convert_block().is_none()),
            FocusButton::AddFiles => self.add_button(),
            FocusButton::ExportAll => self.export_button(),
            FocusButton::Clear => l.clear_button,
            FocusButton::Help => Some(l.help_button),
            FocusButton::Settings => Some(l.settings_button),
            FocusButton::Inspector(c) => {
                let r = self.live_panel()?.rect_of(c)?;
                let (_, dx, dy) = self.inspector_shift(Instant::now())?;
                Some(layout::Rect::new(r.x + dx, r.y + dy, r.w, r.h))
            }
        }
    }

    /// The button the keyboard is on, while it's there and nothing covers
    /// it. A button that went, such as Export all once everything is
    /// saved, hands the focus back to the list.
    fn focused_button(&self) -> Option<FocusButton> {
        self.button_focus
            .filter(|&b| self.over().list_live() && self.focus_button_rect(b).is_some())
    }

    /// Enter or Space on the button the keyboard is on: what a click does.
    fn press_button(&mut self, b: FocusButton) {
        match b {
            FocusButton::Convert => self.convert_selection(true),
            FocusButton::AddFiles => self.add_files(true),
            FocusButton::ExportAll => self.export_everything(),
            FocusButton::Clear => self.clear(),
            FocusButton::Help => self.open_help(),
            FocusButton::Settings => self.open_settings(),
            FocusButton::Inspector(c) => self.panel_act(c, true),
        }
    }

    // The inspector.

    /// Whether the inspector carries the selection's actions: files are
    /// listed and the fold is heading to show it, however far it has slid
    /// in yet. Only then do its controls take the pointer, Tab and
    /// assistive tools, and only then do rows show no links of their own
    /// (`row_links`). Folding away, it is drawn sliding out but takes
    /// nothing, and UI Automation's tree leaves it out.
    fn inspector_live(&self) -> bool {
        !self.files.is_empty() && self.motion.inspector_staying()
    }

    /// The inspector while it carries the actions (`inspector_live`).
    fn live_panel(&self) -> Option<&Panel> {
        self.panel.as_ref().filter(|_| self.inspector_live())
    }

    /// The inspector's whole place, as it is once it shows in full, which
    /// its content is laid out on. `None` with no files listed.
    fn full_inspector(&self) -> Option<layout::Rect> {
        if self.files.is_empty() {
            return None;
        }
        let (w, h) = self.renderer.size_dips();
        layout::layout_with(w, h, 1, 1.0).inspector
    }

    /// Where the inspector is drawn at `now`, and how far its content is
    /// moved from where it was laid out: across by the fold's slide (as
    /// far as `fold` has it), and down by new content's rise. `None` while
    /// none of it shows. The pointer goes by it, so it hits what is drawn.
    fn inspector_shift(&self, now: Instant) -> Option<(layout::Rect, f32, f32)> {
        let p = self.panel.as_ref()?;
        if self.files.is_empty() {
            return None;
        }
        let (w, h) = self.renderer.size_dips();
        let drawn = layout::layout_with(w, h, 1, self.motion.held_fold()).inspector?;
        let (_, rise) = self.inspector_rise(now);
        Some((drawn, drawn.x - p.rect.x, rise))
    }

    /// The inspector's content's way in at `now`: see
    /// `Motion::inspector_rise`.
    fn inspector_rise(&self, now: Instant) -> (f32, f32) {
        self.motion.inspector_rise(now, theme::motion::animations())
    }

    /// How far what the inspector showed before still shows at `now`: see
    /// `Motion::inspector_fading`. `None` once it has gone, or with
    /// nothing fading.
    fn inspector_fading(&self, now: Instant) -> Option<f32> {
        let (_, from) = self.inspector_prev.as_ref()?;
        self.motion.inspector_fading(*from, now)
    }

    /// A page fading out draws its rows by their place in the list: once
    /// rows came or went above them, it would show other files, so it goes
    /// at once (`Panel::still_listed`).
    fn drop_moved_page(&mut self) {
        let entries = self.files.entries();
        if self
            .inspector_prev
            .as_ref()
            .is_some_and(|(p, _)| !p.still_listed(entries))
        {
            self.inspector_prev = None;
            self.invalidate();
        }
    }

    /// Whether `(x, y)` is on the inspector as it is drawn, live or
    /// folding away: no part of the list.
    fn on_inspector(&self, x: f32, y: f32) -> bool {
        self.inspector_shift(Instant::now())
            .is_some_and(|(drawn, ..)| drawn.contains(x, y))
    }

    /// The inspector control drawn at `(x, y)`, while it carries the
    /// actions.
    fn panel_control_at(&self, x: f32, y: f32) -> Option<InspectorControl> {
        let p = self.live_panel()?;
        let (drawn, dx, dy) = self.inspector_shift(Instant::now())?;
        if !drawn.contains(x, y) {
            return None;
        }
        p.control_at(x - dx, y - dy)
    }

    /// Build the inspector again for the selection as it is now, when what
    /// it is built from has changed: the list's revision (the selection, a
    /// row's job, its facts or its results), the window's own (Settings,
    /// the habits, a result's size, the Text size) or the window's size.
    /// Otherwise it does nothing at all. Its words are measured only when
    /// they change. A control the keyboard or the pointer was on that has
    /// gone hands the focus back to the list; one still there keeps it.
    /// New content cross-fades with what it showed before
    /// (`new_content`). Run after every message and before every frame.
    fn refresh_panel(&mut self) {
        if self.destroyed {
            return;
        }
        self.drop_moved_page();
        // Minimised, the inspector stays as it was built, so it comes back
        // as it went.
        if self.minimised() {
            return;
        }
        if !self.inspector_live() {
            self.panel_folded();
            return;
        }
        // Nothing it is built from has changed: nothing to do, and nothing
        // allocated to find that out.
        let inputs = (
            self.files.revision(),
            self.panel_rev,
            self.notes.revision(),
            self.renderer.size_dips(),
        );
        if self.panel_built == Some(inputs) {
            return;
        }
        self.panel_built = Some(inputs);
        let Some(rect) = self.full_inspector() else {
            self.panel_folded();
            return;
        };
        let model = {
            let kiln = &self.kiln;
            let sizes = &self.result_sizes;
            let notes = &self.notes;
            let kind = |e: &model::Entry| kind_of(kiln, e);
            let size = |p: &Path| sizes.get(p).copied();
            let note = |p: &Path| notes.get(p).cloned();
            let look = insp::Lookups {
                kind_of: &kind,
                size_of: &size,
                note_of: &note,
            };
            insp::inspect(&self.files, &look, &self.settings.habits)
        };
        // While a file converts, its time left is worked out again each
        // second, as the row's own line is.
        self.keep_clock(matches!(model, insp::Inspector::Converting { .. }));
        let lines = ways_in::empty_lines(&self.settings);
        let nothing = (lines.keys, lines.privacy);
        let entries = self.files.entries();
        let selected = self.files.selected_rows().iter();
        let shows = Shows::of(
            &model,
            selected.filter_map(|&i| entries.get(i).map(|e| e.path.as_path())),
        );
        // The same words for the same files, the same size: nothing to do.
        if self.panel.as_ref().is_some_and(|p| {
            p.rect == rect
                && p.nothing == nothing
                && same_page((&p.model, &p.shows), (&model, &shows))
        }) {
            return;
        }
        let renderer = &self.renderer;
        let measure = |s: &str, how: layout::Wrapped| {
            renderer.inspector_text_height(s, how, layout::INSPECTOR_TEXT_WIDTH)
        };
        let said = [nothing.0.as_str(), nothing.1];
        let shape = layout::inspector_shape(&model, said, renderer.more_width(), &measure);
        let il = layout::inspector_layout(&rect, &shape);
        let controls = a11y::inspector_controls(&model, &il);
        let glazes = self.offer_glazes(&model);
        let fresh = self
            .panel
            .as_ref()
            .is_none_or(|p| new_content(&p.shows, &shows));
        let old = self.panel.replace(Panel {
            rect,
            model,
            nothing,
            layout: il,
            controls,
            glazes,
            shows,
        });
        self.panel_lost();
        // New content rises in as what was there fades out, from as far as
        // it had come in itself. The same files' content changing (their
        // facts, a converting file's figures, a result saved), a row moving
        // in the list, or a new size, is laid out again in place.
        if !fresh {
            self.invalidate();
            return;
        }
        let now = Instant::now();
        let fading = self.inspector_fading(now);
        match self
            .motion
            .content_changed(fading, now, theme::motion::animations())
        {
            // What was fading out still shows more than what was rising in:
            // it carries on fading from where it is, and the other goes.
            Some((Fading::Before, from)) => {
                if let Some((_, a)) = &mut self.inspector_prev {
                    *a = from;
                }
            }
            // What was there fades out, unless its rows moved under it.
            Some((Fading::Now, from)) => {
                let entries = self.files.entries();
                self.inspector_prev = old.filter(|p| p.still_listed(entries)).map(|p| (p, from));
            }
            None => self.inspector_prev = None,
        }
        self.animate();
    }

    /// The inspector isn't carrying the actions (`inspector_live`): no
    /// control of it keeps the keyboard, the pointer or a press, and
    /// nothing is built for it. What it showed slides out with the fold
    /// and goes once it is out of sight; at once with no files listed, or
    /// once rows came or went under it (`Panel::still_listed`).
    fn panel_folded(&mut self) {
        self.panel_built = None;
        self.keep_clock(false);
        if let Some(FocusButton::Inspector(_)) = self.button_focus {
            self.button_focus = None;
        }
        self.panel_hover = None;
        self.panel_pressed = None;
        let entries = self.files.entries();
        let moved = self
            .panel
            .as_ref()
            .is_some_and(|p| !p.still_listed(entries));
        let out = self.files.is_empty() || moved || !self.motion.folding(Instant::now());
        if out && (self.panel.is_some() || self.inspector_prev.is_some()) {
            self.panel = None;
            self.inspector_prev = None;
            self.invalidate();
        }
    }

    /// Whether the window is minimised, as Windows says. Its client area
    /// is then measured as 1 pixel (`Renderer::size_dips`), which nothing
    /// that follows the window's size should follow.
    fn minimised(&self) -> bool {
        unsafe { IsIconic(self.hwnd).as_bool() }
    }

    /// Run `CLOCK_TIMER` while `on`, and only then.
    fn keep_clock(&mut self, on: bool) {
        if on == self.clock {
            return;
        }
        self.clock = on;
        unsafe {
            if on {
                SetTimer(Some(self.hwnd), CLOCK_TIMER, CLOCK_MS, None);
            } else {
                let _ = KillTimer(Some(self.hwnd), CLOCK_TIMER);
            }
        }
    }

    /// Bring the window's motion up to date with whatever changed: the
    /// fold with the window's width, the selection's glide with the
    /// selection, and the buttons' hover and press with the pointer.
    /// Whatever begins to move gets frames. Run after every message and
    /// before every frame: when nothing changed it does nothing, and it
    /// keeps nothing new but a button the first time it is hovered.
    fn follow(&mut self) {
        // Minimised, there is nothing on screen to follow: restoring shows
        // the window as it was.
        if self.destroyed || self.minimised() {
            return;
        }
        let (now, animations) = (Instant::now(), theme::motion::animations());
        let (w, _) = self.renderer.size_dips();
        let listed = !self.files.is_empty();
        let mut began = self.motion.fold_to(w, listed, now, animations);
        began |= self.follow_selection(now, animations);
        began |= self.follow_buttons(now, animations);
        if began {
            self.animate();
        }
    }

    /// The one selected row's wash glides when it moves to a row nearby.
    fn follow_selection(&mut self, now: Instant, animations: bool) -> bool {
        let sel = self.files.selected_rows();
        let seen = match (sel.len(), sel.first()) {
            (1, Some(&i)) => Some((i, self.files.len())),
            _ => None,
        };
        if seen == self.motion.glided {
            return false;
        }
        // Only a change of selection pays for working out the rows in view.
        let reach = self.visible_rows().len();
        self.motion.glide(seen, reach, now, animations)
    }

    /// Each button eases toward what the pointer is doing to it: the
    /// header's and footer's, the inspector's, and those on a sheet or
    /// dialog over the window.
    fn follow_buttons(&mut self, now: Instant, animations: bool) -> bool {
        let buttons = [
            (
                FocusButton::Convert,
                self.convert_hover,
                self.convert_pressed,
            ),
            (FocusButton::AddFiles, self.add_hover, self.add_pressed),
            (
                FocusButton::ExportAll,
                self.export_hover,
                self.export_pressed,
            ),
            (FocusButton::Clear, self.clear_hover, self.clear_pressed),
            (FocusButton::Help, self.help_hover, false),
            (FocusButton::Settings, self.settings_hover, false),
        ];
        let inspector = |c: InspectorControl| ButtonId::Window(FocusButton::Inspector(c));
        // A sheet or dialog takes the pointer; with none, the inspector's
        // controls have it while nothing is over the list.
        let (hover, press) = match self.overlay_buttons() {
            Some(over) => over,
            None => (
                self.panel_hover
                    .filter(|_| self.over().list_live())
                    .map(inspector),
                self.panel_pressed.map(inspector),
            ),
        };
        let mut began = false;
        for (b, hover, press) in buttons {
            let b = ButtonId::Window(b);
            began |= self.motion.ease_button(b, hover, press, now, animations);
        }
        began | self.motion.ease_pointed(hover, press, now, animations)
    }

    /// While a sheet or dialog is open over the window: the button on it
    /// the pointer is on, and the one it pressed. Help is drawn over the
    /// rest, then the options sheet, Settings and the details dialog. The
    /// sheet's and Settings' layouts are worked out only while the pointer
    /// is on one of their controls.
    fn overlay_buttons(&self) -> Option<(Option<ButtonId>, Option<ButtonId>)> {
        if let Some(h) = &self.help {
            let close = |on: bool| on.then_some(ButtonId::Help);
            return Some((close(h.hover), close(h.pressed)));
        }
        if let Some(s) = &self.sheet {
            if s.hover.is_none() && s.pressed.is_none() {
                return Some((None, None));
            }
            let l = self.sheet_layout()?;
            let at = |k: Option<usize>| {
                let (c, _) = l.controls.get(k?)?;
                ButtonId::sheet(*c)
            };
            return Some((at(s.hover), at(s.pressed)));
        }
        if let Some(ui) = &self.settings_ui {
            if ui.hover.is_none() && self.settings_pressed.is_none() {
                return Some((None, None));
            }
            let l = self.settings_layout();
            let at = |k: Option<usize>| {
                let (c, _) = l.controls.get(k?)?;
                ButtonId::settings(*c)
            };
            return Some((at(ui.hover), at(self.settings_pressed)));
        }
        let d = self.details.as_ref()?;
        Some((
            d.hover.map(ButtonId::Details),
            d.pressed.map(ButtonId::Details),
        ))
    }

    /// Something the inspector is built from changed outside the list:
    /// build it again after this message.
    fn panel_stale(&mut self) {
        self.panel_rev = self.panel_rev.wrapping_add(1);
    }

    /// The inspector changed or went: a control the keyboard, the pointer
    /// or a press was on that isn't there any more lets go of it, and the
    /// keyboard goes back to the list.
    fn panel_lost(&mut self) {
        let there =
            |c: InspectorControl| self.panel.as_ref().is_some_and(|p| p.rect_of(c).is_some());
        if let Some(FocusButton::Inspector(c)) = self.button_focus {
            if !there(c) {
                self.button_focus = None;
            }
        }
        self.panel_hover = self.panel_hover.filter(|&c| there(c));
        self.panel_pressed = self.panel_pressed.filter(|&c| there(c));
    }

    /// The glaze of each format the inspector offers, or of each kind when
    /// it offers kinds, as its rows show them.
    fn offer_glazes(&self, model: &insp::Inspector) -> Vec<theme::Glaze> {
        let reg = self.kiln.registry();
        let glaze = |code: &str| theme::glaze_of(reg.format(code).map(|f| f.kind));
        match model {
            insp::Inspector::Several {
                targets, offered, ..
            } if targets.is_empty() => offered.iter().map(|(code, _)| glaze(code)).collect(),
            insp::Inspector::Waiting { targets, .. } | insp::Inspector::Several { targets, .. } => {
                targets.iter().map(|t| glaze(&t.ext)).collect()
            }
            _ => vec![],
        }
    }

    /// Do what inspector control `c` is for, from a click, a key
    /// (`keyboard`) or an assistive tool. A format converts the files the
    /// inspector shows, counted in habits as a pick from the wheel is; More
    /// opens the format choice for them; a kind narrows the selection to
    /// its files; Get opens the download page of the helper app they wait
    /// on; the buttons do what the row's own would.
    fn panel_act(&mut self, c: InspectorControl, keyboard: bool) {
        use insp::Inspector as I;
        use InspectorControl as C;
        let Some(p) = self.live_panel() else {
            return;
        };
        let at = p.rect_of(c);
        let model = p.model.clone();
        let path = |app: &App, i: usize| app.files.entries().get(i).map(|e| e.path.clone());
        match (c, &model) {
            (C::Target(k), I::Waiting { row, targets, .. }) => {
                if let (Some(t), Some(file)) = (targets.get(k), path(self, *row)) {
                    self.convert_now(&t.ext, vec![file]);
                }
            }
            // Every selected file shares the format, not just those pictured.
            (C::Target(k), I::Several { targets, .. }) => {
                if let Some(t) = targets.get(k) {
                    let paths = self.selected_paths();
                    self.convert_now(&t.ext, paths);
                }
            }
            (C::More, I::Waiting { row, .. }) => {
                if let Some(file) = path(self, *row) {
                    let from = at.map_or_else(
                        || self.row_centre(*row),
                        |r| (r.x + r.w / 2.0, r.y + r.h / 2.0),
                    );
                    self.open_choice(WheelKind::Convert, vec![file], from, keyboard);
                }
            }
            // Every selected file, as their shared formats are.
            (C::More, I::Several { rows, .. }) => {
                let from = at
                    .map(|r| (r.x + r.w / 2.0, r.y + r.h / 2.0))
                    .or_else(|| rows.first().map(|&i| self.row_centre(i)));
                if let Some(from) = from {
                    let paths = self.selected_paths();
                    self.open_choice(WheelKind::Convert, paths, from, keyboard);
                }
            }
            (C::Kind(k), I::Several { offered, .. }) => {
                if let Some((code, _)) = offered.get(k) {
                    self.select_kind(code, keyboard);
                }
            }
            // The row's own way to get its helper app. Several files are
            // offered Get only when they all wait on the same one.
            (C::GetHelper, I::Waiting { .. } | I::Several { .. }) => {
                let row = match &model {
                    I::Waiting { row, .. } => Some(*row),
                    I::Several { rows, .. } => rows.first().copied(),
                    _ => None,
                };
                let backend = row
                    .and_then(|i| self.files.entries().get(i))
                    .and_then(|e| e.needs_helper())
                    .map(str::to_owned);
                if let Some(backend) = backend {
                    self.get_helper(&backend);
                }
            }
            (C::Cancel, I::Converting { row, .. }) => self.cancel_row(*row),
            (C::Export, I::Done { row, .. }) => self.export_row(*row),
            (C::ShowInFolder, I::Done { row, .. }) => self.show_row(*row),
            (C::Retry, I::Failed { row, .. }) => self.retry_row(*row),
            (C::ShowDetails, I::Failed { row, .. }) => self.show_details(*row),
            _ => {}
        }
        self.invalidate();
    }

    /// Convert `paths` to `ext` from the inspector, unless one of them is
    /// converting already: then the footer says so, as the format choice
    /// would.
    fn convert_now(&mut self, ext: &str, paths: Vec<PathBuf>) {
        match self.files.convert_busy(&paths) {
            Some(why) => self.alert(why.to_owned()),
            None => self.start(Action::Convert(ext.to_owned()), paths, None),
        }
    }

    /// Narrow the selection to its files of type `code`, as the inspector
    /// offers when several files share no format.
    fn select_kind(&mut self, code: &str, keyboard: bool) {
        let entries = self.files.entries();
        let rows: Vec<usize> = self
            .files
            .selection()
            .into_iter()
            .filter(|&i| entries.get(i).is_some_and(|e| insp::type_code(e) == code))
            .collect();
        let Some(&first) = rows.first() else {
            return;
        };
        self.files.select_only(&rows);
        if keyboard {
            self.files.keyboard_used();
        }
        self.scroll_to_row(first);
    }

    /// Add `paths` to the list. `keys` says a key asked for them (Ctrl+O,
    /// Ctrl+V), which shows the focus ring on the new focus row.
    fn add(&mut self, paths: Vec<PathBuf>, keys: bool) {
        // Folders give their files; a big one asks first.
        let opened = model::expand_dropped(paths);
        if opened.from_folders > model::FOLDER_ASK_ABOVE
            && system::ask(self.hwnd, &model::big_folder_prompt(opened.from_folders))
                != model::PromptButton::AddAll
        {
            self.say("Nothing added.");
            self.invalidate();
            return;
        }
        let from = opened.source();
        let skipped = opened.skipped;
        let available = self.availability().clone();
        let r = self.files.add(&self.kiln, &available, opened.files);
        // The new rows pour in on the end of the list, one after another.
        let poured = self.files.len().saturating_sub(self.motion.rows.len());
        self.motion
            .rows_added(self.motion.rows.len(), poured, Instant::now());
        let skipped_count = r.skipped + skipped.len();
        let mut said = model::drop_line(
            self.files.len(),
            r.added,
            r.duplicates,
            from.as_deref(),
            &skipped,
        );
        if r.skipped > 0 {
            said = format!("{said} {} didn't open.", r.skipped);
        }
        self.say(said);
        // An add that added nothing says why, whatever is selected.
        if r.added == 0 {
            self.alert = Some(self.status.clone());
        }
        // Only the plain "Added 2 files." gives way to the idle hint; a line
        // about duplicates or skipped items stays until something replaces
        // it.
        self.added = ways_in::hint_follows(r.added, r.duplicates, skipped_count)
            .then(|| (self.status.clone(), Instant::now()));
        if self.added.is_some() {
            self.arm_hint(ways_in::ADDED_FOR);
        }
        // New rows go on the end of the list.
        let fresh: Vec<PathBuf> = self.files.entries()[self.files.len() - r.added..]
            .iter()
            .map(|e| e.path.clone())
            .collect();
        self.inspect(fresh.clone());
        // One calm notice per run that a helper app is missing, for the
        // first file that can't be converted for want of it; the rows
        // carry the rest.
        if !self.helper_notice && skipped_count == 0 {
            let lacking = fresh.iter().find_map(|p| {
                let e = self.files.entries().get(self.files.index_of(p)?)?;
                e.missing
                    .as_ref()
                    .filter(|_| e.targets.is_empty())
                    .map(|m| m.0.clone())
            });
            if let Some(backend) = lacking {
                self.helper_notice = true;
                let name = model::helper_name(&backend);
                self.alert(format!(
                    "{name} isn't installed, so some files can't be converted yet. Use Get {name} to download it."
                ));
            }
        }
        if r.added > 0 {
            self.last_add = self.files.entries()[self.files.len() - 1]
                .path
                .parent()
                .map(Path::to_path_buf);
            // The list shows where the new files start, with that row
            // selected, and the keyboard is on it: Enter converts.
            self.button_focus = None;
            self.files.added(self.files.len() - r.added, keys);
            self.scroll_to_selection();
        }
        self.animate();
    }

    /// Say `text` in the footer as something that went wrong or didn't
    /// happen, which the selection count doesn't hide.
    fn alert(&mut self, text: String) {
        self.alert = Some(text.clone());
        self.say(text);
    }

    /// Set the status line. Each set is told to UI Automation on its own,
    /// once the message is done, so an alert set again with the same words
    /// is said again.
    fn say(&mut self, text: impl Into<String>) {
        self.status = text.into();
        self.status_sets.push(self.status.clone());
    }

    /// Whether the status is an alert that still stands.
    fn alerting(&self) -> bool {
        self.alert.as_deref() == Some(self.status.as_str())
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
    /// would. `keys`: a key asked, not the pointer.
    fn add_files(&mut self, keys: bool) {
        let picked = system::pick_files(
            self.hwnd,
            ways_in::ADD_FILES_TITLE,
            self.last_add.as_deref(),
        );
        if !picked.is_empty() {
            self.add(picked, keys);
        }
    }

    /// Add the files copied in File Explorer, as a drop would. `keys`: a
    /// key asked, not the pointer.
    fn paste(&mut self, keys: bool) {
        let files = system::clipboard_files(self.hwnd);
        if files.is_empty() {
            self.alert(ways_in::NOTHING_TO_PASTE.to_owned());
            self.invalidate();
        } else {
            self.add(files, keys);
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
            self.export_all_count() > 0,
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

    /// The files of the selected rows, in list order.
    fn selected_paths(&self) -> Vec<PathBuf> {
        self.paths_of(&self.files.selection())
    }

    /// The files of rows `rows`.
    fn paths_of(&self, rows: &[usize]) -> Vec<PathBuf> {
        rows.iter()
            .filter_map(|&i| self.files.entries().get(i))
            .map(|e| e.path.clone())
            .collect()
    }

    /// Remove every selected row, asking first as one leave when that
    /// would lose held results or stop running jobs.
    fn remove_selected(&mut self) {
        let paths = self.selected_paths();
        if paths.is_empty() {
            return;
        }
        self.leave(
            Leave::Remove,
            Rows::Files(paths.clone()),
            Then::Remove(paths),
        );
    }

    /// Take the rows for `paths` out of the list, stopping their jobs, and
    /// say the new count once.
    fn remove_rows(&mut self, paths: &[PathBuf]) {
        // Kept so that a removal of several rows can be undone.
        let mut gone: Vec<(usize, model::Entry)> = paths
            .iter()
            .filter_map(|p| {
                let i = self.files.index_of(p)?;
                Some((i, self.files.entries()[i].clone()))
            })
            .collect();
        gone.sort_by_key(|(i, _)| *i);
        for path in paths {
            self.remove_row(path);
        }
        self.previews_rows_changed();
        if gone.len() >= model::UNDO_FROM {
            let line = model::removed_line(gone.len());
            self.say(line.clone());
            self.undo = Some((gone, line));
        } else {
            self.undo = None;
            self.say(status_line(self.files.len(), 0, 0, 0));
        }
    }

    /// The rows the footer's Undo link would bring back, while it shows:
    /// the footer still says they were removed.
    fn undo_offer(&self) -> Option<&str> {
        self.undo
            .as_ref()
            .filter(|(_, said)| *said == self.status)
            .map(|(_, said)| said.as_str())
    }

    /// Bring back the rows just removed, where they were, selected.
    fn undo_remove(&mut self) {
        let Some((gone, _)) = self.undo.take() else {
            return;
        };
        let now = Instant::now();
        let mut back = vec![];
        let mut paths = vec![];
        for (i, entry) in gone {
            // Already listed again, by an add meanwhile: leave that be.
            if self.files.index_of(&entry.path).is_some() {
                continue;
            }
            let i = i.min(self.files.len());
            paths.push(entry.path.clone());
            self.files.insert(i, entry.restored());
            // The rows come back together, pouring in one after another.
            self.motion.rows_added(i, 1, now);
            if let Some(r) = self.motion.rows.get_mut(i) {
                r.order = back.len();
            }
            back.push(i);
        }
        self.previews_rows_changed();
        self.files.select_only(&back);
        self.inspect(paths);
        self.say(model::restored_line(back.len()));
        self.scroll_to_selection();
        // The rows pour back in: frames run until they have.
        self.animate();
    }

    /// Take `path`'s row out of the list, stopping its job. The caller
    /// says the new count.
    fn remove_row(&mut self, path: &Path) {
        // The prompt can take a while; the list may have moved meanwhile.
        let Some(i) = self.files.index_of(path) else {
            return;
        };
        let path = self.files.entries()[i].path.clone();
        self.cancel_jobs_for(&path);
        self.inspector.cancel(&path);
        self.files.remove(i);
        if i < self.motion.rows.len() {
            self.motion.rows.remove(i);
        }
        self.notes.remove(&path);
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
        self.motion.rows.clear();
        self.notes.clear();
        self.previews_rows_changed();
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
    /// export is already moving and any removed by hand.
    fn held_by(&self, rows: &Rows) -> Vec<PathBuf> {
        let mask = self.mask(rows);
        let saving = self.saving_files();
        self.files
            .held_only_by(|i| mask.get(i).copied().unwrap_or(false))
            .into_iter()
            .filter(|f| !saving.contains(f) && f.exists())
            .collect()
    }

    /// The exports a leave of `rows` by `then` has to wait for: every one
    /// for a close, which would cut any short, and otherwise those moving
    /// the rows' results.
    fn exports_for(&self, rows: &Rows, then: &Then) -> HashSet<u64> {
        if matches!(then, Then::Close) {
            return self.exports.keys().copied().collect();
        }
        let mask = self.mask(rows);
        let files: HashSet<PathBuf> = self
            .files
            .held_files(|i| mask.get(i).copied().unwrap_or(false))
            .into_iter()
            .collect();
        self.exports
            .iter()
            .filter(|(_, x)| x.files.iter().any(|f| files.contains(f)))
            .map(|(&id, _)| id)
            .collect()
    }

    /// Stop the jobs of the rows `rows` names.
    fn stop_jobs(&self, rows: &Rows) {
        match rows {
            Rows::All => self.runner.cancel_all(),
            Rows::Files(files) => {
                for f in files {
                    self.cancel_jobs_for(f);
                }
            }
        }
    }

    /// Handle the job events already posted to this window, now rather
    /// than after whatever runs next. Nothing is dispatched: only Kiln's
    /// own job messages are taken off the queue.
    fn take_job_news(&mut self) {
        let mut msg = MSG::default();
        while unsafe {
            PeekMessageW(
                &mut msg,
                Some(self.hwnd),
                WM_KILN_JOB,
                WM_KILN_JOB,
                PM_REMOVE,
            )
        }
        .as_bool()
        {
            let news = unsafe { Box::from_raw(msg.lParam.0 as *mut JobNews) };
            self.on_job_news(*news);
        }
    }

    /// A job's news from the runner's thread: its results' sizes, then
    /// the event.
    fn on_job_news(&mut self, news: JobNews) {
        if !news.sizes.is_empty() {
            // A result written where an older one was has its new size.
            self.result_sizes.extend(news.sizes);
            self.panel_stale();
        }
        self.on_job(news.event);
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
        self.leave_after(verb, rows, then, false);
    }

    /// `leave`, where `stopped` says the rows' jobs were just stopped: they
    /// still show as running until they report, but aren't asked about.
    fn leave_after(&mut self, verb: Leave, rows: Rows, then: Then, stopped: bool) {
        // An export under way finishes first.
        if !self.exports.is_empty() {
            if matches!(then, Then::Close) {
                self.close_when_saved = true;
                self.say("Saving… Kiln will close when it's done.".to_owned());
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
                self.say(model::SAVING.to_owned());
                self.invalidate();
                return;
            }
        }
        // A leave whose exports are done may still be waiting for a dialog
        // to close. It goes first, from this message's `run_due`; a close
        // follows it.
        if self.leaving.is_some() {
            if matches!(then, Then::Close) {
                self.close_when_saved = true;
                self.say("Saving… Kiln will close when it's done.".to_owned());
            } else {
                self.say(model::SAVING.to_owned());
            }
            self.invalidate();
            return;
        }
        let mask = self.mask(&rows);
        let held = self.held_by(&rows).len();
        // A new job on a row only replaces what it holds.
        let running: Vec<&Action> = match then {
            Then::Start(..) => vec![],
            _ if stopped => vec![],
            _ => self
                .files
                .entries()
                .iter()
                .zip(&mask)
                .filter_map(|(e, &picked)| match &e.job {
                    JobState::Running { action, .. } if picked => Some(action),
                    _ => None,
                })
                .collect(),
        };
        let moving = running.iter().any(|a| matches!(a, Action::File(_)));
        let Some(prompt) = leave_prompt(held, running.len(), moving, verb) else {
            self.proceed(then);
            return;
        };
        match system::ask(self.hwnd, &prompt) {
            // The other prompts are asked elsewhere, never on a leave.
            PromptButton::Cancel
            | PromptButton::AddAll
            | PromptButton::SaveNextTo
            | PromptButton::KeepHeld => {}
            PromptButton::ExportAll => {
                let listed = self
                    .files
                    .entries()
                    .iter()
                    .map(|e| e.path.clone())
                    .collect();
                let (rows, then) = pin(rows, then, listed);
                self.export_then(rows, then);
            }
            PromptButton::Discard => {
                // Results that finished while the prompt was open go too:
                // the user chose to lose what was held.
                self.take_job_news();
                let held = self.held_by(&rows);
                self.discard(&held);
                self.proceed(then);
            }
            PromptButton::Stop(_) => {
                // Stop first, so nothing more finishes, then look again: a
                // job that finished while the prompt was open left a result
                // the prompt never mentioned. Its news may still be queued.
                self.stop_jobs(&rows);
                self.take_job_news();
                let now = self.held_by(&rows);
                match model::after_stop(held, now.len()) {
                    Next::AskAgain => self.leave_after(verb, rows, then, true),
                    _ => self.proceed(then),
                }
            }
        }
    }

    /// Export all, from a leave prompt: pick the folder, export on a
    /// worker thread, and go ahead with `then` once everything is saved.
    /// Cancelling the picker cancels the leave and leaves running jobs be.
    fn export_then(&mut self, rows: Rows, then: Then) {
        let held = self.held_by(&rows);
        match model::leave_next(held.len(), self.exporting(&rows, &then)) {
            Next::Export => {}
            Next::Wait => return self.wait_for_exports(rows, None, then),
            Next::Proceed | Next::AskAgain => return self.proceed(then),
        }
        let Some(dir) = self.pick_export_folder(&held) else {
            return;
        };
        // Results that finished while the picker was open go too, and an
        // export that started meanwhile is waited for.
        let held = self.held_by(&rows);
        match model::leave_next(held.len(), self.exporting(&rows, &then)) {
            Next::Export => {
                if self.export(held, Dest::Into(dir.clone()), true).is_some() {
                    self.wait_for_exports(rows, Some(dir), then);
                }
            }
            Next::Wait => self.wait_for_exports(rows, Some(dir), then),
            Next::Proceed | Next::AskAgain => self.proceed(then),
        }
    }

    /// Whether an export is moving results a leave of `rows` by `then`
    /// has to wait for.
    fn exporting(&self, rows: &Rows, then: &Then) -> bool {
        !self.exports_for(rows, then).is_empty()
    }

    /// Hold a leave until the exports it has to wait for are done.
    fn wait_for_exports(&mut self, rows: Rows, folder: Option<PathBuf>, then: Then) {
        // One leave at a time: `leave_after` turns a second one away.
        debug_assert!(self.leaving.is_none());
        let waits = self.exports_for(&rows, &then);
        if waits.is_empty() {
            return self.proceed(then);
        }
        self.leaving = Some(Leaving {
            waits,
            rows,
            folder,
            then,
        });
        self.invalidate();
    }

    /// A leave whose exports are all done: go ahead, or first export what
    /// finished meanwhile.
    fn resume_leave(&mut self, l: Leaving) {
        let late = self.held_by(&l.rows);
        match model::leave_next(late.len(), self.exporting(&l.rows, &l.then)) {
            Next::Export => match l.folder.clone() {
                Some(dir) => {
                    if self.export(late, Dest::Into(dir), true).is_some() {
                        self.wait_for_exports(l.rows, l.folder, l.then);
                    }
                }
                None => self.export_then(l.rows, l.then),
            },
            Next::Wait => self.wait_for_exports(l.rows, l.folder, l.then),
            Next::Proceed | Next::AskAgain => self.proceed(l.then),
        }
    }

    /// Go ahead with a leave. Only now are its rows' jobs stopped.
    fn proceed(&mut self, then: Then) {
        match then {
            Then::Close => {
                // The run's folder goes with the window: an export still
                // moving files out of it finishes first, then Kiln closes,
                // asking again if anything new is at stake.
                if model::leave_next(0, !self.exports.is_empty()) == Next::Wait {
                    self.close_when_saved = true;
                    self.say("Saving… Kiln will close when it's done.".to_owned());
                    self.invalidate();
                    return;
                }
                self.close_when_saved = false;
                // Nothing goes ahead on a closed window.
                self.leaving = None;
                self.runner.cancel_all();
                unsafe {
                    let _ = DestroyWindow(self.hwnd);
                }
            }
            Then::Clear => self.clear_rows(),
            Then::ClearOnly(paths) => {
                let all = self.files.entries().iter().all(|e| paths.contains(&e.path));
                if all {
                    self.clear_rows();
                } else {
                    self.remove_rows(&paths);
                }
            }
            Then::Remove(paths) => self.remove_rows(&paths),
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
        let n = self.files.held_count_present(|p| p.exists());
        if n == self.shutdown_told {
            return;
        }
        self.shutdown_told = n;
        let reason = leave_prompt(n, 0, false, Leave::Close).map(|p| p.title);
        system::shutdown_reason(self.hwnd, reason.as_deref());
    }

    /// Say on each row whose held result was removed by hand that it's
    /// gone, and stop saying it once it's back. A row being exported is
    /// left be: its files are on their way out.
    fn check_gone(&mut self) {
        // A dragged result that went after its drag was moved by the drop.
        let exporting = self.saving_files();
        let present = |p: &Path| still_there(&exporting, p);
        if self.files.settle_drags(&self.dragged, present) {
            self.dragged.retain(|p| present(p));
            self.dragged_away();
            self.invalidate();
        }
        let saving = self.saving_rows();
        let rows: Vec<(PathBuf, bool)> = self
            .files
            .entries()
            .iter()
            .filter(|e| e.held().is_some() && !saving.contains(&e.path))
            .map(|e| (e.path.clone(), e.is_gone(|p| p.exists())))
            .collect();
        for (path, gone) in rows {
            let noted = self.notes.gone(&path);
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

    /// After a drag moved held results out. Windows is told what's still
    /// held, and once nothing is, the footer drops its "Export to keep
    /// them." as an export's does.
    fn dragged_away(&mut self) {
        self.forget_dropped_notes();
        self.held_changed();
        if self.files.held_count() == 0 {
            let plain = status_line(self.files.len(), 0, 0, 0);
            if self.status != plain {
                self.say(plain);
            }
        }
    }

    /// A row whose results a drag moved out says so: a note from before,
    /// such as an export that failed, no longer stands in for its line.
    fn forget_dropped_notes(&mut self) {
        for e in self.files.entries() {
            if matches!(
                e.job,
                JobState::Done {
                    saved: Some(model::Saved::Dropped),
                    ..
                }
            ) {
                self.notes.remove(&e.path);
            }
        }
    }

    /// Whether a press on row `i` may become a drag out: the row holds
    /// results that are all there and no export is moving, Kiln has a
    /// holding folder, and nothing is over the list.
    fn may_drag(&self, i: usize) -> bool {
        self.holding.is_some()
            && self.over().list_live()
            && !system::modal_open()
            && !self.row_saving(i)
            && self
                .files
                .entries()
                .get(i)
                .is_some_and(|e| e.held().is_some() && !e.is_gone(|p| p.exists()))
    }

    /// Whether the pointer, at `(x, y)` with `keys` (MK_ flags), has gone
    /// past the system drag distance from a press that may drag.
    fn drag_due(&self, x: f32, y: f32, keys: u32) -> bool {
        let Some(p) = self.list_press.filter(|p| p.drag) else {
            return false;
        };
        if keys & MK_LBUTTON_FLAG == 0 {
            return false;
        }
        // The drag box in pixels at the window's DPI, as the pointer's
        // client point is; then to layout units, as `dips` measures it.
        let dpi = self.dpi as u32;
        let (cx, cy) = unsafe {
            (
                GetSystemMetricsForDpi(SM_CXDRAG, dpi),
                GetSystemMetricsForDpi(SM_CYDRAG, dpi),
            )
        };
        let k = 1.0 / layout::px_per_unit(self.dpi, self.zoom);
        model::past_drag(
            x - p.point.0,
            y - p.point.1,
            (cx / 2) as f32 * k,
            (cy / 2) as f32 * k,
        )
    }

    /// Drag the selection's held results out of Kiln. When the drag
    /// returns, a file that's gone was moved by the drop; one still there
    /// was copied, or the drag was cancelled, and stays held. Returns
    /// whether a drag ran: with nothing to carry, the press stays a click.
    fn drag_out(&mut self) -> bool {
        let saving = self.saving_files();
        let files = self.files.drag_files(|p| saving.contains(p) || !p.exists());
        if files.is_empty() {
            if let Some(p) = &mut self.list_press {
                p.drag = false;
            }
            return false;
        }
        self.list_press = None;
        unsafe {
            let _ = ReleaseCapture();
        }
        // Remembered first: the drop can move a file while the drag still
        // runs, and a look for gone results meanwhile must see it as
        // dragged out.
        self.dragged.extend(files.iter().cloned());
        // The drag's loop delivers the window's messages, as a dialog's
        // does: work that would start meanwhile waits.
        self.own_drag.set(true);
        system::window_loop(true);
        let result = drag_out::drag_files(self.hwnd, &files);
        system::window_loop(false);
        self.own_drag.set(false);
        if self.destroyed {
            return true;
        }
        // Only whether each file is still there says it moved: Explorer's
        // own move reports no effect. An export started during the drag
        // may be moving one: that's the export's to record.
        let exporting = self.saving_files();
        let present = |p: &Path| still_there(&exporting, p);
        let moved: Vec<PathBuf> = files.iter().filter(|p| !present(p)).cloned().collect();
        self.dragged.retain(|p| present(p));
        if !moved.is_empty() {
            self.files.dragged_out(&moved);
            self.dragged_away();
        }
        if let Err(e) = result {
            // A drag that failed changes nothing: what it would have
            // carried isn't remembered as dragged.
            for f in &files {
                self.dragged.remove(f);
            }
            log(&format!("couldn't start the drag: {e}"));
            // Unless the drop moved some first: those rows say so.
            if moved.is_empty() {
                self.alert(model::DRAG_FAILED.to_owned());
            }
        }
        self.invalidate();
        true
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
        let title = match files {
            [one] => format!(
                "Save {} to",
                one.file_name().unwrap_or_default().to_string_lossy()
            ),
            _ => format!("Save {n} files to"),
        };
        system::pick_folder_in(self.hwnd, &title, &start)
    }

    /// Whether `path` is inside Kiln's holding folders, which are no place
    /// to export to: this run's is emptied when Kiln closes, and the next
    /// launch sweeps the others.
    fn in_holding(&self, path: &Path) -> bool {
        let run = self.holding.as_ref().map(|h| h.dir());
        let root = settings::held_root();
        run.is_some_and(|dir| inside(path, dir))
            || root.as_deref().is_some_and(|dir| inside(path, dir))
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
            self.say(model::SAVING.to_owned());
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
        if self.notes.gone(&source) {
            return;
        }
        // One dialog for one result or many: the folder to save into.
        if let Some(dir) = self.pick_export_folder(&held) {
            let held = self.still_held(&source, &held);
            if !held.is_empty() {
                self.export(held, Dest::Into(dir), false);
            }
        }
    }

    /// Export one held result of row `i` under a name of its own, with the
    /// Save dialog: the menu's "Export as…".
    fn export_as_row(&mut self, i: usize) {
        if self.leaving.is_some() || self.row_saving(i) {
            return;
        }
        let Some(e) = self.files.entries().get(i) else {
            return;
        };
        let source = e.path.clone();
        if let Some([one]) = e.held().map(<[PathBuf]>::to_vec).as_deref() {
            self.save_one(&source, one);
        }
    }

    /// Export one held result, `file` of the row for `source`, with the
    /// Save dialog.
    fn save_one(&mut self, source: &Path, file: &Path) {
        let start = self.export_start(source);
        let name = file.file_name().unwrap_or_default().to_string_lossy();
        let ext = file.extension().unwrap_or_default().to_string_lossy();
        let Some(dest) = system::save_as(self.hwnd, "Export", &start, &name, &ext) else {
            return;
        };
        let held = self.still_held(source, &[file.to_path_buf()]);
        if !held.is_empty() {
            self.export(held, Dest::As(dest), false);
        }
    }

    /// Export the held results of rows `rows`: one held row goes the way
    /// its own Export link does, and several go together into one folder.
    fn export_rows(&mut self, rows: &[usize]) {
        let held: Vec<usize> = rows
            .iter()
            .copied()
            .filter(|&i| {
                self.files
                    .entries()
                    .get(i)
                    .is_some_and(|e| e.held().is_some())
            })
            .collect();
        match held.as_slice() {
            [] => {}
            [one] => self.export_row(*one),
            _ => {
                let sources = self.paths_of(&held);
                self.export_together(&sources);
            }
        }
    }

    /// The held files of the rows for `sources`, once each, less those a
    /// row says are gone and those an export is already moving.
    fn held_of(&self, sources: &[PathBuf]) -> Vec<PathBuf> {
        let mask = self.mask(&Rows::Files(sources.to_vec()));
        let saving = self.saving_files();
        let entries = self.files.entries();
        self.files
            .held_files(|i| {
                mask.get(i).copied().unwrap_or(false) && !self.notes.gone(&entries[i].path)
            })
            .into_iter()
            .filter(|f| !saving.contains(f))
            .collect()
    }

    /// The held results of several rows, into one folder, summed up in the
    /// footer.
    fn export_together(&mut self, sources: &[PathBuf]) {
        if self.leaving.is_some() {
            self.say(model::SAVING.to_owned());
            self.invalidate();
            return;
        }
        self.check_gone();
        let held = self.held_of(sources);
        if held.is_empty() {
            return;
        }
        let Some(dir) = self.pick_export_folder(&held) else {
            return;
        };
        // The list can change while the picker is open.
        let held = self.held_of(sources);
        if !held.is_empty() {
            self.export(held, Dest::Into(dir), true);
        }
    }

    /// Every held result, into one folder.
    fn export_everything(&mut self) {
        if self.leaving.is_some() {
            self.say(model::SAVING.to_owned());
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
        // Only results this run holds: anything else is the user's own
        // file, which an export must never move.
        let files: Vec<PathBuf> = match &self.holding {
            Some(h) => files
                .into_iter()
                .filter(|f| f.parent() == Some(h.dir()))
                .collect(),
            None => vec![],
        };
        // Nothing to move: no thread, and no message to wait for.
        if files.is_empty() {
            return None;
        }
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
            let results = export_each(files, |f| match &dest {
                Dest::Into(dir) => kiln_core::export_into(f, dir).ok(),
                Dest::As(path) => kiln_core::export_as(f, path).ok(),
            });
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
        let waited = self
            .leaving
            .as_mut()
            .is_some_and(|l| l.waits.remove(&ex.id));
        if waited && !saved {
            // Something wasn't saved: the leave is off, and the footer and
            // the row say what.
            self.leaving = None;
        }
        // A leave or close this lets go ahead does so in `run_due`, once no
        // dialog is open.
    }

    /// Go ahead with a leave whose exports are done, or close as asked
    /// once everything is saved, unless a dialog is open: then it waits
    /// for the dialog to close. Runs after every message.
    fn run_due(&mut self) {
        if self.destroyed {
            return;
        }
        self.ask_autosave();
        // A resumed leave may make a close due too, as one step did before.
        for _ in 0..2 {
            let ready = self.leaving.as_ref().is_some_and(|l| l.waits.is_empty());
            let due = model::leave_due(
                system::modal_open(),
                ready,
                self.close_when_saved,
                !self.exports.is_empty(),
            );
            match due {
                Due::Nothing => return,
                Due::Resume => {
                    if let Some(l) = self.leaving.take() {
                        self.resume_leave(l);
                    }
                }
                Due::Close => {
                    // Close as asked; anything still at stake asks first.
                    self.close_when_saved = false;
                    unsafe {
                        let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                    }
                    return;
                }
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
        // A result keeps its size where it went.
        for (from, to) in &moved {
            if let Some(size) = self.result_sizes.remove(from) {
                self.result_sizes.insert(to.clone(), size);
            }
        }
        self.files.exported(&moved, folder);
        let failed_files: Vec<PathBuf> = failed.iter().map(|f| (*f).clone()).collect();
        self.files.export_failed(&failed_files);
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
        self.say(if all {
            model::export_summary(moved.len(), results.len())
        } else {
            status_line(self.files.len(), 0, 0, 0)
        });
        if all && !failed.is_empty() {
            self.alert = Some(self.status.clone());
        }
        self.held_changed();
        self.invalidate();
        failed.is_empty()
    }

    /// Open Explorer on a row's saved result, or else on the file itself.
    fn show_row(&mut self, i: usize) {
        if self
            .files
            .entries()
            .get(i)
            .is_some_and(|e| e.saved().is_some())
        {
            return self.show_saved(i);
        }
        let Some(path) = self.files.entries().get(i).map(|e| e.path.clone()) else {
            return;
        };
        let folder = path.parent().map(Path::to_path_buf).unwrap_or_default();
        if system::show_in_folder(&folder, std::slice::from_ref(&path)).is_err() {
            self.alert(format!("Couldn't open {}.", folder.display()));
            self.invalidate();
        }
    }

    /// Open Explorer on an exported row's results.
    fn show_saved(&mut self, i: usize) {
        let Some((folder, files)) = self.files.entries().get(i).and_then(|e| e.saved()) else {
            return;
        };
        if system::show_in_folder(folder, files).is_err() {
            self.alert(format!("Couldn't open {}.", folder.display()));
            self.invalidate();
        }
    }

    /// Double-click on a held row: open its result in its usual app, so it
    /// can be checked before it's kept. False if `(x, y)` isn't on one.
    fn open_held_at(&mut self, x: f32, y: f32) -> bool {
        if !self.over().list_live() {
            return false;
        }
        let (Some(i), false, false, false) = self.hit_row(x, y) else {
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
            self.alert(format!("Couldn't open {name}."));
            self.check_gone();
            self.invalidate();
        }
        true
    }

    /// Follow a row's link: a failed row's details, a held row's export, or
    /// an exported row's folder.
    fn follow_link(&mut self, i: usize, keyboard: bool) {
        if self.row_saving(i) {
            return;
        }
        let Some(e) = self.files.entries().get(i) else {
            return;
        };
        if let Some(backend) = e.needs_helper().map(str::to_owned) {
            // Nothing can be done with this file until its helper app is
            // installed: the link goes to where to get it.
            self.get_helper(&backend);
        } else if matches!(e.job, JobState::Idle) {
            // A file nothing has been done to: its link asks what to do.
            let paths = vec![e.path.clone()];
            let at = self.row_anchor(i).unwrap_or_else(|| self.row_centre(i));
            self.open_choice(WheelKind::Convert, paths, at, keyboard);
        } else if e.held().is_some() {
            self.export_row(i);
        } else if e.saved().is_some() {
            self.show_saved(i);
        } else {
            self.show_details(i);
        }
    }

    /// Open the download page of the helper app behind `backend`
    /// ("ffmpeg") in the browser.
    fn get_helper(&mut self, backend: &str) {
        let engine = match backend {
            "ffmpeg" => Engine::Ffmpeg,
            "pandoc" => Engine::Pandoc,
            "libreoffice" => Engine::LibreOffice,
            "calibre" => Engine::Calibre,
            _ => return,
        };
        if !system::open_download_page(self.hwnd, engine) {
            self.alert(format!("Couldn't open {}'s download page.", engine.label()));
            self.invalidate();
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

    /// Scroll the focus row into view.
    fn scroll_to_selection(&mut self) {
        if let Some(i) = self.files.focus() {
            self.scroll_to_row(i);
        }
    }

    fn scroll_to_row(&mut self, i: usize) {
        let l = self.layout();
        self.scroll = layout::scroll_to_show(i, self.scroll, l.panel.h);
        self.clamp_scroll();
    }

    // The wheel.

    fn open_wheel(
        &mut self,
        kind: WheelKind,
        paths: Vec<PathBuf>,
        at: (f32, f32),
        keyboard: bool,
        pour: Pour,
    ) {
        let (w, h) = self.renderer.size_dips();
        let (items, more) = wheel::split_common(wheel::items_for(
            kind,
            &self.kiln,
            self.availability(),
            &paths,
        ));
        let mut wheel = Wheel {
            kind,
            items,
            more,
            center: (0.0, 0.0),
            scale: 1.0,
            source_code: wheel::source_code(&paths),
        };
        // Placed so the outer ring fits too, should More be opened. A wheel
        // asked for from a key or beside a row keeps to the list beside the
        // inspector, so it never covers it; one under a dragging pointer
        // opens at the pointer. `pour` says which, not where the pointer is.
        let l = self.layout();
        let list = l.inspector.map(|_| (l.list.x, l.list.right()));
        (wheel.center, wheel.scale) =
            wheel::place_pour(pour, at.0, at.1, w, h, wheel.extent(), list);
        let suggestion = self.wheel_suggestion(kind, &paths);
        // From the keyboard it starts on the suggestion, or on More when
        // the suggestion is in it.
        let start = wheel::start_spot(&wheel, suggestion.as_ref().map(|s| &s.action));
        let hover = (keyboard && wheel.segments() > 0).then_some(start);
        self.wheel = Some(WheelUi {
            wheel,
            paths,
            hover,
            more: Tween::new(0.0, Instant::now()),
            opened: Instant::now(),
            closing: None,
            pulse: None,
            keyboard,
            suggestion,
            dot_at: None,
            options_hover: false,
        });
        self.animate();
    }

    /// Open a wheel for every selected file, from the keyboard, as a drop
    /// of those files would.
    fn open_wheel_for_selection(&mut self, kind: WheelKind) {
        let paths = self.selected_paths();
        if paths.is_empty() {
            // Nothing to act on is said, not ignored.
            if kind == WheelKind::Convert {
                self.alert("Select a file to convert.".to_owned());
                self.invalidate();
            }
            return;
        }
        // The wheel pours beside the file the keyboard is on, so its name
        // stays readable; with no such row, from the middle of the list,
        // not of the panel the inspector shares.
        let at = self
            .files
            .focus()
            .or_else(|| self.files.selection().first().copied())
            .and_then(|i| self.row_anchor(i))
            .unwrap_or_else(|| {
                let l = self.layout();
                (l.list.x + l.list.w / 2.0, l.list.y + l.list.h / 2.0)
            });
        self.open_choice(kind, paths, at, true);
    }

    /// Where a choice for row `i` pours from: the right end of the row,
    /// where its link and cross are, so its name stays clear. `None` when
    /// the row is out of view.
    fn row_anchor(&self, i: usize) -> Option<(f32, f32)> {
        let r = self.row_rect(i)?;
        let hit = layout::row_link(&r);
        Some((hit.x + hit.w / 2.0, r.y + r.h / 2.0))
    }

    /// Open `kind`'s wheel for `paths` as the Convert button, a row's
    /// link, a key or the menu ask for it. A wheel with nothing in it
    /// never opens: the footer says why, and what to do. Converting a row
    /// that is converting already isn't offered either.
    fn open_choice(
        &mut self,
        kind: WheelKind,
        paths: Vec<PathBuf>,
        at: (f32, f32),
        keyboard: bool,
    ) {
        let busy = match kind {
            WheelKind::Convert => self.files.convert_busy(&paths),
            _ => None,
        };
        let said = if let Some(why) = busy {
            Some(why.to_owned())
        } else {
            let offer = wheel::offer(kind, &self.kiln, self.availability(), &paths);
            wheel::offer_sentence(kind, &offer)
        };
        match said {
            Some(text) => {
                self.alert(text);
                self.invalidate();
            }
            // Every way in here is a key or a row's own: the wheel keeps to
            // the list.
            None => self.open_wheel(kind, paths, at, keyboard, Pour::OverList),
        }
    }

    /// The footer's Convert: the format choice for the selection, poured
    /// from the button, or the reason it can't be used.
    fn convert_selection(&mut self, keyboard: bool) {
        if self.footer_cancel() {
            for i in self.selected_running() {
                self.cancel_row(i);
            }
            return;
        }
        if let Some(why) = self.convert_block() {
            self.alert(why.to_owned());
            self.invalidate();
            return;
        }
        let Some(b) = self.convert_rect() else {
            return;
        };
        let paths = self.selected_paths();
        self.open_choice(WheelKind::Convert, paths, (b.x + b.w / 2.0, b.y), keyboard);
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
        let k = layout::px_per_unit(self.dpi, self.zoom);
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
    /// which opens it at the focus row. The selection gets the row menu,
    /// whose items apply to every selected row; an empty part of the list
    /// gets Add files and Paste.
    fn context_menu(&mut self, at: Option<(i32, i32)>) {
        let (rows, focus) = match at {
            Some((x, y)) => {
                let (x, y) = self.screen_dips(x, y);
                // The list's menu, not the inspector's beside it.
                if !self.layout().list.contains(x, y) {
                    return;
                }
                // Right-clicking a row outside the selection selects only
                // it first, and one inside keeps the selection. An empty
                // part of the list clears it, as File Explorer does.
                self.files.pointer_used();
                match self.hit_row(x, y).0 {
                    Some(i) => (self.files.context_target(i), Some(i)),
                    None => {
                        self.files.clear_selection();
                        (vec![], None)
                    }
                }
            }
            None => {
                // From the keyboard: the focus ring shows, as for any key.
                self.files.keyboard_used();
                let rows = self.files.selection();
                let focus = self.files.selected().or(rows.first().copied());
                (rows, focus)
            }
        };
        // The menu acts on rows, so a Tab-focused button lets the focus go
        // back to the list.
        self.button_focus = None;
        let items = if rows.is_empty() {
            ways_in::list_menu(system::clipboard_has_files())
        } else {
            let paths = self.paths_of(&rows);
            let wheels: Vec<WheelKind> = WheelKind::ALL
                .into_iter()
                .filter(|&k| {
                    !wheel::items_for(k, &self.kiln, self.availability(), &paths).is_empty()
                })
                .collect();
            let states: Vec<RowState> = rows.iter().map(|&i| self.row_state(i)).collect();
            ways_in::menu_items(&states, &wheels)
        };
        let texts: Vec<Option<String>> = items
            .iter()
            .map(|&item| (item != MenuItem::Separator).then(|| item.text(&self.settings.keys)))
            .collect();
        let (x, y) = match (at, focus) {
            (Some(p), _) => p,
            // From the keyboard: at the row's bottom left, scrolled into
            // view, or the list's top left with nothing selected.
            (None, Some(i)) => {
                self.scroll_to_row(i);
                let r = self.layout().rows[i];
                self.to_screen(r.x, r.bottom() - self.scroll)
            }
            (None, None) => {
                let panel = self.layout().panel;
                self.to_screen(panel.x + 16.0, panel.y + 16.0)
            }
        };
        self.invalidate();
        // The rows by file: the list can change while the menu is up.
        let files = self.paths_of(&rows);
        let focus_file = focus.and_then(|i| self.files.entries().get(i).map(|e| e.path.clone()));
        let Some(k) = system::popup_menu(self.hwnd, &texts, x, y) else {
            return;
        };
        let rows: Vec<usize> = files
            .iter()
            .filter_map(|f| self.files.index_of(f))
            .collect();
        if !files.is_empty() && rows.is_empty() {
            return;
        }
        let focus = focus_file
            .and_then(|f| self.files.index_of(&f))
            .or(rows.first().copied());
        self.menu_chosen(items[k], &rows, focus, at.is_none());
    }

    /// Do what the menu item `item` says, for the rows `rows`. A wheel
    /// pours from row `focus`.
    fn menu_chosen(
        &mut self,
        item: MenuItem,
        rows: &[usize],
        focus: Option<usize>,
        keyboard: bool,
    ) {
        match (item, rows) {
            (MenuItem::AddFiles, _) => self.add_files(keyboard),
            (MenuItem::Paste, _) => self.paste(keyboard),
            (_, []) => {}
            (MenuItem::Wheel(kind), _) => {
                let paths = self.paths_of(rows);
                let at = self.row_centre(focus.unwrap_or(rows[0]));
                self.open_choice(kind, paths, at, keyboard);
            }
            (MenuItem::Export, _) => self.export_rows(rows),
            (MenuItem::ExportAs, [i]) => self.export_as_row(*i),
            (MenuItem::Open, [i]) => {
                self.open_held(*i);
            }
            (MenuItem::ShowInFolder, [i]) => self.show_row(*i),
            (MenuItem::CopyPath, _) => {
                let paths = self.paths_of(rows);
                self.file_action(FileAction::CopyPath, paths);
            }
            (MenuItem::Retry, [i]) => self.retry_row(*i),
            (MenuItem::Stop, _) => {
                for &i in rows {
                    self.cancel_row(i);
                }
            }
            (MenuItem::Remove, _) => {
                let paths = self.paths_of(rows);
                self.leave(
                    Leave::Remove,
                    Rows::Files(paths.clone()),
                    Then::Remove(paths),
                );
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
                w.more
                    .set(1.0, OPEN_MS, Instant::now(), theme::motion::animations());
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

    /// Point the open wheel at whatever is under `(x, y)`. Outside a drag,
    /// the centre leaves lit what was lit, so the pointer can reach its
    /// "Options… (Tab)". A drop on the centre still chooses nothing.
    fn wheel_point(&mut self, x: f32, y: f32) {
        let dragging = !self.drag_paths.is_empty();
        if let Some(w) = self.wheel.as_mut().filter(|w| w.is_open()) {
            if !dragging && wheel::on_centre(&w.wheel, x, y) {
                let hot = wheel::on_options(&w.wheel, x, y);
                if w.options_hover != hot {
                    w.options_hover = hot;
                    self.invalidate();
                }
                return;
            }
            let was = std::mem::take(&mut w.options_hover);
            let spot = wheel::hit(&w.wheel, x, y, w.more_open());
            if w.point(spot) || was {
                self.animate();
            }
        }
    }

    /// Whether the open wheel's centre shows "Options… (Tab)": the lit
    /// target has options, and no drag is under way. `Some(true)` while
    /// the pointer is on it.
    fn wheel_options(&self) -> Option<bool> {
        let w = self.wheel.as_ref().filter(|w| w.is_open())?;
        if !self.drag_paths.is_empty() {
            return None;
        }
        self.sheet_target(w.hover).map(|_| w.options_hover)
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
    /// nothing else is lit, or what the lit format is.
    fn wheel_hint(&self) -> Option<String> {
        let w = self.wheel.as_ref().filter(|w| w.is_open())?;
        let marks = w.marks();
        // The suggestion's line, while it shows. The label is the item as
        // the wheel shows it: the last mark (its outer slot when it is in
        // More).
        let suggestion = w
            .suggestion
            .as_ref()
            .filter(|_| wheel::shows_suggestion(w.hover, &marks))
            .and_then(|s| {
                let item = marks.last().and_then(|&m| w.wheel.item(m))?;
                Some(wheel::suggestion_footer(&item.label, &s.reason))
            });
        // A lit convert target says what it is, ahead of the suggestion if
        // it is the suggested one.
        wheel::wheel_footer(w.hover.and_then(|spot| w.wheel.item(spot)), suggestion)
    }

    /// Advance the animations: start a pulsed job, drop a faded wheel or
    /// sheet, and what the inspector showed once it has faded out.
    fn tick(&mut self) {
        let (now, animations) = (Instant::now(), theme::motion::animations());
        if self.inspector_prev.is_some() && self.inspector_fading(now).is_none() {
            self.inspector_prev = None;
        }
        if self.sheet.as_ref().is_some_and(|s| {
            s.closing
                .is_some_and(|t| !anim::within(t, theme::motion::QUICK_MS, now, animations))
        }) {
            self.sheet = None;
        }
        let Some(w) = &mut self.wheel else {
            return;
        };
        if let Some((spot, t)) = w.pulse {
            if !anim::within(t, PULSE_MS, now, animations) {
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
        if w.closing
            .is_some_and(|t| !anim::within(t, theme::motion::QUICK_MS, now, animations))
        {
            self.wheel = None;
        }
    }

    fn wheel_view<'a>(w: Option<&'a WheelUi>, keys: &settings::KeyMap) -> Option<WheelView<'a>> {
        let (now, animations) = (Instant::now(), theme::motion::animations());
        let w = w?;
        Some(WheelView {
            wheel: &w.wheel,
            title: ways_in::wheel_title(w.wheel.kind, keys),
            hover: w.hover,
            more: w.more.value(now).clamp(0.0, 1.0),
            open_ms: anim::elapsed_ms(w.opened, now),
            animations,
            fade: w.closing.map_or(1.0, |t| {
                1.0 - anim::eased(t, theme::motion::QUICK_MS, now, animations)
            }),
            pulse: w.pulse.map(|(s, t)| {
                let ms = theme::motion::ms(PULSE_MS);
                let done = if ms == 0 {
                    1.0
                } else {
                    t.elapsed().as_secs_f32() * 1000.0 / ms as f32
                };
                (s, done)
            }),
            marks: w.marks(),
            dot: w
                .dot_at
                .map_or(1.0, |t| anim::eased(t, FACTS_MS, now, animations)),
            options: None,
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
                groups: groups.len(),
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
                    let entry = &mut self.files.entries_mut()[i];
                    entry.job = JobState::Running {
                        action: action.clone(),
                        fraction: None,
                        started: Some(Instant::now()),
                    };
                    entry.save_failed = false;
                    // What Retry runs again. A merge takes several files,
                    // so it can't be retried from one row.
                    entry.last =
                        (!matches!(action, Action::Tool(Tool::Merge))).then(|| action.clone());
                    self.motion.row_started(i);
                    if let Some(r) = self.motion.rows.get_mut(i) {
                        r.job = Some(id);
                    }
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
        self.say_working(self.working_status());
        self.sync_taskbar();
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
        // The inspector's formats follow the habits.
        self.panel_stale();
        let changed = favourites(&self.settings.habits) != before;
        // Picks can't change the engines, so the cached search stands.
        // Not over a settings.toml that couldn't be read: see Store.
        if let Err(e) = self.settings_store.save_picks(&self.settings) {
            self.alert(e);
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
                    if let Some(i) = self.job_row(path, id) {
                        if let JobState::Running { fraction, .. } =
                            &mut self.files.entries_mut()[i].job
                        {
                            *fraction = f;
                        }
                    }
                }
                self.sync_taskbar();
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
                                .map(|p| model::Saved::Folder(p.to_path_buf())),
                        },
                    },
                    Err(e) if e.cancelled => JobState::Cancelled,
                    Err(e) => JobState::Failed {
                        // One plain sentence in the row; the engine's own
                        // words are in the details.
                        message: self.no_room(e).unwrap_or_else(|| e.message.clone()),
                        details: e.details.clone(),
                        engine: e.engine,
                    },
                };
                for path in &job.paths {
                    if let Some(i) = self.job_row(path, id) {
                        self.files.entries_mut()[i].job = state.clone();
                        self.row_ended(i, now);
                        if let Some(r) = self.motion.rows.get_mut(i) {
                            r.job = None;
                        }
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
                        let failed = b.failed > 0;
                        let line = model::with_export_hint(summary, b.held);
                        self.say(line);
                        if failed {
                            self.alert = Some(self.status.clone());
                        }
                        self.batches.remove(&job.batch);
                    } else if !self.alerting() {
                        // Each finish is said once, with how far the batch is.
                        let name = job
                            .paths
                            .first()
                            .and_then(|p| p.file_name())
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        let cancelled = matches!(&result, Err(e) if e.cancelled);
                        let said = model::job_outcome(&name, result.is_ok(), cancelled);
                        self.say_working(format!("{said} {}", self.working_status()));
                    }
                }
                if let Ok(outputs) = &result {
                    self.result_ready(&job, outputs);
                }
                self.tally_finish(&job, &result);
                self.sync_taskbar();
                self.held_changed();
                self.animate();
            }
        }
        self.invalidate();
    }

    /// A result is ready and held. With Save next to the original on, it
    /// goes beside its file at once, never over anything there; until the
    /// person has been asked, once, it waits for the question.
    fn result_ready(&mut self, job: &JobInfo, outputs: &[PathBuf]) {
        if matches!(job.action, Action::File(_)) {
            return;
        }
        match model::autosave_for(
            self.holding.is_some(),
            self.settings.save_asked,
            self.settings.save_next_to_original,
        ) {
            model::AutoSave::Hold => {}
            model::AutoSave::Ask => self
                .autosave_waiting
                .push((job.paths.clone(), outputs.to_vec())),
            model::AutoSave::Beside => self.save_beside(&job.paths, outputs),
        }
    }

    /// Save held `outputs` into the folder of the first of `sources`, as an
    /// export does: under a free name, held still if it fails.
    fn save_beside(&mut self, sources: &[PathBuf], outputs: &[PathBuf]) {
        let Some(dir) = sources
            .first()
            .and_then(|p| p.parent())
            .map(Path::to_path_buf)
        else {
            return;
        };
        // Only what a row still holds: the list can change meanwhile.
        let held = self.held_of(sources);
        let files: Vec<PathBuf> = outputs
            .iter()
            .filter(|f| held.contains(f))
            .cloned()
            .collect();
        if !files.is_empty() {
            self.export(files, Dest::Into(dir), false);
        }
    }

    /// Ask, once, whether results should be saved next to their originals
    /// from now on, when the first one is ready and nothing else is open.
    /// Closing the question without an answer asks again at the next one.
    fn ask_autosave(&mut self) {
        if self.autosave_waiting.is_empty() || self.settings.save_asked || system::modal_open() {
            return;
        }
        let on = match system::ask(self.hwnd, &model::autosave_prompt()) {
            PromptButton::SaveNextTo => true,
            PromptButton::KeepHeld => false,
            _ => {
                self.autosave_waiting.clear();
                return;
            }
        };
        self.settings.save_next_to_original = on;
        self.settings.save_asked = true;
        self.save_settings();
        for (sources, outputs) in std::mem::take(&mut self.autosave_waiting) {
            if on {
                self.save_beside(&sources, &outputs);
            }
        }
        self.invalidate();
    }

    /// Show how far the work has got on the taskbar button.
    fn sync_taskbar(&mut self) {
        let total: usize = self.batches.values().map(|b| b.groups).sum();
        let done: usize = self.batches.values().map(|b| b.groups - b.pending).sum();
        let running: Vec<Option<f32>> = self
            .files
            .entries()
            .iter()
            .filter_map(|e| match &e.job {
                JobState::Running { fraction, .. } => Some(*fraction),
                _ => None,
            })
            .collect();
        let now = model::taskbar_progress(total, done, &running);
        let key = match now {
            model::TaskbarProgress::None => (0, 0),
            model::TaskbarProgress::Indeterminate => (1, 0),
            model::TaskbarProgress::Fraction(f) => (2, (f * 1000.0) as u32),
        };
        if self.taskbar_shown == Some(key) {
            return;
        }
        self.taskbar_shown = Some(key);
        let hwnd = self.hwnd;
        self.taskbar
            .get_or_insert_with(system::Taskbar::new)
            .set(hwnd, now);
    }

    /// Count a finished job towards the finish notice, and when the last
    /// one is in, tell the person if Kiln isn't in front: the taskbar
    /// button flashes (unless Windows has animations off) and a notice
    /// names the result.
    fn tally_finish(
        &mut self,
        job: &JobInfo,
        result: &std::result::Result<Vec<PathBuf>, JobError>,
    ) {
        match result {
            Ok(outputs) => {
                self.tally.0 += 1;
                if self.tally.3.is_none() {
                    self.tally.3 = outputs
                        .first()
                        .and_then(|p| p.file_name())
                        .map(|n| n.to_string_lossy().into_owned());
                    self.notice_row = job.paths.first().cloned();
                }
            }
            Err(e) if e.cancelled => self.tally.2 += 1,
            Err(_) => {
                self.tally.1 += 1;
                if self.notice_row.is_none() {
                    self.notice_row = job.paths.first().cloned();
                }
            }
        }
        if !self.jobs.is_empty() {
            return;
        }
        let (ok, failed, cancelled, first) = std::mem::take(&mut self.tally);
        if system::in_front(self.hwnd) {
            self.notice_row = None;
            return;
        }
        if let Some((title, text)) = model::finish_notice(ok, failed, cancelled, first.as_deref()) {
            if theme::motion::animations() {
                system::flash_until_front(self.hwnd);
            }
            system::notify(self.hwnd, WM_KILN_NOTIFY, &title, &text);
        }
    }

    /// What the footer says while jobs run: how many of them are done.
    /// How to stop the selected one is added as the footer is drawn, from
    /// the selection then (`ways_in::with_cancel_hint`).
    fn working_status(&self) -> String {
        let total: usize = self.batches.values().map(|b| b.groups).sum();
        let pending: usize = self.batches.values().map(|b| b.pending).sum();
        model::batch_progress(total - pending, total)
    }

    /// Say `line` about the work under way, and remember it as that line,
    /// so the footer adds how to cancel the selected file while it still
    /// says it.
    fn say_working(&mut self, line: String) {
        self.say(line);
        self.working = Some(self.status.clone());
    }

    /// The row for `path`, if it's waiting on job `id`: a row removed and
    /// added again, or given a new job, isn't the old job's any more.
    fn job_row(&self, path: &Path, id: JobId) -> Option<usize> {
        self.files
            .index_of(path)
            .filter(|&i| self.motion.rows.get(i).is_some_and(|r| r.job == Some(id)))
    }

    /// Row `i`'s job has just ended, as its entry now says: a result cools
    /// in its glaze, a failure cracks, and a cancelled job just stops.
    fn row_ended(&mut self, i: usize, now: Instant) {
        match self.files.entries().get(i).map(|e| &e.job) {
            Some(JobState::Done { .. }) => self.motion.row_finished(i, now),
            Some(JobState::Failed { .. }) => self.motion.row_failed(i, now),
            _ => self.motion.row_started(i),
        }
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
                match system::set_clipboard(self.hwnd, &text) {
                    Ok(()) => self.say(actions::copied_summary(paths.len())),
                    Err(e) => self.alert(format!("Couldn't copy the path: {}", e.message())),
                }
            }
            FileAction::OpenTerminal => {
                let Some(folder) = actions::terminal_folder(&paths) else {
                    return;
                };
                match actions::terminal_command(&folder).spawn() {
                    Ok(_) => {
                        self.status =
                            format!("Opened Terminal in {}.", actions::folder_name(&folder));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => self.alert(
                        "Windows Terminal isn't installed. Get it from the Microsoft Store."
                            .to_owned(),
                    ),
                    Err(e) => self.alert(format!("Couldn't open Terminal: {e}")),
                }
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
            self.alert("Kiln can't move a file while a job is using it.".to_owned());
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
                    started: Some(Instant::now()),
                };
                self.motion.row_started(i);
                if let Some(r) = self.motion.rows.get_mut(i) {
                    r.job = None;
                }
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
                        engine: None,
                    }
                }
            };
            if let Some(i) = index {
                self.files.entries_mut()[i].job = state;
                self.row_ended(i, now);
            }
        }
        // A moved file is listed at its new place: its previews there are
        // asked for, and those of where it was are dropped.
        self.previews_rows_changed();
        self.say(actions::moved_summary(ok, failed, &m.dest));
        if failed > 0 {
            self.alert = Some(self.status.clone());
        }
        self.held_changed();
        self.animate();
    }

    // Settings.

    /// Settings.toml was read before the window was made, so its first
    /// frame is in their theme. Once the window is on screen, say if it
    /// couldn't be read.
    fn settings_loaded(&mut self, err: Option<settings::SettingsError>) {
        if let Some(e) = err {
            self.alert(format!("{e}. Using the defaults."));
            log(&self.status);
        }
        self.apply_theme();
    }

    /// Fill the client area with the ground colour, before the first
    /// Direct2D frame is ready.
    fn fill_ground(&self) {
        use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
        unsafe {
            let dc = GetDC(Some(self.hwnd));
            if dc.is_invalid() {
                return;
            }
            let mut r = windows::Win32::Foundation::RECT::default();
            let _ = GetClientRect(self.hwnd, &mut r);
            let brush = CreateSolidBrush(COLORREF(self.palette.ground.colorref()));
            FillRect(dc, &r, brush);
            let _ = DeleteObject(brush.into());
            ReleaseDC(Some(self.hwnd), dc);
        }
    }

    /// Close the Settings panel. Back from it, the helper apps are looked
    /// for again, as one may have been installed or chosen meanwhile, and
    /// every row's formats follow.
    fn close_settings(&mut self) {
        self.settings_ui = None;
        self.offers_changed();
    }

    /// The helper apps found may have changed: look again, and work out
    /// every row's formats and suggestions afresh.
    fn offers_changed(&mut self) {
        self.engines.forget();
        let available = self.availability().clone();
        self.files.refresh_offers(&self.kiln, &available);
        self.habits_changed();
    }

    /// Save after an explicit change in Settings.
    fn save_settings(&mut self) {
        // The engines may be others now, and the inspector's words with
        // the keys.
        self.engines.forget();
        self.panel_stale();
        if let Err(e) = self.settings_store.save(&self.settings) {
            self.alert(e);
        }
    }

    /// A contrast theme's colours while one is on; otherwise Kiln's dark
    /// or light palette, as Settings and the app mode say.
    fn apply_theme(&mut self) {
        let p = palette_for(
            self.contrast,
            self.settings.theme.is_light(self.system_light),
        );
        if p != self.palette {
            self.palette = p;
            style_title_bar(self.hwnd, &p);
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
                engine_missing: self
                    .settings_ui
                    .as_ref()
                    .map_or([false; 4], |ui| ui.engine_missing),
            },
        )
    }

    /// The text beside each chooser, and which engines are missing,
    /// refreshed whenever settings change.
    fn settings_lines(&self) -> (ChooserLine, [ChooserLine; 4], [bool; 4]) {
        let output = match &self.settings.output_folder {
            Some(p) => (p.display().to_string(), false),
            None => ("Last export folder".to_owned(), true),
        };
        let path_var = std::env::var_os("PATH").unwrap_or_default();
        let status = Engine::ALL.map(|e| settings::engine_status(&self.settings, e, &path_var));
        let line = |st: &settings::EngineStatus| {
            let muted = !matches!(st, settings::EngineStatus::Chosen { .. });
            (st.describe(), muted)
        };
        let missing = status
            .each_ref()
            .map(|st| *st == settings::EngineStatus::Missing);
        (output, status.each_ref().map(line), missing)
    }

    /// Open help, unless a panel or dialog is up. An open wheel closes.
    fn open_help(&mut self) {
        if self.sheet.is_some() || self.settings_ui.is_some() || self.details.is_some() {
            return;
        }
        self.close_wheel();
        self.help = Some(HelpUi {
            sections: help::help_content(
                &self.settings.keys,
                help::Results::of(
                    self.holding.is_some(),
                    self.settings.save_asked,
                    self.settings.save_next_to_original,
                ),
            ),
            scroll: 0.0,
            hover: false,
            pressed: false,
            opened: Instant::now(),
        });
        // Nothing beneath stays lit or acts on a release under help.
        self.help_hover = false;
        self.settings_hover = false;
        self.hover_row = None;
        self.hover_cancel = false;
        self.hover_link = false;
        self.hover_retry = false;
        (self.clear_hover, self.export_hover, self.add_hover) = (false, false, false);
        (self.clear_pressed, self.export_pressed, self.add_pressed) = (false, false, false);
        (self.convert_hover, self.convert_pressed) = (false, false);
        self.animate();
    }

    /// Scroll help's text, kept inside the text.
    fn scroll_help(&mut self, by: TextScroll) {
        let (w, h) = self.renderer.size_dips();
        let body = layout::help(w, h).body;
        let Some(ui) = &self.help else {
            return;
        };
        let content = self.renderer.help_height(&ui.sections, body.w);
        let to = layout::scroll_text(by, ui.scroll, content, body.h);
        if let Some(ui) = self.help.as_mut().filter(|ui| ui.scroll != to) {
            ui.scroll = to;
            self.invalidate();
        }
    }

    /// Keys while help is open: Escape, Enter and Space close it, as its
    /// focused Close does. The arrows, Page Up, Page Down, Home and End
    /// scroll it. Every other key stays with it.
    fn help_key(&mut self, key: VIRTUAL_KEY) {
        let scroll = match key {
            VK_UP => Some(TextScroll::LineUp),
            VK_DOWN => Some(TextScroll::LineDown),
            VK_PRIOR => Some(TextScroll::PageUp),
            VK_NEXT => Some(TextScroll::PageDown),
            VK_HOME => Some(TextScroll::Home),
            VK_END => Some(TextScroll::End),
            _ => None,
        };
        if let Some(by) = scroll {
            return self.scroll_help(by);
        }
        if matches!(key, VK_ESCAPE | VK_RETURN | VK_SPACE) {
            self.help = None;
            self.invalidate();
        }
    }

    fn open_settings(&mut self) {
        self.close_wheel();
        let (output_line, engine_lines, engine_missing) = self.settings_lines();
        self.settings_ui = Some(SettingsUi {
            tab: SettingsTab::General,
            tab_marker: Tween::new(0.0, Instant::now()),
            switched: Instant::now() - Duration::from_secs(1),
            opened: Instant::now(),
            hover: None,
            // Focus starts on the first tab, so the keyboard has a place
            // to start and the ring shows where it is.
            focus: Some(layout::SETTINGS_FIRST_FOCUS),
            output_line,
            engine_lines,
            engine_missing,
            note: None,
        });
        self.settings_hover = false;
        self.animate();
    }

    /// The one network call, and only on request. Once a newer version
    /// is known the same button opens its page instead.
    fn check_updates(&mut self) {
        // The update check's own note takes the line back.
        if let Some(ui) = &mut self.settings_ui {
            ui.note = None;
        }
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
                        ui.tab_marker.set(
                            t.index() as f32,
                            OPEN_MS,
                            Instant::now(),
                            theme::motion::animations(),
                        );
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
                self.close_settings();
                self.invalidate();
                return;
            }
            Control::OutputChoose => {
                match system::pick_folder(hwnd, "Folder shown first when saving") {
                    Some(p) => self.settings.output_folder = Some(p),
                    None => return,
                }
            }
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
            Control::AutoSave(on) => {
                if self.settings.save_next_to_original == on && self.settings.save_asked {
                    return;
                }
                // Choosing either way answers the one-time question.
                self.settings.save_next_to_original = on;
                self.settings.save_asked = true;
                self.save_settings();
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
            // Only the engine's fixed official page. Kiln still never
            // downloads anything itself.
            Control::EngineGet(e) => {
                // Said in the panel's own note line, not the footer
                // behind it.
                let opened = system::open_download_page(hwnd, e);
                if let Some(ui) = &mut self.settings_ui {
                    ui.note =
                        (!opened).then(|| format!("Couldn't open {}'s download page.", e.label()));
                }
                self.invalidate();
                return;
            }
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
        let (output_line, engine_lines, engine_missing) = self.settings_lines();
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
            ui.engine_missing = engine_missing;
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
            VK_ESCAPE => self.close_settings(),
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
            // Tab goes round the controls with the page tabs as one stop;
            // the arrow keys step through every control.
            VK_TAB => {
                let controls: Vec<Control> = l.controls.iter().map(|(c, _)| *c).collect();
                ui.focus = layout::settings_tab_stop(&controls, &enabled, ui.tab, ui.focus, shift);
            }
            VK_DOWN | VK_RIGHT | VK_UP | VK_LEFT => {
                let back = key == VK_UP || key == VK_LEFT;
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

    /// Run again what row `i` was last asked to do, after it was
    /// cancelled or failed in a way trying again can fix.
    fn retry_row(&mut self, i: usize) {
        let Some(e) = self.files.entries().get(i).filter(|e| e.retryable()) else {
            return;
        };
        let (path, action) = (e.path.clone(), e.last.clone());
        if let Some(action) = action {
            self.start(action, vec![path], None);
        }
    }

    /// Open the details dialog on row `i`: the engine's output, or the
    /// whole sentence when the row cut it short.
    fn show_details(&mut self, i: usize) {
        let Some(e) = self.files.entries().get(i) else {
            return;
        };
        let cut = self
            .row_rect(i)
            .is_some_and(|r| self.renderer.line_cut(&r, e));
        if let Some((title, text)) = e.details(cut) {
            self.details = Some(DetailsUi {
                title,
                text: text.trim().to_owned(),
                scroll: 0.0,
                focus: DialogButton::Copy,
                hover: None,
                pressed: None,
                last_copy: None,
            });
            self.invalidate();
        }
    }

    /// Row `i`'s rectangle in the window, as scrolled.
    fn row_rect(&self, i: usize) -> Option<layout::Rect> {
        let mut r = *self.layout().rows.get(i)?;
        r.y -= self.scroll;
        Some(r)
    }

    /// Scroll the details dialog's text, kept inside the text.
    fn scroll_details(&mut self, by: TextScroll) {
        let (w, h) = self.renderer.size_dips();
        let body = layout::dialog(w, h).body;
        let Some(d) = &self.details else {
            return;
        };
        let content = self.renderer.details_height(&d.text, body.w);
        let to = layout::scroll_text(by, d.scroll, content, body.h);
        if let Some(d) = self.details.as_mut().filter(|d| d.scroll != to) {
            d.scroll = to;
            self.invalidate();
        }
    }

    /// Press a details dialog button: Copy puts all of the text on the
    /// clipboard and says "Copied", or "Couldn't copy", for a moment;
    /// Close closes.
    fn press_details(&mut self, b: DialogButton) {
        match b {
            DialogButton::Close => self.details = None,
            DialogButton::Copy => {
                let Some(text) = self
                    .details
                    .as_ref()
                    .map(|d| model::clipboard_lines(&d.text))
                else {
                    return;
                };
                // The button itself says how it went, in front of the
                // scrim, without Windows' own error text.
                let worked = system::set_clipboard(self.hwnd, &text).is_ok();
                if let Some(d) = &mut self.details {
                    d.last_copy = Some((worked, Instant::now()));
                }
                let ms = model::COPIED_FOR.as_millis() as u32 + 20;
                unsafe {
                    SetTimer(Some(self.hwnd), COPIED_TIMER, ms, None);
                }
            }
        }
        self.invalidate();
    }

    /// Keys in the details dialog. Escape and Enter close it. Tab and the
    /// left and right arrows move focus between Copy and Close, and Space
    /// presses the focused one. Ctrl+C copies. The other arrows, Page Up,
    /// Page Down, Home and End scroll the text.
    fn details_key(&mut self, key: VIRTUAL_KEY, ctrl: bool) {
        let scroll = match key {
            VK_UP => Some(TextScroll::LineUp),
            VK_DOWN => Some(TextScroll::LineDown),
            VK_PRIOR => Some(TextScroll::PageUp),
            VK_NEXT => Some(TextScroll::PageDown),
            VK_HOME => Some(TextScroll::Home),
            VK_END => Some(TextScroll::End),
            _ => None,
        };
        if let Some(by) = scroll {
            return self.scroll_details(by);
        }
        match key {
            VK_ESCAPE | VK_RETURN => self.details = None,
            VK_C if ctrl => return self.press_details(DialogButton::Copy),
            VK_SPACE => {
                if let Some(b) = self.details.as_ref().map(|d| d.focus) {
                    return self.press_details(b);
                }
            }
            VK_TAB | VK_LEFT | VK_RIGHT => {
                if let Some(d) = &mut self.details {
                    d.focus = match d.focus {
                        DialogButton::Copy => DialogButton::Close,
                        DialogButton::Close => DialogButton::Copy,
                    };
                }
            }
            _ => return,
        }
        self.invalidate();
    }

    /// Whether `(x, y)` is on a row's link, with nothing over the list.
    fn on_link(&self, x: f32, y: f32) -> bool {
        self.over().list_live() && {
            let hit = self.hit_row(x, y);
            hit.2 || hit.3
        }
    }

    /// What sits over the list now.
    fn over(&self) -> Over {
        Over::of(
            self.wheel.is_some(),
            self.sheet.is_some(),
            self.settings_ui.is_some(),
            self.details.is_some(),
            self.help.is_some(),
        )
    }

    // Input.

    /// The row under a point and whether the point is on its cancel cross,
    /// its link ("Show details", "Export", or an exported row's "Saved
    /// to" line) or its Retry.
    fn hit_row(&self, x: f32, y: f32) -> (Option<usize>, bool, bool, bool) {
        let l = self.layout();
        let Some(i) = layout::row_at(&l, x, y, self.scroll) else {
            return (None, false, false, false);
        };
        let mut r = l.rows[i];
        r.y -= self.scroll;
        let e = &self.files.entries()[i];
        // No row has a link, a Cancel or a "Saved to" line to click while
        // the inspector carries the actions, and a row being exported has
        // no link until it's done.
        let links = self.row_links();
        let running = links && matches!(e.job, JobState::Running { .. });
        let saving = self.row_saving(i);
        let link = links && !saving && e.link(self.renderer.line_cut(&r, e)).is_some();
        let saved = links && !saving && e.saved().is_some();
        let retry = !saving && e.retryable();
        let part = layout::row_part_with_retry(&r, x, y, running, link, saved, retry);
        (
            Some(i),
            part == layout::RowPart::Cancel,
            part == layout::RowPart::Link,
            part == layout::RowPart::Retry,
        )
    }

    /// The pointer moved to `(x, y)`, with the buttons and keys `keys`
    /// (the message's MK_ flags) says.
    fn mouse_move(&mut self, x: f32, y: f32, keys: u32) {
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
            let hot = layout::dialog(w, h).hit(x, y);
            if let Some(d) = self.details.as_mut().filter(|d| d.hover != hot) {
                d.hover = hot;
                self.invalidate();
            }
            return;
        }
        if self.help.is_some() {
            let (w, h) = self.renderer.size_dips();
            let hot = layout::help(w, h).close.contains(x, y);
            if let Some(ui) = self.help.as_mut().filter(|ui| ui.hover != hot) {
                ui.hover = hot;
                self.invalidate();
            }
            return;
        }
        if self.wheel.is_some() {
            self.wheel_point(x, y);
            return;
        }
        // With nothing to carry, the move goes on to the hover below.
        if self.drag_due(x, y, keys) && self.drag_out() {
            return;
        }
        let (row, cancel, link, retry) = self.hit_row(x, y);
        let l = self.layout();
        let clear = l.clear_button.is_some_and(|b| b.contains(x, y));
        let export = self.export_button().is_some_and(|b| b.contains(x, y));
        if export != self.export_hover {
            self.export_hover = export;
            self.invalidate();
        }
        let convert =
            self.convert_block().is_none() && self.convert_rect().is_some_and(|b| b.contains(x, y));
        if convert != self.convert_hover {
            self.convert_hover = convert;
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
        let hot = self.panel_control_at(x, y);
        if hot != self.panel_hover {
            self.panel_hover = hot;
            self.invalidate();
        }
        let ask = l.help_button.contains(x, y);
        if ask != self.help_hover {
            self.help_hover = ask;
            self.invalidate();
        }
        if (row, cancel, link, retry, clear)
            != (
                self.hover_row,
                self.hover_cancel,
                self.hover_link,
                self.hover_retry,
                self.clear_hover,
            )
        {
            self.hover_row = row;
            self.hover_cancel = cancel;
            self.hover_link = link;
            self.hover_retry = retry;
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
        let convert = std::mem::take(&mut self.convert_pressed);
        let setting = self.settings_pressed.take();
        let dialog = self.details.as_mut().and_then(|d| d.pressed.take());
        let sheet = self.sheet.as_mut().and_then(|s| s.pressed.take());
        let help_close = self
            .help
            .as_mut()
            .is_some_and(|ui| std::mem::take(&mut ui.pressed));
        let press = self.list_press.take();
        let panel = self.panel_pressed.take();
        unsafe {
            let _ = ReleaseCapture();
        }
        // A press that kept the selection and didn't become a drag: the
        // click, or the Ctrl-click, it held back.
        if let Some(p) = press {
            match p.release {
                Release::Nothing => {}
                Release::Click => self.files.click(p.row),
                Release::CtrlClick => self.files.ctrl_click(p.row),
            }
            self.invalidate();
        }
        if help_close {
            let (w, h) = self.renderer.size_dips();
            if layout::help(w, h).close.contains(x, y) {
                self.help = None;
            }
            self.invalidate();
            return;
        }
        if let Some(i) = sheet {
            if let (Some(l), true) = (self.sheet_layout(), self.sheet_hit(x, y) == Some(i)) {
                let (c, r) = l.controls[i];
                self.sheet_activate(c, step_delta(&r, x));
            }
            self.invalidate();
            return;
        }
        if !clear
            && !export
            && !add
            && !convert
            && setting.is_none()
            && dialog.is_none()
            && panel.is_none()
        {
            return;
        }
        // An inspector control acts if the pointer is still on it.
        if let Some(c) = panel {
            if self.panel_control_at(x, y) == Some(c) {
                self.panel_act(c, false);
            }
        }
        if convert && self.convert_rect().is_some_and(|b| b.contains(x, y)) {
            self.convert_selection(false);
        }
        if add && self.add_button().is_some_and(|b| b.contains(x, y)) {
            self.add_files(false);
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
        if let Some(b) = dialog {
            let (w, h) = self.renderer.size_dips();
            if layout::dialog(w, h).hit(x, y) == Some(b) {
                self.press_details(b);
            }
        }
        self.invalidate();
    }

    /// The left button went down at `(x, y)`, with Shift or Ctrl held as
    /// `keys` (the message's MK_ flags) says.
    fn mouse_down(&mut self, x: f32, y: f32, keys: u32) {
        let (shift, ctrl) = (keys & MK_SHIFT_FLAG != 0, keys & MK_CONTROL_FLAG != 0);
        // The pointer takes over from the keyboard: no button keeps a ring.
        self.button_focus = None;
        self.list_press = None;
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
                self.close_settings();
            }
            self.invalidate();
            return;
        }
        if self.details.is_some() {
            let (w, h) = self.renderer.size_dips();
            let hit = layout::dialog(w, h).hit(x, y);
            if let (Some(d), Some(b)) = (&mut self.details, hit) {
                d.pressed = Some(b);
                d.focus = b;
                unsafe { SetCapture(self.hwnd) };
            }
            self.invalidate();
            return;
        }
        if self.help.is_some() {
            let (w, h) = self.renderer.size_dips();
            let d = layout::help(w, h);
            if d.close.contains(x, y) {
                if let Some(ui) = &mut self.help {
                    ui.pressed = true;
                }
                unsafe { SetCapture(self.hwnd) };
            } else if !d.panel.contains(x, y) {
                // A click on the scrim closes help.
                self.help = None;
            }
            self.invalidate();
            return;
        }
        if let Some(w) = &self.wheel {
            // "Options… (Tab)" in the centre opens the lit target's sheet.
            let options = w.is_open() && wheel::on_options(&w.wheel, x, y);
            if options && self.wheel_options().is_some() {
                let hover = w.hover;
                self.open_sheet(hover);
                return;
            }
            if w.is_open() {
                match wheel::hit(&w.wheel, x, y, w.more_open()) {
                    Some(spot) => self.choose(spot),
                    // A group's heading on the outer ring is a label, not
                    // a way out: a stray click on it leaves the wheel open.
                    None if wheel::on_outer_band(&w.wheel, x, y, w.more_open()) => {}
                    None => self.close_wheel(),
                }
            }
            return;
        }
        let l = self.layout();
        if l.settings_button.contains(x, y) {
            self.open_settings();
        } else if l.help_button.contains(x, y) {
            self.open_help();
        } else if l.clear_button.is_some_and(|b| b.contains(x, y)) {
            self.clear_pressed = true;
            unsafe { SetCapture(self.hwnd) };
        } else if self.export_button().is_some_and(|b| b.contains(x, y)) {
            self.export_pressed = true;
            unsafe { SetCapture(self.hwnd) };
        } else if self.undo_link_rect().is_some_and(|b| b.contains(x, y)) {
            self.undo_remove();
        } else if self.convert_rect().is_some_and(|b| b.contains(x, y)) {
            // Disabled, a press says why and nothing else.
            match self.convert_block() {
                None => {
                    self.convert_pressed = true;
                    unsafe { SetCapture(self.hwnd) };
                }
                Some(why) => {
                    self.alert(why.to_owned());
                    self.invalidate();
                }
            }
        } else if self.add_button().is_some_and(|b| b.contains(x, y)) {
            self.add_pressed = true;
            unsafe { SetCapture(self.hwnd) };
        } else if self.on_inspector(x, y) {
            // A control in the inspector acts on release, as a button does.
            // The rest of it, and all of it while it folds away, is no part
            // of the list: the selection stays.
            if let Some(c) = self.panel_control_at(x, y) {
                self.panel_pressed = Some(c);
                unsafe { SetCapture(self.hwnd) };
            }
        } else {
            // A click in the list hides the focus ring until a key.
            self.files.pointer_used();
            let (row, cancel, link, retry) = self.hit_row(x, y);
            let release = match row {
                // As in Explorer, a Ctrl-press on a selected row keeps it
                // until release, so the selection can be dragged out.
                Some(i) if ctrl => Some(if self.files.ctrl_press(i) {
                    Release::CtrlClick
                } else {
                    Release::Nothing
                }),
                Some(i) if shift => {
                    self.files.shift_click(i);
                    Some(Release::Nothing)
                }
                Some(i) if cancel || link || retry => {
                    self.files.click(i);
                    if cancel {
                        self.cancel_row(i);
                    } else if retry {
                        self.retry_row(i);
                    } else {
                        self.follow_link(i, false);
                    }
                    None
                }
                // A press inside a selection keeps it, so the whole
                // selection can be dragged out.
                Some(i) => Some(if self.files.press(i) {
                    Release::Click
                } else {
                    Release::Nothing
                }),
                // A plain click off the rows clears the selection.
                None if !ctrl && !shift => {
                    self.files.clear_selection();
                    None
                }
                None => None,
            };
            // Held until release or a drag.
            if let (Some(i), Some(release)) = (row, release) {
                let drag = self.may_drag(i);
                self.list_press = Some(ListPress {
                    row: i,
                    point: (x, y),
                    release,
                    drag,
                });
                unsafe { SetCapture(self.hwnd) };
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
            self.details_key(key, ctrl);
            return true;
        }
        if self.help.is_some() {
            self.help_key(key);
            return true;
        }
        // A fading wheel no longer takes keys; a pulsing one is busy.
        if self.wheel.as_ref().is_some_and(|w| w.pulse.is_some()) {
            return true;
        }
        // F1 opens help from the list or a wheel.
        if key == VK_F1 {
            self.open_help();
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
        // Tab and Shift+Tab go round the list and the header and footer
        // buttons. On a button, Enter or Space presses it, and Escape or an
        // arrow key goes back to the list.
        if key == VK_TAB && !ctrl && !alt {
            // The inspector's controls that are drawn, in its reading order,
            // while it carries the actions.
            let inspector: Vec<InspectorControl> = self
                .live_panel()
                .map(|p| p.controls.iter().map(|(c, _)| *c).collect())
                .unwrap_or_default();
            self.button_focus = model::tab_stop(
                self.focused_button(),
                shift,
                !self.files.is_empty(),
                self.export_button().is_some(),
                self.convert_block().is_none() && !self.files.is_empty(),
                &inspector,
            );
            self.files.keyboard_used();
            self.invalidate();
            return true;
        }
        if let Some(b) = self.focused_button() {
            match key {
                VK_RETURN | VK_SPACE => {
                    self.press_button(b);
                    self.invalidate();
                    return true;
                }
                VK_ESCAPE | VK_UP | VK_DOWN | VK_LEFT | VK_RIGHT | VK_HOME | VK_END | VK_PRIOR
                | VK_NEXT => {
                    self.button_focus = None;
                    self.files.keyboard_used();
                    self.invalidate();
                    return true;
                }
                // Delete acts on the list's selection, so the focus goes
                // back to it; shortcuts leave it where it is.
                VK_DELETE | VK_BACK => self.button_focus = None,
                _ => {}
            }
        }
        // Keys that move through or act on the list show its focus ring.
        if matches!(
            key,
            VK_UP
                | VK_DOWN
                | VK_HOME
                | VK_END
                | VK_PRIOR
                | VK_NEXT
                | VK_DELETE
                | VK_BACK
                | VK_ESCAPE
        ) || (key == VK_A && ctrl)
        {
            self.files.keyboard_used();
        }
        match key {
            // Shift with an arrow, Home or End extends the selection.
            VK_UP if shift => self.files.extend(-1),
            VK_DOWN if shift => self.files.extend(1),
            VK_UP => self.files.step(-1),
            VK_DOWN => self.files.step(1),
            // A screenful at a time; Shift extends the selection.
            VK_PRIOR | VK_NEXT => {
                let page = layout::rows_per_page(self.layout().panel.h);
                let by = if key == VK_PRIOR { -page } else { page };
                if shift {
                    self.files.extend(by);
                } else {
                    self.files.step(by);
                }
            }
            VK_HOME | VK_END => {
                let i = match key {
                    VK_HOME => 0,
                    _ => self.files.len().saturating_sub(1),
                };
                if shift {
                    self.files.shift_click(i);
                } else {
                    self.files.click(i);
                }
            }
            VK_A if ctrl => self.files.select_all(),
            VK_DELETE | VK_BACK => self.remove_selected(),
            VK_ESCAPE => self.files.clear_selection(),
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
            VK_Z if ctrl => self.undo_remove(),
            VK_O if ctrl => self.add_files(true),
            VK_V if ctrl => self.paste(true),
            // Ctrl+S exports the selected rows, Ctrl+Shift+S everything.
            VK_S if ctrl && shift => self.export_everything(),
            VK_S if ctrl => {
                let rows = self.files.selection();
                if rows.is_empty() {
                    self.alert("Select a result to export.".to_owned());
                } else {
                    self.export_rows(&rows);
                }
            }
            // Space follows the selected row's link, with one row selected,
            // while rows show their links; it stops a running row as the
            // footer's Cancel does, whose key it is.
            VK_SPACE => {
                if let [i] = self.files.selection()[..] {
                    if self.row_state(i) == RowState::Running {
                        self.cancel_row(i);
                    } else if self.row_links() {
                        self.follow_link(i, true);
                    }
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
        if !self.over().wheel_may_pour() {
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
                // Opens at the pointer, whatever is under it.
                self.open_wheel(k, self.drag_paths.clone(), at, false, Pour::AtPointer);
            }
            (Some(_), None) => self.close_wheel(),
            _ => {}
        }
        self.wheel_point(at.0, at.1);
    }

    /// Whether files that arrive now wait to be added: a panel or one of
    /// Kiln's dialogs is up, and a row added beneath it would go unseen or
    /// change the list a dialog was asked about.
    fn adds_wait(&self) -> bool {
        system::modal_open() || self.over() == Over::Panel
    }

    /// Add `paths` now, or once the list is in view again.
    fn add_when_live(&mut self, paths: Vec<PathBuf>) {
        if self.adds_wait() {
            self.queued.extend(paths);
        } else {
            self.add(paths, false);
        }
    }

    fn dropped(&mut self, ev: &DragEvent) {
        self.drag.set(
            0.0,
            theme::motion::QUICK_MS,
            Instant::now(),
            theme::motion::animations(),
        );
        // A drop acts on the list, so the focus goes back to it.
        self.button_focus = None;
        if self.adds_wait() {
            self.queued.extend(ev.paths.iter().cloned());
            self.drag_paths.clear();
            self.invalidate();
            return;
        }
        let chosen = self
            .wheel
            .as_mut()
            .filter(|w| w.is_open() && !w.keyboard)
            .and_then(|w| {
                // The drop is what counts: use its own paths.
                w.paths = ev.paths.clone();
                w.hover
            });
        let before = self.files.len();
        self.add(ev.paths.clone(), false);
        match chosen {
            Some(spot) => self.choose(spot),
            None => self.close_wheel(),
        }
        self.drag_paths.clear();
        // A plain drop teaches the keys once; the tip stands in the footer
        // until something replaces it.
        let added = self.files.len().saturating_sub(before);
        if chosen.is_none() && ways_in::show_drop_tip(added, self.settings.drop_tip_shown) {
            self.settings.drop_tip_shown = true;
            if let Err(e) = self.settings_store.save_picks(&self.settings) {
                self.alert(e);
            } else {
                self.alert(ways_in::drop_tip(&self.settings.keys));
            }
            self.invalidate();
        }
    }

    // Accessibility.

    /// Where help's lines are while it is open, and which of them are
    /// scrolled out of the panel.
    fn help_places(&self) -> Vec<a11y::HelpPlace> {
        let Some(ui) = &self.help else {
            return vec![];
        };
        let (w, h) = self.renderer.size_dips();
        let body = layout::help(w, h).body;
        help::line_places(&ui.sections, body.w, |s, width| {
            self.renderer.help_line_height(s, width)
        })
        .into_iter()
        .map(|(section, line, r)| {
            let rect = layout::Rect::new(body.x + r.x, body.y + r.y - ui.scroll, r.w, r.h);
            a11y::HelpPlace {
                section,
                line,
                rect,
                offscreen: a11y::clipped_out(&rect, &body),
            }
        })
        .collect()
    }

    /// What the window shows, as a tree for UI Automation. `progress`
    /// holds each row's percent as UI Automation is told it.
    fn a11y_tree(&self, progress: &[Option<u32>]) -> a11y::Node {
        let size = self.renderer.size_dips();
        // As the tree lays the window out (`a11y::tree`): where the fold is
        // going, not each frame of the slide.
        let l = self.settled_layout();
        let saving = self.saving_rows();
        let row_links = self.row_links();
        let rows: Vec<a11y::RowIn> = self
            .files
            .entries()
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let busy = saving.contains(&e.path);
                let instead = if busy {
                    Some(model::SAVING)
                } else {
                    self.notes.get(&e.path).map(|(line, _)| line.as_str())
                };
                let link = l.rows.get(i).and_then(|r| {
                    let mut r = *r;
                    r.y -= self.scroll;
                    (row_links && !busy)
                        .then(|| e.link(self.renderer.line_cut(&r, e)))
                        .flatten()
                });
                a11y::RowIn {
                    entry: e,
                    selected: self.files.is_selected(i),
                    instead,
                    link,
                    progress: progress.get(i).copied().flatten(),
                }
            })
            .collect();
        let status = self.footer_text();
        let empty_status = self.empty_status();
        let empty = ways_in::empty_lines(&self.settings);
        let held = self.export_all_count();
        let export_all = (held > 0).then(|| model::export_all_label(held));
        let settings_layout = self.settings_ui.as_ref().map(|_| self.settings_layout());
        let update_note = self.update.note();
        let (sheet_rows, sheet_lines) = self.sheet_texts();
        let sheet_layout = self.sheet_layout();
        let help_places = self.help_places();
        let input = a11y::Input {
            size,
            rows,
            row_links,
            scroll: self.scroll,
            focus_row: self.files.focus(),
            focus_button: self.focused_button(),
            status: &status,
            empty: &empty,
            empty_status: &empty_status,
            export_all: export_all.as_deref(),
            convert_block: self.convert_block(),
            convert_cancel: self.footer_cancel(),
            undo: self.undo_offer(),
            wheel: self
                .wheel
                .as_ref()
                .filter(|w| w.is_open())
                .map(|w| a11y::WheelIn {
                    wheel: &w.wheel,
                    paths: &w.paths,
                    hover: w.hover,
                    more_open: w.more_open(),
                    options: self.wheel_options().is_some(),
                }),
            settings: match (&self.settings_ui, &settings_layout) {
                (Some(ui), Some(l)) => Some(a11y::SettingsIn {
                    layout: l,
                    focus: ui.focus,
                    quality: &self.settings.quality,
                    theme: self.settings.theme,
                    suggestions: self.settings.suggestions,
                    autosave: self.settings.save_next_to_original,
                    has_habits: !self.settings.habits.is_empty(),
                    keys: self.settings.keys,
                    output: &ui.output_line.0,
                    engines: ui.engine_lines.each_ref().map(|(s, _)| s.as_str()),
                    update_button: self.update.button(),
                    update_note: ui.note.as_deref().unwrap_or(&update_note.0),
                }),
                _ => None,
            },
            details: self.details.as_ref().map(|d| a11y::DetailsIn {
                title: &d.title,
                text: &d.text,
                copy: model::copy_label(d.last_copy.map(|(ok, t)| (ok, t.elapsed()))),
                focus: d.focus,
            }),
            help: self.help.as_ref().map(|ui| &ui.sections[..]),
            help_places: &help_places,
            sheet: match (
                self.sheet.as_ref().filter(|s| s.is_open()),
                &sheet_layout,
                &sheet_lines,
            ) {
                (Some(s), Some(l), Some((title, target, context, result))) => Some(a11y::SheetIn {
                    layout: l,
                    title,
                    target,
                    context,
                    result,
                    rows: &sheet_rows,
                    preset: s.quality.preset,
                    focus: s.focus,
                }),
                _ => None,
            },
            // Folding away, it carries nothing and is left out.
            inspector: self.live_panel().map(|p| a11y::InspectorIn {
                rect: p.rect,
                model: &p.model,
                layout: &p.layout,
                file: panel_file(&p.model, self.files.entries()),
            }),
        };
        let mut tree = a11y::tree(&input);
        // Settings controls that can't be used now say so.
        if let Some(l) = &settings_layout {
            for (c, _) in &l.controls {
                if let Some(n) = tree.find_mut(&a11y::Id::Setting(*c)) {
                    n.states.disabled = !self.control_enabled(*c);
                }
            }
        }
        tree
    }

    /// Give UI Automation the window as it is now, once a client has
    /// asked for it. What to tell clients, a new focus or rows, waits in
    /// `a11y_events` until the message is done.
    fn sync_a11y(&mut self, announce: bool) {
        if !self.uia.active() {
            return;
        }
        let now = Instant::now();
        self.a11y_built = Some(now);
        let entries = self.files.entries();
        let progress: Vec<Option<u32>> = entries
            .iter()
            .map(|e| {
                let fraction = match &e.job {
                    JobState::Running { fraction, .. } => *fraction,
                    _ => None,
                };
                self.progress.shown(&e.path, fraction, now)
            })
            .collect();
        let listed: HashSet<&Path> = entries.iter().map(|e| e.path.as_path()).collect();
        self.progress.keep(|p| listed.contains(p));
        let tree = self.a11y_tree(&progress);
        let unit_px = layout::px_per_unit(self.dpi, self.zoom);
        let has_focus = unsafe { GetFocus() } == self.hwnd;
        let events = self.uia.update(tree, unit_px, has_focus, announce);
        self.a11y_events.extend(events);
        // This build carries every change held back so far, except a
        // percent the gate still holds: a flush stays set for that alone.
        self.a11y_due = None;
        match self.progress.due() {
            Some(due) => self.flush_a11y_at(due),
            None => unsafe {
                let _ = KillTimer(Some(self.hwnd), A11Y_TIMER);
            },
        }
    }

    /// Bring UI Automation up to date after a message that may change
    /// little: an animation frame, a job's progress, a drag. At most
    /// every `a11y::PROGRESS_EVERY`; a flush catches up what's held.
    fn sync_a11y_soon(&mut self) {
        if !self.uia.active() {
            return;
        }
        match self.a11y_built.map(|t| t + a11y::PROGRESS_EVERY) {
            Some(due) if Instant::now() < due => self.flush_a11y_at(due),
            _ => self.sync_a11y(true),
        }
    }

    /// Build UI Automation's tree again at `due` or just after, unless a
    /// flush is already set for sooner.
    fn flush_a11y_at(&mut self, due: Instant) {
        if self.a11y_due.is_some_and(|d| d <= due) {
            return;
        }
        self.a11y_due = Some(due);
        let ms = a11y::timer_ms(Instant::now(), due);
        unsafe {
            SetTimer(Some(self.hwnd), A11Y_TIMER, ms, None);
        }
    }

    /// Do what a UI Automation client asked, as the click or key that
    /// does the same would.
    fn a11y_do(&mut self, req: a11y::Request) {
        use a11y::{Id, Verb};
        // A dialog, prompt or menu of Kiln's is open, and its loop still
        // delivers these. A click couldn't reach the window now, so neither
        // does this: it could start an export or a leave inside another.
        if system::modal_open() {
            return;
        }
        // Asked of an older tree: what it named may have gone, or can't be
        // used any more.
        if !a11y::accepts(&self.a11y_tree(&[]), &req) {
            return;
        }
        // Taken once the message is done: SetFocus sends focus messages
        // straight back to this window.
        if req.verb == Verb::Focus {
            self.take_focus = true;
        }
        let row = |app: &App, path: &Path| app.files.entries().iter().position(|e| e.path == path);
        if req.verb == Verb::Focus {
            // A button, or back to the list or one of its rows.
            self.button_focus = a11y::button_of(&req.id);
            if self.button_focus.is_some() {
                self.files.keyboard_used();
                self.invalidate();
                return;
            }
        }
        match (&req.id, &req.verb) {
            (Id::HelpButton, Verb::Invoke) => self.open_help(),
            (Id::SettingsButton, Verb::Invoke) => self.open_settings(),
            // Like the other UI Automation actions on the list, not a click.
            (Id::UndoRemove, Verb::Invoke) => self.undo_remove(),
            (Id::ConvertButton, Verb::Invoke) => self.convert_selection(true),
            (Id::AddFiles, Verb::Invoke) => self.add_files(true),
            (Id::ExportAll, Verb::Invoke) => self.export_everything(),
            (Id::Clear, Verb::Invoke) => self.clear(),
            (Id::Row(p), verb) => {
                let Some(i) = row(self, p) else {
                    return;
                };
                match verb {
                    Verb::Select => self.files.click(i),
                    Verb::AddToSelection if !self.files.is_selected(i) => self.files.ctrl_click(i),
                    Verb::RemoveFromSelection if self.files.is_selected(i) => {
                        self.files.ctrl_click(i)
                    }
                    Verb::Focus => self.files.set_focus(i),
                    Verb::ScrollIntoView => {}
                    _ => return,
                }
                self.files.keyboard_used();
                self.scroll_to_row(i);
            }
            (Id::RowCancel(p), Verb::Invoke) => {
                if let Some(i) = row(self, p) {
                    self.cancel_row(i);
                }
            }
            (Id::RowRetry(p), Verb::Invoke) => {
                if let Some(i) = row(self, p) {
                    self.files.click(i);
                    self.retry_row(i);
                }
            }
            (Id::RowLink(p), Verb::Invoke) => {
                if let Some(i) = row(self, p) {
                    self.files.click(i);
                    self.follow_link(i, true);
                }
            }
            (Id::InspectorControl(c), Verb::Invoke) => self.panel_act(*c, true),
            (Id::WheelItem(spot), Verb::Invoke | Verb::Select) => self.choose(*spot),
            (Id::WheelItem(spot), Verb::Focus) => {
                if let Some(w) = self.wheel.as_mut().filter(|w| w.is_open()) {
                    w.point(Some(*spot));
                    self.animate();
                }
            }
            (Id::WheelOptions, Verb::Invoke) => {
                let hover = self.wheel.as_ref().and_then(|w| w.hover);
                self.open_sheet(hover);
            }
            (Id::Setting(c), verb) => {
                if !self.control_enabled(*c) {
                    return;
                }
                let l = self.settings_layout();
                let at = l.controls.iter().position(|(k, _)| k == c);
                if let Some(ui) = &mut self.settings_ui {
                    ui.focus = at.or(ui.focus);
                }
                match (c, verb) {
                    (Control::Step(s), Verb::SetValue(v)) => {
                        let Some(to) = s
                            .values()
                            .into_iter()
                            .find(|&x| s.label(x).eq_ignore_ascii_case(v.trim()))
                        else {
                            return;
                        };
                        self.settings.quality.set(*s, to);
                        self.save_settings();
                    }
                    (_, Verb::Invoke | Verb::Select) => return self.activate(*c, 1),
                    _ => {}
                }
                self.invalidate();
            }
            (Id::DetailsButton(b), Verb::Invoke) => self.press_details(*b),
            (Id::DetailsButton(b), Verb::Focus) => {
                if let Some(d) = &mut self.details {
                    d.focus = *b;
                    self.invalidate();
                }
            }
            (Id::HelpClose, Verb::Invoke) => {
                self.help = None;
                self.invalidate();
            }
            (Id::SheetControl(c), verb) => {
                let Some(l) = self.sheet_layout() else {
                    return;
                };
                let at = l.controls.iter().position(|(k, _)| k == c);
                let Some(s) = self.sheet.as_mut().filter(|s| s.is_open()) else {
                    return;
                };
                s.focus = at.or(s.focus);
                match (c, verb) {
                    (SheetControl::Step(row), Verb::SetValue(v)) => {
                        let Some(&knob) = s.knobs.get(*row) else {
                            return;
                        };
                        let unit = q::size_unit(&s.to);
                        let to = s
                            .values(knob)
                            .into_iter()
                            .find(|&x| q::label(knob, x, unit).eq_ignore_ascii_case(v.trim()));
                        if let Some(to) = to {
                            q::set(&mut s.quality, knob, to);
                        }
                    }
                    (_, Verb::Invoke | Verb::Select) => return self.sheet_activate(*c, 1),
                    _ => {}
                }
                self.invalidate();
            }
            _ => {}
        }
    }

    /// The options sheet's knobs, each with its value as it reads, and its
    /// title, target, source and result lines.
    #[allow(clippy::type_complexity)]
    fn sheet_texts(
        &self,
    ) -> (
        Vec<(String, String)>,
        Option<(String, String, String, String)>,
    ) {
        let rows: Vec<(String, String)> = self
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
        let lines = self.sheet.as_ref().map(|s| {
            let title = match s.paths.len() {
                1 => format!("Convert to {}", s.to.code()),
                n => format!("Convert {n} files to {}", s.to.code()),
            };
            let target = wheel::target_label(&s.to.ext);
            let infos = s.infos.as_deref();
            let one = infos.and_then(|i| (i.len() == 1).then(|| &i[0]));
            let context = q::source_line(&s.names, s.bytes, one);
            let result = match infos {
                None => "Reading the file…".to_owned(),
                Some(i) => q::result_line(&s.from, &s.to, &s.quality, i),
            };
            (title, target, context, result)
        });
        (rows, lines)
    }

    /// What the footer says, the open wheel's hint aside.
    fn footer_text(&self) -> String {
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
        // While the work under way is what it says, it says how to cancel
        // a selected file that is converting, as the selection is now. The
        // selection is looked through only then.
        let status =
            ways_in::with_cancel_hint(status, self.working.as_deref(), || self.footer_cancel());
        // While several rows are selected, their count stands over all of
        // that, unless something is running or went wrong.
        let count = model::selected_line(self.files.selected_rows().len());
        ways_in::footer_line(&status, count.as_deref(), running, self.alerting()).to_owned()
    }

    /// The empty window has no footer: what the status says, past the
    /// plain "0 files.", sits under its Add files button.
    fn empty_status(&self) -> String {
        match self.status.as_str() {
            s if s == status_line(self.files.len(), 0, 0, 0) => String::new(),
            s => s.to_owned(),
        }
    }

    fn paint(&mut self) {
        // A frame can come before the message that changed the selection
        // is done, such as the first after files arrive at start.
        self.follow();
        self.refresh_panel();
        let (now, animations) = (Instant::now(), theme::motion::animations());
        let settings_layout = self.settings_layout();
        let empty = ways_in::empty_lines(&self.settings);
        let update_note = self.update.note();
        let wheel_hint = self.wheel_hint();
        let sheet_layout = self.sheet_layout();
        let (sheet_rows, sheet_lines) = self.sheet_texts();
        let held = self.export_all_count();
        let export_all = (held > 0).then(|| model::export_all_label(held));
        let saving = self.saving_rows();
        let status = self.footer_text();
        let empty_status = self.empty_status();
        let row_links = self.row_links();
        let painted = self.paint_previews(now, animations);
        let preview = |i: usize, size: PreviewSize| {
            painted
                .iter()
                .find(|(row, s, ..)| *row == i && *s == size)
                .map(|(_, _, px, id, fade)| (px.clone(), *id, *fade))
        };
        let entries = self.files.entries();
        let kiln = &self.kiln;
        let kind = |i: usize| entries.get(i).and_then(|e| kind_of(kiln, e));
        let glaze = |i: usize| theme::glaze_of(kind(i));
        let moves = &self.motion;
        let anim = |i: usize| moves.row_anim(i, entries.get(i), now, animations);
        let button = |b: ButtonId| moves.button(b, now);
        // The inspector's content rising in, over what it said before
        // fading out.
        let (alpha, rise) = self.inspector_rise(now);
        let live = self.inspector_live();
        let before = self.inspector_fading(now);
        let inspector = self.panel.as_ref().map(|p| InspectorView {
            now: page(p, alpha, rise, live),
            before: self
                .inspector_prev
                .as_ref()
                .zip(before)
                .map(|((p, _), a)| page(p, a, 0.0, false)),
        });
        let selected = self.files.selected_rows();
        let view = View {
            palette: &self.palette,
            // One ring at a time: on a button Tab moved to, or on the list.
            focus: self
                .files
                .visible_focus()
                .filter(|_| self.focused_button().is_none()),
            button_ring: self
                .focused_button()
                .and_then(|b| self.focus_button_rect(b)),
            shown: self.motion.held_fold(),
            // One selected row's wash glides; several draw their own.
            selection_y: (selected.len() == 1).then(|| self.motion.selection_at(now)),
            button: &button,
            entries,
            notes: self.notes.by_file(),
            saving: &saving,
            export_all: export_all.as_deref(),
            selected,
            glaze: &glaze,
            kind: &kind,
            preview: &preview,
            row_links,
            hover_row: self.hover_row,
            hover_cancel: self.hover_cancel,
            hover_link: self.hover_link,
            hover_retry: self.hover_retry,
            scroll: self.scroll,
            drag: self.drag.value(now).clamp(0.0, 1.0),
            heat: heat_now(),
            row_anim: &anim,
            status: wheel_hint.as_deref().unwrap_or(&status),
            empty: &empty,
            empty_status: &empty_status,
            convert_enabled: self.convert_block().is_none(),
            convert_label: if self.footer_cancel() {
                "Cancel"
            } else {
                "Convert"
            },
            convert_key: !self.footer_cancel(),
            undo: self.undo_offer().is_some(),
            wheel: Self::wheel_view(self.wheel.as_ref(), &self.settings.keys).map(|wv| WheelView {
                options: self.wheel_options(),
                ..wv
            }),
            details: self.details.as_ref().map(|d| DetailsView {
                title: &d.title,
                text: &d.text,
                scroll: d.scroll,
                copy_label: model::copy_label(d.last_copy.map(|(ok, t)| (ok, t.elapsed()))),
                focus: d.focus,
                button: &button,
            }),
            help: self.help.as_ref().map(|ui| {
                let (open, rise) = sheet_rise(ui.opened, now, animations);
                HelpView {
                    sections: &ui.sections,
                    scroll: ui.scroll,
                    button: &button,
                    open,
                    rise,
                }
            }),
            inspector,
            sheet: match (&self.sheet, &sheet_layout, &sheet_lines) {
                (Some(s), Some(l), Some((title, target, context, result))) => {
                    let (open, rise) = sheet_rise(s.opened, now, animations);
                    Some(SheetView {
                        layout: l,
                        title,
                        target,
                        context,
                        result,
                        rows: &sheet_rows,
                        preset: s.quality.preset,
                        hover: s.hover,
                        pressed: s.pressed,
                        focus: s.focus,
                        button: &button,
                        open,
                        rise,
                        fade: s.closing.map_or(1.0, |t| {
                            1.0 - anim::eased(t, theme::motion::QUICK_MS, now, animations)
                        }),
                    })
                }
                _ => None,
            },
            settings: self.settings_ui.as_ref().map(|ui| {
                let (open, rise) = sheet_rise(ui.opened, now, animations);
                SettingsView {
                    open,
                    rise,
                    layout: &settings_layout,
                    tab_marker: ui.tab_marker.value(now),
                    page: anim::eased(ui.switched, theme::motion::QUICK_MS, now, animations),
                    quality: &self.settings.quality,
                    output: (ui.output_line.0.as_str(), ui.output_line.1),
                    theme: self.settings.theme,
                    suggestions: self.settings.suggestions,
                    autosave: self.settings.save_next_to_original,
                    has_habits: !self.settings.habits.is_empty(),
                    keys: self.settings.keys,
                    engines: ui.engine_lines.each_ref().map(|(s, m)| (s.as_str(), *m)),
                    hover: ui.hover,
                    pressed: self.settings_pressed,
                    button: &button,
                    update: ui
                        .note
                        .as_deref()
                        .map_or((update_note.0.as_str(), update_note.1), |note| {
                            (note, false)
                        }),
                    update_button: self.update.button(),
                    focus: ui.focus,
                }
            }),
        };
        if let Err(e) = self.renderer.draw(&view) {
            log(&format!("draw failed: {e}"));
        }
        self.painted_previews = painted;
        let key = self.tip_key();
        if self.tip_cache.as_ref().is_none_or(|(k, _)| *k != key) {
            let tools = self.tip_tools();
            self.tip_cache = Some((key, tools));
        }
        // Removing the tool that's showing hides it at once, and the
        // control tells the window. Out of `self`, that notice finds no
        // tooltip control and goes to DefWindowProc.
        let mut tips = self.tips.take();
        if let (Some(t), Some((_, tools))) = (&mut tips, &self.tip_cache) {
            t.sync(tools, self.unit_dpi());
        }
        self.tips = tips;
    }

    // Previews.

    /// Device pixels per layout DIP: the screen's scale times the Text
    /// size zoom.
    fn px_per_dip(&self) -> f32 {
        layout::px_per_unit(self.dpi, self.zoom)
    }

    /// The key for row `i`'s preview at `size`, as the list knows the file
    /// now. It reads nothing from the disk: the file's time is the one the
    /// list holds.
    fn preview_key(&self, i: usize, size: PreviewSize) -> Option<PreviewKey> {
        let e = self.files.entries().get(i)?;
        Some(PreviewKey {
            path: e.path.clone(),
            modified: e.modified,
            size,
            pixels: previews::pixel_size(size, self.px_per_dip()),
        })
    }

    /// Ask the thumbnail worker for the previews the window needs now,
    /// most wanted first, in place of what it was asked for before. Run
    /// after every message, it asks only when what the previews depend on
    /// has changed: the selection, the rows in view (scrolling, a resize),
    /// the files listed, or the screen's scale. It reads nothing from the
    /// disk and never calls the Shell.
    fn ask_previews(&mut self) {
        if self.destroyed {
            return;
        }
        let ask = PreviewAsk {
            // Only while the inspector shows: a narrow window never spends
            // the worker on pictures at its size.
            pictured: self.pictured(),
            visible: self.visible_rows(),
            total: self.files.len(),
            px_per_dip: self.px_per_dip(),
        };
        if self.previews_asked.as_ref() == Some(&ask) {
            return;
        }
        // The inspector's file first; with several, the rest of its stack
        // right after the first card.
        let mut order = previews::request_order(
            ask.pictured.first().copied(),
            ask.visible.clone(),
            ask.total,
        );
        for (k, &i) in ask.pictured.iter().enumerate().skip(1) {
            order.insert(k, (i, PreviewSize::Hero));
        }
        // Audio keeps its glaze tile, so its files are never asked for.
        let keys: Vec<PreviewKey> = order
            .into_iter()
            .filter(|&(i, _)| previews::wants_preview(self.row_kind(i)))
            .filter_map(|(i, size)| self.preview_key(i, size))
            .collect();
        // The worker's queue is replaced, so what it dropped will never
        // come: what is pending is exactly what is asked now.
        self.previews.clear_pending();
        let wanted: Vec<PreviewKey> = keys
            .into_iter()
            .filter(|k| self.previews.pending(k))
            .collect();
        // Asked even when empty, so the worker stops on rows scrolled away.
        self.thumbs.ask(wanted);
        self.previews_asked = Some(ask);
    }

    /// Rows were removed, cleared, restored or moved: drop the previews of
    /// files no longer listed, and ask again.
    fn previews_rows_changed(&mut self) {
        let listed: HashSet<&Path> = self
            .files
            .entries()
            .iter()
            .map(|e| e.path.as_path())
            .collect();
        self.previews.keep_paths(&|p| listed.contains(p));
        self.previews_asked = None;
    }

    /// A preview from the worker, made from the file as it was at the
    /// key's time. The list decides whether it is kept, and for which row
    /// (`FileList::preview_arrived`).
    fn on_preview(&mut self, key: PreviewKey, preview: Preview) {
        let listed = self.files.index_of(&key.path);
        let was = listed.and_then(|i| self.files.entries()[i].modified);
        let Some(i) = self.files.preview_arrived(&key.path, key.modified) else {
            return;
        };
        // The file changed since it was listed: its other sizes are asked
        // for again at its new time, not left waiting for the next scroll.
        if key.modified != was {
            self.previews_asked = None;
        }
        self.previews.put(key, preview, Instant::now());
        // Only a picture that shows needs a frame: a row in view (the rows
        // `paint_previews` draws), or one the inspector pictures, the
        // selected file or the cards of several. Those asked ahead of the
        // scroll wait in the cache. A picture in view fades in, so frames
        // run until it has.
        if self.visible_rows().contains(&i) || self.pictured().contains(&i) {
            self.motion.preview_shown = Some(Instant::now());
            self.animate();
        }
    }

    /// The pictures to paint this frame: each row in view, and each the
    /// inspector pictures at its own size, that wants a picture
    /// (`previews::wants_preview`) and whose picture has arrived, with its
    /// key's id and how far it has faded in. Looking one up
    /// counts as using it, which keeps it in the cache. A picture still
    /// fading in, such as one asked for ahead of the scroll that has just
    /// come into view, keeps frames coming until it has.
    fn paint_previews(&mut self, now: Instant, animations: bool) -> Vec<Painted> {
        let mut painted = std::mem::take(&mut self.painted_previews);
        painted.clear();
        let mut fading: Option<Instant> = None;
        // The rows in view at their size, and the inspector's at its own:
        // what it shows and, while that rises in, what it showed before.
        let rows = self.visible_rows().map(|i| (i, PreviewSize::Row));
        let heroes = self
            .panel
            .iter()
            .chain(self.inspector_prev.iter().map(|(p, _)| p))
            .flat_map(|p| p.pictured().iter().map(|&i| (i, PreviewSize::Hero)));
        for (i, size) in rows.chain(heroes) {
            if !previews::wants_preview(self.row_kind(i)) {
                continue;
            }
            let Some(key) = self.preview_key(i, size) else {
                continue;
            };
            if let Some((Preview::Ready(px), arrived)) = self.previews.get(&key, now) {
                // It fades in from when it arrived.
                let (fade, _) = anim::rise(
                    anim::elapsed_ms(arrived, now),
                    anim::PREVIEW_MS,
                    0.0,
                    animations,
                );
                if fade < 1.0 {
                    fading = fading.max(Some(arrived));
                }
                painted.push((i, size, px.clone(), key.id(), fade));
            }
        }
        // One still fading in, such as one asked for ahead of the scroll
        // that has just come into view, keeps frames coming until it has.
        if let Some(arrived) = fading {
            self.motion.preview_shown = self.motion.preview_shown.max(Some(arrived));
            self.frames();
        }
        painted
    }

    /// The rows the inspector pictures at its own size: none while it
    /// doesn't show.
    fn pictured(&self) -> Vec<usize> {
        self.panel
            .as_ref()
            .map(|p| p.pictured().to_vec())
            .unwrap_or_default()
    }

    /// Row `i`'s kind of file, from its type.
    fn row_kind(&self, i: usize) -> Option<kiln_core::Kind> {
        kind_of(&self.kiln, self.files.entries().get(i)?)
    }

    /// Whether rows show their own action links (their link, Cancel and a
    /// "Saved to" line): not while the inspector shows, as it carries the
    /// actions (`inspector_live`). As the fold begins, the links follow
    /// where it is going: they are back the moment the inspector starts
    /// to fold away, and gone the moment it starts to slide in. Drawing,
    /// hit-testing, tooltips, the UI Automation tree and Space all go by
    /// it.
    fn row_links(&self) -> bool {
        !self.inspector_live()
    }

    // Tooltips.

    /// The line row `i` shows: a note standing in for it, "Saving…", or
    /// its own.
    fn shown_line(&self, i: usize) -> Option<String> {
        let e = self.files.entries().get(i)?;
        Some(if self.row_saving(i) {
            model::SAVING.to_owned()
        } else {
            self.notes
                .get(&e.path)
                .map_or_else(|| e.line().0, |(line, _)| line.clone())
        })
    }

    /// The rows in view, or partly in view.
    fn visible_rows(&self) -> std::ops::Range<usize> {
        let panel = self.layout().panel;
        let first = (self.scroll / ROW_HEIGHT).floor().max(0.0) as usize;
        let visible = (panel.h / ROW_HEIGHT).ceil() as usize + 1;
        first.min(self.files.len())..(first + visible).min(self.files.len())
    }

    /// What the tooltips' rectangles depend on now. Cheap: nothing is
    /// measured.
    fn tip_key(&self) -> TipKey {
        let rows = self
            .visible_rows()
            .filter_map(|i| {
                let e = self.files.entries().get(i)?;
                Some((
                    i,
                    self.row_rect(i)?,
                    e.code.clone(),
                    self.shown_line(i)?,
                    self.row_saving(i),
                    std::mem::discriminant(&e.job),
                    e.held().is_some(),
                ))
            })
            .collect();
        TipKey {
            size: self.renderer.size_dips(),
            scroll: self.scroll,
            over: self.over(),
            add: self.add_button(),
            export: self.export_button(),
            rows,
        }
    }

    /// Every tooltip's id and rectangle as the window is now. None while
    /// a wheel, panel or dialog is up.
    fn tip_tools(&self) -> Vec<(usize, layout::Rect)> {
        if !self.over().list_live() {
            return vec![];
        }
        let l = self.layout();
        let mut tools = vec![(TIP_SETTINGS, l.settings_button), (TIP_HELP, l.help_button)];
        tools.extend(self.convert_rect().map(|b| (TIP_CONVERT, b)));
        tools.extend(self.add_button().map(|b| (TIP_ADD, b)));
        tools.extend(self.export_button().map(|b| (TIP_EXPORT_ALL, b)));
        // Rows, cut to the part of the list that shows.
        let panel = l.panel;
        let clip = |r: layout::Rect| {
            let top = r.y.max(panel.y);
            let bottom = r.bottom().min(panel.bottom());
            layout::Rect::new(r.x, top, r.w, (bottom - top).max(0.0))
        };
        let links = self.row_links();
        for i in self.visible_rows() {
            let (Some(r), Some(line)) = (self.row_rect(i), self.shown_line(i)) else {
                continue;
            };
            let e = &self.files.entries()[i];
            let saving = self.row_saving(i);
            let (area, _) = self.renderer.row_tip(&r, e, &line, saving, links);
            let id = TIP_ROWS + 3 * i;
            tools.push((id, clip(area)));
            if links && matches!(e.job, JobState::Running { .. }) {
                tools.push((id + 1, clip(layout::row_cancel(&r))));
            }
            // Only a failed row's link depends on its line being cut.
            if links && !saving && e.link(false) == Some("Export") {
                tools.push((id + 2, clip(layout::row_link(&r))));
            }
        }
        tools
    }

    /// What tooltip `id` says, worked out as it shows.
    fn tip_text(&self, id: usize) -> String {
        match id {
            TIP_SETTINGS => help::SETTINGS_TIP.to_owned(),
            TIP_HELP => help::HELP_TIP.to_owned(),
            TIP_CONVERT if self.footer_cancel() => help::CANCEL_ALL_TIP.to_owned(),
            TIP_CONVERT => self.convert_block().unwrap_or(help::CONVERT_TIP).to_owned(),
            TIP_ADD => help::ADD_FILES_TIP.to_owned(),
            TIP_EXPORT_ALL => help::EXPORT_ALL_TIP.to_owned(),
            _ if id >= TIP_ROWS => {
                let (i, part) = ((id - TIP_ROWS) / 3, (id - TIP_ROWS) % 3);
                match part {
                    1 => help::STOP_TIP.to_owned(),
                    2 => help::EXPORT_TIP.to_owned(),
                    _ => {
                        let (Some(r), Some(line), Some(e)) = (
                            self.row_rect(i),
                            self.shown_line(i),
                            self.files.entries().get(i),
                        ) else {
                            return String::new();
                        };
                        let saving = self.row_saving(i);
                        let links = self.row_links();
                        let (_, cut) = self.renderer.row_tip(&r, e, &line, saving, links);
                        e.tooltip(&line, cut)
                    }
                }
            }
            _ => String::new(),
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
            | Control::AutoSave(_)
            | Control::Preset(..)
            | Control::Step(_)
    )
}

/// Whether two controls are segments of one group.
fn same_group(a: Control, b: Control) -> bool {
    match (a, b) {
        (Control::Tab(_), Control::Tab(_))
        | (Control::Theme(_), Control::Theme(_))
        | (Control::Suggestions(_), Control::Suggestions(_))
        | (Control::AutoSave(_), Control::AutoSave(_)) => true,
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
    // The first frame, and the window's own background before it, are in
    // the theme settings.toml chooses, or the Windows app mode's. A small
    // file, read before the window is made, so a light Kiln never shows a
    // dark frame first.
    let system_light = system::system_light();
    let contrast = system::high_contrast();
    theme::motion::set_animations(system::animations_on());
    let mut settings_store = settings::Store::new(settings::default_path());
    let (settings, settings_error) = settings_store.load();
    let palette = palette_for(contrast, settings.theme.is_light(system_light));
    let instance: HINSTANCE = unsafe { GetModuleHandleW(None)? }.into();
    let class = handoff::CLASS;
    let icon = unsafe { LoadIconW(Some(instance), PCWSTR(1 as _)) }.unwrap_or_default();
    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
        lpfnWndProc: Some(wndproc),
        hInstance: instance,
        hIcon: icon,
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
        // The theme's ground, so the first frame is already the window's
        // own colour, light or dark.
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
    style_title_bar(hwnd, &palette);
    // Text size: its change event comes on a thread pool thread, so it
    // only posts a message and the window reads the new factor itself.
    let target = PostTarget(hwnd.0 as usize);
    let text_scale_source = system::TextScale::new(move || target.post(WM_KILN_TEXT_SCALE, ()));
    let text_scale = text_scale_source.factor();
    let zoom = layout::text_zoom(text_scale, layout::default_window(text_scale));

    let holding = open_holding();
    // Without a holding folder results go next to each file, as before,
    // and the footer keeps saying so.
    let sticky = holding.is_none().then_some(model::NO_HOLDING);

    let kiln = Arc::new(Kiln::new());
    let target = PostTarget(hwnd.0 as usize);
    // A finished job's results are read for their sizes here, on the
    // runner's thread, not by the window.
    let runner = Runner::new(kiln.clone(), Runner::default_workers(), move |ev| {
        target.post(WM_KILN_JOB, JobNews::of(ev))
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
        contrast,
        text_scale,
        text_scale_source,
        zoom,
        settings,
        settings_store,
        system_light,
        settings_ui: None,
        settings_hover: false,
        last_mouse: None,
        renderer: Renderer::new(hwnd, layout::px_per_unit(dpi, zoom) * 96.0)?,
        files: FileList::default(),
        motion: Motion::new(Instant::now(), 1.0),
        scroll: 0.0,
        hover_row: None,
        hover_cancel: false,
        hover_link: false,
        hover_retry: false,
        drag: Tween::new(0.0, Instant::now()),
        drag_paths: vec![],
        wheel: None,
        sheet: None,
        next_sheet: 1,
        details: None,
        help: None,
        help_hover: false,
        tips: None,
        tip_cache: None,
        status: String::new(),
        clear_hover: false,
        clear_pressed: false,
        settings_pressed: None,
        update: UpdateState::default(),
        tracking_mouse: false,
        dpi,
        holding,
        last_export: None,
        notes: model::Notes::default(),
        export_hover: false,
        export_pressed: false,
        shutdown_told: 0,
        exports: HashMap::new(),
        next_export: 1,
        leaving: None,
        close_when_saved: false,
        destroyed: false,
        sticky,
        painted_once: false,
        helper_notice: false,
        taskbar: None,
        taskbar_shown: None,
        undo: None,
        autosave_waiting: vec![],
        tally: (0, 0, 0, None),
        notice_row: None,
        add_hover: false,
        add_pressed: false,
        convert_hover: false,
        convert_pressed: false,
        last_add: None,
        added: None,
        working: None,
        alert: None,
        uia: uia::Host::new(hwnd),
        progress: a11y::ProgressGate::default(),
        a11y_built: None,
        a11y_due: None,
        a11y_events: Vec::new(),
        status_sets: Vec::new(),
        teller: a11y::StatusTeller::default(),
        take_focus: false,
        button_focus: None,
        queued: vec![],
        own_drag: Rc::new(Cell::new(false)),
        list_press: None,
        dragged: HashSet::new(),
        previews: PreviewCache::new(CACHE_CAP),
        thumbs: Thumbs::start(target),
        previews_asked: None,
        painted_previews: Vec::new(),
        panel: None,
        inspector_prev: None,
        clock: false,
        panel_hover: None,
        panel_pressed: None,
        panel_built: None,
        panel_rev: 0,
        result_sizes: HashMap::new(),
    });
    let app = Box::into_raw(app);
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, app as isize) };
    // The tooltip control is the window's own, and goes with it.
    unsafe { (*app).tips = Tips::new(hwnd, instance, app_ref(app).unit_dpi()) };

    // Size the window for its monitor's scale and the Text size, centred
    // where Windows put it.
    resize_for_dpi(hwnd, dpi, text_scale);
    unsafe { (*app).rezoom() };

    if !app_ref(app).renderer.display_font_bundled() {
        log("bundled serif unavailable; using the fallback");
    }

    let target: IDropTarget = DropTarget::new(hwnd, app_ref(app).own_drag.clone()).into();
    unsafe { RegisterDragDrop(hwnd, &target)? };

    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = UpdateWindow(hwnd);
        (*app).settings_loaded(settings_error);
    }

    if !files.is_empty() {
        unsafe { (*app).add(files, false) };
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

/// The palette to draw with: a contrast theme's own colours while one is
/// on, else Kiln's light or dark.
fn palette_for(contrast: Option<theme::SysColors>, light: bool) -> Palette {
    match contrast {
        Some(c) => theme::theme_from_system(c),
        None if light => theme::LIGHT,
        None => theme::DARK,
    }
}

fn style_title_bar(hwnd: HWND, p: &Palette) {
    // Dark title bar (Windows 10 20H1+), and on Windows 11 the caption in
    // the palette's own ground, text and border colours. Under a contrast
    // theme the caption is Windows' own. Errors are fine: older systems
    // keep the default frame.
    const USE_IMMERSIVE_DARK_MODE: i32 = 20;
    const BORDER_COLOR: i32 = 34;
    const CAPTION_COLOR: i32 = 35;
    const TEXT_COLOR: i32 = 36;
    /// DWMWA_COLOR_DEFAULT: let Windows choose.
    const COLOR_DEFAULT: u32 = 0xFFFF_FFFF;
    let dark: BOOL = (theme::luminance(p.ground) < 0.5).into();
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
            let v = if p.high_contrast {
                COLOR_DEFAULT
            } else {
                c.colorref()
            };
            set(attr, &v as *const u32 as _, 4);
        }
    }
}

/// The outer size of a window whose client area is `client` DIPs at
/// `dpi`, kept within the work area of the monitor it's on.
fn window_size(hwnd: HWND, dpi: f32, client: (f32, f32)) -> (i32, i32) {
    let scale = dpi / 96.0;
    let mut rc = RECT {
        left: 0,
        top: 0,
        right: (client.0 * scale).ceil() as i32,
        bottom: (client.1 * scale).ceil() as i32,
    };
    unsafe {
        let _ = windows::Win32::UI::HiDpi::AdjustWindowRectExForDpi(
            &mut rc,
            WS_OVERLAPPEDWINDOW,
            false,
            WINDOW_EX_STYLE::default(),
            dpi as u32,
        );
    }
    let (mut w, mut h) = (rc.right - rc.left, rc.bottom - rc.top);
    if let Some(area) = work_area(hwnd) {
        w = w.min(area.right - area.left);
        h = h.min(area.bottom - area.top);
    }
    (w, h)
}

/// The work area of the monitor `hwnd` is on, in pixels.
fn work_area(hwnd: HWND) -> Option<RECT> {
    use windows::Win32::Graphics::Gdi::MONITOR_DEFAULTTONEAREST;
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO};
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    unsafe {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        GetMonitorInfoW(monitor, &mut info)
            .as_bool()
            .then_some(info.rcWork)
    }
}

/// Resize the window, where it is, so its client area is `client` DIPs.
fn grow_client(hwnd: HWND, dpi: f32, client: (f32, f32)) {
    let (w, h) = window_size(hwnd, dpi, client);
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            w,
            h,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

fn resize_for_dpi(hwnd: HWND, dpi: f32, text_scale: f32) {
    grow_client(hwnd, dpi, layout::default_window(text_scale));
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
    let (answer, events, take_focus) = {
        let app = unsafe { &mut *ptr };
        // The fold is read once for the whole message, so everything it
        // works out sees the inspector at one place (`Motion::hold_fold`).
        let outer = app.motion.hold_fold(Instant::now());
        let answer = handle(app, msg, wparam, lparam);
        after(app, msg, wparam);
        app.motion.release_fold(outer, Instant::now());
        let events = std::mem::take(&mut app.a11y_events);
        (answer, events, std::mem::take(&mut app.take_focus))
    };
    // With `App` no longer borrowed: these send messages straight back to
    // this window, and a client told something may call straight back in.
    if take_focus {
        unsafe {
            let _ = SetFocus(Some(hwnd));
        }
    }
    uia::raise(events);
    match answer {
        Some(r) => r,
        None => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// After each message, tell UI Automation each status set, at once, then
/// bring its tree up to date, even while minimised, when nothing paints.
fn after(app: &mut App, msg: u32, wparam: WPARAM) {
    app.run_due();
    // Files that arrived under a panel or a dialog join the list once it's
    // back in view.
    if !app.queued.is_empty() && !app.adds_wait() {
        let paths = std::mem::take(&mut app.queued);
        app.add(paths, false);
    }
    // Whatever the message moved (the scroll, the size, the selection,
    // the files, the pointer, the screen), the motion, the inspector and
    // the previews follow it. Nothing is built or asked when none of that
    // changed.
    app.follow();
    app.refresh_panel();
    app.ask_previews();
    let sets: Vec<(String, bool)> = app
        .status_sets
        .drain(..)
        .map(|t| {
            let alert = app.alert.as_deref() == Some(t.as_str());
            (t, alert)
        })
        .collect();
    let said = !sets.is_empty();
    let active = app.uia.active();
    for (text, alert) in app.teller.tell(sets, active) {
        let event = app.uia.status(text, alert);
        app.a11y_events.push(event);
    }
    if !active {
        return;
    }
    match sync_kind(msg, wparam.0, said) {
        SyncKind::Skip => {}
        SyncKind::Now => app.sync_a11y(true),
        SyncKind::Soon => app.sync_a11y_soon(),
    }
}

/// When a message's changes reach UI Automation's tree.
#[derive(Debug, PartialEq, Eq)]
enum SyncKind {
    /// It changes nothing the tree shows, or builds the tree itself.
    Skip,
    Now,
    /// At most every `a11y::PROGRESS_EVERY`.
    Soon,
}

/// Input, a status set and the window's own news go at once; anything
/// else, such as animation frames and job progress, a few times a second
/// at most. Painting never builds the tree. `timer` is a WM_TIMER's id.
fn sync_kind(msg: u32, timer: usize, said: bool) -> SyncKind {
    match msg {
        _ if said => SyncKind::Now,
        WM_PAINT | WM_ERASEBKGND | WM_GETOBJECT | WM_NCHITTEST | WM_SETCURSOR | WM_DESTROY
        | WM_NCDESTROY | WM_KILN_PREVIEW => SyncKind::Skip,
        WM_TIMER if timer == A11Y_TIMER => SyncKind::Skip,
        WM_TIMER if timer == HINT_TIMER || timer == COPIED_TIMER => SyncKind::Now,
        WM_KEYDOWN | WM_SYSKEYDOWN | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_LBUTTONDBLCLK
        | WM_CONTEXTMENU | WM_MOUSEWHEEL | WM_SETFOCUS | WM_KILLFOCUS | WM_ACTIVATE
        | WM_COPYDATA | WM_SIZE | WM_DPICHANGED | WM_KILN_DROP | WM_KILN_EXPORTED
        | WM_KILN_MOVED | WM_KILN_FACTS | WM_KILN_PROBE | WM_KILN_UPDATE | WM_KILN_NOTIFY => {
            SyncKind::Now
        }
        m if m == uia::WM_KILN_A11Y => SyncKind::Now,
        _ => SyncKind::Soon,
    }
}

fn handle(app: &mut App, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    let hwnd = app.hwnd;
    match msg {
        WM_PAINT => {
            // The first frame takes a while to build (Direct2D, fonts,
            // icons). Until it is on screen the window would show the
            // blank white Windows gives a new surface, so it is filled
            // with the theme's ground colour first, by GDI, at once.
            if !app.painted_once {
                app.fill_ground();
            }
            app.paint();
            app.painted_once = true;
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
                app.add_when_live(files);
            }
            Some(LRESULT(1))
        }
        WM_SIZE => {
            app.renderer.resize();
            // Minimised, the client area is empty: keep the zoom and the
            // scroll for when the window comes back.
            if wparam.0 == SIZE_MINIMIZED as usize {
                return Some(LRESULT(0));
            }
            app.rezoom();
            app.clamp_scroll();
            app.invalidate();
            Some(LRESULT(0))
        }
        WM_DPICHANGED => {
            app.dpi = (wparam.0 & 0xFFFF) as f32;
            app.renderer.set_dpi(app.unit_dpi());
            if let Some(t) = &app.tips {
                t.set_dpi(app.unit_dpi());
            }
            app.tip_cache = None;
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
        // The smallest window holds the smallest layout at the Text size:
        // its client area is `layout::min_window`, kept within the screen.
        WM_GETMINMAXINFO => {
            let info = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
            let (w, h) = window_size(hwnd, app.dpi, layout::min_window(app.text_scale));
            info.ptMinTrackSize = POINT { x: w, y: h };
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
        // "Copied" has been read: the button says "Copy" again.
        WM_TIMER if wparam.0 == COPIED_TIMER => {
            unsafe {
                let _ = KillTimer(Some(hwnd), COPIED_TIMER);
            }
            app.invalidate();
            Some(LRESULT(0))
        }
        // A change held back is due. A timer that fires early waits out
        // the rest: building early would hold a percent again.
        WM_TIMER if wparam.0 == A11Y_TIMER => {
            let now = Instant::now();
            match app.a11y_due {
                Some(due) if now < due => unsafe {
                    SetTimer(Some(hwnd), A11Y_TIMER, a11y::timer_ms(now, due), None);
                },
                _ => app.sync_a11y(true),
            }
            Some(LRESULT(0))
        }
        // A second has passed while the inspector shows a file converting:
        // it is built again after this message, its time left with it, and
        // the row's own line is drawn again beside it.
        WM_TIMER if wparam.0 == CLOCK_TIMER => {
            app.panel_stale();
            app.invalidate();
            Some(LRESULT(0))
        }
        WM_TIMER if wparam.0 == ANIM_TIMER => {
            app.tick();
            match anim::frame_ms(app.moving(), app.burning(), theme::motion::animations()) {
                None => unsafe {
                    let _ = KillTimer(Some(hwnd), ANIM_TIMER);
                },
                // `animate` already set the timer to the frame rate, so
                // only the slower heat rate needs setting.
                Some(anim::FRAME_MS) => {}
                Some(ms) => unsafe {
                    SetTimer(Some(hwnd), ANIM_TIMER, ms, None);
                },
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
                app.mouse_move(x, y, wparam.0 as u32);
            }
            Some(LRESULT(0))
        }
        // A tooltip about to show asks what to say.
        WM_NOTIFY => {
            let id = app.tips.as_ref()?.asking(lparam)?;
            let text = app.tip_text(id);
            if let Some(t) = &mut app.tips {
                t.answer(lparam, &text);
            }
            Some(LRESULT(0))
        }
        // Another window took the mouse mid-press: no drag or collapse is
        // left waiting for a release that won't come.
        WM_CAPTURECHANGED => {
            if lparam.0 != hwnd.0 as isize {
                app.list_press = None;
            }
            None
        }
        WM_MOUSELEAVE => {
            app.tracking_mouse = false;
            app.last_mouse = None;
            app.help_hover = false;
            app.hover_row = None;
            app.hover_cancel = false;
            app.hover_link = false;
            app.hover_retry = false;
            app.clear_hover = false;
            app.clear_pressed = false;
            app.export_hover = false;
            app.export_pressed = false;
            app.add_hover = false;
            app.add_pressed = false;
            app.convert_hover = false;
            app.convert_pressed = false;
            app.panel_hover = None;
            app.panel_pressed = None;
            app.invalidate();
            Some(LRESULT(0))
        }
        WM_LBUTTONDOWN => {
            unsafe {
                let _ = SetFocus(Some(hwnd));
            }
            let (x, y) = app.dips(lo(lparam), hi(lparam));
            app.mouse_down(x, y, wparam.0 as u32);
            Some(LRESULT(0))
        }
        // The second click of a double-click: on a held row it opens the
        // result; anywhere else it's a click like any other.
        WM_LBUTTONDBLCLK => {
            unsafe {
                let _ = SetFocus(Some(hwnd));
            }
            let (x, y) = app.dips(lo(lparam), hi(lparam));
            let keys = wparam.0 as u32;
            // With Ctrl or Shift it's a second selection click, not an open.
            // Unmodified, a link acted on the first click; a second would
            // open it twice.
            let modified = keys & (MK_SHIFT_FLAG | MK_CONTROL_FLAG) != 0;
            // An inspector control acted on the first click; a second
            // could act again on whatever took its place.
            let inspector = app.on_inspector(x, y);
            if !inspector && (modified || (!app.open_held_at(x, y) && !app.on_link(x, y))) {
                app.mouse_down(x, y, keys);
            }
            Some(LRESULT(0))
        }
        WM_LBUTTONUP => {
            let (x, y) = app.dips(lo(lparam), hi(lparam));
            app.mouse_up(x, y);
            Some(LRESULT(0))
        }
        WM_MOUSEWHEEL => {
            let raw = ((wparam.0 >> 16) & 0xFFFF) as i16 as i32;
            if app.details.is_some() {
                // In proportion, as the list scrolls: a precise touchpad
                // sends many small steps of less than a notch.
                app.scroll_details(TextScroll::Wheel(raw as f32 / WHEEL_DELTA as f32));
            } else if app.help.is_some() {
                app.scroll_help(TextScroll::Wheel(raw as f32 / WHEEL_DELTA as f32));
            } else if app.over().list_live() {
                let delta = raw as f32 / WHEEL_DELTA as f32;
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
            if !app.over().list_live() {
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
        // A contrast theme, Animation effects or Text size may have
        // changed: each is cheap to read again. Text size also has its own
        // event (WM_KILN_TEXT_SCALE); this catches it if that never comes.
        WM_SETTINGCHANGE => {
            let name = lparam.0 as *const u16;
            if !name.is_null()
                && unsafe { PCWSTR(name).to_string() }.ok().as_deref() == Some("ImmersiveColorSet")
            {
                app.system_light = system::system_light();
            }
            app.display_settings_changed();
            app.text_scale_changed();
            None
        }
        WM_THEMECHANGED | WM_SYSCOLORCHANGE => {
            app.display_settings_changed();
            None
        }
        WM_KILN_TEXT_SCALE => {
            // The posted box holds nothing, but it's still ours to free.
            drop(unsafe { Box::from_raw(lparam.0 as *mut ()) });
            app.text_scale_changed();
            Some(LRESULT(0))
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
        // The finish notice: a click opens Kiln on the result; anything
        // else means it has gone.
        WM_KILN_NOTIFY => {
            match (lparam.0 & 0xFFFF) as u32 {
                NIN_BALLOONUSERCLICK => {
                    system::notify_done(hwnd);
                    system::bring_forward(hwnd);
                    if let Some(i) = app.notice_row.take().and_then(|p| app.files.index_of(&p)) {
                        app.files.click(i);
                        app.scroll_to_row(i);
                        app.invalidate();
                    }
                }
                NIN_BALLOONTIMEOUT | NIN_BALLOONHIDE => system::notify_done(hwnd),
                _ => {}
            }
            Some(LRESULT(0))
        }
        WM_KILN_EXPORTED => {
            let ex = unsafe { Box::from_raw(lparam.0 as *mut Exported) };
            app.on_exported(*ex);
            Some(LRESULT(0))
        }
        WM_KILN_PREVIEW => {
            let arrived = unsafe { Box::from_raw(lparam.0 as *mut (PreviewKey, Preview)) };
            let (key, preview) = *arrived;
            app.on_preview(key, preview);
            Some(LRESULT(0))
        }
        // Kiln's own drag back over Kiln: the drop target ignores it too.
        WM_KILN_DRAG | WM_KILN_DROP if app.own_drag.get() => Some(LRESULT(0)),
        WM_KILN_DRAG => {
            let ev = unsafe { &*(lparam.0 as *const DragEvent) };
            match wparam.0 {
                DRAG_ENTER => {
                    app.drag_paths = ev.paths.clone();
                    app.drag.set(
                        1.0,
                        theme::motion::QUICK_MS,
                        Instant::now(),
                        theme::motion::animations(),
                    );
                    app.animate();
                    app.drag_over(ev);
                }
                DRAG_OVER => app.drag_over(ev),
                DRAG_LEAVE => {
                    app.drag.set(
                        0.0,
                        theme::motion::QUICK_MS,
                        Instant::now(),
                        theme::motion::animations(),
                    );
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
            let news = unsafe { Box::from_raw(lparam.0 as *mut JobNews) };
            app.on_job_news(*news);
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
        WM_QUERYENDSESSION => Some(LRESULT(
            (app.files.held_count_present(|p| p.exists()) == 0) as isize,
        )),
        // UI Automation asks for the window's tree.
        WM_GETOBJECT if lparam.0 as i32 == uia::ROOT_OBJECT_ID => {
            let first = !app.uia.active();
            let root = app.uia.root();
            if first {
                // The tree is simply there for the first client: nothing
                // to announce, not even a status set before it came.
                let sets = std::mem::take(&mut app.status_sets);
                app.teller.tell(sets.into_iter().map(|t| (t, false)), false);
                app.sync_a11y(false);
            }
            Some(uia::answer(hwnd, wparam, lparam, root))
        }
        uia::WM_KILN_A11Y => {
            let req = unsafe { Box::from_raw(lparam.0 as *mut a11y::Request) };
            app.a11y_do(*req);
            Some(LRESULT(0))
        }
        // Moving, sizing and the system menu run modal loops of their own.
        WM_ENTERSIZEMOVE | WM_ENTERMENULOOP => {
            system::window_loop(true);
            None
        }
        WM_EXITSIZEMOVE | WM_EXITMENULOOP => {
            system::window_loop(false);
            None
        }
        WM_DESTROY => {
            app.destroyed = true;
            system::notify_done(hwnd);
            // UI Automation lets go of the tree first.
            app.uia.shutdown(hwnd);
            // Stop the inspect thread with the window, without waiting
            // for a decode under way: nothing is reported after this.
            app.inspector.shutdown();
            // The same for the thumbnail thread: nothing is posted after this.
            app.thumbs.shutdown();
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
    #[test]
    fn a_status_set_reaches_ui_automation_at_once() {
        use super::*;
        // Job progress and animation frames wait their turn...
        assert_eq!(sync_kind(WM_KILN_JOB, 0, false), SyncKind::Soon);
        assert_eq!(sync_kind(WM_TIMER, ANIM_TIMER, false), SyncKind::Soon);
        // ...but not a job that set the status, such as a batch summary.
        assert_eq!(sync_kind(WM_KILN_JOB, 0, true), SyncKind::Now);
        assert_eq!(sync_kind(WM_TIMER, ANIM_TIMER, true), SyncKind::Now);
        // Input and the window's own news go at once.
        assert_eq!(sync_kind(WM_KEYDOWN, 0, false), SyncKind::Now);
        assert_eq!(sync_kind(uia::WM_KILN_A11Y, 0, false), SyncKind::Now);
        // A preview changes nothing the tree shows.
        assert_eq!(sync_kind(WM_KILN_PREVIEW, 0, false), SyncKind::Skip);
        assert_eq!(sync_kind(WM_SETFOCUS, 0, false), SyncKind::Now);
        // Painting never builds the tree; the flush timer builds its own.
        assert_eq!(sync_kind(WM_PAINT, 0, false), SyncKind::Skip);
        assert_eq!(sync_kind(WM_TIMER, A11Y_TIMER, false), SyncKind::Skip);
    }

    #[test]
    fn help_covers_the_list_like_any_panel() {
        use super::Over;
        // Help alone, or over a fading wheel, is a panel: the list beneath
        // takes no clicks, links or tooltips, and a drag pours no wheel.
        for wheel in [false, true] {
            let help = Over::of(wheel, false, false, false, true);
            assert_eq!(help, Over::Panel);
            assert!(!help.list_live() && !help.wheel_may_pour());
        }
        for (sheet, settings, details) in [
            (true, false, false),
            (false, true, false),
            (false, false, true),
        ] {
            assert_eq!(
                Over::of(false, sheet, settings, details, false),
                Over::Panel
            );
        }
        // A wheel keeps the list's clicks but lets a drag change it.
        let wheel = Over::of(true, false, false, false, false);
        assert!(!wheel.list_live() && wheel.wheel_may_pour());
        let nothing = Over::of(false, false, false, false, false);
        assert!(nothing.list_live() && nothing.wheel_may_pour());
    }

    use super::*;

    #[cfg(windows)]
    #[test]
    fn no_export_goes_into_any_holding_folder() {
        let root = Path::new(r"C:\Users\a\AppData\Local\Kiln\held");
        // Another run's folder, or the root itself, is swept at launch.
        assert!(inside(&root.join("99-1"), root));
        assert!(inside(root, root));
        // Windows doesn't care about case, so neither does the check.
        assert!(inside(
            Path::new(r"c:\users\A\appdata\local\kiln\HELD\99-1"),
            root
        ));
        assert!(!inside(Path::new(r"C:\Users\a\AppData\Local\Kiln"), root));
        assert!(!inside(
            Path::new(r"C:\Users\a\AppData\Local\Kiln\held2"),
            root
        ));
    }

    #[test]
    fn an_export_that_panics_reports_every_file() {
        let files: Vec<PathBuf> = ["a.jpg", "b.jpg", "c.jpg"]
            .into_iter()
            .map(PathBuf::from)
            .collect();
        // a.jpg went out before the panic and still counts as saved; the
        // rest count as not saved, so no row is left saying "Saving…".
        let results = export_each(files.clone(), |f| {
            if f == Path::new("b.jpg") {
                panic!("export worker test panic");
            }
            Some(Path::new("out").join(f))
        });
        assert_eq!(
            results,
            [
                (files[0].clone(), Some(PathBuf::from("out/a.jpg"))),
                (files[1].clone(), None),
                (files[2].clone(), None),
            ]
        );
        // Without a panic, each file reports as it went.
        let results = export_each(files.clone(), |f| {
            (f != Path::new("c.jpg")).then(|| f.into())
        });
        assert_eq!(
            results,
            [
                (files[0].clone(), Some(files[0].clone())),
                (files[1].clone(), Some(files[1].clone())),
                (files[2].clone(), None),
            ]
        );
        assert!(export_each(vec![], |_| None).is_empty());
    }

    #[test]
    fn a_clear_that_exports_first_clears_only_the_rows_it_asked_about() {
        let listed = vec![PathBuf::from("a.png"), PathBuf::from("b.png")];
        let (rows, then) = pin(Rows::All, Then::Clear, listed.clone());
        assert!(matches!(&rows, Rows::Files(f) if *f == listed));
        assert!(matches!(&then, Then::ClearOnly(f) if *f == listed));
        // Close and Remove keep their own rows.
        let (rows, then) = pin(Rows::All, Then::Close, listed.clone());
        assert!(matches!((rows, then), (Rows::All, Then::Close)));
        let one = vec![PathBuf::from("a.png")];
        let (rows, then) = pin(Rows::Files(one.clone()), Then::Remove(one.clone()), listed);
        assert!(matches!(&rows, Rows::Files(f) if *f == one));
        assert!(matches!(&then, Then::Remove(f) if *f == one));
    }

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
