//! The Win32 window: creation, the message loop, input and animation.

mod arrive;
use arrive::{Incoming, Walked};
use jobs::Jobs;
use prompts::Prompts;
use wndproc::*;
mod cell;
mod channel;
pub mod crash;

mod drop;
mod effects;
pub mod float_wheel;
#[cfg(test)]
mod flow_tests;
mod frame_log;
mod handoff;
mod held;
pub mod hooks;
mod inspect;
mod jobs;
mod motion;
mod placement;
mod prefs;
mod prompts;
mod status;
mod surfaces;
use surfaces::{
    look, page, panel_file, panel_folder, CardsFound, Ctx, Details, Frame, Help, Input,
    InspectorUi, Layer, ListUi, Mods, Opening, Probed, Reply, SettingsPanel, Sheet, Surface, Tick,
    WheelOpening, WheelUi, WindowButton,
};
mod render;
mod system;
mod text;
mod thumbs;
mod tips;
mod titlebar;
mod toast;
pub mod tray;
pub mod tray_menu;
mod uia;
mod updates;
mod wndproc;
mod workers;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows::core::{w, Result, BOOL, HSTRING, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_CLOAK};
use windows::Win32::Graphics::Gdi::{
    GetUpdateRect, InvalidateRect, ScreenToClient, UpdateWindow, ValidateRect,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::{
    IDropTarget, OleInitialize, OleUninitialize, RegisterDragDrop, RevokeDragDrop,
};
use windows::Win32::System::Threading::INFINITE;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::{
    GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use kiln_core::{
    suggest, Availability, CancelToken, Facts, Format, JobError, JobEvent, JobId, JobKind, Kiln,
    Knob, MediaInfo, Offered, Options, Places, Progress, Quality, Runner, Suggestion, Tool,
    ToolOptions,
};

use crate::a11y;
use crate::actions;
use crate::anim::{self, Tween};
use crate::autostart::{self, Autostart, Launch, RunKey};
use crate::diag;
use crate::drag_watch::Verdict;
use crate::help::{self, HelpSection};
use crate::inspector::{self as insp, InspectorControl};
use crate::layout::{
    self, Control, DialogButton, HelpButton, SettingsLayout, SheetControl, SheetLayout, TextScroll,
    ROW_HEIGHT,
};
use crate::model::{
    self, batch_summary, leave_prompt, status_line, Due, FileList, FocusButton, JobState, Leave,
    PromptButton, Tone,
};
use crate::previews::{self, Preview, PreviewCache, PreviewKey, PreviewSize, CACHE_CAP};
use crate::quality::{self as q, Value};
use crate::settings::{self, Engine, Settings};
use crate::settings_page;
use crate::theme::fluent::FluentPalette;
use crate::theme::{self};
use crate::tune_sheet::{SheetKind, TuneField};
use crate::update::UpdateState;
use crate::ways_in::{self, MenuItem, RowState};
use crate::wheel::{self, Action, Origin, Picked, Pour, Spot, Wheel, WheelKind};
use cell::AppCell;
use channel::{Event, Inbox, Wake, WM_KILN_WAKE};
use drop::{DragEvent, DragKind, DropTarget};
use effects::{run_effects, Effect, Effects, Place};
use float_wheel::{FloatNews, FloatNote, FloatWheel};
use held::{Held, Note, Notes};
use hooks::{Hooks, WM_KILN_HOOK};
use inspect::Inspector;
use motion::{new_content, same_page, sheet_rise, ButtonId, Fading, Motion, Shows};
use prefs::Prefs;
use render::{
    AskView, DetailsView, HelpView, InspectorPage, InspectorView, Renderer, SettingsView,
    SheetView, TitleView, View, WheelView,
};
use status::Status;
use thumbs::Thumbs;
use tips::Tips;
use titlebar::Part;
use tray::{Closing, Tray, TrayCommand, TrayEvent, WM_KILN_TRAY};

/// Sent to the window by the message loop to draw one frame: when the
/// display can take one and every pending message is handled, or on the
/// progress clock's schedule (`run_frames`). A frame is a step of the animations
/// (`App::tick`) and a paint. Its `wparam` and `lparam` are never read.
const WM_KILN_FRAME: u32 = WM_APP + 22;
/// What the footer says while a close waits for a save to finish.
const CLOSING_WHEN_SAVED: &str = "Saving… Kiln will close when it's done.";
/// Tooltip ids. Rows take three each from `TIP_ROWS`: the row itself, its
/// cancel cross and its Export link.
const TIP_SETTINGS: usize = 1;
const TIP_HELP: usize = 2;
const TIP_ADD: usize = 3;
/// The side pane's Folder link: the whole path of the folder.
const TIP_FOLDER: usize = 4;

const TIP_CONVERT: usize = 5;
/// The side pane's title, when a long file name is cut: the whole name.
const TIP_TITLE: usize = 6;
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
/// Puts the tray icon back a second time when Explorer, just restarted,
/// didn't take it the first.
const TRAY_TIMER: usize = 6;
const TRAY_RETRY_MS: u32 = 2000;
/// A button came up over a lit slice of the drag-wheel: it closes once
/// the drop has been heard, or after this long if none comes.
const FLOAT_TIMER: usize = 7;
const FLOAT_DROP_MS: u32 = 500;
/// Draws the window's frames while a modal loop runs (a dialog's, a
/// picker's, a menu's, a drag's, or moving and sizing the window), which
/// the message loop's frame clock can't: every `MODAL_FRAME_MS` while
/// something moves, every `anim::PROGRESS_FRAME_MS` while only a running
/// job's progress moves (`App::modal_frames`). The message loop stops it when it gets
/// it, being back in charge.
const MODAL_FRAME_TIMER: usize = 8;
/// Fires once, `ENGINES_WAIT_MS` after the start: if the engines thread
/// hasn't answered by then (a search of PATH stuck on a drive that is
/// gone, say), rows, wheels and menus wait for it no longer.
const ENGINES_TIMER: usize = 9;
const ENGINES_WAIT_MS: u32 = 2000;

/// How long after one activation's look for helper apps the next
/// activation looks again.
const LOOK_ON_ACTIVATE: Duration = Duration::from_secs(5);
/// Fires once, `STALE_WAIT_MS` after the start: if the look for what
/// crashed or killed runs left hasn't answered by then (a drive spinning
/// up, say), files that arrive wait for it no longer. Its prompt still
/// comes when it answers.
const STALE_TIMER: usize = 10;
const STALE_WAIT_MS: u32 = 2000;
/// Fires once files that arrived by Open with, Send to, a second launch
/// or Add files have stopped arriving, to open the format choice.
const ARRIVALS_TIMER: usize = 11;
/// How soon it looks again while something is in the way: a dialog, or
/// a folder still being listed.
const ARRIVALS_RETRY_MS: u32 = 100;
/// Windows' shortest timer: a frame is drawn at the first tick that finds
/// the swap chain ready for it.
const MODAL_FRAME_MS: u32 = USER_TIMER_MINIMUM;
/// Added to the hint timer's wait, so it fires after the two seconds.
const HINT_SLACK: Duration = Duration::from_millis(50);

// Worker threads, UI Automation and the floating wheel reach the window
// through `channel`, never with a pointer in a message. The `WM_APP`
// messages left carry plain values only. `WM_APP + 1` to `+ 3`, `+ 5` to
// `+ 10`, `+ 12` to `+ 14` and `+ 17` to `+ 19` once carried pointers, and
// are no longer answered; `+ 15` and `+ 16` are the hooks' and the tray's
// (`WM_KILN_HOOK`, `WM_KILN_TRAY`), `+ 21` is `channel::WM_KILN_WAKE`, and
// `+ 22` is `WM_KILN_FRAME`.

/// Posted once by `run_window` when the window is up with its settings,
/// its hooks and the files it started with. What a crashed run left is
/// offered only from then (`App::offer_recovery`). Its `wparam` and
/// `lparam` are never read.
const WM_KILN_STARTED: u32 = WM_APP + 20;
/// The notification area's message for Kiln's finish notice: lparam's low
/// word says what happened to it. Only that number is read: no `lparam`
/// is dereferenced.
const WM_KILN_NOTIFY: u32 = WM_APP + 11;
/// The notice was clicked, timed out, or was hidden.
const NIN_BALLOONHIDE: u32 = 0x403;
const NIN_BALLOONTIMEOUT: u32 = 0x404;
const NIN_BALLOONUSERCLICK: u32 = 0x405;
/// How long a row's second line takes to cross over to what Kiln found.
const FACTS_MS: u32 = 160;
/// Drag modifier flags (MK_SHIFT, MK_CONTROL).
const MK_SHIFT_FLAG: u32 = 0x0004;
const MK_CONTROL_FLAG: u32 = 0x0008;
/// The left button is down (a mouse message's MK_LBUTTON).
const MK_LBUTTON_FLAG: u32 = 0x0001;

/// The clock indeterminate progress bars run on, in ms: every bar shares
/// it, so they move together. `None` with Animation effects off, when
/// they stand still.
fn progress_clock() -> Option<f32> {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let ms = EPOCH.get_or_init(Instant::now).elapsed().as_secs_f32() * 1000.0;
    theme::fluent::motion::animations().then_some(ms)
}

/// The Store extensions whose decoders a look found installed now ("HEVC").
struct ExtensionsLooked {
    installed: Vec<&'static str>,
}

/// Media Foundation's codecs before a look and what the look found.
struct MfLooked {
    before: kiln_media::MfCodecs,
    found: kiln_media::MfCodecs,
}

/// Said when Tools is asked for on a file that looks cut short.
pub(crate) const CUT_SHORT_NO_TOOLS: &str = "Tools can't run on a file that looks cut short.";

/// What the inspect thread found in one listed file.
struct Inspected {
    path: PathBuf,
    facts: Facts,
}

/// What an export thread did, for `Event::Exported`.
struct Exported {
    id: u64,
    folder: PathBuf,
    /// Each held file and where it went, or `None` where it couldn't be
    /// saved.
    results: Vec<(PathBuf, Option<PathBuf>)>,
    /// Those of them a cancel left held, whole: not saved, and not failed.
    cancelled: Vec<PathBuf>,
}

/// How far an export thread has copied a held file to another drive, for
/// `Event::Copying`: sent a few times a second at most (`CopyPace`).
struct Copying {
    id: u64,
    file: PathBuf,
    fraction: f32,
}

/// How often a copy's progress reaches the window, at most.
const COPY_PROGRESS_EVERY: Duration = Duration::from_millis(100);

/// When a copy's progress is worth sending: once the percent it shows has
/// changed, and no sooner than `COPY_PROGRESS_EVERY` after the last,
/// except the end. One per file.
#[derive(Default)]
struct CopyPace {
    last: Option<(Instant, u32)>,
}

impl CopyPace {
    /// The fraction to send now, `done` of `total` bytes in, if any.
    fn due(&mut self, now: Instant, done: u64, total: u64) -> Option<f32> {
        if total == 0 {
            return None;
        }
        let fraction = (done as f64 / total as f64).clamp(0.0, 1.0);
        let percent = (fraction * 100.0) as u32;
        if let Some((at, shown)) = self.last {
            let soon = now.saturating_duration_since(at) < COPY_PROGRESS_EVERY;
            if shown == percent || (soon && done < total) {
                return None;
            }
        }
        self.last = Some((now, percent));
        Some(fraction as f32)
    }
}

/// A job waiting for its source's holding folder to be opened, which a
/// worker does (`Held::open_for`): started as asked once it is.
struct PendingLaunch {
    action: Action,
    paths: Vec<PathBuf>,
    quality: Option<Quality>,
    origin: Origin,
    via: Via,
    /// What a drag-wheel drop's notice says of the files it left out
    /// (`Batch::left`).
    left: Vec<String>,
    /// A tool's options, as its dialog chose them.
    tool: Option<ToolOptions>,
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

/// Move each of the held `files` to `dest`, one at a time, as an export
/// does: where each went, or `None` where it couldn't be saved; and those
/// `cancel` left held. Each that went is cleared from its run folder's
/// list of sources, through `left`. A move to another drive is a copy, whose progress goes
/// to `progress` as the file, bytes done and bytes in all; cancelled, the
/// held file stays whole and only Kiln's temp copy goes. Once cancelled,
/// the files not yet started stay held too.
fn move_out_with(
    files: Vec<PathBuf>,
    (dest, fallback): (&Dest, Option<&Path>),
    progress: &mut dyn FnMut(&Path, u64, u64),
    cancel: &CancelToken,
    left: &dyn Fn(&Path, &Path),
) -> (Vec<(PathBuf, Option<PathBuf>)>, Vec<PathBuf>) {
    let mut cancelled = vec![];
    let results = export_each(files, |f| {
        let mut each = |done: u64, total: u64| progress(f, done, total);
        let moved = match dest {
            // Checked before the held copy goes; where it can't go, into
            // Documents\Kiln results.
            Dest::Into(dir) => {
                match kiln_core::deliver_with(f, Some(dir), fallback, &mut each, cancel) {
                    kiln_core::Delivery::Kept if cancel.is_cancelled() => {
                        Err(kiln_core::Error::Cancelled)
                    }
                    kiln_core::Delivery::Kept => Err(kiln_core::Error::Io {
                        path: dir.clone(),
                        source: std::io::Error::other("couldn't be saved"),
                    }),
                    d => Ok(d.path().map(Path::to_path_buf).unwrap_or_default()),
                }
            }
        };
        match moved {
            Ok(to) => {
                if let Some(dir) = f.parent() {
                    left(dir, f);
                }
                Some(to)
            }
            Err(kiln_core::Error::Cancelled) => {
                cancelled.push(f.to_path_buf());
                None
            }
            Err(_) => None,
        }
    });
    (results, cancelled)
}

/// Move each of the held `files` in the run folder `held` to `dest`, one
/// at a time, as an export does: where each went, or `None` where it
/// couldn't be saved. Each that went is cleared from `held`'s list of
/// sources; a file not in `held` isn't moved.
#[cfg(test)]
fn move_out(files: Vec<PathBuf>, dest: &Dest, held: &Path) -> Vec<(PathBuf, Option<PathBuf>)> {
    let (mine, others): (Vec<_>, Vec<_>) =
        files.into_iter().partition(|f| f.parent() == Some(held));
    let (mut results, _) = move_out_with(
        mine,
        (dest, None),
        &mut |_, _, _| {},
        &CancelToken::new(),
        &|dir, f| left_holding(dir, f),
    );
    results.extend(others.into_iter().map(|f| (f, None)));
    results
}

/// Note that the held `file` has left the holding folder `dir`, so a later
/// result of its name, that Kiln ends before noting, isn't saved next to
/// this one's original (`kiln_core::forget_source`).
#[cfg(test)]
fn left_holding(dir: &Path, file: &Path) {
    if let Err(e) = kiln_core::forget_source(dir, file) {
        log(&format!("couldn't note that {} left: {e}", file.display()));
    }
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
    /// A save next to the original as the result was ready, and the
    /// notice that waits for it (`App::save_beside`).
    beside: Option<Feeds>,
    /// Stops a copy to another drive, leaving the held result as it was.
    cancel: CancelToken,
    /// Its thread, which the end of a session waits on a moment, so a
    /// cancelled copy can take its temp file away.
    thread: Option<std::thread::JoinHandle<()>>,
}

/// Which notification a save next to the original tells about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Feeds {
    /// The window's finish notice (`App::finish_due`).
    Finish,
    /// A drag-wheel drop's, by its batch (`Batch::drag_notice`).
    Drop(u64),
}

/// Where a delivery sends its files.
enum Dest {
    /// Into a folder, never over a file that's there.
    Into(PathBuf),
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
    /// Hide the window to the notification area, as closing does while
    /// the drag-wheel is on (`App::hide_to_tray`). Jobs run on meanwhile,
    /// and nothing is asked.
    Hide,
    Clear,
    Remove(Vec<PathBuf>),
    /// Start a new job on rows, which replaces what they hold, as `Via`
    /// says it came about (`App::start_as`).
    Start(
        Action,
        Vec<PathBuf>,
        Option<Quality>,
        Via,
        Option<Box<ToolOptions>>,
    ),
}

/// A leave waiting on exports already moving its rows' results.
struct Leaving {
    /// The exports it waits on. It goes ahead once they're all done.
    waits: HashSet<u64>,
    rows: Rows,
    then: Then,
}

/// Whether the held result `p` is there, as the window can say without
/// waiting on another drive: one in the local holding folder,
/// `held`'s in `%LOCALAPPDATA%`, is looked at; any other, on a drive that
/// may be slow to answer, is there unless the gone thread's latest look
/// found it gone (`gone`). That look is asked again on each action that
/// depends on it (`App::ask_gone`), and its answer corrects the rows.
fn present_in<'a>(held: Option<&'a Held>, gone: &'a GoneLooks) -> impl Fn(&Path) -> bool + 'a {
    move |p: &Path| match held {
        Some(h) if p.parent() == Some(h.local()) => p.exists(),
        _ => !gone.is_gone(p),
    }
}

/// What the gone thread's looks found, for the held results on other
/// drives (`present_in`). A result is gone only by a look asked after it
/// was written: a redo can give its new result the name of one removed
/// by hand, or a result put back from the Recycle Bin can be written
/// over by one, and an older look must never hide it from a leave's
/// prompt. Asking about a result that went is harmless; skipping one that
/// is there is not.
#[derive(Debug, Default)]
struct GoneLooks {
    /// How many looks have been asked: each is numbered by this count.
    asked: u64,
    /// The look whose answer stands, by number. An answer to an older
    /// one changes nothing.
    answered: u64,
    /// What that look found gone.
    gone: HashSet<PathBuf>,
    /// Results written since that look was asked, each with how many
    /// looks had been asked by then: only a later look speaks for it.
    written: HashMap<PathBuf, u64>,
}

impl GoneLooks {
    /// A new look is asked: its number.
    fn ask(&mut self) -> u64 {
        self.asked += 1;
        self.asked
    }

    /// The look numbered `ask` found `gone`. True if its answer stands:
    /// it is no older than the one that does.
    fn found(&mut self, ask: u64, gone: HashSet<PathBuf>) -> bool {
        if ask < self.answered {
            return false;
        }
        self.answered = ask;
        self.gone = gone;
        // A look asked after a result was written speaks for it from now
        // on, and so does every later one.
        self.written.retain(|_, looks| *looks >= ask);
        true
    }

    /// A row has just been given `paths`, results newly written: no look
    /// asked before now says any of them is gone.
    fn held_anew(&mut self, paths: &[PathBuf]) {
        for p in paths {
            self.written.insert(p.clone(), self.asked);
        }
    }

    /// Whether the look that stands found `p` gone, and was asked after
    /// `p` was last written.
    fn is_gone(&self, p: &Path) -> bool {
        self.gone.contains(p)
            && self
                .written
                .get(p)
                .is_none_or(|&looks| self.answered > looks)
    }

    /// What the look that stands found gone, as `is_gone` counts it.
    fn gone_now(&self) -> HashSet<PathBuf> {
        self.gone
            .iter()
            .filter(|p| self.is_gone(p))
            .cloned()
            .collect()
    }
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
    fn of(layers: &[Layer]) -> Self {
        match layers.first() {
            Some(Layer::Wheel) => Over::Wheel,
            Some(Layer::List) | None => Over::Nothing,
            Some(_) => Over::Panel,
        }
    }

    /// Whether the list beneath takes clicks, double-clicks, links, the
    /// right-click menu, scrolling and tooltips.
    fn list_live(self) -> bool {
        self == Over::Nothing
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
    /// Where the side pane's Folder link is.
    folder: Option<layout::Rect>,
    /// Where the side pane's title is, when its name is cut.
    title: Option<layout::Rect>,
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

struct JobInfo {
    paths: Vec<PathBuf>,
    action: Action,
    batch: u64,
    /// What the conversion leaves out, as the job sent it (`JobEvent::Notes`)
    /// before it started.
    drops: Vec<String>,
    /// A sentence the job sent for its details ("Encoded on the
    /// processor."), for its rows' finished line.
    note: Option<&'static str>,

    /// The run folder its results are written to: the one for its first
    /// file's drive. `None` without holding folders, when they are written
    /// next to their files.
    out_dir: Option<PathBuf>,
}

/// How a job a person started came about, for what its rows offer once
/// done and where its results go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Via {
    /// Chosen in Kiln's window.
    Window,
    /// Dropped on the drag-wheel, even when Kiln's window told it because
    /// its rows held results: its rows offer Options once done, as
    /// any drop's do.
    Drop,
    /// Options picked on the drag-wheel, then converted from the sheet.
    DropSheet,
}

/// Jobs started by one choice on the wheel, for the status line.
struct Batch {
    action: Action,
    /// Over Kiln's window, or a drop on the drag-wheel, which tells how
    /// it went in one notification of its own once its last job is done.
    origin: Origin,
    /// Each finished job's row, the file's name as the notification says
    /// it, and the row's reason if it failed. Cancelled jobs aren't here.
    finished: Vec<(PathBuf, String, Option<String>)>,
    /// What each job that converted left out (`JobInfo::drops`), for the
    /// drop's notice.
    drops: Vec<Vec<String>>,
    /// What the drop's notice adds for the files it left out of the
    /// conversion (`wheel::left_out_lines`): their rows say so too, but
    /// the window may be hidden.
    left: Vec<String>,
    files: usize,
    ok: usize,
    failed: usize,
    cancelled: usize,
    /// How many jobs the batch started, and how many are still to finish.
    groups: usize,
    pending: usize,
    /// The saves next to the original of its results still under way, by
    /// export id: a drop's notice waits for them.
    pending_saves: HashSet<u64>,
    /// Where those that are in went.
    saves: model::Saves,
}

impl Batch {
    /// What a drag-wheel drop's batch says once its last job is in and its
    /// results are saved next to their originals, and the row a click on
    /// it opens: the one it names, or the first that failed. With nothing
    /// failed, the drop's sentence ("Converted photo.png to WEBP.") heads
    /// where the results went; failures read as they did. What every file
    /// that converted left out follows in the text (`model::with_drops`),
    /// then the files the drop left out (`left`). A batch chosen over
    /// Kiln's window says nothing here.
    fn drag_notice(&self) -> Option<(String, String, PathBuf)> {
        if self.origin != Origin::Drag {
            return None;
        }
        let jobs: Vec<(String, Option<String>)> = self
            .finished
            .iter()
            .map(|(_, name, failed)| (name.clone(), failed.clone()))
            .collect();
        let (title, text, i) = model::drag_notice(&self.action, &jobs)?;
        let named = self.finished.get(i)?.0.clone();
        // What the converted files lose, then the files left out.
        let mut drops = model::shared_notes(&self.drops);
        drops.extend(self.left.iter().cloned());
        Some(match self.saves.notice() {
            // One it couldn't save opens on that one's row.
            Some(went) if !self.any_failed() => {
                let row = self.saves.opens().map_or(named, Path::to_path_buf);
                (text, model::with_drops(&went, &drops), row)
            }
            _ => (title, model::with_drops(&text, &drops), named),
        })
    }

    /// Whether a job of the batch failed.
    fn any_failed(&self) -> bool {
        self.finished.iter().any(|(_, _, failed)| failed.is_some())
    }

    /// Whether a click on the drop's notice shows its results where they
    /// were saved: none failed, and every one is next to its original.
    fn all_saved(&self) -> bool {
        !self.any_failed() && self.saves.all_saved()
    }
}

/// The drag-wheel's notice for a drop that left every file out: what
/// `left` says of them, opening the first one's row. `None` when there is
/// nothing to say (a choice over the window, whose rows say it).
fn left_alone_notice(split: &wheel::Split, left: &[String]) -> Option<(String, String, PathBuf)> {
    let row = split
        .already
        .first()
        .or(split.cant.first())
        .or(split.needs.first().map(|(p, _)| p))?;
    if left.is_empty() {
        return None;
    }
    Some(("Kiln".to_owned(), left.join(" "), row.clone()))
}

/// What a notification is about, kept by the id its `kiln:` link carries
/// (`App::notices`): the row a click on it opens, and whether it told
/// where its results were saved, so a click, or its Show in folder, shows
/// them there instead.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct NoticeAbout {
    row: Option<PathBuf>,
    saved: bool,
    /// The saved results' one folder, and the results, which Show in
    /// folder shows (Kiln's own results, never anything a link names).
    reveal: Option<(PathBuf, Vec<PathBuf>)>,
}

/// Which engines are installed, as the engines thread last found them:
/// looking searches PATH and the usual install folders, and the first
/// look starts Media Foundation, all too slow for the window's thread.
/// It is asked at the start, after Settings change and on the re-checks
/// (`App::look_for_engines`), and the answer is kept until the next.
#[derive(Default)]
struct Engines {
    /// The latest answer, once one has come.
    latest: Option<Availability>,
    /// What is used before the first answer: nothing found. Rows added
    /// meanwhile are worked out again once it comes.
    none: Availability,
    /// The id of the last look asked, and of the answer kept.
    asked: u64,
    kept: u64,
    /// What waits for the first answer, the latest asked for only: a
    /// wheel or a menu, which would offer nothing without it.
    waiting: Option<Later>,
    /// The first answer took longer than `ENGINES_WAIT_MS`: nothing waits
    /// for it any more, and what is used meanwhile is "nothing found".
    waited_out: bool,
    /// Where each engine was found by the latest look kept, in
    /// `Engine::ALL` order, for Settings' lines: searching PATH is the
    /// engines thread's, never the window's.
    status: Option<[settings::EngineStatus; 4]>,
    /// What the latest look kept found of Microsoft Office, for Settings.
    office: Option<settings::OfficeStatus>,
    /// The chosen folder the latest look kept was asked with, and whether
    /// it was missing: the engines thread looks, never the window's.
    save_to: Option<(PathBuf, bool)>,
    /// Where the latest look kept found `winget.exe`, for Settings'
    /// Install with winget.
    winget: Option<PathBuf>,
}

impl Engines {
    /// The engines found, or none before the first answer.
    fn get(&self) -> &Availability {
        self.latest.as_ref().unwrap_or(&self.none)
    }

    /// Whether an answer has come.
    fn known(&self) -> bool {
        self.latest.is_some()
    }

    /// Whether rows, wheels and menus go ahead: an answer has come, or
    /// the wait for the first was given up.
    fn ready(&self) -> bool {
        self.known() || self.waited_out
    }

    /// The id for a new look.
    fn ask(&mut self) -> u64 {
        self.asked += 1;
        self.asked
    }

    /// Keep look `id`'s answer, unless a later one is kept already. True
    /// when what is kept changed.
    fn found(&mut self, id: u64, available: Availability) -> bool {
        if id < self.kept {
            return false;
        }
        self.kept = id;
        if self.latest.as_ref() == Some(&available) {
            return false;
        }
        self.latest = Some(available);
        true
    }
}

/// What the estimate thread finds for `a`: `None` when it was stopped
/// for another choice, as there is nothing to tell.
fn estimate_of(kiln: &Kiln, a: &workers::EstimateAsk) -> Option<crate::smaller::Estimate> {
    use crate::smaller::Estimate;
    if a.cancel.is_cancelled() {
        return None;
    }
    let found = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        kiln.smaller_size(&a.path, &a.options, &a.cancel)
    }));
    Some(match found {
        Ok(Ok(n)) => Estimate::About(n),
        Ok(Err(kiln_core::Error::OutOfReach {
            max_bytes,
            smallest,
        })) => Estimate::OutOfReach {
            max_bytes,
            smallest,
        },
        Ok(Err(kiln_core::Error::NothingToDo(said))) => Estimate::Already(said),
        // Stopped for another: nothing to tell.
        Ok(Err(kiln_core::Error::Cancelled)) => return None,
        Ok(Err(_)) | Err(_) => Estimate::Failed,
    })
}

/// What runs later with a fresh borrow of `App`.
type Later = Box<dyn FnOnce(&mut App)>;

/// A picture painted in a frame: its row and size, its pixels, its key's
/// id and how far it has faded in.
type Painted = (usize, PreviewSize, Arc<previews::Pixels>, u64, f32);

/// The kind of file `e` is, by its type; `None` for a type Kiln has no
/// kind for.
fn kind_of(kiln: &Kiln, e: &model::Entry) -> Option<kiln_core::Kind> {
    kiln.registry().format(&e.code).map(|f| f.kind)
}

/// `App::no_common_format`'s last answer, with the list revision, the engines'
/// answer and whether their wait was given up that it was worked out from.
type NoCommon = ((u64, u64, bool), Option<String>);

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
    /// The way to this window, cloned for each worker and for the
    /// floating wheel.
    wake: Wake,
    /// What the workers sent, read on `WM_KILN_WAKE`. Events still in it
    /// when `App` goes are dropped with it.
    inbox: Inbox,
    /// Events taken off the channel and not yet handled, oldest first:
    /// those a wake is working through, and those left by a look for
    /// only some kinds (`take_events`).
    later: VecDeque<Event>,
    /// The strongest tree update the events handled for this message
    /// need (`event_sync`), for `after`.
    woke: SyncKind,
    kiln: Arc<Kiln>,
    /// Jobs and their results: what runs, what is waiting to run, the
    /// tally for the finish notice, and the hand-off of what earlier runs
    /// left (`jobs`).
    work: Jobs,
    /// Looks inside listed files for suggestions, one at a time.
    inspector: Inspector,
    /// The engines suggestions are worked out with.
    engines: Engines,
    /// The thread that looks for engines, and for Media Foundation's
    /// codecs again.
    engines_thread: workers::Engines,
    /// The thread that looks for held results removed by hand.
    gone_thread: workers::Gone,
    /// What its looks found gone: what the window goes by for held
    /// results outside the local holding folder (`present_in`).
    gone_seen: GoneLooks,
    /// Files coming in: walks of dropped paths, arrivals, and what waits
    /// for the list to be in view (`arrive`).
    incoming: Incoming,
    /// Settings, the theme and the palette and backdrop they make.
    prefs: Prefs,
    /// Windows' Text size, and where it's read from.
    text_scale: f32,
    text_scale_source: system::TextScale,
    /// DIPs per layout unit: the Text size, or less where the screen is
    /// too small for it (`layout::text_zoom`).
    zoom: f32,
    /// Keeps the start-up entry in step with the drag-wheel, off this
    /// thread.
    autostart: Autostart,
    settings_ui: Option<SettingsPanel>,
    /// Where the pointer last was, to ignore the moves Windows reports
    /// when a window appears under a still pointer.
    last_mouse: Option<(i32, i32)>,
    renderer: Renderer,
    files: FileList,
    /// What moves in the window, and each row's times: see `Motion`.
    motion: Motion,
    /// The list's scroll, the row and the header and footer buttons the
    /// pointer is on, and the press on a row.
    list: ListUi,
    drag: Tween,
    /// The InfoBar at the top of the list, while one shows: its title and
    /// message.
    top_info: Option<(String, String)>,
    wheel: Option<WheelUi>,
    /// The right-click menu, while it is up.
    row_menu: Option<surfaces::RowMenu>,
    /// The Menu key's press just closed the menu: the WM_CONTEXTMENU its
    /// release sends doesn't open it again.
    menu_closed_by_key: bool,
    sheet: Option<Sheet>,
    next_sheet: u64,
    /// The details dialog, while it's open.
    details: Option<Details>,
    /// The prompts and dialogs open in the window (`prompts`).
    prompts: Prompts,
    /// Whether the key down being handled is Windows repeating a held key.
    key_repeat: bool,
    /// Tests: the window can't draw, as after the graphics device was lost
    /// again and again (`Losses::given_up`).
    #[cfg(test)]
    draw_given_up_for_test: bool,
    /// Tests: whether the window is out of sight, hidden or minimised.
    #[cfg(test)]
    out_of_sight_for_test: Option<bool>,
    /// The help overlay, while it's open.
    help: Option<Help>,
    /// The native tooltips over the drawn controls, if Windows made them.
    tips: Option<Tips>,
    /// The tooltips' rectangles as last measured, and what they were
    /// measured for.
    tip_cache: Option<(TipKey, Vec<(usize, layout::Rect)>)>,
    /// The footer's text, its results and alerts, and when they expire.
    status: Status,
    update: UpdateState,
    tracking_mouse: bool,
    /// The pointer is tracked leaving Windows' buttons on the title bar
    /// (`WM_NCMOUSELEAVE`).
    tracking_nc: bool,
    /// Back's and Windows' buttons' hover and press on the title bar.
    caption: titlebar::Caption,
    /// Whether the window is the active one: the title bar's text is
    /// secondary while it isn't.
    active: bool,
    dpi: f32,
    /// This run's private folders for finished results, one per drive.
    /// `None` if the local one couldn't be made; results then go next to
    /// each file, as before.
    held: Option<Held>,
    /// Where Kiln holds results and its record of drive roots, if
    /// LOCALAPPDATA is set.
    places: Option<Places>,
    /// How far each held file being copied to another drive is, 0 to 1.
    copying: HashMap<PathBuf, f32>,
    /// Writes what the run notes in its folders, in order, off this thread.
    held_notes: Notes,
    /// The window is up with its settings and first files: see
    /// `WM_KILN_STARTED`.
    started: bool,
    /// The folder the last export in this run went to.
    last_export: Option<PathBuf>,
    /// Lines that stand in for a row's own second line, by the row's file:
    /// an export that failed, or a held result that's gone.
    notes: model::Notes,

    /// Exports under way, by id.
    exports: HashMap<u64, Export>,
    next_export: u64,
    /// A close, clear, remove or new job waiting on exports already
    /// moving its rows' results.
    leaving: Option<Leaving>,
    /// Close was asked for while an export ran: close once it's done.
    close_when_saved: bool,
    /// The window is gone: nothing more goes ahead.
    destroyed: bool,
    /// Windows is signing out or shutting down: the holding folder is
    /// kept, not emptied, when the message loop ends, so the next start
    /// delivers what it holds.
    keep_holding: bool,
    /// Whether this run has said that a helper app is missing, once, so
    /// it isn't said for every file.
    helper_notice: bool,
    /// Looking for the Media Feature Pack again after Get Media Feature Pack.
    mf_recheck: model::MfRecheck,
    /// When activating the window last looked for helper apps.
    looked_on_activate: Option<Instant>,
    /// The selected row that needs a helper app, once it was looked for.
    helper_row_looked: Option<PathBuf>,
    /// Looking for an extension's decoder again after Get it.
    ext_recheck: model::ExtensionRecheck,
    /// Rows just removed, with where they were, so Undo can bring them
    /// back, and what the footer said then (Undo shows while it still does).
    undo: Option<(Vec<(usize, model::Entry)>, String)>,
    /// `no_common_format`'s last answer, and what it was worked out from.
    no_common: RefCell<Option<NoCommon>>,
    /// The fold the last frame was drawn with, and whether it had the
    /// inspector's content (a test reads them).
    #[cfg(test)]
    drawn: (f32, bool),
    /// This run's notifications, by the id their `kiln:` link carries.
    notices: toast::Issued<NoticeAbout>,
    /// The notification last shown as a balloon, which a click on a
    /// balloon opens.
    balloon_notice: Option<u64>,
    /// How this Kiln shows notifications: toasts when installed, else
    /// balloons.
    notify_by: toast::Channel,
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
    /// A UI Automation client moved the focus: the window takes the
    /// keyboard focus once the message is done.
    take_focus: bool,
    /// What this message's handler does once `App` is let go: dialogs,
    /// pickers, menus, drags and calls that send straight back to the
    /// window (`effects`).
    effects: Effects,
    /// The header or footer button Tab moved the keyboard to. `None` is
    /// the list. See `focused_button` for whether it's still there.
    button_focus: Option<FocusButton>,
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
    /// The inspector beside the list: its page, the one it replaced, its
    /// clock and its pointer.
    inspector_ui: InspectorUi,
    /// The drag-wheel's floating wheel while it is open over another app.
    float: Option<FloatWheel>,
    /// The floating wheel last closed, while it fades out
    /// (`FloatWheel::fade_out`). It is no drop target and nothing hears
    /// from it; dropped, its window goes at once.
    float_fading: Option<FloatWheel>,
    /// A button came up over its lit slice: it closes when the drop is
    /// heard, or when `FLOAT_TIMER` says none is coming.
    float_closing: bool,
    /// `CLOCK_TIMER` is set: the inspector shows a converting file's
    /// time left, and the window is on screen.
    clock_timer: bool,
    /// `MODAL_FRAME_TIMER`'s period while it is set: a modal loop runs and
    /// something on screen moves (`modal_frames`).
    modal_frames: Cell<Option<u32>>,
    /// Something other than a running row's progress changed since the
    /// last paint (`invalidate`): the next frame draws the whole window.
    whole: Cell<bool>,
    /// The pace of the last frame drawn (`paint_as`): a frame of the
    /// progress clock draws only the rows when the one before it was the
    /// progress clock's too, so the end of an animation is drawn whole
    /// first.
    last_pace: Cell<anim::Pace>,
    /// The drag-wheel's hooks, while the setting is on and Windows
    /// allowed them.
    hooks: Option<Hooks>,
    /// Windows refused the hooks the last time they were asked for: the
    /// setting stays on, its note says so, and nothing watches.
    hooks_refused: bool,
    /// Kiln's icon in the notification area: there only while the window
    /// is hidden (`tray::wanted`).
    tray: Option<Tray>,
    /// The tray icon's menu while it is open: Kiln's own small window.
    tray_menu: Option<tray_menu::TrayMenu>,
}

/// A job's news as the runner's thread posts it: the event and, for a
/// finished job, its results' sizes, read there and then.
struct JobNews {
    event: JobEvent,
    sizes: Vec<(PathBuf, u64)>,
}

impl JobNews {
    /// What the runner's thread posts for `event`.
    fn on_runner_thread(event: JobEvent) -> Self {
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
        // The menu's labels were measured at the old size.
        self.close_menu();
        let dpi = self.unit_dpi();
        self.renderer.set_zoom(zoom);
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
            let grown = client_place(self.hwnd, self.dpi, (w.max(mw), h.max(mh)));
            self.effect(Effect::Place(grown));
        }
        self.rezoom();
        self.invalidate();
    }

    /// Read the contrast theme and Animation effects again; repaint in
    /// whatever changed.
    fn display_settings_changed(&mut self) {
        theme::fluent::motion::set_animations(system::animations_on());
        // Animation effects off: whatever was easing is where it was going.
        if !theme::fluent::motion::animations() {
            self.motion.settle(Instant::now());
            self.inspector_ui.drop_prev();
        }
        self.prefs.contrast = system::high_contrast();
        self.apply_theme();
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
        self.renderer
            .layout(self.files.len(), self.motion.held_fold())
    }

    /// The window as it is once the fold has got where it is going: what
    /// UI Automation is told, so its tree doesn't move with each frame.
    fn settled_layout(&self) -> layout::Layout {
        self.renderer
            .layout(self.files.len(), self.motion.fold_target())
    }

    /// The title bar as it is drawn now, in layout units: Back on it while
    /// Settings is open, Help and Settings while it isn't.
    fn title_bar(&self) -> titlebar::Bar {
        self.renderer.title_bar(self.settings_ui.is_some())
    }

    /// Whether the window is maximised.
    fn maximized(&self) -> bool {
        // SAFETY: asks only about this thread's own window.
        unsafe { IsZoomed(self.hwnd) }.as_bool()
    }

    /// What a title bar button does. Back closes Settings, as Esc does;
    /// Help and Settings open theirs; Windows' buttons ask the window to
    /// minimise, maximise or restore, or close, as their own would, once
    /// this message is done.
    fn title_act(&mut self, part: Part) {
        let command = match part {
            // Under an open prompt, Settings stays: the prompt is answered
            // first.
            Part::Back => {
                if self.settings_ui.is_some() && self.prompts.ask_ui.is_none() {
                    self.close_settings();
                    self.invalidate();
                }
                return;
            }
            Part::Help => return self.open_help(),
            Part::Settings => return self.open_settings(),
            Part::Min => SC_MINIMIZE,
            Part::Max if self.maximized() => SC_RESTORE,
            Part::Max => SC_MAXIMIZE,
            Part::Close => SC_CLOSE,
        };
        // SAFETY: a plain post to this thread's own window.
        unsafe {
            let _ = PostMessageW(
                Some(self.hwnd),
                WM_SYSCOMMAND,
                WPARAM(command as usize),
                LPARAM(0),
            );
        }
    }

    /// Back under a client point in layout units, if it's there: the only
    /// title bar button the client area's own messages light. Windows'
    /// buttons come as non-client messages, and Help and Settings are the
    /// list's (`list_down`).
    fn back_at(&self, x: f32, y: f32) -> Option<Part> {
        self.title_bar().part_at(x, y).filter(|&p| p == Part::Back)
    }

    /// The footer's parts while files are listed (mock screen 2): the
    /// status, Add files, Save all while results are held, Clear list and
    /// Convert, wide enough for "Convert N files" when several are selected.
    pub(in crate::win) fn footer_row(&self) -> Option<layout::FooterRow> {
        let footer = self.layout().footer?;
        // The accent button is as wide as "Convert to JPG" needs.
        let label = self.convert_label();
        let words = self.renderer.run_width(&label, false).ceil();
        let convert_w = layout::kit::button_width(words, false).max(layout::CONVERT_ONE);
        let row = layout::footer_row_for(&footer, convert_w);
        Some(crate::credit::in_footer(row, self.renderer.credit_widths()))
    }

    /// Where the credit is now: under the footer's status, or in the empty
    /// window's bottom-left corner, where there is room for it.
    pub(in crate::win) fn credit(&self) -> Option<crate::credit::Credit> {
        match self.empty_state() {
            Some(e) => crate::credit::in_empty(&e, self.renderer.credit_widths()),
            None => self.footer_row()?.credit,
        }
    }

    /// Open the author's page from the credit's link. Where it doesn't
    /// open, the status says so.
    pub(in crate::win) fn open_credit(&mut self) {
        self.effect(Effect::Open {
            target: PathBuf::from(crate::credit::PAGE),
            then: Box::new(|app, opened| {
                if !opened {
                    app.alert(crate::credit::DIDNT_OPEN.to_owned());
                }
                app.invalidate();
            }),
        });
    }

    /// What the footer's accent button says: "Convert", or "Convert N
    /// files" for several (UX-05). Never Cancel: the accent doesn't mean
    /// stop.
    fn convert_label(&self) -> String {
        let to = self.footer_target().map(|(ext, _)| ext);
        model::convert_label(self.files.selection().len(), to.as_deref())
    }

    /// Where the Undo link is now, while the footer offers it.
    fn undo_link_rect(&self) -> Option<layout::Rect> {
        self.undo_offer()?;
        layout::undo_link(&self.footer_row()?.status)
    }

    /// The footer's Convert button, while files are listed.
    fn convert_rect(&self) -> Option<layout::Rect> {
        Some(self.footer_row()?.convert)
    }

    /// The footer's Clear list button, while files are listed.
    pub(in crate::win) fn clear_rect(&self) -> Option<layout::Rect> {
        Some(self.footer_row()?.clear)
    }

    /// The selected rows that are converting now.
    fn selected_converting(&self) -> Vec<usize> {
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

    /// The selected rows whose results are being copied to another drive.
    fn selected_copying(&self) -> Vec<usize> {
        self.files
            .selection()
            .into_iter()
            .filter(|&i| self.row_copying(i).is_some())
            .collect()
    }

    /// Whether Space stops the selection: one row is selected and it is
    /// converting, or running a tool (`RowState::Running`).
    fn space_cancels(&self) -> bool {
        matches!(self.files.selection()[..], [i] if self.row_state(i) == RowState::Running)
    }

    /// Why the footer's Convert can't be used on the selection now, if it
    /// can't: nothing is selected, a selected file is converting or its
    /// results are being copied (its row has the Cancel), or the files
    /// share no format.
    fn convert_block(&self) -> Option<String> {
        let selected = self.files.selection().len();
        let running = self.selected_converting().len();
        let copying = self.selected_copying().len();
        if let Some(why) = model::convert_block(selected, running, copying) {
            return Some(why.to_owned());
        }
        self.no_common_format()
    }

    /// The sentence the wheel would give for a selection of several files
    /// that share no format to convert to, kept for as long as the
    /// list and the engines are as they were: asking is for every
    /// frame and every pointer move.
    fn no_common_format(&self) -> Option<String> {
        if self.files.selection().len() < 2 || !self.engines.ready() {
            return None;
        }
        let key = (
            self.files.revision(),
            self.engines.kept,
            self.engines.waited_out,
        );
        if let Some((k, said)) = &*self.no_common.borrow() {
            if *k == key {
                return said.clone();
            }
        }
        let paths = self.selected_paths();
        let said = match wheel::offer(WheelKind::Convert, &self.kiln, self.available(), &paths) {
            offer @ wheel::Offer::Mixed(_) => wheel::offer_sentence(WheelKind::Convert, &offer),
            _ => None,
        };
        *self.no_common.borrow_mut() = Some((key, said.clone()));
        said
    }

    fn invalidate(&self) {
        self.whole.set(true);
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    // Effects: what runs once `App` is let go.

    /// Queue `effect`, to run once this message is answered (`effects`).
    fn effect(&mut self, effect: Effect) {
        self.effects.push(effect);
    }

    /// The UI Automation root, for a client asking for the tree. The
    /// tree is simply there for the first client: nothing to announce,
    /// not even a status set before it came.
    fn uia_root(&mut self) -> windows::Win32::UI::Accessibility::IRawElementProviderSimple {
        let first = !self.uia.active();
        let root = self.uia.root();
        if first {
            self.status.forget_sets();
            self.sync_a11y(false);
        }
        root
    }

    /// Whether the window has gone, or the session is ending: a dialog's
    /// answer that comes back then does nothing, and what's held stays
    /// for the next start.
    fn ending(&self) -> bool {
        self.destroyed || self.keep_holding
    }

    /// Run `f` once the effects queued so far have run: at once when none
    /// is queued. A picker or menu ahead of it has closed by then, but a
    /// prompt (`Effect::Ask`) opens inside the window and doesn't hold
    /// back what follows: `f` runs while it is still open, unanswered.
    /// Work that needs the answer goes in the prompt's `then`, never here.
    /// (No check stops a prompt queued ahead: `open_window` runs here
    /// with a waiting prompt still queued, and needs no answer.)
    fn afterwards(&mut self, f: impl FnOnce(&mut App) + 'static) {
        if self.effects.is_empty() {
            f(self);
        } else {
            self.effect(Effect::Run(Box::new(f)));
        }
    }

    /// Something began to move, or changed: draw it. The message loop
    /// draws the frames that follow while anything moves (`pace`), and
    /// `MODAL_FRAME_TIMER` does while a modal loop runs instead.
    fn animate(&self) {
        self.invalidate();
        self.modal_frames();
    }

    /// Set `MODAL_FRAME_TIMER` to the pace the window, or a floating wheel
    /// (fading out, say), is drawn at while a modal loop of Kiln's runs
    /// (`system::modal_open`), or stop it when none runs or nothing moves.
    /// Run as a modal loop begins, when something starts to move, and at
    /// each of the timer's frames.
    fn modal_frames(&self) {
        let period = if !system::modal_loop_open() {
            None
        } else if !float_wheel::frame_waits().is_empty() {
            Some(MODAL_FRAME_MS)
        } else {
            match self.pace() {
                anim::Pace::Idle => None,
                anim::Pace::Progress => Some(anim::PROGRESS_FRAME_MS),
                anim::Pace::Display => Some(MODAL_FRAME_MS),
            }
        };
        if period == self.modal_frames.get() {
            return;
        }
        self.modal_frames.set(period);
        // SAFETY: plain calls on this thread's own window.
        unsafe {
            match period {
                Some(ms) => {
                    SetTimer(Some(self.hwnd), MODAL_FRAME_TIMER, ms, None);
                }
                None => {
                    let _ = KillTimer(Some(self.hwnd), MODAL_FRAME_TIMER);
                }
            }
        }
    }

    /// The message loop is in charge of frames again: `MODAL_FRAME_TIMER`
    /// is stopped.
    fn modal_frames_stop(&self) {
        if self.modal_frames.take().is_some() {
            // SAFETY: a plain call on this thread's own window.
            unsafe {
                let _ = KillTimer(Some(self.hwnd), MODAL_FRAME_TIMER);
            }
        }
    }

    /// How the message loop draws this window's frames now: at the
    /// display's pace while something moves, at the progress clock's while
    /// only a running job's progress moves, and not at all otherwise, or
    /// while the
    /// window is hidden or minimised.
    fn pace(&self) -> anim::Pace {
        anim::pace(
            self.moving(),
            self.burning(),
            theme::fluent::motion::animations(),
            self.on_screen(),
        )
    }

    /// Whether the window is on screen to be seen: shown, rather than in
    /// the notification area, and not minimised.
    fn on_screen(&self) -> bool {
        self.window_shown() && !self.minimised()
    }

    /// Whether any job is running, so its progress moves. Not while
    /// the window is hidden or minimised.
    fn burning(&self) -> bool {
        self.on_screen()
            && self
                .files
                .entries()
                .iter()
                .any(|e| matches!(e.job, JobState::Running { .. }))
    }

    /// Whether anything is in motion that needs full frame rate: the
    /// window's own motion (`Motion::moving_at`), a row's facts crossing
    /// over, the wheel, a sheet, Settings, help or a drag.
    fn moving(&self) -> bool {
        let (now, animations) = (Instant::now(), theme::fluent::motion::animations());
        let facts = self.files.entries().iter().any(|e| {
            e.facts_at
                .is_some_and(|t| anim::within(t, FACTS_MS, now, animations))
        });
        let wheel = self
            .wheel
            .as_ref()
            .is_some_and(|w| w.animating(now, animations));
        let rising = |t: Instant| anim::within(t, anim::SHEET_MS, now, animations);
        let sheet = self
            .sheet
            .as_ref()
            .is_some_and(|s| s.animating(now, animations));
        let settings = self
            .settings_ui
            .as_ref()
            .is_some_and(|ui| ui.animating(now, animations));
        let help = self.help.as_ref().is_some_and(|ui| rising(ui.opened()));
        let ask = self
            .prompts
            .ask_ui
            .as_ref()
            .is_some_and(|a| a.rising(now, animations));
        let tool = self
            .prompts
            .tool_ui
            .as_ref()
            .is_some_and(|t| t.rising(now, animations));
        self.motion.moving_at(now, animations)
            || ask
            || tool
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
        match b {
            FocusButton::Convert => self
                .convert_rect()
                .filter(|_| self.convert_block().is_none()),
            FocusButton::AddFiles => self.add_button(),
            FocusButton::Dismiss => self.dismiss_button(),
            FocusButton::InfoDismiss => self.top_info_dismiss(),
            FocusButton::Credit => self.credit().map(|c| c.name),

            FocusButton::Clear => self.clear_rect(),
            FocusButton::Help => self.title_bar().help,
            FocusButton::Settings => self.title_bar().settings,
            FocusButton::Inspector(c) => {
                let r = self.live_panel()?.rect_of(c)?;
                let (_, dx, dy) = self.inspector_shift(Instant::now())?;
                Some(layout::Rect::new(r.x + dx, r.y + dy, r.w, r.h))
            }
        }
    }

    /// The button the keyboard is on, while it's there and nothing covers
    /// it. A button that went hands the focus back to the list.
    fn focused_button(&self) -> Option<FocusButton> {
        self.button_focus
            .filter(|&b| self.over().list_live() && self.focus_button_rect(b).is_some())
    }

    /// Enter or Space on the button the keyboard is on: what a click does.
    fn press_button(&mut self, b: FocusButton) {
        match b {
            FocusButton::Convert => self.convert_selection(true),
            FocusButton::AddFiles => self.add_files(true),
            FocusButton::Dismiss => self.dismiss_first_run(),
            FocusButton::InfoDismiss => self.set_top_info(None),
            FocusButton::Credit => self.open_credit(),

            FocusButton::Clear => self.clear(),
            FocusButton::Help => self.open_help(),
            FocusButton::Settings => self.open_settings(),
            FocusButton::Inspector(c) => self.panel_act(c, true),
        }
    }

    /// Whether the window can't be seen: hidden in the notification area,
    /// or minimised to its taskbar button.
    fn out_of_sight(&self) -> bool {
        #[cfg(test)]
        if let Some(out) = self.out_of_sight_for_test {
            return out;
        }
        !self.window_shown() || self.minimised()
    }

    /// Whether the window is minimised, as Windows says. Its client area
    /// is then measured as 1 pixel (`Renderer::size_dips`), which nothing
    /// that follows the window's size should follow.
    fn minimised(&self) -> bool {
        unsafe { IsIconic(self.hwnd).as_bool() }
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
        let (now, animations) = (Instant::now(), theme::fluent::motion::animations());
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
            (FocusButton::Convert, WindowButton::Convert),
            (FocusButton::AddFiles, WindowButton::Add),
            (FocusButton::Dismiss, WindowButton::Dismiss),
            (FocusButton::InfoDismiss, WindowButton::InfoDismiss),
            (FocusButton::Credit, WindowButton::Credit),
            (FocusButton::Clear, WindowButton::Clear),
            (FocusButton::Help, WindowButton::Help),
            (FocusButton::Settings, WindowButton::Settings),
        ];
        let inspector = |c: InspectorControl| ButtonId::Window(FocusButton::Inspector(c));
        // A sheet or dialog takes the pointer; with none, the inspector's
        // controls have it while nothing is over the list.
        let (hover, press) = match self.overlay_buttons() {
            Some(over) => over,
            None => (
                self.inspector_ui
                    .hover()
                    .filter(|_| self.over().list_live())
                    .map(inspector),
                self.inspector_ui.pressed().map(inspector),
            ),
        };
        let mut began = false;
        for (b, own) in buttons {
            let (hover, press) = self.list.state(own);
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
            let (hover, pressed) = h.buttons();
            return Some((hover.map(ButtonId::Help), pressed.map(ButtonId::Help)));
        }
        if let Some(s) = &self.sheet {
            return Some(s.buttons(self.renderer.size_dips(), Instant::now()));
        }
        if let Some(ui) = &self.settings_ui {
            let (hover, pressed) = ui.pointer();
            return Some((
                hover.and_then(ButtonId::settings),
                pressed.and_then(ButtonId::settings),
            ));
        }
        let d = self.details.as_ref()?;
        let (hover, pressed) = d.buttons();
        Some((hover.map(ButtonId::Details), pressed.map(ButtonId::Details)))
    }

    /// Say `text` in the footer as something that went wrong or didn't
    /// happen, which the selection count doesn't hide.
    fn alert(&mut self, text: String) {
        self.status.alert(text);
    }

    /// Set the status line (`Status::say`).
    fn say(&mut self, text: impl Into<String>) {
        self.status.say(text);
    }

    /// Whether the status is an alert that still stands.
    fn alerting(&self) -> bool {
        self.status.alerting()
    }

    /// The count of listed files, which a result gives way to.
    fn plain_status(&self) -> String {
        status_line(self.files.len(), 0, 0, 0)
    }

    /// Whether something keeps a result or an alert up: a removal that can
    /// still be undone, a burn or an export.
    fn status_held_up(&self) -> bool {
        self.undo_offer().is_some() || self.burning() || !self.exports.is_empty()
    }

    /// Let a result or an alert go once the selection has changed since it
    /// was first drawn, or it has been up [`ways_in::STATUS_FOR`]: the
    /// status goes back to the count, and the footer to its hint.
    /// Run as each frame starts; arms the timer that wakes for the rest.
    fn expire_status(&mut self) {
        let (plain, held_up) = (self.plain_status(), self.status_held_up());
        let files = &self.files;
        if let Some(rest) = self.status.expire(plain, held_up, || files.selection()) {
            // Sooner still if "Added 2 files." is about to give way.
            let wait = self
                .status
                .hint_wait()
                .map_or(rest, |added| added.min(rest));
            self.arm_hint(wait);
        }
    }

    /// Wake in `wait` to show the idle hint, with a little slack, as a
    /// timer can fire a few milliseconds early.
    fn arm_hint(&self, wait: Duration) {
        let ms = (wait + HINT_SLACK).as_millis().min(u32::MAX as u128) as u32;
        unsafe {
            SetTimer(Some(self.hwnd), HINT_TIMER, ms, None);
        }
    }

    /// Ask for files with the Windows Open dialog, and add them as a drop
    /// would. `keys`: a key asked, not the pointer.
    fn add_files(&mut self, keys: bool) {
        self.effect(Effect::PickFiles {
            title: ways_in::ADD_FILES_TITLE.to_owned(),
            start: self.incoming.last_add.clone(),
            then: Box::new(move |app, picked| {
                if !picked.is_empty() {
                    app.arrive(&picked);
                    app.add(picked, keys);
                }
            }),
        });
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
        if let Some(e) = self.empty_state() {
            return Some(e.button);
        }
        Some(self.footer_row()?.add)
    }

    /// The first-run card's words, while the empty window shows it: until
    /// it is dismissed, or again after Help's Show first-run help.
    fn first_run(&self) -> Option<ways_in::FirstRun> {
        let shows = self.files.is_empty() && ways_in::first_run_shows(&self.prefs.settings);
        shows.then(|| {
            // Where the window's results go, as Help says it.
            ways_in::first_run(help::Results::of(self.held.is_some(), &self.prefs.settings))
        })
    }

    /// The empty window's parts as they are drawn, the first-run card's
    /// lines measured as the renderer sets them. `None` with files listed.
    fn empty_state(&self) -> Option<layout::EmptyState> {
        if !self.files.is_empty() {
            return None;
        }
        let l = self.layout();
        let card = self
            .first_run()
            .map(|fr| self.renderer.first_run_text(&fr, l.panel.w));
        Some(layout::empty_state(&l.panel, card))
    }

    /// Show an InfoBar at the top of the list saying `info` (title,
    /// message), or take it away. The rows move down under it.
    fn set_top_info(&mut self, info: Option<(String, String)>) {
        // One that waited behind the one going takes its place.
        let info = match info {
            None => self.work.info_after.take(),
            some => some,
        };
        if self.top_info == info {
            return;
        }
        if info.is_none() && self.button_focus == Some(FocusButton::InfoDismiss) {
            self.button_focus = None;
        }
        if let Some((title, message)) = &info {
            // Told to screen readers as it shows, as a live InfoBar is
            // (spec 5.4), without taking the status line's place.
            self.status
                .sets
                .push(format!("{title} {}", message.trim()).trim().to_owned());
        }
        self.renderer.set_top_info(info.clone());
        self.top_info = info;
        self.invalidate();
    }

    /// The InfoBar at the top of the list as it is laid out now.
    fn top_info_bar(&self) -> Option<layout::kit::InfoBarLayout> {
        self.renderer.top_info_at(self.layout().top_info?)
    }

    /// The Dismiss of the InfoBar at the top of the list, while it shows.
    fn top_info_dismiss(&self) -> Option<layout::Rect> {
        self.top_info_bar()?.dismiss
    }

    /// The first-run card's Dismiss, while it shows.
    fn dismiss_button(&self) -> Option<layout::Rect> {
        self.empty_state()?.card.map(|c| c.dismiss)
    }

    /// Dismiss the first-run card: it stays away, and that is saved. The
    /// keyboard, if it was on Dismiss, goes to Add files.
    fn dismiss_first_run(&mut self) {
        if ways_in::dismiss_first_run(&mut self.prefs.settings) {
            if let Err(e) = self.prefs.store.save_background(&self.prefs.settings) {
                log(&e);
            }
        }
        if self.button_focus == Some(FocusButton::Dismiss) {
            self.button_focus = Some(FocusButton::AddFiles);
        }
        self.invalidate();
    }

    /// Help's Show first-run help: the card shows again in the empty
    /// window, and that is saved.
    fn show_first_run(&mut self) {
        if ways_in::show_first_run(&mut self.prefs.settings) {
            if let Err(e) = self.prefs.store.save_background(&self.prefs.settings) {
                log(&e);
            }
        }
        self.invalidate();
    }

    /// Cancel every job that involves `path`, and take it off a job still
    /// waiting for its drive's folder: its row ends cancelled.
    fn cancel_jobs_for(&mut self, path: &std::path::Path) {
        for (id, job) in &self.work.jobs {
            if job.paths.iter().any(|p| p == path) {
                self.work.runner.cancel(*id);
            }
        }
        self.cancel_pending(|p| p == path);
    }

    /// Take the files `picked` names off the jobs waiting for their drive's
    /// folder. Each of their rows that is still queued, with no job of its
    /// own, ends cancelled, as a cancelled job's does.
    fn cancel_pending(&mut self, picked: impl Fn(&Path) -> bool) {
        let mut off = vec![];
        for p in &mut self.work.pending {
            p.paths.retain(|f| {
                let pick = picked(f);
                if pick {
                    off.push(f.clone());
                }
                !pick
            });
        }
        self.work.pending.retain(|p| !p.paths.is_empty());
        let now = Instant::now();
        for f in off {
            let Some(i) = self.files.index_of(&f) else {
                continue;
            };
            let queued = matches!(self.files.entries()[i].job, JobState::Running { .. })
                && self.motion.rows.get(i).is_none_or(|r| r.job.is_none());
            if queued {
                self.files.entries_mut()[i].job = JobState::Cancelled;
                self.row_ended(i, now);
            }
        }
        self.invalidate();
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
        // The InfoBar at the top was about the files as they were.
        self.set_top_info(None);
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
            .filter(|(_, said)| *said == self.status.text)
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
        self.work.runner.cancel_all();
        self.inspector.cancel_all();
        self.work.jobs.clear();
        self.work.pending.clear();
        // A redo's old result not yet saved stays in the holding folder:
        // never removed, it is saved at the next start.
        self.work.redos.drain_held();
        self.work.batches.clear();
        self.files.clear();
        self.motion.rows.clear();
        self.notes.clear();
        self.previews_rows_changed();
        self.list.rewind();
        self.status.text.clear();
        self.set_top_info(None);
        // Walks under way add nothing more; what waited on them runs.
        self.stop_walks();
        self.invalidate();
    }

    // Held results.

    /// Which rows of the list as it is now `rows` names.
    fn mask(&self, rows: &Rows) -> Vec<bool> {
        rows_mask(&self.files, rows)
    }

    /// Stop the redos of the rows `rows` names now, as a leave of them goes
    /// ahead: each row has its old result back at once (`give_back`), and
    /// its job stops.
    fn give_back(&mut self, rows: &Rows) {
        let mask = self.mask(rows);
        let stopped = give_back(
            &mut self.files,
            &mut self.notes,
            &mut self.motion,
            &mut self.work.redos,
            |i| mask.get(i).copied().unwrap_or(false),
        );
        if stopped.is_empty() {
            return;
        }
        for id in stopped {
            self.work.runner.cancel(id);
        }
        self.panel_stale();
        self.invalidate();
    }

    /// The held files that only the rows `rows` names hold, the old
    /// results their redos keep included, less any an export is already
    /// moving and any removed by hand.
    fn held_by(&self, rows: &Rows) -> Vec<PathBuf> {
        let mask = self.mask(rows);
        held_for_leave(
            &self.files,
            &self.work.redos,
            |i| mask.get(i).copied().unwrap_or(false),
            &self.saving_files(),
            present_in(self.held.as_ref(), &self.gone_seen),
        )
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
    fn stop_jobs(&mut self, rows: &Rows) {
        match rows {
            Rows::All => {
                self.work.runner.cancel_all();
                self.cancel_pending(|_| true);
            }
            Rows::Files(files) => {
                for f in files {
                    self.cancel_jobs_for(f);
                }
            }
        }
    }

    /// Handle the job events already sent to this window, now rather
    /// than after whatever runs next. Nothing is dispatched: only job
    /// events are taken, and the rest wait for the next wake, in order.
    fn take_job_news(&mut self) {
        for event in self.take_events(|e| matches!(e, Event::Job(_))) {
            if let Event::Job(news) = event {
                self.on_job_news(news);
            }
        }
    }

    /// Take the events `pick` chooses off the channel, oldest first. The
    /// rest wait, in order, for the next wake, which is posted if it isn't
    /// pending already.
    fn take_events(&mut self, pick: impl Fn(&Event) -> bool) -> Vec<Event> {
        let queued = self.inbox.queued();
        let (picked, rest): (VecDeque<Event>, VecDeque<Event>) =
            self.later.drain(..).chain(queued).partition(|e| pick(e));
        self.later = rest;
        if !self.later.is_empty() {
            self.wake.ring();
        }
        picked.into()
    }

    /// `WM_KILN_WAKE`: handle every event sent so far, oldest first, and
    /// note the strongest tree update they need for `after`.
    fn woken(&mut self) {
        let arrived = self.inbox.woken();
        self.later.extend(arrived);
        // One at a time from `later`, so a look for some kinds made while
        // handling one (`take_events`) finds the rest still in order.
        while let Some(event) = self.later.pop_front() {
            let kind = event_sync(&event);
            fold_after(self, kind, |app| &mut app.woke, |app| app.on_event(event));
        }
    }

    /// One event from the channel, to the handler its message had.
    fn on_event(&mut self, event: Event) {
        match event {
            Event::Job(news) => self.on_job_news(news),
            Event::Update(reply) => {
                self.update = UpdateState::from_reply(&reply);
                self.settings_facts_changed();
                self.invalidate();
            }
            Event::Probed(p) => self.on_probed(p),
            // Only for the dialog it was asked for: one closed since, and
            // opened again with the same choices, waits for its own.
            Event::Estimated(dialog, opts, e) if dialog == self.prompts.tool_dialog => {
                self.drive_tool(|t, cx| t.estimated(cx, opts, e));
                // Told to screen readers, when it is for the choice made.
                let line = self.prompts.tool_ui.as_ref().and_then(|t| {
                    let m = t.model();
                    let answered = m
                        .smaller()
                        .and_then(|s| s.estimate())
                        .is_some_and(|e| !matches!(e, crate::smaller::Estimate::Working));
                    answered
                        .then(|| m.estimate_lines(t.saving()).map(|(bold, _)| bold))
                        .flatten()
                });
                if let Some(bold) = line.filter(|b| !b.is_empty()) {
                    self.status.sets.push(bold);
                }
            }
            Event::Estimated(..) => {}
            Event::Cards(c) => self.on_cards(c),
            Event::Extensions(l) => self.on_extensions_looked(l),
            Event::Inspected(found) => self.on_inspected(found),
            Event::Exported(ex) => self.on_exported(ex),
            Event::Copying(c) => self.on_copying(c),
            Event::HeldOpened(o) => self.held_opened(o),
            Event::Swept => self.swept(),
            Event::Delivered(report) => self.delivered_from_last_time(report),
            Event::TextScale => self.text_scale_changed(),
            Event::Preview(key, preview) => self.on_preview(key, preview),
            Event::Mf(l) => self.on_mf_looked(l),
            Event::Engines(found) => self.engines_found(found),
            Event::Gone(found) => self.gone_found(found),
            Event::Walked(walked) => self.walked(walked),
            Event::WalkOn(id) => self.walk_on(id),
            Event::SettingsWritten => self.settings_written(),
            Event::Float(note) => self.float_news(note),
            Event::A11y(req) => self.a11y_do(req),
        }
    }

    /// Drop the floating wheel's news still queued: it is from a wheel
    /// that has gone.
    fn drop_float_notes(&mut self) {
        drop(self.take_events(|e| matches!(e, Event::Float(_))));
    }

    /// A job's news from the runner's thread: its results' sizes, then
    /// the event.
    fn on_job_news(&mut self, news: JobNews) {
        if !news.sizes.is_empty() {
            // A result written where an older one was has its new size.
            self.work.result_sizes.extend(news.sizes);
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
    /// would stop running jobs, ask first. It goes ahead at once otherwise,
    /// or once the exports already moving the rows' results are done.
    fn leave(&mut self, verb: Leave, rows: Rows, then: Then) {
        // What it asks about goes by the gone thread's last look for held
        // results on other drives; this one corrects the rows after.
        self.ask_gone();
        self.leave_after(verb, rows, then, false);
    }

    /// `leave`, where `stopped` says the rows' jobs were just stopped: they
    /// still show as running until they report, but aren't asked about.
    fn leave_after(&mut self, verb: Leave, rows: Rows, then: Then, stopped: bool) {
        // A save of this run's results under way finishes first (the
        // saving of what last time left is stopped by the quit instead). A
        // hide cuts none short: they run on with the window hidden, and
        // what a save moves isn't asked about (`held_by`).
        if self.close_waits() && !matches!(then, Then::Hide) {
            if matches!(then, Then::Close) {
                self.close_when_saved = true;
                self.say(CLOSING_WHEN_SAVED);
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
                self.say(CLOSING_WHEN_SAVED);
            } else {
                self.say(model::SAVING.to_owned());
            }
            self.invalidate();
            return;
        }
        let mask = self.mask(&rows);
        // A new job on a row only replaces what it holds, and a hide stops
        // no job.
        let running: Vec<&Action> = match then {
            Then::Start(..) => vec![],
            _ if stopped || !stops_jobs(&then) => vec![],
            _ => running_of(&self.files, &mask),
        };
        let Some(prompt) = leave_prompt(running.len(), verb) else {
            self.proceed(then);
            return;
        };
        self.ask(prompt, move |app, button| {
            app.leave_answered(button, (rows, then))
        });
    }

    /// The leave prompt for a leave of `rows`, then `then`, was answered
    /// `button`.
    fn leave_answered(&mut self, button: PromptButton, (rows, then): (Rows, Then)) {
        // The session ended while the prompt was open, and the window went
        // with it: every result not yet saved stays for the next start.
        if self.ending() {
            return;
        }
        // Cancel, Esc, or the prompt closed: nothing changes.
        if let PromptButton::Stop(_) = button {
            // A redo's row has its old result back at once, before its
            // job's news can come.
            self.give_back(&rows);
            self.stop_jobs(&rows);
            self.take_job_news();
            self.proceed(then);
        }
    }

    /// Whether a delivery is moving results a leave of `rows` by `then`
    /// has to wait for.
    fn exporting(&self, rows: &Rows, then: &Then) -> bool {
        !self.exports_for(rows, then).is_empty()
    }

    /// Whether a close has to wait: a delivery of this run's results is
    /// still under way, and closing would cut it short.
    fn close_waits(&self) -> bool {
        !self.exports.is_empty()
    }

    /// Hold a leave until the deliveries it has to wait for are done.
    fn wait_for_exports(&mut self, rows: Rows, then: Then) {
        // One leave at a time: `leave_after` turns a second one away.
        debug_assert!(self.leaving.is_none());
        let waits = self.exports_for(&rows, &then);
        if waits.is_empty() {
            return self.proceed(then);
        }
        self.leaving = Some(Leaving { waits, rows, then });
        self.invalidate();
    }

    /// A leave whose deliveries are all done: go ahead, or wait for one
    /// that started meanwhile.
    fn resume_leave(&mut self, l: Leaving) {
        if self.exporting(&l.rows, &l.then) {
            self.wait_for_exports(l.rows, l.then);
        } else {
            self.proceed(l.then);
        }
    }

    /// Go ahead with a leave. Only now are its rows' jobs stopped.
    fn proceed(&mut self, then: Then) {
        match then {
            Then::Close => {
                // A save of this run's results still under way finishes
                // first, then Kiln closes, asking again if anything new is
                // at stake. The saving of what last time left is never
                // waited for: the quit stops it, and what it hadn't saved
                // is kept, whole, for the next start.
                if self.close_waits() {
                    self.close_when_saved = true;
                    self.say(CLOSING_WHEN_SAVED);
                    self.invalidate();
                    return;
                }
                self.close_when_saved = false;
                // Nothing goes ahead on a closed window.
                self.leaving = None;
                self.work.runner.cancel_all();
                self.effect(Effect::Destroy);
            }
            Then::Hide => self.hide_to_tray(),
            Then::Clear => self.clear_rows(),

            Then::Remove(paths) => self.remove_rows(&paths),
            Then::Start(action, paths, quality, via, tool) => self.launch_full(
                action,
                paths,
                quality,
                Origin::Window,
                via,
                tool.map(|t| *t),
            ),
        }
    }

    /// Every held result the rows hold.
    fn held_paths(&self) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = vec![];
        for e in self.files.entries() {
            if let Some(held) = e.held() {
                paths.extend(held.iter().cloned());
            }
        }
        paths
    }

    /// A held result may have been removed by hand: the window was
    /// activated, or the person acts on what the rows hold. The gone
    /// thread looks at every one held, and
    /// `gone_found` notes what it finds.
    fn ask_gone(&mut self) {
        let paths = self.held_paths();
        if paths.is_empty() {
            return;
        }
        let ask = self.gone_seen.ask();
        if !self.gone_thread.ask(ask, paths) {
            log("couldn't look for removed results: their thread has gone");
        }
    }

    /// The gone thread's answer: a result it didn't name is there, or
    /// came since it looked.
    fn gone_found(&mut self, found: workers::GoneFound) {
        if !self.gone_seen.found(found.ask, found.gone) {
            return;
        }
        let gone = self.gone_seen.gone_now();
        self.note_gone(|p| !gone.contains(p));
    }

    /// `check_gone`, with `exists` saying which held results are there.
    /// A row being exported is left be: its files are on their way out.
    fn note_gone(&mut self, exists: impl Fn(&Path) -> bool) {
        let saving = self.saving_rows();
        let rows: Vec<(PathBuf, bool)> = self
            .files
            .entries()
            .iter()
            .filter(|e| e.held().is_some() && !saving.contains(&e.path))
            .map(|e| (e.path.clone(), e.is_gone(&exists)))
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

    /// Whether `path` is inside Kiln's holding folders, on any drive, which
    /// are no place to export to: this run's are emptied when Kiln closes,
    /// and the next launch offers or sweeps the others.
    fn in_holding(&self, path: &Path) -> bool {
        match (&self.held, &self.places) {
            (Some(h), _) => h.holds(path),
            (None, Some(places)) => kiln_core::in_holding_place(path, places),
            (None, None) => kiln_core::in_holding_place(path, &Places::new(Path::new(""))),
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

    /// Deliver `files` to `dest` on a worker thread, one at a time; it
    /// reports back with `Event::Exported`. `beside` names the notice a
    /// save next to the original tells, sent as the result was ready.
    /// Returns the delivery's id, or `None` when it was settled on the
    /// spot.
    fn export_for(
        &mut self,
        files: Vec<PathBuf>,
        dest: Dest,
        beside: Option<Feeds>,
    ) -> Option<u64> {
        // Only results this run holds: anything else is the user's own
        // file, which an export must never move.
        let files: Vec<PathBuf> = match &self.held {
            Some(h) => files
                .into_iter()
                .filter(|f| h.folder_of(f).is_some())
                .collect(),
            None => vec![],
        };
        // Nothing to move: no thread, and no message to wait for.
        if files.is_empty() {
            return None;
        }
        let Dest::Into(folder) = &dest;
        let folder = folder.clone();
        if self.in_holding(&folder) {
            let results = files.into_iter().map(|f| (f, None)).collect();
            self.exported(&folder, (results, vec![]), beside);
            return None;
        }
        let id = self.next_export;
        self.next_export += 1;
        let cancel = CancelToken::new();
        // Across drives a move is a copy, which can take a while: keep it
        // off the UI thread. Its progress reaches the rows, a few times a
        // second at most.
        let wake = self.wake.clone();
        let (moving, stop) = (files.clone(), cancel.clone());
        // What left is noted by the notes worker, after where it came from.
        let notes = self.held_notes.sender();
        // A result on its way to where it was meant to go that can't go
        // there goes to Documents\Kiln results.
        let fallback = beside.and_then(|_| held::kiln_results());
        let thread = std::thread::spawn(move || {
            let mut pace: Option<(PathBuf, CopyPace)> = None;
            let mut progress = |file: &Path, done: u64, total: u64| {
                if pace.as_ref().is_none_or(|(f, _)| f != file) {
                    pace = Some((file.to_path_buf(), CopyPace::default()));
                }
                let Some((_, p)) = pace.as_mut() else {
                    return;
                };
                if let Some(fraction) = p.due(Instant::now(), done, total) {
                    wake.send(Event::Copying(Copying {
                        id,
                        file: file.to_path_buf(),
                        fraction,
                    }));
                }
            };
            let left = |dir: &Path, file: &Path| {
                notes.send(Note::Left {
                    dir: dir.to_path_buf(),
                    file: file.to_path_buf(),
                })
            };
            let (results, cancelled) = move_out_with(
                moving,
                (&dest, fallback.as_deref()),
                &mut progress,
                &stop,
                &left,
            );
            wake.send(Event::Exported(Exported {
                id,
                folder,
                results,
                cancelled,
            }));
        });
        self.exports.insert(
            id,
            Export {
                files,
                beside,
                cancel,
                thread: Some(thread),
            },
        );
        // The inspector's Options waits while the result is saved.
        self.panel_stale();
        self.invalidate();
        Some(id)
    }

    /// An export thread has copied part of a held file to another drive:
    /// its row says how far.
    fn on_copying(&mut self, c: Copying) {
        let moving = self
            .exports
            .get(&c.id)
            .is_some_and(|x| x.files.contains(&c.file));
        if moving {
            self.copying.insert(c.file, c.fraction);
            self.invalidate();
        }
    }

    /// The held files of row `i` being copied to another drive, if any:
    /// how far the first is.
    fn row_copying(&self, i: usize) -> Option<f32> {
        let held = self.files.entries().get(i)?.held()?;
        held.iter().find_map(|f| self.copying.get(f).copied())
    }

    /// The line a row being exported shows: "Saving…", with how far once
    /// a copy to another drive is under way.
    fn saving_line(&self, i: usize) -> String {
        model::saving_line(self.row_copying(i))
    }

    /// Stop the copies to another drive moving row `i`'s results: each
    /// stops where it is, removes only Kiln's temp copy, and leaves the
    /// held result as it was. True if one was under way.
    fn cancel_copies(&self, i: usize) -> bool {
        let Some(held) = self.files.entries().get(i).and_then(|e| e.held()) else {
            return false;
        };
        let mut any = false;
        for x in self.exports.values() {
            if x.files
                .iter()
                .any(|f| held.contains(f) && self.copying.contains_key(f))
            {
                x.cancel.cancel();
                any = true;
            }
        }
        any
    }

    /// A worker has opened a holding folder: the jobs that waited for it
    /// start, as they were asked for.
    fn held_opened(&mut self, o: held::Opened) {
        let Some(h) = &mut self.held else {
            return;
        };
        h.opened(o);
        if self.ending() {
            self.work.pending.clear();
            return;
        }
        // Each starts now, or waits again for another drive's folder.
        for p in std::mem::take(&mut self.work.pending) {
            self.launch_now(p);
        }
    }

    /// An export thread is done. A leave waiting on it goes ahead if
    /// everything was saved, exporting anything that finished meanwhile
    /// first; otherwise it's off, and the footer says what wasn't saved.
    /// A notice waiting on it as a save next to the original goes once it
    /// waits on nothing more.
    fn on_exported(&mut self, ex: Exported) {
        let Some(export) = self.exports.remove(&ex.id) else {
            return;
        };
        self.panel_stale();
        for f in &export.files {
            self.copying.remove(f);
        }
        // What couldn't be saved anywhere, by the name it would have had.
        let failed: Vec<String> = ex
            .results
            .iter()
            .filter(|(f, to)| to.is_none() && !ex.cancelled.contains(f))
            .filter_map(|(f, _)| f.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .collect();
        let saved = self.exported(&ex.folder, (ex.results, ex.cancelled), export.beside);
        if let Some(feeds) = export.beside {
            self.save_done(feeds, ex.id);
        }
        if !saved {
            // A close waiting on this save is off: Kiln stays open, and
            // the footer says why rather than that it will close. A save
            // that was only cancelled went wrong nowhere: the footer goes
            // back to the count.
            if self.close_when_saved {
                match model::kept_text(&failed) {
                    Some(why) => self.alert(why),
                    None if self.status.text == CLOSING_WHEN_SAVED => {
                        let plain = self.plain_status();
                        self.say(plain);
                    }
                    None => {}
                }
            }
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
        // A resumed leave may make a close due too, as one step did before.
        for _ in 0..2 {
            let ready = self.leaving.as_ref().is_some_and(|l| l.waits.is_empty());
            let due = model::leave_due(
                system::modal_open(),
                ready,
                self.close_when_saved,
                self.close_waits(),
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
                    // It is a quit, not a close: with the drag-wheel on, a
                    // close would only hide the window. Done here, not
                    // posted, so no modal loop can lose it on the way.
                    self.close_when_saved = false;
                    self.quit();
                    return;
                }
            }
        }
    }

    /// Record what a delivery did: where each held file went, or `None`
    /// where it couldn't be saved, and which of those a cancel left held.
    /// Returns whether every one was saved. The rows are told as
    /// `record_export` says: a cancelled file is held still, with nothing
    /// said against it. The notice `beside` names is told where each went,
    /// and which row holds the first it couldn't save, a cancelled one
    /// included.
    fn exported(
        &mut self,
        folder: &Path,
        (results, cancelled): (Vec<(PathBuf, Option<PathBuf>)>, Vec<PathBuf>),
        beside: Option<Feeds>,
    ) -> bool {
        let moved: Vec<(PathBuf, PathBuf)> = results
            .iter()
            .filter_map(|(f, to)| Some((f.clone(), to.clone()?)))
            .collect();
        let kept: Vec<PathBuf> = results
            .iter()
            .filter(|(_, to)| to.is_none())
            .map(|(f, _)| f.clone())
            .collect();
        let failed: Vec<PathBuf> = kept
            .iter()
            .filter(|f| !cancelled.contains(f))
            .cloned()
            .collect();
        if !moved.is_empty() {
            self.last_export = Some(folder.to_path_buf());
        }
        // A result keeps its size where it went.
        for (from, to) in &moved {
            if let Some(size) = self.work.result_sizes.remove(from) {
                self.work.result_sizes.insert(to.clone(), size);
            }
        }
        record_export(
            &mut self.files,
            &mut self.notes,
            folder,
            (&moved, &failed),
            present_in(self.held.as_ref(), &self.gone_seen),
        );
        if let Some(feeds) = beside {
            // Saved into the folder chosen in Settings, not next to the
            // originals: the notice says which.
            let chosen = self.prefs.settings.save_to.as_deref() == Some(folder);
            let kept_row = self
                .files
                .entries()
                .iter()
                .find(|e| e.held().is_some_and(|h| h.iter().any(|p| kept.contains(p))))
                .map(|e| e.path.clone());
            if let Some(saves) = self.saves_of(feeds) {
                let name = |p: &Path| {
                    p.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                };
                let folder_name = crate::actions::folder_name(folder);
                if chosen && !moved.is_empty() {
                    saves.into = Some(folder_name.clone());
                }
                // Every one went to Documents\Kiln results instead.
                let elsewhere = |to: &PathBuf| to.parent() != Some(folder);
                if !moved.is_empty() && moved.iter().all(|(_, to)| elsewhere(to)) {
                    saves.into = Some(model::KILN_RESULTS_PLACE.to_owned());
                }
                saves.saved.extend(moved.iter().map(|(_, to)| name(to)));
                saves.paths.extend(moved.iter().map(|(_, to)| to.clone()));
                saves.folders.extend(moved.iter().map(|(_, to)| {
                    to.parent()
                        .map_or_else(|| folder_name.clone(), crate::actions::folder_name)
                }));
                saves
                    .kept
                    .extend(kept.iter().map(|f| (folder_name.clone(), name(f))));
                if saves.first_kept.is_none() {
                    saves.first_kept = kept_row;
                }
            }
        }
        self.invalidate();
        kept.is_empty()
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
        self.reveal(folder, vec![path]);
    }

    /// Open Explorer at `folder` with `files` selected, once `App` is let
    /// go; it says so if it couldn't.
    fn reveal(&mut self, folder: PathBuf, files: Vec<PathBuf>) {
        let shown = folder.clone();
        self.effect(Effect::ShowInFolder {
            folder,
            files,
            then: Box::new(move |app, opened| {
                if !opened {
                    app.alert(format!("Couldn't open {}.", shown.display()));
                    app.invalidate();
                }
            }),
        });
    }

    /// Open Explorer on an exported row's results.
    fn show_saved(&mut self, i: usize) {
        let Some((folder, files)) = self.files.entries().get(i).and_then(|e| e.saved()) else {
            return;
        };
        let (folder, files) = (folder.to_path_buf(), files.to_vec());
        self.reveal(folder, files);
    }

    /// Open a finished row's saved result in its usual app. It says so if
    /// it couldn't.
    fn open_result(&mut self, i: usize) {
        let Some(file) = self
            .files
            .entries()
            .get(i)
            .and_then(|e| e.saved())
            .and_then(|(_, files)| files.first())
            .cloned()
        else {
            return;
        };
        self.effect(Effect::Open {
            target: file.clone(),
            then: Box::new(move |app, opened| {
                if !opened {
                    let name = file.file_name().unwrap_or_default().to_string_lossy();
                    app.alert(format!("Couldn't open {name}."));
                    app.invalidate();
                }
            }),
        });
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
        if let Some(backend) = e.waits_on.clone() {
            // A conversion left it out for a missing helper: the link
            // gets that helper.
            self.get_helper(&backend);
        } else if let Some(backend) = e.needs_helper().map(str::to_owned) {
            // Nothing can be done with this file until its helper app is
            // installed: the link goes to where to get it.
            self.get_helper(&backend);
        } else if let Some(product) = e
            .store_product()
            .filter(|p| e.link(false) == Some(model::store_label(p)))
        {
            // The Microsoft Store extension its job needed. A file waiting
            // on one still converts, so its link is Convert.
            self.get_extension(product);
        } else if matches!(e.job, JobState::Idle) {
            // A file nothing has been done to: its link asks what to do.
            let paths = vec![e.path.clone()];
            let at = self.row_anchor(i).unwrap_or_else(|| self.row_centre(i));
            self.open_choice(WheelKind::Convert, paths, at, keyboard);
        } else if e.saved().is_some() {
            self.show_saved(i);
        } else {
            self.show_details(i);
        }
    }

    /// Open the download page of the helper app behind `backend`
    /// ("ffmpeg") in the browser, or for "audio-mf" Settings' Optional
    /// features, where Windows' Media Feature Pack is added.
    fn get_helper(&mut self, backend: &str) {
        // FFmpeg comes with Kiln and has no page: Settings' FFmpeg card is
        // where another copy is chosen.
        if backend == "ffmpeg" && model::helper_page(backend).is_none() {
            self.open_settings_at(settings_page::Helper::Ffmpeg);
            return;
        }
        // Only ever one of `helper_page`'s fixed addresses.
        let Some(page) = model::helper_page(backend) else {
            return;
        };
        let backend = backend.to_owned();
        self.effect(Effect::Open {
            target: PathBuf::from(page),
            then: Box::new(move |app, opened| {
                if opened {
                    app.mf_recheck.helper_opened(&backend);
                } else {
                    app.alert(model::helper_page_failed(&backend));
                    app.invalidate();
                }
            }),
        });
    }

    /// Open the Microsoft Store's page for the extension `product`
    /// (HEVC, AV1, VP9 or HEIF), as Get it does: only ever one of
    /// `store_page`'s four.
    fn get_extension(&mut self, product: &str) {
        let Some(page) = model::store_page(product) else {
            return;
        };
        self.effect(Effect::Open {
            target: PathBuf::from(page),
            then: Box::new(|app, opened| {
                if opened {
                    app.ext_recheck.store_opened();
                } else {
                    app.alert("Couldn't open the Microsoft Store.".to_owned());
                    app.invalidate();
                }
            }),
        });
    }

    /// The window was activated. After Get it, look again for the decoders
    /// of the extensions rows wait on, on a thread of its own: the look
    /// starts Media Foundation and lists its decoders.
    fn recheck_extensions(&mut self) {
        if !self.ext_recheck.activated() {
            return;
        }
        let awaited = self.files.awaited_extensions();
        let wake = self.wake.clone();
        std::thread::spawn(move || {
            let installed = awaited
                .into_iter()
                .filter(|name| {
                    model::extension_codec(name)
                        .is_some_and(|c| kiln_media::video::missing_extension(&c).is_none())
                })
                .collect();
            wake.send(Event::Extensions(ExtensionsLooked { installed }));
        });
    }

    /// The look is back: rows whose extension is there now stop saying they
    /// need it, and their facts are read again.
    fn on_extensions_looked(&mut self, l: ExtensionsLooked) {
        let cleared = self.files.extensions_installed(&l.installed);
        let waiting = !self.files.awaited_extensions().is_empty();
        if self.ext_recheck.finished(!cleared.is_empty(), waiting) {
            self.inspect(cleared);
            self.panel_stale();
            self.invalidate();
        }
    }

    /// The window was activated. After Get Media Feature Pack, look for Media
    /// Foundation's codecs again on the engines thread: the look starts
    /// Media Foundation and asks Windows for six kinds of codec.
    fn recheck_media_foundation(&mut self) {
        if !self.mf_recheck.activated() {
            return;
        }
        self.engines_thread.ask(workers::EnginesAsk::Mf);
    }

    /// The look is back: if it found something new, the rows offer what
    /// they can now.
    fn on_mf_looked(&mut self, l: MfLooked) {
        if self.mf_recheck.finished(l.before, l.found) {
            self.offers_changed();
            self.invalidate();
        }
    }

    // Suggestions.

    /// Queue `paths` for inspecting, while suggestions are on. Each job
    /// carries the options a conversion between files of its type would
    /// run with, so the inspect thread never reads the app's state.
    fn inspect(&mut self, paths: Vec<PathBuf>) {
        if !self.prefs.settings.suggestions {
            return;
        }
        for path in paths {
            let opts = self.inspect_options(&path);
            self.inspector.queue(path, opts);
        }
    }

    fn inspect_options(&self, path: &Path) -> Options {
        match self.kiln.registry().format_of(path) {
            Some(f) => self.prefs.settings.options_for(f, f),
            None => self.prefs.settings.options(),
        }
    }

    /// Which engines are installed, as last looked for: none before the
    /// first answer.
    fn available(&self) -> &Availability {
        self.engines.get()
    }

    /// Look for the engines again, with the settings as they are now, on
    /// the engines thread. Until its answer comes, the last one stands.
    fn look_for_engines(&mut self) {
        let id = self.engines.ask();
        let settings = Box::new(self.prefs.settings.clone());
        self.engines_thread
            .ask(workers::EnginesAsk::Look { id, settings });
    }

    /// The window was activated: a helper app may have been installed
    /// meanwhile, so look again, at most once every
    /// `LOOK_ON_ACTIVATE` (DIST-07).
    fn look_again_on_activate(&mut self, now: Instant) {
        if self
            .looked_on_activate
            .is_some_and(|t| now.saturating_duration_since(t) < LOOK_ON_ACTIVATE)
        {
            return;
        }
        self.looked_on_activate = Some(now);
        self.look_for_engines();
    }

    /// A WM_SETTINGCHANGE naming `what`: "Environment" means PATH may
    /// have changed, so the helper apps are looked for again.
    fn environment_changed(&mut self, what: Option<&str>) {
        if what == Some("Environment") {
            self.look_for_engines();
        }
    }

    /// A row that needs a helper app was just selected: look for it again,
    /// once while it stays selected.
    fn look_if_helper_selected(&mut self) {
        let wanting = self
            .files
            .selected()
            .and_then(|i| self.files.entries().get(i))
            .filter(|e| e.missing.is_some())
            .map(|e| e.path.clone());
        if wanting == self.helper_row_looked {
            return;
        }
        self.helper_row_looked = wanting;
        if self.helper_row_looked.is_some() {
            self.look_for_engines();
        }
    }

    /// The engines thread has answered. When what it found changed, every
    /// row's formats and suggestions follow; with the first answer, what
    /// waited for it goes ahead.
    fn engines_found(&mut self, mut found: workers::EnginesFound) {
        let first = !self.engines.known();
        // Settings' lines follow the latest look that says where each
        // engine is.
        if let Some(status) = found.status.take() {
            if found.id >= self.engines.kept {
                self.engines.status = Some(status);
                self.engines.office = found.office.take();
                self.engines.save_to = found.save_to.take();
                self.engines.winget = found.winget.take();
                self.settings_facts_changed();
            }
        }
        // A look that failed stands in for an answer only until a real
        // one has come: it never replaces one.
        if found.failed && !first {
            return;
        }
        if self.engines.found(found.id, found.available) {
            let available = self.available().clone();
            self.files.refresh_offers(&self.kiln, &available);
            // A row left out for a helper that is here now says its own
            // line again.
            let found: Vec<PathBuf> = self
                .files
                .entries()
                .iter()
                .filter(|e| e.waits_on.as_deref().is_some_and(|h| available.has(h)))
                .map(|e| e.path.clone())
                .collect();
            for p in &found {
                self.notes.remove(p);
                self.set_waits_on(p, None);
            }
            self.refresh_suggestions();
            self.invalidate();
        }
        if !first || !self.engines.known() {
            return;
        }
        let fresh = std::mem::take(&mut self.incoming.helper_pending);
        self.helper_check(&fresh);
        // Walks held their rows back for this.
        self.resume_walks();
        if let Some(f) = self.engines.waiting.take() {
            f(self);
        }
    }

    /// `ENGINES_TIMER`: the engines thread hasn't answered in time. Rows,
    /// wheels and menus go ahead with nothing found; when the answer
    /// comes, the rows are worked out again (`engines_found`).
    fn engines_waited_out(&mut self) {
        if self.engines.ready() {
            return;
        }
        log("the engines weren't found in time; going on without them");
        self.engines.waited_out = true;
        self.resume_walks();
        if let Some(f) = self.engines.waiting.take() {
            f(self);
        }
    }

    /// Run `f` once the engines are known: at once when they are.
    fn when_engines_known(&mut self, f: impl FnOnce(&mut App) + 'static) {
        if self.engines.ready() {
            f(self);
        } else {
            // Only the latest: a wheel or menu asked for again replaces
            // the one asked before.
            self.engines.waiting = Some(Box::new(f));
        }
    }

    /// The settings writer has written: a write that failed says so in
    /// the footer, as a save that failed did.
    fn settings_written(&mut self) {
        if let Some(e) = self.prefs.store.take_error() {
            self.alert(e);
            self.invalidate();
        }
    }

    /// What Kiln puts forward for one file, given what it offers for it:
    /// its conversion, or else its tool.
    fn suggestion_for(&self, facts: &Facts, offered: &Offered) -> Option<Suggestion> {
        let s = suggest(std::slice::from_ref(facts), offered);
        s.convert.or(s.tool)
    }

    fn on_inspected(&mut self, found: Inspected) {
        // What the inspect thread's Kiln found in the sound: a video with
        // none loses its audio formats. Neither step reads the disk.
        self.kiln.learn(&found.path, &found.facts);
        // A Make smaller or Resize dialog open on this file follows its
        // picture size; `drive_tool` asks for the estimate again if that
        // changed what it asks.
        if self.prompts.tool_ui.is_some() {
            let picture = found.facts.picture.as_ref().map(|p| (p.width, p.height));
            let fps = found.facts.media.as_ref().and_then(|m| m.fps);
            let path = found.path.clone();
            self.drive_tool(|t, cx| t.facts_arrived(cx, &path, picture, fps));
        }
        if found.facts.silent {
            let available = self.available().clone();
            self.files
                .refresh_offers_for(&self.kiln, &available, &found.path);
            // A Convert wheel already open on it loses its sound formats
            // too, so none can be picked.
            let open = self
                .wheel
                .as_ref()
                .filter(|w| {
                    w.is_open() && w.kind() == WheelKind::Convert && w.paths().contains(&found.path)
                })
                .map(|w| w.paths().to_vec());
            if let Some(paths) = open {
                let files = &self.files;
                let (items, flyout) =
                    wheel::offering(WheelKind::Convert, &self.kiln, &available, &paths, &|p| {
                        files.has_subtitles(p)
                    });
                if let Some(w) = self.wheel.as_mut() {
                    w.restock(items, flyout);
                }
            }
            // So does the floating wheel a drag holds it on.
            if let Some(fw) = self.float.as_mut().filter(|fw| fw.holds(&found.path)) {
                fw.restock(&self.kiln, &available);
            }
            self.invalidate();
        }
        // Posted before Suggestions was turned off.
        if !self.prefs.settings.suggestions {
            return;
        }
        let offered = wheel::offered(&self.kiln, &[found.path.as_path()], self.available());
        let suggestion = self.suggestion_for(&found.facts, &offered);
        if self.files.set_facts(&found.path, found.facts, suggestion) {
            self.animate();
        }
        // An open wheel waiting on this file gets its suggestion now, for
        // where the keyboard starts in More. Nothing else on it changes.
        let waiting = self
            .wheel
            .as_ref()
            .filter(|w| w.awaits(&found.path))
            .map(|w| w.subject());
        let Some((kind, paths)) = waiting else {
            return;
        };
        if let Some(s) = self.wheel_suggestion(kind, &paths) {
            if let Some(w) = self.wheel.as_mut() {
                w.suggest(s);
            }
        }
    }

    /// What a wheel of `kind` suggests for `paths`, once every file's
    /// facts are in: the conversion on Convert, the tool on Tools.
    fn wheel_suggestion(&self, kind: WheelKind, paths: &[PathBuf]) -> Option<Suggestion> {
        if !self.prefs.settings.suggestions {
            return None;
        }
        let facts = self.files.facts_for(paths)?;
        let refs: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
        let offered = wheel::offered(&self.kiln, &refs, self.available());
        let s = suggest(&facts, &offered);
        match kind {
            WheelKind::Convert => s.convert,
            WheelKind::Tools => s.tool,
        }
    }

    /// Engines changed: every row's suggestion follows from its facts, with
    /// no new fade. Engines are looked for at most once, and
    /// what Kiln offers is worked out once per type, except for videos:
    /// one with no sound is offered no audio formats, so each video is
    /// worked out for itself.
    fn refresh_suggestions(&mut self) {
        let available = self.available();
        let mut offers: HashMap<(String, Option<&Path>), Offered> = HashMap::new();
        let fresh: Vec<Option<Option<Suggestion>>> = self
            .files
            .entries()
            .iter()
            .map(|e| {
                let f = e.facts.as_ref()?;
                let own = (f.kind == kiln_core::Kind::Video).then_some(e.path.as_path());
                let offered = offers
                    .entry((f.ext.clone(), own))
                    .or_insert_with(|| wheel::offered(&self.kiln, &[e.path.as_path()], available));
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

    // The wheel.

    /// What a wheel of `kind` offers for `paths`: its ring and More. A JPG
    /// cut short can have no tool run on it, Make smaller included, as its
    /// pane and menu say: only its formats.
    fn offering_for(
        &self,
        kind: WheelKind,
        paths: &[PathBuf],
    ) -> (Vec<wheel::Item>, Vec<wheel::Group>) {
        let files = &self.files;
        let (mut items, mut flyout) =
            wheel::offering(kind, &self.kiln, self.available(), paths, &|p| {
                files.has_subtitles(p)
            });
        if self.any_cut_short(paths) {
            let format = |i: &wheel::Item| !matches!(i.action, Action::Tool(..));
            items.retain(format);
            for g in &mut flyout {
                g.items.retain(format);
            }
            flyout.retain(|g| !g.items.is_empty());
        }
        (items, flyout)
    }

    /// Open the list of formats and tools for `paths` beside `at`, a point
    /// on the right edge of whatever asked for it, with the list's own
    /// place to keep to: the footer's Convert, the pane's More and a row's
    /// Convert to… use it. It never goes over the pane.
    fn open_list(&mut self, paths: Vec<PathBuf>, at: (f32, f32), keyboard: bool) {
        if !self.engines.ready() {
            return;
        }
        self.incoming.arrivals.clear();
        let (items, flyout) = self.offering_for(WheelKind::Convert, &paths);
        let own = wheel::source_kind(&self.kiln, &paths);
        let mut wheel = Wheel {
            kind: WheelKind::Convert,
            origin: wheel::Origin::Window,
            items: vec![],
            flyout: wheel::all_groups(items, flyout, own),
            center: at,
            scale: 1.0,
            source_code: wheel::source_code(&paths),
            source_kind: wheel::source_kind(&self.kiln, &paths),
            list_only: true,
            settable: vec![],
        };
        let suggestion = self.wheel_suggestion(WheelKind::Convert, &paths);
        wheel.settable = self.settable_formats(&wheel, &paths, suggestion.as_ref());
        let estimates = self.estimates_for(&paths);
        // The list stays inside the list's own panel: never over the pane,
        // and above the footer.
        let bounds = self.layout().list;
        self.wheel = Some(WheelUi::new(
            WheelOpening {
                wheel,
                paths,
                keyboard,
                suggestion,
                hover: None,
                bounds,
                estimates,
            },
            Instant::now(),
        ));
        self.animate();
    }

    /// One file's likely size as each format, as its row's facts hold
    /// them; empty for several files.
    fn estimates_for(&self, paths: &[PathBuf]) -> std::collections::BTreeMap<String, u64> {
        match paths {
            [one] => self
                .files
                .index_of(one)
                .and_then(|i| self.files.entries().get(i))
                .and_then(|e| e.facts.as_ref())
                .map(|f| f.estimates.clone())
                .unwrap_or_default(),
            _ => Default::default(),
        }
    }

    /// Open `kind`'s wheel for `paths` at `at`, placed as `pour` says.
    /// `keyboard` when a key asked for it.
    fn open_wheel(
        &mut self,
        kind: WheelKind,
        paths: Vec<PathBuf>,
        at: (f32, f32),
        keyboard: bool,
        pour: Pour,
    ) {
        // Before the engines are known it would offer nothing. A drag asks
        // again as it moves; the other ways in wait in `open_choice`.
        if !self.engines.ready() {
            return;
        }
        // A wheel opening is the choice for whatever arrived: none more
        // comes for it.
        self.incoming.arrivals.clear();
        let (w, h) = self.renderer.size_dips();
        let (items, flyout) = self.offering_for(kind, &paths);
        let mut wheel = Wheel {
            kind,
            origin: wheel::Origin::Window,
            items,
            flyout,
            center: (0.0, 0.0),
            scale: 1.0,
            source_code: wheel::source_code(&paths),
            source_kind: wheel::source_kind(&self.kiln, &paths),
            list_only: false,
            settable: vec![],
        };
        // Placed so the outer ring fits too, should More be opened. A wheel
        // asked for from a key or beside a row keeps to the list beside the
        // inspector, so it never covers it; one under a dropped pointer
        // opens at the pointer. `pour` says which, not where the pointer is.
        let l = self.layout();
        let list = l.inspector.map(|_| (l.list.x, l.list.right()));
        // On the content layer, with the ring's shadow inside it.
        let layer = layout::content_layer(w, h, self.renderer.title_bar(false).rect.h);
        (wheel.center, wheel.scale) =
            wheel::place_pour(pour, at.0, at.1, layer, wheel.extent(), list);
        let suggestion = self.wheel_suggestion(kind, &paths);
        wheel.settable = self.settable_formats(&wheel, &paths, suggestion.as_ref());
        // From the keyboard it starts on the suggestion, or on More when
        // the suggestion is in it.
        let start = wheel::start_spot(&wheel, suggestion.as_ref().map(|s| &s.action));
        let hover = (keyboard && wheel.segments() > 0).then_some(start);
        // One file's estimates, as its row's facts hold them, for the hub.
        let estimates = self.estimates_for(&paths);
        self.wheel = Some(WheelUi::new(
            WheelOpening {
                wheel,
                paths,
                keyboard,
                suggestion,
                hover,
                bounds: self.menu_bounds(),
                estimates,
            },
            Instant::now(),
        ));
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
        // A row with no link anchors where a link of the usual room would be.
        let link = self.files.entries().get(i).and_then(|e| e.link(false));
        let hit = layout::row_link_at(&r, link.unwrap_or(""), self.options_shown(i));
        Some((hit.x + hit.w / 2.0, r.y + r.h / 2.0))
    }

    /// Whether row `i` shows its Options: a finished drag-wheel
    /// conversion's, unless its result is being saved.
    fn options_shown(&self, i: usize) -> bool {
        self.files
            .entries()
            .get(i)
            .is_some_and(|e| e.options_for().is_some())
            && !self.row_saving(i)
    }

    /// Whether any of `paths` is a listed file the look on adding found
    /// cut short.
    fn any_cut_short(&self, paths: &[PathBuf]) -> bool {
        self.files
            .entries()
            .iter()
            .any(|e| e.check == kiln_core::FileCheck::CutShort && paths.contains(&e.path))
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
        // Before the engines are known the wheel would offer nothing: it
        // opens once they are.
        if !self.engines.ready() {
            return self.when_engines_known(move |app| app.open_choice(kind, paths, at, keyboard));
        }
        let busy = match kind {
            WheelKind::Convert => self.files.convert_busy(&paths),
            // A JPG cut short can have no tool run on it, as its menu
            // says.
            _ if self.any_cut_short(&paths) => Some(CUT_SHORT_NO_TOOLS),
            _ => None,
        };
        let said = if let Some(why) = busy {
            Some(why.to_owned())
        } else {
            let offer = wheel::offer(kind, &self.kiln, self.available(), &paths);
            wheel::offer_sentence(kind, &offer)
        };
        match said {
            Some(text) => {
                self.alert(text);
                self.invalidate();
            }
            // The wheel is for a drop and the drag-wheel; the choice of a
            // format from the window is the list, which keeps off the pane.
            None if kind == WheelKind::Convert => self.open_list(paths, at, keyboard),
            // The tools wheel is a key's or the menu's: it keeps to the
            // list.
            None => self.open_wheel(kind, paths, at, keyboard, Pour::OverList),
        }
    }

    /// The footer's Convert, and Enter on the list: with the pane showing,
    /// convert to the format the pane suggests (its outlined one); with it
    /// folded, or with nothing suggested, open the list of formats beside
    /// the button. Or the reason it can't be used.
    fn convert_selection(&mut self, keyboard: bool) {
        if let Some(why) = self.convert_block() {
            self.alert(why);
            self.invalidate();
            return;
        }
        if let Some((ext, picks)) = self.footer_target() {
            let paths = match picks {
                insp::Picks::Row(row) => self
                    .files
                    .entries()
                    .get(row)
                    .map(|e| e.path.clone())
                    .into_iter()
                    .collect(),
                insp::Picks::Selection => self.selected_paths(),
            };
            self.choose_format(&ext, paths);
            self.invalidate();
            return;
        }
        let Some(b) = self.convert_rect() else {
            return;
        };
        let paths = self.selected_paths();
        // The list ends at the button's right edge and sits above the
        // footer.
        let above = self.layout().list.bottom();
        self.open_choice(WheelKind::Convert, paths, (b.right(), above), keyboard);
    }

    /// The format the footer's Convert converts to while the pane shows:
    /// the pane's suggested one, or else its first, and for whom. `None`
    /// with the pane folded, or when it lists no formats (a failed file, a
    /// selection with nothing in common).
    fn footer_target(&self) -> Option<(String, insp::Picks)> {
        let model = &self.live_panel()?.model;
        let k = model
            .targets()
            .iter()
            .position(|t| t.suggested)
            .unwrap_or(0);
        let (ext, picks) = model.format_pick(InspectorControl::Target(k))?;
        Some((ext.to_owned(), picks))
    }

    // The right-click menu.

    /// The middle of row `i` as it shows now, where a wheel opened from
    /// the menu pours.
    fn row_centre(&self, i: usize) -> (f32, f32) {
        let r = self.layout().rows[i];
        (r.x + r.w / 2.0, r.y - self.list.scroll() + r.h / 2.0)
    }

    /// What the menu offers for row `i`.
    fn row_state(&self, i: usize) -> RowState {
        self.files.entries()[i].row_state(self.row_saving(i))
    }

    /// The right-click menu. `at` is where it was asked for, in screen
    /// pixels, or `None` from the keyboard (the Menu key or Shift+F10),
    /// which opens it at the focus row. The selection gets the row menu,
    /// whose items apply to every selected row; an empty part of the list
    /// gets Add files and Paste.
    fn context_menu(&mut self, at: Option<(i32, i32)>) {
        // Its wheels follow the engines: it opens once they are known, if
        // the window still has the keys and nothing has opened over the
        // list meanwhile (a prompt, or another menu).
        if !self.engines.ready() {
            return self.when_engines_known(move |app| {
                if app.active && app.over().list_live() {
                    app.context_menu(at);
                }
            });
        }
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
                match self.hit_row(x, y).row {
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
            // A file the look on adding ruled out, such as a JPG cut short,
            // can have no tool run on it.
            let checked = rows.iter().all(|&i| {
                self.files
                    .entries()
                    .get(i)
                    .is_some_and(|e| e.check == kiln_core::FileCheck::Ok)
            });
            let wheels: Vec<WheelKind> = WheelKind::ALL
                .into_iter()
                .filter(|&k| checked || k != WheelKind::Tools)
                .filter(|&k| {
                    let files = &self.files;
                    let subs = |p: &Path| files.has_subtitles(p);
                    !wheel::items_for(k, &self.kiln, self.available(), &paths, &subs).is_empty()
                })
                .collect();
            let states: Vec<RowState> = rows.iter().map(|&i| self.row_state(i)).collect();
            // Make smaller, where the tools offer it for the selection.
            let smaller = checked && {
                let files = &self.files;
                let subs = |p: &Path| files.has_subtitles(p);
                wheel::items_for(
                    WheelKind::Tools,
                    &self.kiln,
                    self.available(),
                    &paths,
                    &subs,
                )
                .iter()
                .any(|i| matches!(i.action, Action::Tool(Tool::Smaller, _)))
            };
            // One finished drop's conversion, not being saved, offers its
            // Options, as its row does.
            let options = match rows.as_slice() {
                [i] => !self.row_saving(*i) && self.files.entries()[*i].options_for().is_some(),
                _ => false,
            };
            let items = ways_in::with_smaller(ways_in::menu_items(&states, &wheels), smaller);
            ways_in::with_options(items, options)
        };
        let at_dips = match (at, focus) {
            (Some((x, y)), _) => self.screen_dips(x, y),
            // From the keyboard: at the row's bottom left, scrolled into
            // view, or the list's top left with nothing selected.
            (None, Some(i)) => {
                self.scroll_to_row(i);
                let r = self.layout().rows[i];
                (r.x, r.bottom() - self.list.scroll())
            }
            (None, None) => {
                let panel = self.layout().panel;
                (panel.x + 16.0, panel.y + 16.0)
            }
        };
        // The rows by file: the list can change while the menu is up.
        let files = self.paths_of(&rows);
        let focus_file = focus.and_then(|i| self.files.entries().get(i).map(|e| e.path.clone()));
        let renderer = &self.renderer;
        let width = |s: &str, caption: bool| renderer.menu_label_width(s, caption);
        let lines = surfaces::menu_rows(&items, &width);
        let bounds = self.menu_bounds();
        self.row_menu = Some(surfaces::RowMenu::new(
            items,
            &lines,
            at_dips,
            bounds,
            files,
            focus_file,
            at.is_none(),
        ));
        self.invalidate();
    }

    /// Where the right-click menu may go: the window below the title bar,
    /// whose caption buttons would be drawn over it.
    fn menu_bounds(&self) -> layout::Rect {
        let top = self.renderer.title_bar(false).rect.h;
        let (w, h) = self.renderer.size_dips();
        layout::Rect::new(0.0, top, w, (h - top).max(0.0))
    }

    /// Close the right-click menu with nothing chosen.
    pub(super) fn close_menu(&mut self) {
        if self.row_menu.take().is_some() {
            self.invalidate();
        }
    }

    /// Do the right-click menu's line `k`, for the rows it was opened for
    /// that are still listed; it closes.
    fn menu_pick(&mut self, k: usize) {
        let Some(menu) = self.row_menu.take() else {
            return;
        };
        self.invalidate();
        let Some(&item) = menu.items.get(k) else {
            return;
        };
        let rows: Vec<usize> = menu
            .files
            .iter()
            .filter_map(|f| self.files.index_of(f))
            .collect();
        if !menu.files.is_empty() && rows.is_empty() {
            return;
        }
        let focus = menu
            .focus
            .and_then(|f| self.files.index_of(&f))
            .or(rows.first().copied());
        self.menu_chosen(item, &rows, focus, menu.keyboard);
    }

    /// What the right-click menu made of an input.
    fn menu_did(&mut self, did: surfaces::MenuDid) -> surfaces::Reply {
        match did {
            surfaces::MenuDid::Nothing => {}
            surfaces::MenuDid::Lit => self.invalidate(),
            surfaces::MenuDid::Chose(k) => self.menu_pick(k),
            surfaces::MenuDid::Close => self.close_menu(),
        }
        surfaces::Reply::Block
    }

    /// The button went down: off the menu, it closes.
    pub(super) fn menu_down(&mut self, x: f32, y: f32) -> surfaces::Reply {
        let off = self.row_menu.as_ref().is_some_and(|m| !m.contains(x, y));
        if off {
            self.close_menu();
        }
        surfaces::Reply::Block
    }

    /// The button came up: on an item, it is done.
    pub(super) fn menu_up(&mut self, x: f32, y: f32) -> surfaces::Reply {
        let Some(m) = &self.row_menu else {
            return surfaces::Reply::Pass;
        };
        let did = match m.release(x, y) {
            // Released off the menu after the press that opened it: the
            // menu stays, as a right-click's does.
            surfaces::MenuDid::Close => surfaces::MenuDid::Nothing,
            did => did,
        };
        self.menu_did(did)
    }

    /// The pointer moved: the item under it is lit.
    pub(super) fn menu_move(&mut self, x: f32, y: f32) -> surfaces::Reply {
        let Some(m) = &mut self.row_menu else {
            return surfaces::Reply::Pass;
        };
        let did = m.hover(x, y);
        self.menu_did(did)
    }

    /// A key, while the menu is up.
    pub(super) fn menu_key(&mut self, key: VIRTUAL_KEY) -> surfaces::Reply {
        let Some(m) = &mut self.row_menu else {
            return surfaces::Reply::Pass;
        };
        let did = m.key(key);
        if key == VK_APPS && did == surfaces::MenuDid::Close {
            self.menu_closed_by_key = true;
        }
        self.menu_did(did)
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
            (MenuItem::Smaller, _) => {
                let paths = self.paths_of(rows);
                self.make_smaller(paths);
            }

            (MenuItem::ShowInFolder, [i]) => self.show_row(*i),
            (MenuItem::CopyPath, _) => {
                let paths = self.paths_of(rows);
                self.copy_paths(paths);
            }
            (MenuItem::Retry, [i]) => self.retry_row(*i),
            (MenuItem::Options, [i]) => self.options_row(*i, keyboard),
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
            if w.close(Instant::now()) {
                self.animate();
            }
        }
    }

    /// Choose `spot` on the wheel: More opens its list, and anything else
    /// is picked as the wheel fades.
    fn choose(&mut self, spot: Spot) {
        if self
            .wheel
            .as_mut()
            .is_some_and(|w| w.choose(spot, Instant::now(), false))
        {
            self.animate();
        }
    }

    /// The formats a sheet converting `paths` to `ext` is between, and the
    /// settings it offers, which may be none: a pair with nothing to set
    /// still has its sheet, with what the file becomes and Convert.
    fn sheet_formats(&self, paths: &[PathBuf], ext: &str) -> Option<(Format, Format, Vec<Knob>)> {
        let reg = self.kiln.registry();
        let from = reg.format_of(paths.first()?)?.clone();
        let to = reg.format(ext)?.clone();
        // The sound slider is for formats Kiln writes itself.
        let native = kiln_media::Target::from_ext(&to.ext).is_some();
        let knobs: Vec<Knob> = kiln_core::quality::knobs(&from, &to)
            .into_iter()
            .filter(|k| *k != Knob::AudioTune || native)
            .collect();
        Some((from, to, knobs))
    }

    /// The formats on `wheel`, its ring first and then More, that have
    /// something to set for `paths`: what the footer row's options can be
    /// for. The suggestion's format, if it is one of them, goes first.
    fn settable_formats(
        &self,
        wheel: &Wheel,
        paths: &[PathBuf],
        suggestion: Option<&Suggestion>,
    ) -> Vec<String> {
        let mut found: Vec<String> = wheel
            .items
            .iter()
            .chain(wheel.more_items())
            .filter_map(|i| match &i.action {
                Action::Convert(ext) => Some(ext.as_str()),
                Action::Tool(..) => None,
            })
            .filter(|ext| {
                self.sheet_formats(paths, ext)
                    .is_some_and(|(_, _, knobs)| !knobs.is_empty())
            })
            .map(str::to_owned)
            .collect();
        if let Some(kiln_core::Suggested::Convert(ext)) = suggestion.map(|s| &s.action) {
            if let Some(at) = found.iter().position(|e| e == ext) {
                let first = found.remove(at);
                found.insert(0, first);
            }
        }
        found
    }

    /// The footer's hint on the open wheel: the suggestion's reason while
    /// nothing else is lit, or what the lit format is.
    fn wheel_hint(&self) -> Option<String> {
        self.wheel.as_ref()?.hint(&self.kiln)
    }

    /// Advance the animations: start a pulsed job, drop a faded wheel or
    /// sheet, and what the inspector showed once it has faded out.
    fn tick(&mut self) {
        let (now, animations) = (Instant::now(), theme::fluent::motion::animations());
        if self.inspector_ui.prev().is_some() && self.inspector_fading(now).is_none() {
            self.inspector_ui.drop_prev();
        }
        if self.sheet.as_ref().is_some_and(|s| s.gone(now, animations)) {
            self.sheet = None;
        }
        let Some(w) = &mut self.wheel else {
            return;
        };
        match w.tick(now, animations) {
            Tick::Idle => {}
            Tick::Gone => self.wheel = None,
            Tick::Picked {
                picked,
                paths,
                keyboard,
            } => match picked {
                // A format opens its options sheet for the same files
                // the pick was for; the sheet's Convert converts.
                Some(Picked::Options(ext)) => {
                    self.open_sheet_for(&ext, paths, keyboard);
                }
                Some(Picked::Run(action)) => self.start(action, paths, None),
                Some(Picked::More) | None => {}
            },
        }
    }

    // The options sheet.

    /// Open the options sheet for the format at `spot` on the wheel, as
    /// Tab and a right-click do, at once. The wheel fades as the sheet
    /// rises. `false` when `spot` is no format.
    fn open_sheet(&mut self, spot: Option<Spot>) -> bool {
        let Some((ext, paths, keyboard)) = self.wheel.as_ref().and_then(|w| w.options_for(spot))
        else {
            return false;
        };
        if !self.open_sheet_for(&ext, paths, keyboard) {
            return false;
        }
        self.close_wheel();
        true
    }

    /// Open the options sheet converting `paths` to `ext`: every way of
    /// choosing a format comes here, and only its Convert starts the job.
    /// From the keyboard the focus starts on Convert. `false` when Kiln
    /// doesn't know the formats.
    fn open_sheet_for(&mut self, ext: &str, paths: Vec<PathBuf>, keyboard: bool) -> bool {
        let Some((from, to, knobs)) = self.sheet_formats(&paths, ext) else {
            return false;
        };
        // A pair with nothing to set never opens it (UX-08).
        if knobs.is_empty() {
            return false;
        }
        let id = self.next_sheet;
        self.next_sheet += 1;
        // Before the sheet's own look at the files comes back: what needs
        // no reading, and what the rows' facts already say of their tracks.
        let notes = model::notes_from_rows(&self.kiln, &self.files, &paths, &to.ext);
        let text_only = model::text_only_for(&self.kiln, &paths, &to.ext);
        let quality = self.prefs.settings.quality.quality_for(&from, &to);
        // As the rows read them when they were added: nothing is read here.
        let bytes = paths
            .iter()
            .filter_map(|p| self.files.entries().get(self.files.index_of(p)?))
            .map(|e| e.size)
            .sum();
        // One file: its row's own facts line, for the dialog's sub-line.
        let facts = match paths.as_slice() {
            [one] => self
                .files
                .index_of(one)
                .and_then(|i| self.files.entries().get(i))
                .map(|e| model::row_facts(e.facts.as_ref(), &e.code, e.size)),
            _ => None,
        };
        let sheet = Sheet::new(
            Opening {
                id,
                paths,
                from,
                to,
                knobs,
                quality,
                notes,
                text_only,
                bytes,
                facts,
            },
            self.renderer.size_dips(),
            keyboard,
            Instant::now(),
        );
        // Whether Graphics card can encode is found with the files, off
        // this thread: the first look starts Media Foundation.
        let cards = sheet.wants_cards();
        // Reading media takes a moment; the sheet shows meanwhile.
        look(
            self.kiln.clone(),
            self.prefs.settings.options(),
            self.wake.clone(),
            id,
            sheet.paths().to_vec(),
            cards,
        );
        self.sheet = Some(sheet);
        self.animate();
        true
    }

    fn on_probed(&mut self, p: Probed) {
        let kiln = self.kiln.clone();
        self.drive_sheet(|s, cx| {
            s.probed(cx, p, |paths, to, infos| {
                model::sheet_notes(&kiln, paths, to, infos)
            })
        });
    }

    /// The graphics card's encoders were looked for.
    fn on_cards(&mut self, c: CardsFound) {
        self.drive_sheet(|s, cx| s.carded(cx, c));
    }

    // Copy path.

    fn copy_paths(&mut self, paths: Vec<PathBuf>) {
        let text = actions::clipboard_text(&paths);
        let n = paths.len();
        self.effect(Effect::Clipboard {
            text,
            then: Box::new(move |app, copied| {
                match copied {
                    Ok(()) => app.say(actions::copied_summary(n)),
                    Err(e) => app.alert(format!("Couldn't copy the path: {}", e.message())),
                }
                app.invalidate();
            }),
        });
    }

    // Settings.

    /// Settings.toml was read before the window was made, so its first
    /// frame is in their theme. Once the window is on screen, say if it
    /// couldn't be read.
    fn settings_loaded(&mut self, err: Option<settings::SettingsError>) {
        if let Some(e) = err {
            self.alert(format!("{e}. Using the defaults."));
            log(&self.status.text);
        }
        // Loading changed something to be told once (the drag-wheel turned
        // off, decision 3): it is said in an InfoBar at the top of the
        // window, and the file is written at once so the next start
        // doesn't say it again.
        if let Some(n) = self.prefs.settings.pending_notice.take() {
            if let Err(e) = self.prefs.store.save_background(&self.prefs.settings) {
                log(&e);
            }
            self.set_top_info(Some((String::new(), n.text().to_owned())));
        }
        self.apply_theme();
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
        // The rows follow once the answer comes (`engines_found`).
        self.look_for_engines();
    }

    /// Save after an explicit change in Settings.
    fn save_settings(&mut self) {
        // The engines may be others now, and the inspector's words with
        // the keys.
        self.look_for_engines();
        self.panel_stale();
        if let Err(e) = self.prefs.store.save(&self.prefs.settings) {
            self.alert(e);
        }
    }

    /// A contrast theme's colours while one is on; otherwise Kiln's dark
    /// or light palette, as Settings and the app mode say.
    fn apply_theme(&mut self) {
        if self.prefs.restyle() {
            style_frame(self.hwnd, dark(&self.prefs.palette), self.prefs.backdrop);
            // Tips and the frame's tooltips follow the theme's base too.
            if let Some(t) = &self.tips {
                t.set_dark(dark(&self.prefs.palette));
            }
        }
        let p = self.prefs.palette;
        // The drag-wheel follows the theme, as the window does.
        if let Some(fw) = &mut self.float {
            fw.set_palette(p);
        }
        self.invalidate();
    }

    /// Settings' page as it is laid out now, while it is open.
    #[cfg(test)]
    fn settings_layout(&self) -> Option<SettingsLayout> {
        let cx = self.surface_cx();
        Some(self.settings_ui.as_ref()?.layout(&cx))
    }

    /// What Kiln has found out that Settings shows: where each helper is
    /// (from the engines thread's latest look: nothing here searches PATH
    /// or reads the disk), Microsoft Office, and the update check.
    fn settings_facts(&self) -> settings_page::Facts {
        let known = self.engines.status.as_ref();
        let engines = known.map(|k| {
            std::array::from_fn(|i| {
                // Reset and not looked for again yet: the last look's
                // answer stands until the next.
                status_now(&self.prefs.settings, Engine::ALL[i], Some(&k[i]))
                    .unwrap_or_else(|| k[i].clone())
            })
        });
        settings_page::Facts {
            engines,
            office: self.engines.office.clone(),
            update: self.update.clone(),
            alert: None,
            system_light: self.prefs.system_light,
            contrast: self.prefs.contrast.is_some(),
            winget: self.engines.winget.is_some(),
            // As the engines thread last found it, and only for the folder
            // chosen now.
            save_to_missing: self.engines.save_to.as_ref().is_some_and(|(d, missing)| {
                *missing && self.prefs.settings.save_to.as_ref() == Some(d)
            }),
        }
    }

    /// UI Automation asked control `c` on Settings' page to do `verb`.
    fn settings_a11y(&mut self, c: Control, verb: &a11y::Verb) {
        use a11y::Verb;
        use settings_page::Set;
        let cx = Ctx::new(
            &self.renderer,
            &self.prefs.settings,
            self.renderer.size_dips(),
        )
        .hooks_refused(self.hooks_refused);
        let Some(ui) = self.settings_ui.as_mut() else {
            return;
        };
        let on = self.prefs.settings.drag_wheel;
        match (c, verb) {
            (_, Verb::Focus | Verb::ScrollIntoView) => {
                ui.focus_on(&cx, c);
                return self.invalidate();
            }
            (Control::Helper(h), Verb::Expand(open)) => {
                let open = *open;
                self.drive_settings(|s, cx| s.set_open(cx, h, open));
                return;
            }
            (Control::Theme | Control::SaveWhere | Control::Choice(_), Verb::Expand(open)) => {
                let open = *open;
                self.drive_settings(|s, cx| s.expand_combo(cx, c, open));
                return;
            }
            (Control::DragWheel, Verb::Toggle | Verb::Invoke) => {
                ui.focus_on(&cx, c);
                self.activate(c, Set::Turn(!on));
            }
            (Control::Preset(_, p), Verb::Invoke | Verb::Select) => {
                ui.focus_on(&cx, c);
                let i = kiln_core::Preset::ALL
                    .iter()
                    .position(|x| *x == p)
                    .unwrap_or(0);
                self.activate(c, Set::Pick(i));
            }
            (Control::Theme | Control::SaveWhere | Control::Choice(_), _) => {}
            (_, Verb::Invoke) => {
                ui.focus_on(&cx, c);
                self.activate(c, Set::Press);
            }
            _ => {}
        }
        self.invalidate();
    }

    /// What Settings shows changed: an open page says it, the keyboard
    /// staying on the control it was on.
    fn settings_facts_changed(&mut self) {
        let facts = self.settings_facts();
        if let Some(ui) = &mut self.settings_ui {
            ui.set_facts(facts);
            self.invalidate();
        }
    }

    /// Open help, unless a panel or dialog is up. An open wheel closes.
    fn open_help(&mut self) {
        if self.dialog_up() {
            return;
        }
        self.close_wheel();
        self.help = Some(Help::new(
            help::help_content(help::Results::of(self.held.is_some(), &self.prefs.settings)),
            Instant::now(),
        ));
        self.quiet_beneath();
        self.animate();
    }

    /// Nothing beneath a panel stays lit or acts on a release under it.
    fn quiet_beneath(&mut self) {
        self.list.quiet();
    }

    /// Open Settings with helper `h`'s card open and the keyboard on it.
    fn open_settings_at(&mut self, h: settings_page::Helper) {
        if self.settings_ui.is_none() {
            self.open_settings();
        }
        self.drive_settings(|s, cx| {
            s.set_open(cx, h, true);
            s.focus_on(cx, settings_page::Control::Helper(h));
        });
    }

    fn open_settings(&mut self) {
        self.close_wheel();
        // A helper app may have been installed since the last look: the
        // engines thread looks again, and the lines follow its answer.
        self.look_for_engines();
        self.settings_ui = Some(SettingsPanel::new(
            self.settings_facts(),
            &self.prefs.settings,
            Instant::now(),
        ));
        self.list.unlight(WindowButton::Settings);
        self.animate();
    }

    /// The one network call, and only on request. Once a newer version
    /// is known the same button opens its page instead.
    fn check_updates(&mut self) {
        match self.update {
            UpdateState::Checking => {}
            UpdateState::Available(_) => self.effect(Effect::OpenReleases),
            _ => {
                self.update = UpdateState::Checking;
                let wake = self.wake.clone();
                std::thread::spawn(move || {
                    wake.send(Event::Update(updates::fetch()));
                });
            }
        }
        self.settings_facts_changed();
        self.invalidate();
    }

    /// Press, pick or turn a control on Settings' page.
    fn activate(&mut self, c: Control, set: settings_page::Set) {
        self.drive_settings(|s, cx| s.act(cx, c, set));
    }

    /// A control on Settings' page changes a setting, as the page asked.
    fn apply_setting(&mut self, c: Control, set: settings_page::Set) {
        use settings_page::{Card, Set, A_FOLDER_I_CHOOSE, THEMES};
        let pick = |set: Set| match set {
            Set::Pick(i) => Some(i),
            _ => None,
        };
        let turn = |set: Set, now: bool| match set {
            Set::Turn(on) => on,
            _ => !now,
        };
        match c {
            // The page's own: its Expanders open there.
            Control::Helper(_) => return,
            Control::Theme => {
                let Some(t) = pick(set).and_then(|i| THEMES.get(i)) else {
                    return;
                };
                self.prefs.settings.theme = *t;
                self.apply_theme();
            }
            Control::SaveWhere => match pick(set) {
                Some(0) => self.prefs.settings.save_to = None,
                Some(1) => {
                    // A folder is chosen first; cancelled, nothing changes.
                    let start = self.prefs.settings.save_to.clone();
                    return self.effect(Effect::PickFolder {
                        title: A_FOLDER_I_CHOOSE.trim_end_matches('\u{2026}').to_owned(),
                        start,
                        then: Box::new(move |app, picked| {
                            if let Some(p) = picked {
                                app.prefs.settings.save_to = Some(p);
                                app.settings_changed(c);
                                app.settings_facts_changed();
                            } else {
                                app.invalidate();
                            }
                        }),
                    });
                }
                _ => return,
            },

            Control::Preset(s, preset) => self.prefs.settings.quality.set(s, Value::Preset(preset)),
            Control::Choice(s) => {
                let list = settings_page::choices(s, self.prefs.settings.quality.get(s));
                let Some(v) = pick(set).and_then(|i| list.get(i)) else {
                    return;
                };
                self.prefs.settings.quality.set(s, *v);
            }
            Control::DragWheel => {
                let on = turn(set, self.prefs.settings.drag_wheel);
                self.set_drag_wheel(on);
                self.settings_facts_changed();
                return;
            }
            Control::Choose(e) => {
                return self.effect(Effect::PickProgram {
                    title: format!("Choose {}.exe", e.program()),
                    then: Box::new(move |app, picked| {
                        if let Some(p) = picked {
                            app.prefs.settings.engines.set(e, Some(p));
                            app.settings_changed(c);
                        }
                    }),
                });
            }
            Control::Reset(e) => self.prefs.settings.engines.set(e, None),
            // Only the helper's fixed official page. Kiln still never
            // downloads anything itself.
            Control::Get(e) => {
                return self.effect(Effect::OpenDownload {
                    engine: e,
                    then: Box::new(move |app, opened| {
                        if !opened {
                            let card = settings_page::Helper::ALL
                                .into_iter()
                                .find(|h| h.engine() == Some(e))
                                .map(Card::Helper);
                            if let (Some(ui), Some(card)) = (&mut app.settings_ui, card) {
                                ui.set_alert(Some((
                                    card,
                                    format!("Couldn't open {}'s download page.", e.label()),
                                )));
                            }
                        }
                        app.invalidate();
                    }),
                });
            }
            Control::Winget(e) => {
                self.install_with_winget(e);
                return;
            }
            Control::CheckUpdates => {
                self.check_updates();
                return;
            }
            Control::CopyDiagnostics => {
                self.copy_diagnostics();
                return;
            }
            Control::OpenLogFolder => {
                self.open_log_folder();
                return;
            }
            // Only the one fixed page.
            Control::Donate => {
                return self.effect(Effect::Open {
                    target: PathBuf::from(settings_page::DONATE_PAGE),
                    then: Box::new(|app, opened| {
                        if !opened {
                            if let Some(ui) = &mut app.settings_ui {
                                ui.set_alert(Some((
                                    Card::Donate,
                                    settings_page::DONATE_DIDNT_OPEN.to_owned(),
                                )));
                            }
                        }
                        app.invalidate();
                    }),
                });
            }
            Control::Licences => {
                let Some(target) = notices_file() else {
                    if let Some(ui) = &mut self.settings_ui {
                        ui.set_alert(Some((
                            Card::Licences,
                            settings_page::LICENCES_DIDNT_OPEN.to_owned(),
                        )));
                    }
                    return self.invalidate();
                };
                return self.effect(Effect::OpenText {
                    target,
                    then: Box::new(|app, opened| {
                        if !opened {
                            if let Some(ui) = &mut app.settings_ui {
                                ui.set_alert(Some((
                                    Card::Licences,
                                    settings_page::LICENCES_DIDNT_OPEN.to_owned(),
                                )));
                            }
                        }
                        app.invalidate();
                    }),
                });
            }
        }
        self.settings_changed(c);
    }

    /// Install with winget: asks first, saying what runs, then opens a
    /// console that runs `winget install` for `e`. Kiln doesn't wait for
    /// it: the helper is found once it is installed, as when it was
    /// installed by hand.
    fn install_with_winget(&mut self, e: Engine) {
        let Some(prompt) = settings_page::winget_prompt(e) else {
            return;
        };
        self.ask(prompt, move |app, button| {
            if button != PromptButton::Install {
                return;
            }
            let (Some(id), Some(winget)) = (e.winget_id(), app.engines.winget.clone()) else {
                app.winget_didnt_start(e);
                return;
            };
            app.effect(Effect::RunInConsole {
                winget,
                id: id.to_owned(),
                then: Box::new(move |app, started| {
                    if !started {
                        app.winget_didnt_start(e);
                    }
                    app.invalidate();
                }),
            });
        });
    }

    /// Say on the helper's card that winget for `e` didn't start.
    fn winget_didnt_start(&mut self, e: Engine) {
        log(&format!("winget install for {} didn't start", e.label()));
        if let (Some(ui), Some(card)) = (
            &mut self.settings_ui,
            settings_page::Helper::ALL
                .into_iter()
                .find(|h| h.engine() == Some(e))
                .map(settings_page::Card::Helper),
        ) {
            ui.set_alert(Some((
                card,
                format!("Couldn't start winget for {}.", e.label()),
            )));
        }
        self.invalidate();
    }

    /// Copy diagnostics: the environment and the end of the log, on the
    /// clipboard.
    fn copy_diagnostics(&mut self) {
        let text = self.diagnostics_text();
        self.effect(Effect::Clipboard {
            text,
            then: Box::new(|app, copied| {
                match copied {
                    Ok(()) => app.say("Copied diagnostics."),
                    Err(e) => app.alert(format!("Couldn't copy the diagnostics: {}", e.message())),
                }
                app.invalidate();
            }),
        });
    }

    /// What Copy diagnostics copies, as Kiln knows it now.
    fn diagnostics_text(&self) -> String {
        let helpers = self.settings_facts().engines;
        let helper_line = |i: usize| match helpers.as_ref().map(|h| &h[i]) {
            None => "not looked for yet".to_owned(),
            Some(st) => diag::helper_state(st),
        };
        let env = diag::Environment {
            kiln: format!("{} ({})", crate::update::CURRENT, env!("KILN_COMMIT")),
            windows: system::windows_build(),
            edition: system::windows_edition(),
            runs_from: std::env::current_exe().ok(),
            helpers: Engine::ALL
                .iter()
                .enumerate()
                .map(|(i, e)| (e.label().to_owned(), helper_line(i)))
                .collect(),
        };
        let log = settings::logs_dir()
            .map(|dir| diag::read_log_tail(&dir, diag::DIAGNOSTIC_LINES))
            .unwrap_or_default();
        diag::diagnostics_text(&env, &log, std::env::var("USERPROFILE").ok().as_deref())
    }

    /// Open Kiln's log folder in Explorer, making it if there isn't one
    /// yet. The Diagnostics card says if it couldn't.
    fn open_log_folder(&mut self) {
        let didnt_open = |app: &mut App| {
            if let Some(ui) = &mut app.settings_ui {
                ui.set_alert(Some((
                    settings_page::Card::Diagnostics,
                    settings_page::LOG_FOLDER_DIDNT_OPEN.to_owned(),
                )));
            }
            app.invalidate();
        };
        // One mkdir at most, on the press that opens it.
        let Some(dir) = diag::make_logs_dir() else {
            return didnt_open(self);
        };
        self.effect(Effect::Open {
            target: dir,
            then: Box::new(move |app, opened| {
                if !opened {
                    didnt_open(app);
                }
            }),
        });
    }

    /// A control `c` on Settings' page changed a setting: it is saved, and
    /// the page follows.
    fn settings_changed(&mut self, c: Control) {
        self.save_settings();
        // What Kiln offers follows the engines, and so do the rows'
        // suggestions.
        if matches!(c, Control::Choose(_) | Control::Reset(_)) {
            self.refresh_suggestions();
        }
        // A Reset button goes with what it reset: the keyboard goes to the
        // helper's Choose.
        if let (Control::Reset(e), Some(ui)) = (c, &self.settings_ui) {
            if ui.focus() == Some(c) {
                let cx = Ctx::new(
                    &self.renderer,
                    &self.prefs.settings,
                    self.renderer.size_dips(),
                )
                .hooks_refused(self.hooks_refused);
                if let Some(ui) = &mut self.settings_ui {
                    ui.focus_on(&cx, Control::Choose(e));
                }
            }
        }
        self.settings_facts_changed();
        self.invalidate();
    }

    /// Stop row `i`'s job, or the copy of its results to another drive.
    fn cancel_row(&mut self, i: usize) {
        if self.cancel_copies(i) {
            return;
        }
        if let Some(path) = self.files.entries().get(i).map(|e| e.path.clone()) {
            self.cancel_jobs_for(&path);
        }
    }

    /// A finished drag-wheel conversion's Options: Kiln comes forward with
    /// the options sheet for its pair and its file, whose Convert redoes
    /// it (`redo`).
    fn options_row(&mut self, i: usize, keyboard: bool) {
        let Some(e) = self.files.entries().get(i) else {
            return;
        };
        let Some(ext) = e.options_for().map(str::to_owned) else {
            return;
        };
        let path = e.path.clone();
        // A result being saved stays as it is until it's done.
        if self.row_saving(i) {
            self.say(model::SAVING.to_owned());
            return;
        }
        self.open_window();
        if self.open_sheet_for(&ext, vec![path], keyboard) {
            if let Some(s) = &mut self.sheet {
                s.set_redo();
            }
        }
    }

    /// Convert `paths` again from their Options, at `quality`. The person
    /// asked for the redo, so nothing asks. What the rows hold is kept
    /// until the redo ends (`redo_ended`): a new result then takes its
    /// place, and the old one goes, as a replace's Discard takes it; a
    /// redo that fails or is cancelled gives the row its old result back,
    /// untouched. A result already saved is the person's file and stays;
    /// the new one is held, or saved beside the original under a free
    /// name, never over it. A row that is no longer a finished drop's
    /// starts as Kiln's window starts anything, asking first.
    fn redo(&mut self, action: Action, paths: Vec<PathBuf>, quality: Quality) {
        let rows: Vec<usize> = paths
            .iter()
            .filter_map(|p| self.files.index_of(p))
            .collect();
        let entries = self.files.entries();
        if rows.is_empty() || rows.iter().any(|&i| entries[i].options_for().is_none()) {
            return self.start(action, paths, Some(quality));
        }
        if rows.iter().any(|&i| self.row_saving(i)) {
            self.say(model::SAVING.to_owned());
            return;
        }
        let kept: Vec<(PathBuf, Kept)> = rows
            .iter()
            .map(|&i| (entries[i].path.clone(), Kept::of(&entries[i], &self.notes)))
            .collect();
        self.launch_as(action, paths, Some(quality), Origin::Window);
        for (p, kept) in kept {
            let Some(i) = self.files.index_of(&p) else {
                continue;
            };
            // Still a drop's conversion: it offers Options again once done.
            self.files.entries_mut()[i].from_drag = true;
            // Kept, by the job now converting the row, until it ends.
            if let Some(id) = self.motion.rows.get(i).and_then(|r| r.job) {
                self.work.redos.keep(id, p, kept);
            }
        }
        // Its old result is still held meanwhile.
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
        if let Some((title, lead, text)) = e.details(cut) {
            self.details = Some(Details::new(title, lead, text));
            self.invalidate();
        }
    }

    /// What sits over the list now.
    fn over(&self) -> Over {
        Over::of(&self.layers())
    }

    // Input.

    /// A key while a wheel is up. Tab on a lit format opens its options
    /// sheet; every other key is the wheel's.
    fn wheel_key(&mut self, key: VIRTUAL_KEY, mods: Mods) -> Reply {
        // Tab on a lit format opens its options; Shift+Tab steps back.
        if key == VK_TAB && !mods.shift {
            let hover = self.wheel.as_ref().filter(|w| w.is_open()).map(|w| w.lit());
            if let Some(hover) = hover {
                if self.open_sheet(hover) {
                    return Reply::Taken;
                }
            }
        }
        self.drive_wheel(|w, cx| w.key(cx, key, mods))
            .unwrap_or(Reply::Pass)
    }

    // The notification area.

    /// Whether the window is shown, rather than hidden in the notification
    /// area. A minimised window is still shown: it has its taskbar button.
    fn window_shown(&self) -> bool {
        unsafe { IsWindowVisible(self.hwnd) }.as_bool()
    }

    /// Put the icon back after Explorer restarted. False if it should be
    /// there and Explorer didn't take it.
    fn restore_tray(&mut self) -> bool {
        match &self.tray {
            Some(t) => t.readd(),
            None => {
                self.sync_tray();
                self.tray.is_some() || !tray::wanted(self.window_shown())
            }
        }
    }

    /// Add or take away the tray icon, as `tray::wanted` says.
    fn sync_tray(&mut self) {
        match (tray::wanted(self.window_shown()), self.tray.is_some()) {
            (true, false) => self.add_tray(),
            (false, true) => self.tray = None,
            _ => {}
        }
    }

    /// A start in the background with the drag-wheel on: Kiln stays in the
    /// notification area, its window not shown. If the drag-wheel can't
    /// watch (Windows refused its hooks) or the icon didn't take, the
    /// window shows after all: Kiln is never running where it can't be
    /// seen or used.
    fn start_hidden(&mut self) {
        if tray::resident(self.prefs.settings.drag_wheel, self.hooks_refused) {
            self.add_tray();
        }
        if self.tray.is_none() {
            self.open_window();
        }
    }

    /// Put the icon in the notification area, if it isn't there.
    fn add_tray(&mut self) {
        if self.tray.is_some() {
            return;
        }
        match Tray::add(self.hwnd, tray::small_icon(), "Kiln") {
            Ok(t) => self.tray = Some(t),
            Err(e) => log(&format!("couldn't add the tray icon: {e}")),
        }
    }

    /// The window's close button, Alt+F4 or the taskbar's Close: while the
    /// drag-wheel is on and watching, the window hides and Kiln stays in
    /// the notification area. Otherwise, the setting off or its hooks
    /// refused, Kiln quits. Running jobs ask only before a quit; they run
    /// on while hidden.
    fn close_window(&mut self) {
        // A dialog, prompt or menu of Kiln's is open: it comes forward,
        // rather than hiding with the window and waiting where no one
        // can see it.
        if system::modal_open() {
            return self.open_window();
        }
        // The watch starts afresh as Kiln goes to the notification area,
        // where it is all Kiln does: Windows may have taken a timed-out
        // hook away without a word. Refused now, Kiln quits.
        self.rearm_hooks();
        let resident = tray::resident(self.prefs.settings.drag_wheel, self.hooks_refused);
        // Hidden already, a close would only hide it: nothing to ask, and
        // only the icon to make sure of.
        if resident && !self.window_shown() {
            return self.sync_tray();
        }
        let then = if resident { Then::Hide } else { Then::Close };
        self.leave(Leave::Close, Rows::All, then);
    }

    /// Hide the window to the notification area (`Then::Hide`), once any
    /// exports already moving the rows' results are done. The icon comes only now, so a
    /// cancelled prompt leaves none behind. If Kiln can't stay resident
    /// any more (the drag-wheel turned off or its hooks refused while an
    /// export finished, or the icon didn't take), it quits instead, and a
    /// quit asks about running jobs first.
    fn hide_to_tray(&mut self) {
        let resident = tray::resident(self.prefs.settings.drag_wheel, self.hooks_refused);
        // The icon comes as the window goes: it is how Kiln is found again.
        if resident {
            self.add_tray();
        }
        let closing = tray::closing(
            resident,
            self.tray.is_some(),
            self.prefs.settings.told_about_tray,
        );
        match closing {
            Closing::Quit => self.leave(Leave::Close, Rows::All, Then::Close),
            Closing::Hide { tell } => {
                // Hidden, it has no taskbar button either.
                self.effect(Effect::Show(SW_HIDE));
                if tell {
                    self.prefs.settings.told_about_tray = true;
                    if let Err(e) = self.prefs.store.save_background(&self.prefs.settings) {
                        log(&e);
                    }
                    self.notify("Kiln", tray::FIRST_HIDE);
                }
            }
        }
    }

    /// Show the window, from the notification area or a second launch,
    /// and bring it forward. The tray icon goes with it.
    fn open_window(&mut self) {
        if !self.window_shown() {
            self.effect(Effect::Show(SW_SHOW));
            // Back from the notification area: the watch starts afresh.
            self.afterwards(App::rearm_hooks);
        }
        self.effect(Effect::BringForward);
        // Once it shows: the icon goes with the window shown.
        self.afterwards(|app| {
            app.sync_tray();
            // Shown again, what moves moves again.
            app.animate();
        });
    }

    /// Signing out or shutting down (`WM_ENDSESSION`): the holding folder
    /// is kept, results and all, for the next start to deliver; jobs stop
    /// and the window goes. Nothing waits or asks: a prompt or picker
    /// still queued is dropped unasked, and one open goes with the window.
    fn end_session(&mut self) {
        if self.destroyed {
            return;
        }
        self.keep_holding = true;
        self.close_when_saved = false;
        self.leaving = None;
        self.work.runner.cancel_all();
        // A copy to another drive stops, and its held result stays whole.
        for x in self.exports.values() {
            x.cancel.cancel();
        }
        self.work.pending.clear();
        // Folder walks stop, and add nothing more.
        self.incoming.walks.clear();
        self.effects.clear();
        // A prompt open in the window goes unanswered: it never discards,
        // saves or closes anything.
        self.drop_asks();
        self.effect(Effect::Destroy);
    }

    /// Quit Kiln, from the tray menu or once a quit that waited for a
    /// delivery can go ahead. It goes as closing went before the
    /// drag-wheel: jobs still running ask first, over the window.
    fn quit(&mut self) {
        if !self.window_shown() && self.quit_asks() {
            self.open_window();
        }
        self.leave(Leave::Close, Rows::All, Then::Close);
    }

    /// Whether quitting may ask or wait first: jobs running, or a delivery
    /// or a leave under way.
    fn quit_asks(&self) -> bool {
        self.close_waits()
            || self.leaving.is_some()
            || self
                .files
                .entries()
                .iter()
                .any(|e| matches!(e.job, JobState::Running { .. }))
    }

    /// Turn the drag-wheel on or off, from the tray menu or Settings, and
    /// keep it. Its hooks come and go with it. Turned off while the window
    /// is hidden, Kiln stays hidden, and the icon with it, until the window
    /// is opened; its next close quits.
    fn set_drag_wheel(&mut self, on: bool) {
        if self.prefs.settings.drag_wheel == on {
            // On again while Windows refused the hooks: ask once more,
            // and say again if it still refuses.
            if on && self.hooks_refused {
                self.arm_hooks();
                if self.hooks_refused {
                    self.alert(settings::DRAG_WHEEL_REFUSED.to_owned());
                }
            }
            return;
        }
        self.prefs.settings.drag_wheel = on;
        self.save_settings();
        // Kiln starts with Windows while it is on.
        self.autostart.follow(on);
        if !on {
            self.close_float();
        }
        self.arm_hooks();
        self.invalidate();
    }

    /// Start the drag-wheel's hooks afresh as the window hides or shows,
    /// while the setting is on: Windows quietly takes away a low-level hook
    /// that once ran too long. Cheap: only on those two changes, and only
    /// with the setting on; a wheel up stays up (none is, at those times,
    /// unless a drop just opened the window, and it closed first).
    fn rearm_hooks(&mut self) {
        if self.prefs.settings.drag_wheel && self.float.is_none() {
            self.arm_hooks();
        }
    }

    /// Start the drag-wheel's hooks afresh while the setting is on, and
    /// stop them while it is off. Afresh, since Windows quietly takes away
    /// a low-level hook that once ran too long, and after sleep; and the
    /// keys it watches for may have changed. Refused, the setting stays on
    /// and its note says why.
    fn arm_hooks(&mut self) {
        // A wheel up now belongs to the watch being replaced.
        self.close_float();
        if let Some(h) = self.hooks.take() {
            h.stop();
        }
        let was = std::mem::replace(&mut self.hooks_refused, false);
        if !self.prefs.settings.drag_wheel {
            return;
        }
        // A test never hooks the desktop's input.
        if cfg!(test) {
            return;
        }
        match Hooks::start(self.hwnd, WM_KILN_HOOK) {
            Ok(h) => self.hooks = Some(h),
            Err(e) => {
                log(&format!("the drag-wheel's hooks: {e} ({:?})", e.code));
                self.hooks_now_refused(was);
            }
        }
        self.invalidate();
    }

    /// Windows refused the drag-wheel's hooks; `was` says it had already.
    /// The setting stays on, and its card says why; a new refusal is also
    /// an alert, so a screen reader on the switch hears it.
    fn hooks_now_refused(&mut self, was: bool) {
        self.hooks_refused = true;
        if !was {
            self.alert(settings::DRAG_WHEEL_REFUSED.to_owned());
        }
    }

    fn tray_command(&mut self, c: TrayCommand) {
        // A dialog, prompt or menu of Kiln's is open: it is brought
        // forward, and nothing starts inside it, such as a second leave
        // prompt over the first.
        if system::modal_open() {
            return self.open_window();
        }
        match c {
            TrayCommand::Open => self.open_window(),
            TrayCommand::ToggleDragWheel => self.set_drag_wheel(!self.prefs.settings.drag_wheel),
            TrayCommand::Quit => self.quit(),
        }
    }

    /// Show a notification about nothing in particular: a click on it
    /// opens Kiln.
    fn notify(&mut self, title: &str, text: &str) {
        self.notify_about(
            title,
            text,
            toast::plain(title, text),
            NoticeAbout::default(),
        );
    }

    /// Show a notification about `about`. The installed Kiln shows a toast
    /// saying `words`, whose link carries the notice's id; the portable
    /// one, or when Windows refuses the toast, the balloon saying `title`
    /// and `text`: from the tray icon while the window is hidden, so Kiln
    /// never has two icons at once, and otherwise from a short-lived icon
    /// of its own.
    fn notify_about(&mut self, title: &str, text: &str, words: toast::Words, about: NoticeAbout) {
        let id = self.notices.issue(about);
        if self.notify_by == toast::Channel::Toast {
            let xml = toast::xml(&words, &handoff::notice_link(id));
            match toast::show(&xml) {
                Ok(()) => return,
                Err(e) => log(&format!("A toast wasn't shown: {e}")),
            }
        }
        self.balloon_notice = Some(id);
        #[cfg(test)]
        toast::shown::balloon(title, text);
        #[cfg(not(test))]
        {
            if self.tray.as_ref().is_some_and(|t| t.inform(title, text)) {
                return;
            }
            system::notify(self.hwnd, WM_KILN_NOTIFY, title, text);
        }
    }

    /// A balloon was clicked: open what the last one was about.
    fn notice_clicked(&mut self) {
        match self.balloon_notice.take() {
            Some(id) => {
                self.open_notice(id);
            }
            None => self.open_window(),
        }
    }

    /// Notice `id`'s link was followed, or its balloon clicked: open Kiln
    /// on the row it was about. One that told where its results were saved
    /// shows them in their folder instead, as the row's Show in folder
    /// does. An id that isn't one of this run's notices does nothing at
    /// all: false.
    fn open_notice(&mut self, id: u64) -> bool {
        let Some(about) = self.notices.get(id).cloned() else {
            return false;
        };
        if let (true, Some((folder, files))) = (about.saved, about.reveal) {
            self.reveal(folder, files);
            return true;
        }
        let row = about.row.and_then(|p| self.files.index_of(&p));
        if let (true, Some(i)) = (about.saved, row) {
            self.show_row(i);
            return true;
        }
        self.open_window();
        // A dialog, Settings or Help is up: Kiln only comes forward, and
        // nothing moves behind it.
        if system::modal_open() || self.dialog_up() || self.help.is_some() {
            return true;
        }
        if let Some(i) = row {
            self.files.click(i);
            self.scroll_to_row(i);
            self.invalidate();
        }
        true
    }

    // The floating wheel.

    /// What the drag watch decided: the wheel opens under the pointer,
    /// flips between formats and tools, or closes. A release over a lit
    /// slice may be a drop the wheel hasn't heard yet: it closes once it
    /// has, or soon after if none comes.
    fn verdict(&mut self, v: Verdict) {
        if !self.prefs.settings.drag_wheel {
            return;
        }
        match v {
            Verdict::Open { x, y, tools } => {
                // A wheel still up is replaced. The watch has opened the
                // new one, so it isn't told this one closed. One fading
                // out goes at once, so the new one opens alone.
                self.drop_float();
                self.float_fading = None;
                // Over Kiln's own window its in-window wheel is there, and
                // with a dialog of Kiln's open nothing opens.
                let on_kiln = root_at(x, y) == Some(self.hwnd);
                if !float_opens(on_kiln, self.window_shown(), system::modal_open()) {
                    self.wheel_gone();
                    return;
                }
                let kind = if tools {
                    WheelKind::Tools
                } else {
                    WheelKind::Convert
                };
                let own = self.incoming.own_drag.clone();
                // Its news is queued, never handled inside the wheel's call.
                let wake = self.wake.clone();
                let tell = move |note| {
                    wake.send(Event::Float(note));
                };
                match FloatWheel::open(tell, (x, y), kind, &self.prefs.settings, own) {
                    Ok(fw) => self.float = Some(fw),
                    Err(e) => {
                        log(&format!("the drag-wheel didn't open: {e}"));
                        self.wheel_gone();
                    }
                }
            }
            Verdict::Flip => {
                if let Some(fw) = &mut self.float {
                    fw.flip();
                }
            }
            Verdict::Close => match &self.float {
                Some(fw) if fw.lit() => {
                    self.float_closing = true;
                    unsafe {
                        SetTimer(Some(self.hwnd), FLOAT_TIMER, FLOAT_DROP_MS, None);
                    }
                }
                _ => self.close_float(),
            },
        }
    }

    /// What the floating wheel said. News from a wheel that has since
    /// closed is stale, and dropped.
    fn float_news(&mut self, note: FloatNote) {
        if !self
            .float
            .as_ref()
            .is_some_and(|fw| note.is_from(fw.hwnd()))
        {
            return;
        }
        match note.news {
            FloatNews::Files(paths) => {
                // Files dragged from another app are read as listed ones
                // are: a video found silent loses its sound formats on the
                // wheel (`on_inspected`).
                self.inspect(paths.clone());
                // What the wheel offers follows the engines: it is told
                // once they are known, if it is still the same wheel.
                let from = self.float.as_ref().map(|fw| fw.hwnd());
                self.when_engines_known(move |app| {
                    if app.float.as_ref().map(|fw| fw.hwnd()) != from {
                        return;
                    }
                    if let Some(mut fw) = app.float.take() {
                        fw.files_arrived(paths, &app.kiln, app.available());
                        app.float = Some(fw);
                    }
                });
            }
            // A drop the wheel answered with copy names its slice; any
            // other names none, and nothing happens. Either way it closes.
            FloatNews::Dropped { picked, paths } => {
                self.close_float();
                if let Some(pick) = picked {
                    self.drop_landed(paths, wheel::pick_item(&pick.item));
                }
            }
            // It has closed itself: no files came, or the drag left.
            FloatNews::Closed => {
                self.drop_float();
                self.wheel_gone();
            }
        }
    }

    /// Files were dropped on a slice of the drag-wheel, which picks
    /// `picked`. With a dialog of Kiln's open nothing starts inside it: the
    /// dialog comes forward instead. A file whose row is busy says so.
    fn drop_landed(&mut self, paths: Vec<PathBuf>, picked: Picked) {
        if system::modal_open() {
            return self.open_window();
        }
        if let Some(why) = drop_refused(&self.files, &self.saving_rows(), &paths) {
            let why = why.to_owned();
            if !self.window_shown() {
                self.notify("Kiln", &why);
            }
            return self.alert(why);
        }
        drop_on_wheel(self, paths, picked);
    }

    fn close_float(&mut self) {
        if self.drop_float() {
            self.wheel_gone();
        }
    }

    /// Close the drag-wheel's window, if it is up, and free the news it
    /// left queued. It fades out as the in-window wheel closes, and takes
    /// no drop from now on. False if there was none.
    fn drop_float(&mut self) -> bool {
        if std::mem::take(&mut self.float_closing) {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), FLOAT_TIMER);
            }
        }
        let Some(fw) = self.float.take() else {
            return false;
        };
        // One still fading before it goes now.
        self.float_fading = fw.fade_out();
        self.drop_float_notes();
        true
    }

    /// The drag-wheel has closed, however it did: the watch hears so, and
    /// the next tap of the drag opens it again rather than flipping.
    fn wheel_gone(&mut self) {
        if std::mem::take(&mut self.float_closing) {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), FLOAT_TIMER);
            }
        }
        if let Some(h) = &self.hooks {
            h.wheel_closed();
        }
    }

    // Accessibility.

    /// What the window shows, as a tree for UI Automation. `progress`
    /// holds each row's percent as UI Automation is told it.
    fn a11y_tree(&self, progress: &[Option<u32>]) -> a11y::Node {
        let cx = self.surface_cx();
        let size = self.renderer.size_dips();
        // As the tree lays the window out (`a11y::tree`): where the fold is
        // going, not each frame of the slide.
        let l = self.settled_layout();
        let saving = self.saving_rows();
        let copied: HashMap<usize, String> = (0..self.files.len())
            .filter(|&i| self.row_copying(i).is_some())
            .map(|i| (i, self.saving_line(i)))
            .collect();
        let row_links = self.row_links();
        let rows: Vec<a11y::RowIn> = self
            .files
            .entries()
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let busy = saving.contains(&e.path);
                let instead = if busy {
                    Some(copied.get(&i).map_or(model::SAVING, String::as_str))
                } else {
                    self.notes.get(&e.path).map(|(line, _)| line.as_str())
                };
                let link = l.rows.get(i).and_then(|r| {
                    let mut r = *r;
                    r.y -= self.list.scroll();
                    (!busy)
                        .then(|| e.link(self.renderer.line_cut(&r, e)))
                        .flatten()
                        .filter(|l| model::link_on_row(l, row_links))
                });
                a11y::RowIn {
                    entry: e,
                    selected: self.files.is_selected(i),
                    instead,
                    saving: busy,
                    link,
                    progress: progress.get(i).copied().flatten(),
                }
            })
            .collect();
        let status = self.footer_text();
        let convert_label = self.convert_label();
        let empty_status = self.empty_status();
        let empty = ways_in::empty_lines();
        let first_run = self.first_run();
        let empty_at = self.empty_state();
        let convert_block = self.convert_block();
        let sheet_frame = self.sheet.as_ref().map(|s| s.frame(size, cx.now));
        let frame = Frame {
            sheet: sheet_frame.as_ref(),
        };
        let settings_in = self.settings_ui.as_ref().and_then(|ui| ui.a11y(&cx, frame));
        let help_in = self.help.as_ref().map(|h| h.a11y(&cx, frame));
        // The wheel's targets that keep only the text of its files.
        let wheel_seen = self
            .wheel
            .as_ref()
            .filter(|w| w.is_open())
            .map(|w| w.a11y(&cx, frame));
        let wheel_text_only = |ext: &str| {
            wheel_seen
                .as_ref()
                .is_some_and(|w| model::text_only_for(&self.kiln, w.paths, ext))
        };
        let bar = self.title_bar();
        let input = a11y::Input {
            size,
            title: a11y::TitleIn {
                rect: bar.rect,
                back: bar.back,
                help: bar.help,
                settings: bar.settings,
                min: bar.min,
                max: bar.max,
                close: bar.close,
                maximized: self.maximized(),
                back_focused: self.settings_ui.as_ref().is_some_and(|ui| ui.on_back()),
            },
            rows,
            row_links,
            scroll: self.list.scroll(),
            focus_row: self.files.focus(),
            focus_button: self.focused_button(),
            status: &status,
            empty: &empty,
            empty_status: &empty_status,
            first_run: first_run.as_ref(),
            top_info: self.top_info.as_ref().zip(self.top_info_bar()).map(
                |((title, message), bar)| a11y::TopInfoIn {
                    title,
                    message,
                    bar,
                },
            ),
            empty_at,
            credit: empty_at.and_then(|_| self.credit()),

            convert_block: convert_block.as_deref(),
            convert_label: &convert_label,
            footer_row: self.footer_row(),
            undo: self.undo_offer(),
            wheel: wheel_seen.as_ref().map(|w| a11y::WheelIn {
                wheel: w.wheel,
                paths: w.paths,
                hover: w.hover,
                hub: &w.hub,
                flyout: w
                    .flyout
                    .as_ref()
                    .map(|(rows, place)| (rows.as_slice(), place)),
                text_only: &wheel_text_only,
            }),
            settings: settings_in,
            details: self.details.as_ref().map(|d| d.a11y(&cx, frame)),
            ask: self.prompts.ask_ui.as_ref().map(|a| a.a11y(&cx, frame)),
            tool: self.prompts.tool_ui.as_ref().map(|t| t.a11y(&cx, frame)),
            row_menu: self.row_menu.as_ref().map(|m| a11y::RowMenuIn {
                items: &m.items,
                layout: &m.layout,
                lit: m.lit(),
            }),
            help: help_in,
            sheet: self.sheet.as_ref().and_then(|s| s.a11y(&cx, frame)),
            // Folding away, it carries nothing and is left out.
            inspector: self.live_panel().map(|p| a11y::InspectorIn {
                rect: p.rect,
                model: &p.model,
                layout: &p.layout,
                file: panel_file(&p.model, self.files.entries()),
                folder: panel_folder(&p.model, self.files.entries()),
            }),
        };
        let mut tree = a11y::tree(&input);
        // Convert waits while Fit under can't be met.
        if self.sheet.as_ref().is_some_and(|s| !s.can_convert()) {
            if let Some(n) = tree.find_mut(&a11y::Id::SheetControl(SheetControl::Convert)) {
                n.states.disabled = true;
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

    /// What a UI Automation client asked of the open prompt: Invoke on a
    /// button presses it, focus moves to it, and closing the dialog (its
    /// Window pattern) is Cancel. Windows' caption buttons do what a click
    /// on them does under the prompt: Minimise and Maximise as ever, Close
    /// bringing the window and its prompt forward. Anything else waits for
    /// an answer.
    fn a11y_ask(&mut self, req: &a11y::Request) {
        use a11y::{Id, Verb};
        if !a11y::accepts(&self.a11y_tree(&[]), req) {
            return;
        }
        match (&req.id, &req.verb) {
            (Id::AskButton(i), Verb::Invoke) => {
                let i = *i;
                self.drive_ask(|a, cx| a.press_button(cx, i));
            }
            (Id::AskButton(i), Verb::Focus) => {
                let i = *i;
                self.take_focus = true;
                self.drive_ask(|a, cx| a.focus_on(cx, i));
            }
            (Id::Ask, Verb::Close) => {
                self.drive_ask(|a, cx| a.cancel(cx));
            }
            (Id::CaptionButton(c), Verb::Invoke) => self.title_act(match c {
                a11y::Caption::Min => Part::Min,
                a11y::Caption::Max => Part::Max,
                a11y::Caption::Close => Part::Close,
            }),
            _ => {}
        }
    }

    /// What a UI Automation client asked of the Make smaller or Resize
    /// dialog: a choice picked, Custom's value set, More options opened or
    /// closed, a line's switch flipped or its choice picked, a button
    /// invoked, the focus moved; closing it is Cancel. The caption buttons
    /// do what a click does. Nothing under it is reached.
    fn a11y_tool(&mut self, req: &a11y::Request) {
        use a11y::{Id, Verb};
        use surfaces::ToolStop as Stop;
        if !a11y::accepts(&self.a11y_tree(&[]), req) {
            return;
        }
        if req.verb == Verb::Focus {
            self.take_focus = true;
        }
        match (&req.id, &req.verb) {
            (Id::Choice(of, i), verb) if **of == Id::ToolChoices => {
                let i = *i;
                match verb {
                    Verb::Select | Verb::Invoke | Verb::AddToSelection => {
                        self.drive_tool(|t, cx| t.pick(cx, i));
                    }
                    Verb::Focus => {
                        self.drive_tool(|t, cx| t.focus_stop(cx, Stop::Radios));
                    }
                    _ => {}
                }
            }
            (Id::Choice(of, v), Verb::Select | Verb::Invoke) => {
                if let Id::ToolLine(k) = **of {
                    let v = *v;
                    self.drive_tool(|t, cx| t.set_line(cx, k, v));
                }
            }
            (Id::ToolCustom, Verb::SetValue(v)) => {
                let v = v.clone();
                self.drive_tool(|t, cx| t.set_custom(cx, &v));
            }
            (Id::ToolCustom, Verb::Focus) => {
                self.drive_tool(|t, cx| t.focus_stop(cx, Stop::Custom));
            }
            (Id::ToolMore, Verb::Expand(open)) => {
                let open = *open;
                self.drive_tool(|t, cx| t.expand(cx, open));
            }
            (Id::ToolMore, Verb::Invoke) => {
                let open = self
                    .prompts
                    .tool_ui
                    .as_ref()
                    .and_then(|t| t.model().smaller())
                    .is_some_and(|s| s.more.open);
                self.drive_tool(|t, cx| t.expand(cx, !open));
            }
            (Id::ToolMore, Verb::Focus) => {
                self.drive_tool(|t, cx| t.focus_stop(cx, Stop::Expander));
            }
            (Id::ToolLine(k), Verb::Toggle) => {
                let k = *k;
                self.drive_tool(|t, cx| t.toggle_line(cx, k));
            }
            (Id::ToolLine(k), Verb::Expand(open)) => {
                let (k, open) = (*k, *open);
                self.drive_tool(|t, cx| t.open_line_list(cx, k, open));
            }
            (Id::ToolLine(k), Verb::Focus) => {
                let k = *k;
                self.drive_tool(|t, cx| t.focus_stop(cx, Stop::Line(k)));
            }
            (Id::ToolButton(0), Verb::Invoke) => {
                self.drive_tool(|t, cx| t.run(cx));
            }
            (Id::ToolButton(1), Verb::Invoke) | (Id::Tool, Verb::Close) => {
                self.drive_tool(|t, cx| t.cancel(cx));
            }
            (Id::ToolButton(b), Verb::Focus) => {
                let to = if *b == 0 { Stop::Accent } else { Stop::Cancel };
                self.drive_tool(|t, cx| t.focus_stop(cx, to));
            }
            (Id::CaptionButton(c), Verb::Invoke) => self.title_act(match c {
                a11y::Caption::Min => Part::Min,
                a11y::Caption::Max => Part::Max,
                a11y::Caption::Close => Part::Close,
            }),
            _ => {}
        }
    }

    /// Do what a UI Automation client asked, as the click or key that
    /// does the same would.
    fn a11y_do(&mut self, req: a11y::Request) {
        use a11y::{Id, Verb};
        // A dialog, picker or menu of Windows' is open, and its loop still
        // delivers these. A click couldn't reach the window now, so neither
        // does this: it could start an export or a leave inside another.
        if system::modal_loop_open() {
            return;
        }
        // A prompt open in the window hears only what is asked of it, as
        // only it hears a click.
        if self.prompts.ask_ui.is_some() {
            return self.a11y_ask(&req);
        }
        if self.prompts.tool_ui.is_some() {
            return self.a11y_tool(&req);
        }
        // So does the options dialog.
        if self.sheet.is_some() {
            match (&req.id, &req.verb) {
                (Id::SheetControl(c), verb) => {
                    self.drive_sheet(|s, cx| s.uia(cx, *c, verb));
                }
                // An item of a More options line's open list.
                (Id::Choice(inner, i), Verb::Invoke | Verb::Select) => {
                    if let Id::SheetControl(c) = **inner {
                        let i = *i;
                        self.drive_sheet(|s, cx| s.uia_pick(cx, c, i));
                    }
                }
                (Id::Sheet, Verb::Close) => {
                    self.drive_sheet(|s, cx| s.cancel(cx));
                }
                _ => {}
            }
            return;
        }
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
            (Id::BackButton, Verb::Invoke) => self.title_act(Part::Back),
            (Id::CaptionButton(c), Verb::Invoke) => self.title_act(match c {
                a11y::Caption::Min => Part::Min,
                a11y::Caption::Max => Part::Max,
                a11y::Caption::Close => Part::Close,
            }),
            // Like the other UI Automation actions on the list, not a click.
            (Id::UndoRemove, Verb::Invoke) => self.undo_remove(),
            (Id::ConvertButton, Verb::Invoke) => self.convert_selection(true),
            (Id::AddFiles, Verb::Invoke) => self.add_files(true),
            (Id::FirstRunDismiss, Verb::Invoke) => self.dismiss_first_run(),
            (Id::TopInfoDismiss, Verb::Invoke) => self.set_top_info(None),
            (Id::CreditLink, Verb::Invoke) => self.open_credit(),

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
            (Id::RowOptions(p), Verb::Invoke) => {
                if let Some(i) = row(self, p) {
                    self.files.click(i);
                    self.options_row(i, true);
                }
            }
            (Id::RowLink(p), Verb::Invoke) => {
                if let Some(i) = row(self, p) {
                    self.files.click(i);
                    self.follow_link(i, true);
                }
            }
            (Id::InspectorControl(c), Verb::Invoke) => self.panel_act(*c, true),
            (Id::RowMenuItem(k), Verb::Invoke) => self.menu_pick(*k),
            (Id::WheelItem(spot), Verb::Invoke | Verb::Select) => self.choose(*spot),
            (Id::WheelItem(spot), Verb::Focus) => {
                if let Some(w) = self.wheel.as_mut().filter(|w| w.is_open()) {
                    w.point(Some(*spot), Instant::now());
                    self.animate();
                }
            }
            (Id::Setting(c) | Id::Combo(c) | Id::Toggle(c), verb) => {
                self.settings_a11y(*c, verb);
            }
            (Id::Choice(inner, i), Verb::Invoke | Verb::Select)
                if matches!(**inner, Id::Combo(_)) =>
            {
                if let Id::Combo(c) = **inner {
                    let pick = settings_page::Set::Pick(*i);
                    if let Some(ui) = &mut self.settings_ui {
                        ui.close_combo();
                    }
                    self.activate(c, pick);
                }
            }
            (Id::DetailsButton(b), Verb::Invoke) => {
                self.drive_details(|d, cx| d.act(cx, *b));
            }
            (Id::DetailsButton(b), Verb::Focus) => {
                self.drive_details(|d, cx| d.focus(cx, *b));
            }
            // Its Window pattern's Close: as Close, or Esc.
            (Id::Details, Verb::Close) => {
                self.drive_details(|d, cx| d.act(cx, DialogButton::Close));
            }
            // Close, or its Window pattern's Close.
            (Id::HelpClose, Verb::Invoke) | (Id::Help, Verb::Close) => {
                self.drive_help(|h, cx| h.act(cx, HelpButton::Close));
            }
            (Id::HelpGuide, Verb::Invoke) => {
                self.drive_help(|h, cx| h.act(cx, HelpButton::Guide));
            }
            (Id::HelpGuide | Id::HelpClose, Verb::Focus) => {
                let to = if req.id == Id::HelpGuide {
                    HelpButton::Guide
                } else {
                    HelpButton::Close
                };
                self.drive_help(|h, cx| h.focus(cx, to));
            }

            _ => {}
        }
    }

    /// What the footer says, the open wheel's hint aside.
    fn footer_text(&self) -> String {
        let running = self.burning() || !self.exports.is_empty();
        self.status.footer_text(
            &self.plain_status(),
            self.files.len(),
            running,
            self.inspector_live(),
            self.files.selected_rows().len(),
            || self.space_cancels(),
        )
    }

    /// The empty window has no footer: what the status says, past the
    /// plain "0 files.", sits under its Add files button.
    fn empty_status(&self) -> String {
        self.status.empty_text(&self.plain_status())
    }

    /// Draw the window, all of it or, with `progress_only`, just the
    /// running rows that changed (`render::progress_dirty`).
    fn paint(&mut self, progress_only: bool) {
        // A frame can come before the message that changed the selection
        // is done, such as the first after files arrive at start.
        self.follow();
        self.refresh_panel();
        // Decided here: what follow() moved asks for the whole window. The
        // request is taken as it is drawn, so the next frame starts clean.
        let whole = self.whole.replace(false);
        let progress_only = progress_only && !whole;
        let (now, animations) = (Instant::now(), theme::fluent::motion::animations());
        let empty = ways_in::empty_lines();
        let first_run = self.first_run();
        let empty_at = self.empty_state();
        let wheel_hint = self.wheel_hint();
        let saving = self.saving_rows();
        let saving_lines: HashMap<PathBuf, String> = (0..self.files.len())
            .filter(|&i| self.row_copying(i).is_some())
            .filter_map(|i| {
                Some((
                    self.files.entries().get(i)?.path.clone(),
                    self.saving_line(i),
                ))
            })
            .collect();
        self.expire_status();
        let status = self.footer_text();
        let convert_label = self.convert_label();
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
        let moves = &self.motion;
        let anim = |i: usize| moves.row_anim(i, entries.get(i), now, animations);
        let button = |b: ButtonId| moves.button(b, now);
        let cx = Ctx {
            now,
            ..self.surface_cx()
        };
        let sheet_frame = self.sheet.as_ref().map(|s| s.frame(cx.size, now));
        let frame = Frame {
            sheet: sheet_frame.as_ref(),
        };
        // The inspector's content rising in, over what it said before
        // fading out.
        let (alpha, rise) = self.inspector_rise(now);
        let live = self.inspector_live();
        let before = self.inspector_fading(now);
        let inspector = self.inspector_ui.panel().map(|p| InspectorView {
            now: page(p, alpha, rise, live),
            before: self
                .inspector_ui
                .prev()
                .zip(before)
                .map(|((p, _), a)| page(p, a, 0.0, false)),
        });
        let selected = self.files.selected_rows();
        let hover = self.list.hover();
        let view = View {
            backdrop: self.prefs.backdrop,
            title: TitleView {
                bar: self.title_bar(),
                title: titlebar::title(),
                active: self.active,
                maximized: self.maximized(),
                caption: self.caption,
                back_ring: self.settings_ui.as_ref().is_some_and(|ui| ui.on_back()),
                palette: &self.prefs.palette,
            },
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
            saving_lines: &saving_lines,

            selected,
            kind: &kind,
            preview: &preview,
            row_links,
            hover_row: hover.row,
            hover_cancel: hover.cancel,
            hover_link: hover.link,
            hover_retry: hover.retry,
            hover_options: hover.options,
            scroll: self.list.scroll(),
            drag: self.drag.value(now).clamp(0.0, 1.0),
            progress_clock: progress_clock(),
            progress_only,
            row_anim: &anim,
            status: wheel_hint.as_deref().unwrap_or(&status),
            empty: &empty,
            empty_status: &empty_status,
            first_run: first_run.as_ref(),
            top_info: self.top_info.as_ref().zip(self.top_info_bar()).map(
                |((title, message), layout)| render::TopInfoView {
                    layout,
                    title,
                    message,
                },
            ),
            empty_at,
            convert_enabled: self.convert_block().is_none(),
            convert_label: &convert_label,
            footer_row: self.footer_row(),
            credit: self.credit(),
            undo: self.undo_offer().is_some(),
            wheel: self.wheel.as_ref().map(|w| {
                let mut wv = w.view(&cx, frame, &button);
                // The hub shows the first file's thumbnail, as its row has it.
                wv.thumb = w
                    .paths()
                    .first()
                    .and_then(|p| self.files.index_of(p))
                    .and_then(|i| preview(i, PreviewSize::Row));
                wv
            }),
            menu: self.row_menu.as_ref().map(|m| render::MenuView {
                items: &m.items,
                layout: &m.layout,
                lit: m.lit(),
            }),
            details: self.details.as_ref().map(|d| d.view(&cx, frame, &button)),
            ask: self
                .prompts
                .ask_ui
                .as_ref()
                .map(|a| a.view(&cx, frame, &button)),
            tool: self
                .prompts
                .tool_ui
                .as_ref()
                .map(|t| t.view(&cx, frame, &button)),
            help: self.help.as_ref().map(|h| h.view(&cx, frame, &button)),
            inspector,
            sheet: self
                .sheet
                .as_ref()
                .and_then(|s| s.view(&cx, frame, &button)),
            settings: self
                .settings_ui
                .as_ref()
                .and_then(|ui| ui.view(&cx, frame, &button)),
        };
        #[cfg(test)]
        {
            self.drawn = (view.shown, view.inspector.is_some());
        }
        if let Err(e) = self.renderer.draw(&view) {
            log(&format!("draw failed: {e}"));
        }
        self.painted_previews = painted;
        let key = self.tip_key();
        if self.tip_cache.as_ref().is_none_or(|(k, _)| *k != key) {
            let tools = self.tip_tools();
            // Removing the tool that's showing hides it at once, and the
            // control tells the window: the tools are brought in line
            // with `App` let go (`Effect::Tips`). With the same tools at
            // the same scale there is nothing to bring in line, and a new
            // scale measures them again.
            if self.tips.is_some() {
                let pop = self.test_tip_pop(&tools);
                self.effect(Effect::Tips {
                    tools: tools.clone(),
                    dpi: self.unit_dpi(),
                    pop,
                });
            }
            self.tip_cache = Some((key, tools));
        }
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
            pictured: self.inspector_ui.pictured(),
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
        // Audio keeps its kind's tile, so its files are never asked for.
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
        if self.visible_rows().contains(&i) || self.inspector_ui.pictured().contains(&i) {
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
            .inspector_ui
            .panel()
            .into_iter()
            .chain(self.inspector_ui.prev().map(|(p, _)| p))
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
        }
        painted
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

    /// The line row `i` shows: a note standing in for it, "Saving…" (with
    /// how far a copy to another drive is), or its own.
    fn shown_line(&self, i: usize) -> Option<String> {
        let e = self.files.entries().get(i)?;
        Some(if self.row_saving(i) {
            self.saving_line(i)
        } else {
            self.notes
                .get(&e.path)
                .map_or_else(|| e.line().0, |(line, _)| line.clone())
        })
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
            scroll: self.list.scroll(),
            over: self.over(),
            add: self.add_button(),
            folder: self.folder_link(),
            title: self.cut_title().map(|(r, _)| r),
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
        let bar = self.title_bar();
        let mut tools: Vec<(usize, layout::Rect)> =
            [(TIP_SETTINGS, bar.settings), (TIP_HELP, bar.help)]
                .into_iter()
                .filter_map(|(id, r)| Some((id, r?)))
                .collect();
        tools.extend(self.convert_rect().map(|b| (TIP_CONVERT, b)));
        tools.extend(self.add_button().map(|b| (TIP_ADD, b)));
        tools.extend(self.folder_link().map(|b| (TIP_FOLDER, b)));
        tools.extend(self.cut_title().map(|(r, _)| (TIP_TITLE, r)));

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
            if matches!(e.job, JobState::Running { .. }) {
                tools.push((id + 1, clip(layout::row_cancel(&r))));
            }
        }
        tools
    }

    /// The tip `KILN_TEST_TIP` asks to have held open, each time the tools
    /// are brought in line and its tool is there: its words and the screen
    /// point under its control, so it follows the window.
    fn test_tip_pop(&self, tools: &[(usize, layout::Rect)]) -> Option<(String, i32, i32)> {
        let id = match tips::test_tip()? {
            tips::TestTip::Settings => TIP_SETTINGS,
            tips::TestTip::Help => TIP_HELP,
            tips::TestTip::Convert => TIP_CONVERT,
            tips::TestTip::Add => TIP_ADD,
            tips::TestTip::Folder => TIP_FOLDER,
        };
        let r = tools.iter().find(|(i, _)| *i == id)?.1;
        let k = self.unit_dpi() / 96.0;
        let mut p = windows::Win32::Foundation::POINT {
            x: ((r.x + r.w / 2.0) * k) as i32,
            y: (r.bottom() * k) as i32 + 6,
        };
        // SAFETY: a plain call on the window this owns, `p` a local.
        let _ = unsafe { windows::Win32::Graphics::Gdi::ClientToScreen(self.hwnd, &mut p) };
        Some((self.tip_text(id), p.x, p.y))
    }

    /// The side pane's title and the whole file name, while the name is
    /// cut to fit it.
    fn cut_title(&self) -> Option<(layout::Rect, String)> {
        let p = self.live_panel()?;
        let name = self.files.entries().get(p.model.file_row()?)?.name.as_str();
        let at = p.layout.title;
        (at.h > 0.0 && self.renderer.inspector_title(name, at.w) != name)
            .then(|| (at, name.to_owned()))
    }

    /// What tooltip `id` says, worked out as it shows.
    fn tip_text(&self, id: usize) -> String {
        match id {
            TIP_SETTINGS => help::SETTINGS_TIP.to_owned(),
            TIP_HELP => help::HELP_TIP.to_owned(),
            TIP_CONVERT => self
                .convert_block()
                .unwrap_or_else(|| help::CONVERT_TIP.to_owned()),
            TIP_ADD => help::ADD_FILES_TIP.to_owned(),
            TIP_TITLE => self.cut_title().map(|(_, name)| name).unwrap_or_default(),
            // The whole path, which the fact cuts to the folder's name.
            TIP_FOLDER => self
                .live_panel()
                .map(|p| panel_folder(&p.model, self.files.entries()))
                .unwrap_or_default()
                .to_owned(),

            _ if id >= TIP_ROWS => {
                let (i, part) = ((id - TIP_ROWS) / 3, (id - TIP_ROWS) % 3);
                match part {
                    1 => help::STOP_TIP.to_owned(),
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

/// The jobs `action` on `paths` runs: one for each file, or one for them
/// all for a merge, each with its files, its kind and its settings. A
/// convert runs at `quality`, or with no quality given, at the defaults
/// Settings > Quality has for its pair, as a drop on the drag-wheel does.
fn jobs_for(
    kiln: &Kiln,
    settings: &Settings,
    action: &Action,
    paths: &[PathBuf],
    quality: Option<Quality>,
    tool: Option<&ToolOptions>,
) -> Vec<(Vec<PathBuf>, JobKind, Options)> {
    let groups: Vec<Vec<PathBuf>> = match action {
        Action::Tool(Tool::Merge, _) => vec![paths.to_vec()],
        _ => paths.iter().map(|p| vec![p.clone()]).collect(),
    };
    groups
        .into_iter()
        .map(|group| {
            let kind = match action {
                Action::Convert(ext) => JobKind::Convert {
                    input: group[0].clone(),
                    target: ext.clone(),
                },
                Action::Tool(tool, _) => JobKind::Tool {
                    tool: *tool,
                    inputs: group.clone(),
                },
            };
            let options = match (action, quality) {
                (_, Some(q)) => settings.options_with(q),
                (Action::Convert(ext), None) => {
                    let reg = kiln.registry();
                    match (reg.format_of(&group[0]), reg.format(ext)) {
                        (Some(from), Some(to)) => settings.options_for(from, to),
                        _ => settings.options(),
                    }
                }
                _ => settings.options(),
            };
            let mut options = options;
            if let Some(t) = tool {
                options.tool = t.clone();
            }
            (group, kind, options)
        })
        .collect()
}

/// Where engine `e` is, as Settings says it now, without the disk: the
/// engines thread's latest look (`known`) where it is for the setting as
/// it is; a path just chosen as chosen; `None`, said as nothing, until a
/// look for an engine just reset has answered.
fn status_now(
    settings: &Settings,
    e: Engine,
    known: Option<&settings::EngineStatus>,
) -> Option<settings::EngineStatus> {
    use settings::EngineStatus::Chosen;
    match (settings.engines.get(e), known) {
        (Some(p), Some(k @ Chosen { path, .. })) if path == p => Some(k.clone()),
        (Some(p), _) => Some(Chosen {
            path: p.to_path_buf(),
            exists: true,
        }),
        (None, Some(Chosen { .. })) | (None, None) => None,
        (None, Some(k)) => Some(k.clone()),
    }
}

/// The licences and third-party notices beside `kiln-app.exe`, which
/// About's Open shows, if they are there.
fn notices_file() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(f) = NOTICES_HERE.with(|n| n.borrow().clone()) {
        return Some(f);
    }
    let exe = std::env::current_exe().ok()?;
    let file = exe.parent()?.join("THIRD_PARTY_NOTICES.md");
    file.is_file().then_some(file)
}

#[cfg(test)]
thread_local! {
    /// Where a test put the notices for About's Open.
    static NOTICES_HERE: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Let About's Open find the notices at `file` in this test, or (`None`)
/// beside the exe again.
#[cfg(test)]
fn notices_here(file: Option<PathBuf>) {
    NOTICES_HERE.with(|n| *n.borrow_mut() = file);
}

/// What a redo from Options keeps of its row until it ends: the row as it
/// was, with its result, its note, its job's note and whether saving
/// it failed.
#[derive(Debug, Clone, PartialEq)]
struct Kept {
    job: JobState,
    save_failed: bool,
    note: Option<(String, Tone)>,
    job_note: Option<String>,
}

impl Kept {
    fn of(e: &model::Entry, notes: &model::Notes) -> Kept {
        Kept {
            job: e.job.clone(),
            save_failed: e.save_failed,
            note: notes.get(&e.path).cloned(),
            job_note: e.job_note.clone(),
        }
    }
}

/// The rows redos from Options are converting, each with what it kept,
/// by the redo's job and the row's file.
#[derive(Debug, Default)]
struct Redos(HashMap<(JobId, PathBuf), Kept>);

impl Redos {
    fn keep(&mut self, id: JobId, path: PathBuf, kept: Kept) {
        self.0.insert((id, path), kept);
    }

    fn take(&mut self, id: JobId, path: &Path) -> Option<Kept> {
        self.0.remove(&(id, path.to_path_buf()))
    }

    /// The old results the redos of the files `picked` names keep, still
    /// held.
    fn held(&self, picked: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
        self.0
            .iter()
            .filter(|((_, p), _)| picked(p))
            .filter_map(|(_, k)| model::held_in(&k.job))
            .flatten()
            .cloned()
            .collect()
    }

    /// Forget every redo, giving the old results they kept.
    fn drain_held(&mut self) -> Vec<PathBuf> {
        let held = self.held(|_| true);
        self.0.clear();
        held
    }
}

/// Why a drop of `paths` on the drag-wheel starts nothing: a row whose
/// result is being saved (`saving`, by file) stays as it is until it is
/// saved, as Options and a redo wait; a row still converting can't
/// start again. `None` when it can go ahead.
fn drop_refused(
    files: &FileList,
    saving: &HashSet<PathBuf>,
    paths: &[PathBuf],
) -> Option<&'static str> {
    if paths.iter().any(|p| saving.contains(p)) {
        return Some(model::SAVING);
    }
    files.convert_busy(paths)
}

/// Row `entry` starts running `action`, `drop` saying the files were
/// dropped on the drag-wheel: what its row says, whether it offers Options
/// once done, and what its Retry runs.
fn start_row(entry: &mut model::Entry, action: &Action, drop: bool) {
    // A drop's conversion offers Options once it is done, and so does its
    // Retry: asked before the row is running, while it is still the
    // cancelled or failed row Retry was on.
    let from_drag = drop || entry.retries_drop(action);
    entry.job = JobState::Running {
        action: action.clone(),
        fraction: None,
        started: Some(Instant::now()),
    };
    entry.save_failed = false;
    // The last job's note was for its result: this one says its own.
    entry.job_note = None;
    entry.from_drag = from_drag;
    // What Retry runs again. A merge takes several files, so it can't be
    // retried from one row.
    entry.last = (!matches!(action, Action::Tool(Tool::Merge, _))).then(|| action.clone());
}

/// What the rows `mask` picks are running: what a leave of them would stop.
fn running_of<'a>(files: &'a FileList, mask: &[bool]) -> Vec<&'a Action> {
    files
        .entries()
        .iter()
        .zip(mask)
        .filter_map(|(e, &picked)| match &e.job {
            JobState::Running { action, .. } if picked => Some(action),
            _ => None,
        })
        .collect()
}

/// Which rows of `files` `rows` names.
fn rows_mask(files: &FileList, rows: &Rows) -> Vec<bool> {
    let mut mask = vec![matches!(rows, Rows::All); files.len()];
    if let Rows::Files(picked) = rows {
        for f in picked {
            if let Some(i) = files.index_of(f) {
                mask[i] = true;
            }
        }
    }
    mask
}

/// What a leave of the rows `picked` names would lose: the held files only
/// they hold, and the old results their redos keep, less any `saving`
/// names (an export is moving them) and any no longer there (`exists`).
fn held_for_leave(
    files: &FileList,
    redos: &Redos,
    picked: impl Fn(usize) -> bool,
    saving: &HashSet<PathBuf>,
    exists: impl Fn(&Path) -> bool,
) -> Vec<PathBuf> {
    let mut held = files.held_only_by(&picked);
    for f in redos.held(|p| files.index_of(p).is_some_and(&picked)) {
        if !held.contains(&f) {
            held.push(f);
        }
    }
    held.retain(|f| !saving.contains(f) && exists(f));
    held
}

/// Whether a leave by `then` stops its rows' jobs, redos included, once
/// its prompt is answered. A hide stops none: they run on with the window
/// hidden, so it asks only about what is held.
fn stops_jobs(then: &Then) -> bool {
    !matches!(then, Then::Hide)
}

/// Stop the redos of the rows `picked` names, as a leave of them goes
/// ahead: each row has its old result back now, as it was, and lets go
/// of its job, so the job's news when it comes changes nothing. Returns
/// the jobs to cancel.
fn give_back(
    files: &mut FileList,
    notes: &mut model::Notes,
    motion: &mut Motion,
    redos: &mut Redos,
    picked: impl Fn(usize) -> bool,
) -> Vec<JobId> {
    let mine: Vec<(JobId, PathBuf, usize)> = redos
        .0
        .keys()
        .filter_map(|(id, p)| {
            let i = files.index_of(p).filter(|&i| picked(i))?;
            let current = motion.rows.get(i).is_some_and(|r| r.job == Some(*id));
            current.then(|| (*id, p.clone(), i))
        })
        .collect();
    let mut stopped = vec![];
    for (id, p, i) in mine {
        let Some(kept) = redos.take(id, &p) else {
            continue;
        };
        restore(&mut files.entries_mut()[i], notes, kept);
        motion.row_started(i);
        if let Some(r) = motion.rows.get_mut(i) {
            r.job = None;
        }
        stopped.push(id);
    }
    stopped
}

/// Put row `e` back as a redo found it.
fn restore(e: &mut model::Entry, notes: &mut model::Notes, kept: Kept) {
    e.job = kept.job;
    e.save_failed = kept.save_failed;
    e.job_note = kept.job_note;
    match kept.note {
        Some(note) => notes.insert(e.path.clone(), note),
        None => notes.remove(&e.path),
    }
}

/// Job `id`, running on `paths`, has sent the `note` for its details: the
/// rows still waiting on it say it after their line (`Entry::line`), as
/// they say it once the job is done. A row that has gone, or been given a
/// new job, isn't the old job's any more.
fn row_noted(
    files: &mut FileList,
    motion: &Motion,
    id: JobId,
    paths: &[PathBuf],
    note: Option<String>,
) {
    for path in paths {
        let waiting = files
            .index_of(path)
            .filter(|&i| motion.rows.get(i).is_some_and(|r| r.job == Some(id)));
        if let Some(i) = waiting {
            files.entries_mut()[i].job_note = note.clone();
        }
    }
}

/// Job `id` has ended as `state` for the file `path`, with the `note` it
/// sent for its details, if it succeeded. The row still waiting on
/// it takes both, or, ending a redo, keeps the new result or has its old
/// one back with its old note (`redo_ended`). An old result a redo leaves
/// to no row is never removed: one not yet saved stays in the holding
/// folder, and the next start saves it. Returns the row, and whether it is
/// back to its old result.
fn job_ended(
    files: &mut FileList,
    notes: &mut model::Notes,
    motion: &mut Motion,
    redos: &mut Redos,
    (id, path): (JobId, &Path),
    (state, note): (&JobState, Option<String>),
) -> Option<(usize, bool)> {
    let kept = redos.take(id, path);
    let waiting = files
        .index_of(path)
        .filter(|&i| motion.rows.get(i).is_some_and(|r| r.job == Some(id)));
    let i = waiting?;
    let e = &mut files.entries_mut()[i];
    e.job = state.clone();
    e.job_note = note;
    let back = kept.is_some_and(|kept| redo_ended(e, notes, kept));
    if let Some(r) = motion.rows.get_mut(i) {
        r.job = None;
    }
    Some((i, back))
}

/// A redo from Options has ended for row `e`, whose job is now how it
/// ended; `kept` is the row as it was before. A new result replaces the
/// old one on the row; the old one is never removed (one not yet saved
/// stays in the holding folder for the next start to save). A redo that
/// failed or was cancelled gives the row its old result back, untouched,
/// with its note (`Entry::end_redo`). Returns whether the row is back to
/// its old result.
fn redo_ended(e: &mut model::Entry, notes: &mut model::Notes, kept: Kept) -> bool {
    let back = !matches!(e.job, JobState::Done { .. });
    let Kept {
        job,
        save_failed,
        note,
        job_note,
    } = kept;
    let _ = e.end_redo(job);
    if back {
        restore(
            e,
            notes,
            Kept {
                job: e.job.clone(),
                save_failed,
                note,
                job_note,
            },
        );
    }
    back
}

/// Where Save next to the original puts held `outputs` of `sources`, and
/// which: those a row still holds (`held`), into the folder of the first
/// source. `None` when there is nothing to save, or nowhere.
fn beside(
    sources: &[PathBuf],
    outputs: &[PathBuf],
    held: &[PathBuf],
    chosen: Option<&Path>,
) -> Option<(Vec<PathBuf>, PathBuf)> {
    let dir = match chosen {
        Some(dir) => dir.to_path_buf(),
        None => sources.first()?.parent()?.to_path_buf(),
    };
    let files: Vec<PathBuf> = outputs
        .iter()
        .filter(|f| held.contains(f))
        .cloned()
        .collect();
    (!files.is_empty()).then_some((files, dir))
}

/// Where a job writes its results: this run's holding folder `holding`,
/// or without one, straight into the folder chosen in Settings
/// (`chosen`), or else (`None`) next to each file. A chosen folder that
/// is gone or takes no files fails the job before any work, and the row
/// says so; nothing is written anywhere else.
fn written_into(holding: Option<&Path>, chosen: Option<&Path>) -> Option<PathBuf> {
    holding.or(chosen).map(Path::to_path_buf)
}

/// Record on the rows what a delivery into `folder` did, as
/// `App::exported` does: `moved` the held files that went and where, and
/// `failed` those that couldn't be saved. A row whose results all went is
/// saved there, or says it went to Documents\Kiln results instead; the
/// rest stay held, and the row says Kiln tries again at the next start.
/// The footer says nothing of it: its row and its notice speak for it, and
/// the batch's summary, its progress or an alert stands.
fn record_export(
    files: &mut FileList,
    notes: &mut model::Notes,
    folder: &Path,
    (moved, failed): (&[(PathBuf, PathBuf)], &[PathBuf]),
    present: impl Fn(&Path) -> bool,
) {
    // The rows this delivery is of: others were delivered on their own
    // (to their own folders), and what they say is not this folder's to
    // change.
    let delivered: Vec<PathBuf> = files
        .entries()
        .iter()
        .filter(|e| {
            e.held()
                .is_some_and(|h| h.iter().any(|p| moved.iter().any(|(from, _)| from == p)))
        })
        .map(|e| e.path.clone())
        .collect();
    files.exported(moved, folder);
    files.export_failed(failed);
    // A move across drives can leave the held copy behind after a good
    // copy; the export still counts, and the copy goes with the run.
    for e in files.entries() {
        match e.held() {
            Some(h) if h.iter().any(|p| failed.contains(p)) => {
                let note = if e.is_gone(&present) {
                    model::GONE.to_owned()
                } else {
                    model::KEPT_LINE.to_owned()
                };
                notes.insert(e.path.clone(), (note, Tone::Crack));
            }
            // Where it was meant to go couldn't take it: the row says it
            // went to Documents\Kiln results.
            None if delivered.contains(&e.path)
                && e.saved().is_some_and(|(to, _)| to != folder) =>
            {
                let line = model::fallback_line(&crate::actions::folder_name(folder));
                notes.insert(e.path.clone(), (line, Tone::Muted));
            }
            None if delivered.contains(&e.path) && e.saved().is_some() => {
                notes.remove(&e.path);
            }
            _ => {}
        }
    }
}

/// What a drop on the drag-wheel asks of Kiln. `App` does it; the tests
/// write it down.
trait DropDoes {
    /// List the files, as a drop on the window does.
    fn add(&mut self, paths: Vec<PathBuf>, keys: bool);
    /// Start a job as the wheel `origin` chose it, asking nothing first.
    fn launch_as(
        &mut self,
        action: Action,
        paths: Vec<PathBuf>,
        quality: Option<Quality>,
        origin: Origin,
    );
    /// Show the window and bring it forward.
    fn open_window(&mut self);
    /// Open the options sheet converting `paths` to `ext`.
    fn open_sheet_for(&mut self, ext: &str, paths: Vec<PathBuf>, keyboard: bool) -> bool;
    /// Whether any of `paths`' rows holds a result not yet saved.
    fn holds(&self, paths: &[PathBuf]) -> bool;
    /// Start a dropped job as Kiln's window does: asking first about what
    /// the rows hold. It is still a drop's: its rows offer Options once
    /// done.
    fn start(&mut self, action: Action, paths: Vec<PathBuf>, quality: Option<Quality>);
    /// Run `f` once the add is over: its folders walked, and what it
    /// asked first answered.
    fn afterwards(&mut self, f: impl FnOnce(&mut Self) + 'static)
    where
        Self: Sized,
    {
        f(self);
    }
}

impl DropDoes for App {
    fn add(&mut self, paths: Vec<PathBuf>, keys: bool) {
        App::add(self, paths, keys);
    }
    fn launch_as(
        &mut self,
        action: Action,
        paths: Vec<PathBuf>,
        quality: Option<Quality>,
        origin: Origin,
    ) {
        App::launch_as(self, action, paths, quality, origin);
    }
    fn open_window(&mut self) {
        App::open_window(self);
    }
    fn open_sheet_for(&mut self, ext: &str, paths: Vec<PathBuf>, keyboard: bool) -> bool {
        let opened = App::open_sheet_for(self, ext, paths, keyboard);
        // Options picked on the drag-wheel: its Convert is the drop's.
        if let Some(s) = self.sheet.as_mut().filter(|_| opened) {
            s.set_dropped();
        }
        opened
    }
    fn holds(&self, paths: &[PathBuf]) -> bool {
        !self.held_by(&Rows::Files(paths.to_vec())).is_empty()
    }
    fn start(&mut self, action: Action, paths: Vec<PathBuf>, quality: Option<Quality>) {
        App::start_as(self, action, paths, quality, Via::Drop);
    }
    fn afterwards(&mut self, f: impl FnOnce(&mut Self) + 'static) {
        App::after_add(self, f);
    }
}

/// Files dropped on the drag-wheel, on a slice that picks `picked`. They
/// join the list. A format, or a tool with nothing to set, starts at once
/// with the settings' defaults (`quality: None`), without the window
/// coming forward and without asking: a drop is a new request. But when
/// a row already holds a result not yet saved, Kiln comes forward and
/// asks first, as its own window does, so nothing held is lost unseen.
/// Only a pick with settings opens Kiln on its sheet.
fn drop_on_wheel(d: &mut impl DropDoes, paths: Vec<PathBuf>, picked: Picked) {
    if paths.is_empty() {
        return;
    }
    match picked {
        // Each goes on once a big folder's prompt, if one asked, is
        // answered.
        // Make smaller and Resize ask for a size in their dialog, in
        // Kiln's window: it comes forward, and the job starts from there.
        Picked::Run(action) if d.holds(&paths) || asks_first(&action) => {
            d.add(paths.clone(), false);
            d.afterwards(move |d| {
                d.open_window();
                d.start(action, paths, None);
            });
        }
        Picked::Run(action) => {
            d.add(paths.clone(), false);
            d.afterwards(move |d| d.launch_as(action, paths, None, Origin::Drag));
        }
        Picked::Options(ext) => {
            d.add(paths.clone(), false);
            d.afterwards(move |d| {
                d.open_window();
                d.open_sheet_for(&ext, paths, false);
            });
        }
        Picked::More => {}
    }
}

/// Whether `action` opens a dialog before it runs: Make smaller and Resize.
fn asks_first(action: &Action) -> bool {
    matches!(action, Action::Tool(Tool::Smaller | Tool::Resize, _))
}

/// Whether the drag-wheel opens for a tap: not when the window under the
/// pointer is Kiln's own (`on_kiln`) while it shows (`shown`), where its
/// in-window wheel is, and not while a dialog of Kiln's is open (`modal`)
fn float_opens(on_kiln: bool, shown: bool, modal: bool) -> bool {
    !(on_kiln && shown) && !modal
}

/// The top-level window under the screen point `(x, y)`, if any.
fn root_at(x: i32, y: i32) -> Option<HWND> {
    let under = unsafe { WindowFromPoint(POINT { x, y }) };
    if under.is_invalid() {
        return None;
    }
    let root = unsafe { GetAncestor(under, GA_ROOT) };
    (!root.is_invalid()).then_some(root)
}

/// A line in `kiln.log`, and for an attached debugger.
fn log(msg: &str) {
    crate::diag::log(msg);
}

/// What the log says of a job that failed: its files, what it was making,
/// its sentence and the engine's own words.
fn failed_job_line(paths: &[PathBuf], action: &Action, e: &JobError) -> String {
    let input = paths
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let target = match action {
        Action::Convert(ext) => ext.to_ascii_uppercase(),
        Action::Tool(t, _) => t.id().to_owned(),
    };
    let mut line = format!("job failed: {input} -> {target}: {}", e.message);
    if !e.details.is_empty() {
        line.push_str(" | ");
        line.push_str(&e.details);
    }
    line
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
    // Test hook: the frame log's epoch is the start of the run.
    frame_log::start();
    // The time and the Windows facts the log and crash files need.
    crate::diag::set_platform(system::DIAG);
    // Debug builds: whether a test may make the window code panic.
    #[cfg(debug_assertions)]
    crash::arm_test_panic();
    unsafe {
        // The manifest already asks for per-monitor v2; this covers runs
        // without it (for example under a debugger that strips resources).
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    // Files dropped on the app icon arrive as arguments. With Kiln already
    // open they go to that window instead. `--background` (the start-up
    // entry) with no files starts with no window shown.
    let launch = Launch::parse(std::env::args_os().skip(1));
    // A notification's `kiln:` link, which is all a launch with one
    // carries (`Launch::parse`): only the Kiln that showed the
    // notification acts on it, and none is started for it.
    if let Some(link) = &launch.link {
        if let Some(id) = handoff::notice_id(link) {
            handoff::send_notice(id);
        }
        return Ok(());
    }
    // The uninstall: Kiln's start-up entry goes, if it is this Kiln's, and
    // nothing else happens.
    if launch.forget_startup {
        forget_startup();
        return Ok(());
    }
    let hidden = launch.hidden();
    let files: Vec<PathBuf> = launch
        .files
        .into_iter()
        .map(|a| std::path::absolute(&a).unwrap_or(a))
        .collect();
    let Some(_first) = handoff::claim(&files, hidden) else {
        return Ok(());
    };
    // Only the Kiln that stays writes the log: a second one hands its files
    // to it and ends above.
    if let Some(dir) = settings::logs_dir() {
        crate::diag::init(dir);
    }
    kiln_core::log_deliveries_to(crate::diag::log);
    unsafe {
        OleInitialize(None)?;
    }
    let result = run_window(files, hidden);
    // Before the process ends: a device let go of while it ends fails
    // its exit.
    render::gpu::forget_shared_device();
    unsafe { OleUninitialize() };
    frame_log::finish();
    result
}

/// Remove the start-up entry if it is this Kiln's, on this thread: for the
/// uninstall, and a start in the background with the drag-wheel off, both
/// before any window.
fn forget_startup() {
    let (Some(key), Ok(exe)) = (RunKey::user(), std::env::current_exe()) else {
        return;
    };
    if let Err(e) = autostart::remove_if_ours(&key, &exe) {
        log(&format!("couldn't remove the start-up entry: {e}"));
    }
}

/// A Kiln started for the drag-wheel by `--background` when the setting
/// is off (changed in another way, or by a Kiln of another version):
/// it removes its start-up entry and ends. A file that couldn't be read
/// says nothing about the setting: Kiln opens its window instead, so the
/// reason shows, and the entry stays.
fn background_start(hidden: bool, settings: &Settings, unreadable: bool) -> Background {
    match (hidden, unreadable, settings.drag_wheel) {
        (false, _, _) => Background::No,
        (true, true, _) => Background::Shown,
        (true, false, false) => Background::Quit,
        (true, false, true) => Background::Hidden,
    }
}

/// How a start goes, given `--background`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Background {
    /// A start with its window, as ever.
    No,
    /// `--background`, but the window shows after all.
    Shown,
    /// `--background` with the drag-wheel off: Kiln ends.
    Quit,
    /// `--background` with the drag-wheel on: in the notification area.
    Hidden,
}

fn run_window(mut files: Vec<PathBuf>, hidden: bool) -> Result<()> {
    // The first frame, and the window's own background before it, are in
    // the theme settings.toml chooses, or the Windows app mode's. A small
    // file, read before the window is made, so a light Kiln never shows a
    // dark frame first.
    let system_light = system::system_light();
    let contrast = system::high_contrast();
    theme::fluent::motion::set_animations(system::animations_on());
    let mut settings_store = settings::Store::new(settings::default_path());
    let (settings, mut settings_error) = settings_store.load();
    let background = background_start(hidden, &settings, settings_error.is_some());
    if background == Background::Quit {
        forget_startup();
        return Ok(());
    }
    let hidden = background == Background::Hidden;
    let palette = palette_for(contrast, settings.theme, system_light);
    let instance: HINSTANCE = unsafe { GetModuleHandleW(None)? }.into();
    let class = handoff::CLASS;
    let icon = unsafe { LoadIconW(Some(instance), PCWSTR(1 as _)) }.unwrap_or_default();
    // No background brush: with no redirection bitmap nothing GDI paints
    // shows, and the window is cloaked until its first frame is presented.
    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
        lpfnWndProc: Some(wndproc),
        hInstance: instance,
        hIcon: icon,
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
        lpszClassName: class,
        ..Default::default()
    };
    if unsafe { RegisterClassW(&wc) } == 0 {
        return Err(windows::core::Error::from_thread());
    }

    let (w, h) = layout::DEFAULT_WINDOW;
    // The test placement switch opens the window on its monitor from the
    // start, so it is made at that monitor's scale.
    let (at_x, at_y) = placement::area().map_or((CW_USEDEFAULT, CW_USEDEFAULT), |a| (a.x, a.y));
    let hwnd = unsafe {
        // No redirection bitmap: the window shows only what its swap chain
        // holds, through DirectComposition (`render::gpu`).
        CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP,
            class,
            w!("Kiln"),
            WS_OVERLAPPEDWINDOW,
            at_x,
            at_y,
            w as i32,
            h as i32,
            None,
            None,
            Some(instance),
            None,
        )?
    };

    // The frame is worked out again: the title bar is Kiln's own from here
    // on (`frame_message`). What the shell says of hidden taskbars is
    // asked first, so maximising later finds it known.
    refresh_bars(hwnd);
    effects::refresh_frame(hwnd);
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96) as f32;
    style_frame(hwnd, dark(&palette), backdrop_now(contrast, settings.theme));
    // Workers reach the window on this channel; `App` keeps its end.
    let (wake, inbox) = channel::channel(hwnd);
    // Text size: its change event comes on a thread pool thread, so it
    // only sends an event and the window reads the new factor itself.
    let scale_wake = wake.clone();
    let text_scale_source = system::TextScale::new(move || {
        scale_wake.send(Event::TextScale);
    });
    let text_scale = text_scale_source.factor();
    let zoom = layout::text_zoom(text_scale, layout::default_window(text_scale));

    let (held, places) = held::open_holding();
    sweep_old_jobs();
    // Without a holding folder results go next to each file, as before,
    // and the footer keeps saying so.
    let sticky = held.is_none().then_some(model::NO_HOLDING);

    let kiln = Arc::new(Kiln::new());
    let job_wake = wake.clone();
    // A finished job's results are read for their sizes here, on the
    // runner's thread, not by the window.
    let runner = Runner::new(kiln.clone(), Runner::default_workers(), move |ev| {
        job_wake.send(Event::Job(JobNews::on_runner_thread(ev)));
    });
    // The inspect thread has its own Kiln and never touches the app's.
    let inspect_kiln = Kiln::new();
    let facts_wake = wake.clone();
    let inspector = Inspector::new(
        move |path, opts, cancel| kiln_core::inspect(&inspect_kiln, path, opts, cancel),
        move |path, facts| {
            if let Some(facts) = facts {
                facts_wake.send(Event::Inspected(Inspected { path, facts }));
            }
        },
    );

    let app = new_app(
        hwnd,
        Start {
            palette,
            contrast,
            system_light,
            settings,
            settings_store,
            dpi,
            wake,
            inbox,
            text_scale_source,
            text_scale,
            zoom,
            held,
            places,
            sticky,
            kiln,
            runner,
            inspector,
            autostart: match RunKey::user() {
                Some(key) => Autostart::new(key, std::env::current_exe().ok()),
                // Tests that start Kiln and want no start-up entry.
                None => Autostart::new(autostart::MemoryRun::default(), None),
            },
        },
    )?;
    // From here every message reaches `App` through its cell, and so does
    // the set-up below, each step with `App` let go between.
    let installed = cell::install(hwnd, AppCell::new(hwnd, app));
    let cell = installed.cell();
    // Each step of the set-up, as a message's handler: `f`, then the
    // effects it queued.
    let set_up = |f: &mut dyn FnMut(&mut App)| {
        cell.with(|app| f(app));
        run_effects(hwnd, cell);
    };
    // The tooltip control is the window's own, and goes with it. Made with
    // `App` let go: a new control talks to its owner.
    let unit_dpi = cell.with(|app| app.unit_dpi()).unwrap_or(dpi);
    let mut tips = Tips::new(hwnd, instance, unit_dpi);
    set_up(&mut |app| {
        app.tips = tips.take();
        if let Some(t) = &app.tips {
            t.set_dark(dark(&app.prefs.palette));
        }
        // The engines are looked for from the start, off this thread, and
        // waited for at most `ENGINES_WAIT_MS`.
        app.look_for_engines();
        unsafe {
            SetTimer(Some(app.hwnd), ENGINES_TIMER, ENGINES_WAIT_MS, None);
        }
        // What crashed or killed runs left is looked for off this thread
        // too, on every drive, and offered when the look answers.
        app.look_for_stale();
    });

    // Size the window for its monitor's scale and the Text size, centred
    // where Windows put it.
    // With `App` let go, before the message loop: its messages reach `App`
    // through the cell.
    effects::place_now(hwnd, place_for_dpi(hwnd, dpi, text_scale));
    set_up(&mut |app| app.rezoom());

    // Each drag event reaches `App` as a message does (`answer_drag`).
    let on_drag: drop::OnDrag = Rc::new(move |kind, ev: &DragEvent| answer_drag(hwnd, kind, ev));
    let own_drag = cell
        .with(|app| app.incoming.own_drag.clone())
        .unwrap_or_default();
    let target: IDropTarget = DropTarget::new(own_drag, on_drag).into();
    unsafe { RegisterDragDrop(hwnd, &target)? };

    // Started for the drag-wheel (`--background`): no window, only the
    // icon in the notification area, until the person opens it.
    if !hidden {
        show_first_frame(hwnd, cell, SW_SHOW);
    }
    set_up(&mut |app| {
        // A crash writes what was saved before Kiln ends.
        crash::flush_settings_with(app.prefs.store.flusher());
        app.settings_loaded(settings_error.take());
        // The drag-wheel watches from here on, while its setting is on.
        app.arm_hooks();
        if hidden {
            app.start_hidden();
        }
        // While it is on, the start-up entry is made sure of: written
        // again if it names another path, as a moved portable folder's
        // does.
        if app.prefs.settings.drag_wheel {
            app.autostart.follow(true);
        }
    });

    if !files.is_empty() {
        set_up(&mut |app| {
            let files = std::mem::take(&mut files);
            app.arrive(&files);
            app.add(files, false);
        });
    }
    // What a crashed run left is offered from here on, over a window that
    // is all there.
    unsafe {
        let _ = PostMessageW(Some(hwnd), WM_KILN_STARTED, WPARAM(0), LPARAM(0));
    }

    run_frames(hwnd, cell);
    // The loop has ended, so no message is being answered: the cell comes
    // back, and the window no longer reaches it.
    let cell = installed.finish();
    // A session that ended while `App` was borrowed keeps what's held too.
    let ended = cell.ended();
    // Dropping the app stops the workers, and drops the events still
    // queued with its inbox; events they send after that are dropped on
    // their side.
    let mut app = cell.into_app();
    let held = app.held.take();
    let mut notes = std::mem::take(&mut app.held_notes);
    let keep = app.keep_holding || ended;
    // A copy still under way (the session ended) stops, and is waited for
    // a moment, so it takes its temp file away and leaves the held result
    // whole.
    let mut copies: Vec<_> = app
        .exports
        .values_mut()
        .filter_map(|x| {
            x.cancel.cancel();
            x.thread.take()
        })
        .collect();
    // So does the delivery of what last time left: what it hasn't saved
    // yet stays, whole, for the next start.
    if let Some((cancel, t)) = app.work.delivering.take() {
        cancel.cancel();
        copies.push(t);
    }
    // Every save has been made: the settings are written, waiting up to
    // `SETTINGS_STOP`, before anything else goes. Waited too long
    // (`None`): the writer logged it and writes on.
    if let Some(Err(e)) = app.prefs.store.flush_timeout(cell::SETTINGS_STOP) {
        log(&e);
    }
    drop(app);
    wait_for_copies(copies, COPIES_STOP);
    // What the run noted is written before its folders are kept or go.
    notes.finish(NOTES_STOP);
    {
        match held {
            // Signing out or shutting down asked nothing: what's held stays
            // for the next start to deliver. The locks go with `h`.
            Some(h) if keep => {
                for dir in h.keep() {
                    log(&format!("kept {} for the next start", dir.display()));
                }
            }
            // A quit: the folders with nothing left in them go. A result
            // that couldn't be saved stays, for the next start to save.
            Some(h) => {
                for dir in h.close_empty(HELD_STOP) {
                    log(&format!("kept {} for the next start", dir.display()));
                }
            }
            None => {}
        }
    }
    Ok(())
}

/// How long the end of a session waits for cancelled copies to stop.
const COPIES_STOP: Duration = Duration::from_secs(2);
/// How long the end of the run waits for its notes to be written.
const NOTES_STOP: Duration = Duration::from_secs(5);
/// How long a quit waits for the holding folders (a folder still being
/// opened, or the start's look) before it keeps them all as they are, for
/// the next start.
const HELD_STOP: Duration = Duration::from_secs(5);

/// Wait up to `limit` for the export threads `copies` to end.
fn wait_for_copies(copies: Vec<std::thread::JoinHandle<()>>, limit: Duration) {
    let until = Instant::now() + limit;
    for c in copies {
        while !c.is_finished() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        if c.is_finished() {
            let _ = c.join();
        }
    }
}

/// How long a window drawn at the display's pace waits for its swap
/// chain before drawing anyway, in case a frame that presented nothing
/// left its waitable object unsignalled; and how long a window with no
/// device to wait on waits before it is drawn (and a device made).
const STALL_MS: u32 = float_wheel::STALL_MS;
const NO_DEVICE_MS: u32 = 16;

/// Whose frame a waitable object is for.
#[derive(Clone, Copy, PartialEq)]
enum Source {
    Main,
    Float,
}

/// A window the frame clock draws this time round: whose, the waitable it
/// waits on (`None`: none), and when it is drawn even if that never
/// signals.
struct FrameDue {
    source: Source,
    wait: Option<HANDLE>,
    at: Instant,
}

/// The message loop, with the frame clock. Every pending message is
/// handled first. While the window or the floating wheel animates, it then
/// waits for the messages and for the swap chains' frame-latency waitable
/// objects (taken again each time: a new device brings a new one); when one
/// is signalled it draws one frame for its window, through the window's own
/// procedure (`WM_KILN_FRAME`) or `float_wheel::draw_frame`.
///
/// Each window also has a deadline: a running job's progress is drawn
/// every `anim::PROGRESS_FRAME_MS`, a window with no waitable (no device yet, or its
/// signal taken already) after `NO_DEVICE_MS`, and one whose waitable
/// doesn't signal after `STALL_MS`. However the wait ends, by a signal, a
/// message or the time, every window whose deadline has passed is drawn,
/// so a stream of messages never starves a frame.
///
/// A paint taken while the frame clock draws the window is left to its next
/// frame; once nothing moves, the window is drawn once more as it now is.
/// With nothing to animate it just waits for messages, so an idle Kiln, or
/// one hidden or minimised, uses no CPU. Returns at `WM_QUIT`.
fn run_frames(hwnd: HWND, cell: &AppCell) {
    let ms = |n: u32| Duration::from_millis(u64::from(n));
    let mut last_main = Instant::now();
    // A paint was left to the frame clock, or a frame drawn while something
    // moved: owed a frame of the window as it is once nothing moves.
    let mut owed = false;
    let frame = || {
        // SAFETY: a plain send to this thread's own window, whose
        // procedure answers it like any message.
        unsafe { SendMessageW(hwnd, WM_KILN_FRAME, Some(WPARAM(0)), Some(LPARAM(0))) };
    };
    loop {
        // SAFETY: this thread's own messages, handled as a message loop
        // does.
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    return;
                }
                // While the window is drawn at the display's pace, its next
                // frame draws everything: a paint here would present a
                // frame the swap chain hasn't asked for, and wait for it.
                if msg.message == WM_PAINT && msg.hwnd == hwnd && frames_paint(cell) {
                    let _ = ValidateRect(Some(hwnd), None);
                    owed = true;
                    continue;
                }
                // The frames a modal loop drew are over: the frame clock
                // draws them again.
                if msg.message == WM_TIMER
                    && msg.hwnd == hwnd
                    && msg.wParam.0 == MODAL_FRAME_TIMER
                    && cell.with(|app| app.modal_frames_stop()).is_some()
                {
                    continue;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        let pace = cell.with(|app| app.pace()).unwrap_or(anim::Pace::Idle);
        // Nothing moves in the window any more: it is drawn once more as it
        // now is, whatever the floating wheels do.
        if pace == anim::Pace::Idle && std::mem::take(&mut owed) {
            // SAFETY: a plain call on this thread's own window.
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            continue;
        }
        let floats = float_wheel::frame_waits();
        if pace == anim::Pace::Idle && floats.is_empty() {
            let mut msg = MSG::default();
            // SAFETY: as above.
            unsafe {
                if !GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    return;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            continue;
        }
        let mut due = vec![];
        match pace {
            anim::Pace::Idle => {}
            anim::Pace::Progress => due.push(FrameDue {
                source: Source::Main,
                wait: None,
                at: last_main + ms(anim::PROGRESS_FRAME_MS),
            }),
            anim::Pace::Display => {
                // A frame whose signal is taken already is drawn soon, like
                // one with no device to wait on.
                let wait = cell
                    .with(|app| {
                        (!app.renderer.holds_signal())
                            .then(|| app.renderer.frame_waitable())
                            .flatten()
                    })
                    .flatten();
                let after = if wait.is_some() {
                    STALL_MS
                } else {
                    NO_DEVICE_MS
                };
                due.push(FrameDue {
                    source: Source::Main,
                    wait,
                    at: last_main + ms(after),
                });
            }
        }
        // Each wheel's deadlines count from its own last frame.
        for float_wheel::FrameWait { wait, framed } in floats {
            let after = if wait.is_some() {
                STALL_MS
            } else {
                NO_DEVICE_MS
            };
            due.push(FrameDue {
                source: Source::Float,
                wait,
                at: framed + ms(after),
            });
        }
        let handles: Vec<HANDLE> = due.iter().filter_map(|d| d.wait).collect();
        let now = Instant::now();
        let timeout = due
            .iter()
            .map(|d| d.at)
            .map(|at| at.saturating_duration_since(now))
            .min()
            // Rounded up: woken early, nothing would be due yet.
            .map_or(INFINITE, |left| left.as_micros().div_ceil(1000) as u32);
        // SAFETY: the handles are swap chains' waitable objects, open
        // while the windows' devices are, and taken again each time round.
        let woke = unsafe {
            MsgWaitForMultipleObjectsEx(Some(&handles), timeout, QS_ALLINPUT, MWMO_INPUTAVAILABLE)
        };
        let index = woke.0.wrapping_sub(WAIT_OBJECT_0.0) as usize;
        let signalled = handles.get(index).copied();
        let mut drawn = false;
        let mut floats_drawn = vec![];
        if let Some(h) = signalled {
            // A swap chain can take another frame.
            match due.iter().find(|d| d.wait == Some(h)).map(|d| d.source) {
                Some(Source::Main) => {
                    cell.with(|app| app.renderer.took_signal());
                    frame();
                    last_main = Instant::now();
                    drawn = true;
                }
                Some(Source::Float) => {
                    float_wheel::draw_frame(Some(h), true);
                    floats_drawn.push(Some(h));
                }
                None => {}
            }
        }
        // Whatever is due now, however the wait ended.
        let now = Instant::now();
        for d in due.iter().filter(|d| d.at <= now) {
            match d.source {
                Source::Main if !drawn => {
                    frame();
                    last_main = Instant::now();
                    drawn = true;
                }
                Source::Float if !floats_drawn.contains(&d.wait) => {
                    float_wheel::draw_frame(d.wait, false);
                    floats_drawn.push(d.wait);
                }
                _ => {}
            }
        }
        if drawn {
            owed = true;
        }
        // Messages that came are handled at the top.
    }
}

/// Whether the frame clock paints the main window: it is drawn at the
/// display's pace and has a swap chain to wait on, so its next frame comes
/// within one refresh and a `WM_PAINT` the message loop takes can wait for
/// it.
fn frames_paint(cell: &AppCell) -> bool {
    cell.with(|app| app.pace() == anim::Pace::Display && app.renderer.frame_waitable().is_some())
        .unwrap_or(false)
}

/// Have `store` tell the window each time it has written, on `wake`: a
/// write that failed shows in the footer (`App::settings_written`).
fn tell_writes(store: &mut settings::Store, wake: &Wake) {
    let written = wake.clone();
    store.on_written(move || {
        written.send(Event::SettingsWritten);
    });
}

/// What `run_window` makes before `App`, and hands it.
struct Start {
    palette: FluentPalette,
    contrast: Option<theme::SysColors>,
    system_light: bool,
    settings: Settings,
    settings_store: settings::Store,
    dpi: f32,
    wake: Wake,
    inbox: Inbox,
    text_scale_source: system::TextScale,
    text_scale: f32,
    zoom: f32,
    held: Option<Held>,
    places: Option<Places>,
    sticky: Option<&'static str>,
    kiln: Arc<Kiln>,
    runner: Runner,
    inspector: Inspector,
    autostart: Autostart,
}

/// The window's `App`, for `hwnd`, from what `run_window` made first.
fn new_app(hwnd: HWND, start: Start) -> Result<App> {
    let Start {
        palette,
        contrast,
        system_light,
        settings,
        mut settings_store,
        dpi,
        wake,
        inbox,
        text_scale_source,
        text_scale,
        zoom,
        held,
        places,
        sticky,
        kiln,
        runner,
        inspector,
        autostart,
    } = start;
    tell_writes(&mut settings_store, &wake);
    Ok(App {
        hwnd,
        wake: wake.clone(),
        inbox,
        later: VecDeque::new(),
        woke: SyncKind::Skip,
        kiln,
        work: Jobs {
            runner,
            jobs: HashMap::new(),
            redos: Redos::default(),
            batches: HashMap::new(),
            waiting: HashMap::new(),
            next_batch: 1,
            pending: vec![],
            tally: (0, 0, 0, None),
            tally_pending: HashSet::new(),
            tally_saves: model::Saves::default(),
            notice_row: None,
            result_sizes: HashMap::new(),
            delivering: None,
            delivered: None,
            info_after: None,
            scanning: false,
            taskbar: None,
            taskbar_shown: None,
        },
        inspector,
        engines: Engines::default(),
        engines_thread: workers::Engines::start(wake.clone()),
        gone_thread: workers::Gone::start(wake.clone()),
        gone_seen: GoneLooks::default(),
        incoming: Incoming {
            walks: vec![],
            next_walk: 1,
            helper_pending: vec![],
            arrivals: ways_in::Arrivals::default(),
            drag_paths: vec![],
            queued: vec![],
            last_add: None,
            own_drag: Rc::new(Cell::new(false)),
        },
        prefs: Prefs {
            backdrop: backdrop_now(contrast, settings.theme),
            settings,
            store: settings_store,
            palette,
            contrast,
            system_light,
        },
        text_scale,
        text_scale_source,
        zoom,
        autostart,
        settings_ui: None,
        last_mouse: None,
        renderer: {
            let mut r = Renderer::new(hwnd, layout::px_per_unit(dpi, zoom) * 96.0)?;
            r.set_zoom(zoom);
            r
        },
        files: FileList::default(),
        motion: Motion::new(Instant::now(), 1.0),
        list: ListUi::default(),
        drag: Tween::new(0.0, Instant::now()),
        top_info: None,
        wheel: None,
        row_menu: None,
        menu_closed_by_key: false,
        sheet: None,
        next_sheet: 1,
        details: None,
        prompts: Prompts {
            ask_ui: None,
            tool_ui: None,
            estimating: None,
            estimator: None,
            tool_dialog: 0,
            asks_waiting: VecDeque::new(),
        },
        key_repeat: false,
        #[cfg(test)]
        draw_given_up_for_test: false,
        #[cfg(test)]
        out_of_sight_for_test: None,
        help: None,
        tips: None,
        tip_cache: None,
        status: Status::new(sticky),
        update: UpdateState::default(),
        tracking_mouse: false,
        tracking_nc: false,
        caption: titlebar::Caption::default(),
        active: false,
        dpi,
        held,
        places,
        copying: HashMap::new(),
        held_notes: Notes::default(),
        last_export: None,
        started: false,
        notes: model::Notes::default(),

        exports: HashMap::new(),
        next_export: 1,
        leaving: None,
        close_when_saved: false,
        destroyed: false,
        keep_holding: false,
        helper_notice: false,
        mf_recheck: model::MfRecheck::default(),
        looked_on_activate: None,
        helper_row_looked: None,
        ext_recheck: model::ExtensionRecheck::default(),
        no_common: RefCell::new(None),
        #[cfg(test)]
        drawn: (1.0, false),
        undo: None,
        notices: toast::Issued::new(toast::seed()),
        balloon_notice: None,
        notify_by: toast::channel(
            toast::link_command().as_deref(),
            std::env::current_exe().ok().as_deref(),
        ),
        uia: uia::Host::new(hwnd, wake.clone()),
        progress: a11y::ProgressGate::default(),
        a11y_built: None,
        a11y_due: None,
        a11y_events: Vec::new(),
        take_focus: false,
        effects: Effects::default(),
        button_focus: None,

        previews: PreviewCache::new(CACHE_CAP),
        thumbs: Thumbs::start(wake),
        previews_asked: None,
        painted_previews: Vec::new(),
        inspector_ui: InspectorUi::default(),
        float: None,
        float_fading: None,
        float_closing: false,
        clock_timer: false,
        modal_frames: Cell::new(None),
        whole: Cell::new(true),
        last_pace: Cell::new(anim::Pace::Idle),
        hooks: None,
        hooks_refused: false,
        tray: None,
        tray_menu: None,
    })
}

/// Clear the job folders in `%TEMP%\Kiln` that a crash or a kill left,
/// skipping any a running job (this Kiln's or another's) is using. It runs
/// on a thread of its own, so the window never waits on it, and says in the
/// log what it removed.
fn sweep_old_jobs() {
    let root = std::env::temp_dir().join("Kiln");
    let started = std::thread::Builder::new()
        .name("sweep-old-jobs".to_owned())
        .spawn(move || {
            for dir in kiln_core::sweep_stale_jobs(&root) {
                log(&format!("cleared the old job folder {}", dir.display()));
            }
        });
    if let Err(e) = started {
        log(&format!("couldn't start clearing old job folders: {e}"));
    }
}

/// "There isn't room on C: to hold the result.", when job `e` failed for
/// want of room on the drive of the holding folder `dir` it wrote to.
fn no_room(e: &JobError, dir: Option<&Path>) -> Option<String> {
    model::no_room_line(e.no_room.as_deref()?, dir?)
}

/// The notes of where each of `outputs` in the run folder `dir` is to go:
/// the folder chosen in Settings (`chosen`), or else the folder of the
/// first of `sources`, so that if this run ends without a word, or can't
/// save it, the next start saves it there.
fn source_notes(
    dir: &Path,
    chosen: Option<&Path>,
    sources: &[PathBuf],
    outputs: &[PathBuf],
) -> Vec<Note> {
    let Some(origin) = chosen.or_else(|| sources.first().and_then(|p| p.parent())) else {
        return vec![];
    };
    outputs
        .iter()
        .filter(|o| o.parent() == Some(dir))
        .map(|out| Note::Source {
            dir: dir.to_path_buf(),
            result: out.clone(),
            origin: origin.to_path_buf(),
        })
        .collect()
}

/// Write `source_notes` now, on this thread.
#[cfg(test)]
fn record_sources(dir: &Path, sources: &[PathBuf], outputs: &[PathBuf]) {
    for note in source_notes(dir, None, sources, outputs) {
        held::write(note);
    }
}

/// The palette to draw with: a contrast theme's own colours while one is
/// on, else Kiln's light or dark.
fn palette_for(
    contrast: Option<theme::SysColors>,
    choice: settings::ThemeChoice,
    system_light: bool,
) -> FluentPalette {
    theme::fluent::palette(contrast, choice, system_light)
}

/// How many times the first frame is asked for again, while the window is
/// still cloaked, when it was lost with its device.
const FIRST_FRAME_TRIES: usize = 3;

/// Show the window as `show` says, with its first frame: it stays cloaked
/// while it is shown and its first frame is drawn, presented and committed
/// (UpdateWindow paints it), so it never shows an empty, black or white
/// client area first. A first frame lost with its device is asked for
/// again, a few times at most; then the window is uncloaked.
fn show_first_frame(hwnd: HWND, cell: &AppCell, show: SHOW_WINDOW_CMD) {
    // With the test placement switch on, shown without activating.
    let show = placement::show_cmd(show);
    cloak(hwnd, true);
    // SAFETY: plain calls on this thread's own window, with `App` let go:
    // the messages they send reach it through its cell.
    unsafe {
        let _ = ShowWindow(hwnd, show);
        let _ = UpdateWindow(hwnd);
    }
    for _ in 0..FIRST_FRAME_TRIES {
        if cell.with(|app| app.renderer.presented() > 0) != Some(false) {
            break;
        }
        // SAFETY: as above.
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, false);
            let _ = UpdateWindow(hwnd);
        }
    }
    cloak(hwnd, false);
}

/// Cloak the window, or uncloak it: a cloaked window is shown and drawn
/// but the compositor keeps it off the screen. An error leaves it as it
/// is, uncloaked on systems without cloaking.
fn cloak(hwnd: HWND, on: bool) {
    let value: BOOL = on.into();
    // SAFETY: the attribute's value is a BOOL, a local that outlives the
    // call, passed with its size.
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_CLOAK,
            &value as *const BOOL as *const core::ffi::c_void,
            std::mem::size_of::<BOOL>() as u32,
        );
    }
}

/// What is behind the window now: Mica on Windows 11 22H2 and later with
/// Transparency effects on, no contrast theme and no named theme, the
/// solid base otherwise (`titlebar::backdrop`).
fn backdrop_now(
    contrast: Option<theme::SysColors>,
    choice: settings::ThemeChoice,
) -> titlebar::Backdrop {
    titlebar::backdrop(
        system::build_number(),
        system::transparency_on(),
        contrast.is_some() || choice.is_named(),
    )
}

/// The window's frame as Windows draws it behind Kiln: dark or light, as
/// the palette is, and Mica behind the whole window, through what Kiln
/// leaves transparent, or no backdrop for the solid base. Errors are fine:
/// older systems keep the default frame, and Kiln draws its solid base.
///
/// The frame is not extended into the client area: with no redirection
/// bitmap, Mica is drawn behind the swap chain without it, and an
/// extended frame has Windows draw its own caption buttons under Kiln's.
fn style_frame(hwnd: HWND, dark: bool, backdrop: titlebar::Backdrop) {
    use windows::Win32::Graphics::Dwm::{
        DWMSBT_MAINWINDOW, DWMSBT_NONE, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE,
    };
    let dark: BOOL = dark.into();
    let kind = match backdrop {
        titlebar::Backdrop::Mica => DWMSBT_MAINWINDOW,
        titlebar::Backdrop::Solid => DWMSBT_NONE,
    };
    // SAFETY: each attribute's value is a local of its type, passed with
    // its size; the window is this thread's.
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark as *const BOOL as *const core::ffi::c_void,
            std::mem::size_of::<BOOL>() as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &kind as *const _ as *const core::ffi::c_void,
            std::mem::size_of_val(&kind) as u32,
        );
    }
}

/// Whether `p` is a dark palette, for the frame Windows draws.
fn dark(p: &FluentPalette) -> bool {
    theme::luminance(p.base) < 0.5
}

/// The resize frame Windows keeps at the sides and bottom of a window at
/// `dpi` (`titlebar::Frame`).
fn frame_at(dpi: u32) -> titlebar::Frame {
    use windows::Win32::UI::HiDpi::GetSystemMetricsForDpi;
    let dpi = dpi.max(96);
    // SAFETY: plain reads of the system's metrics.
    let m = |i| unsafe { GetSystemMetricsForDpi(i, dpi) };
    let pad = m(SM_CXPADDEDBORDER);
    titlebar::Frame {
        x: m(SM_CXSIZEFRAME) + pad,
        y: m(SM_CYSIZEFRAME) + pad,
        band: m(SM_CYSIZEFRAME),
    }
}

thread_local! {
    /// The hidden taskbars the window's last `WM_NCCALCSIZE` left room for
    /// while maximised, which the hit-test then reads without asking the
    /// taskbar on every pointer move. One main window per thread.
    static AUTOHIDE: std::cell::Cell<titlebar::AutoHide> =
        const { std::cell::Cell::new(titlebar::AutoHide { left: false, top: false, right: false, bottom: false }) };
}

thread_local! {
    /// What the shell said of each monitor's hidden taskbars
    /// (`refresh_bars`), so `WM_NCCALCSIZE` doesn't ask Explorer, which can
    /// be slow to answer, each time it runs.
    static BARS: std::cell::RefCell<titlebar::BarCache> =
        std::cell::RefCell::new(titlebar::BarCache::default());
}

/// The monitor `hwnd` is on, or nearest.
fn monitor_of(hwnd: HWND) -> windows::Win32::Graphics::Gdi::HMONITOR {
    use windows::Win32::Graphics::Gdi::{MonitorFromWindow, MONITOR_DEFAULTTONEAREST};
    // SAFETY: a plain question about this thread's own window.
    unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) }
}

/// The edges of `monitor` with a taskbar that hides itself, asked of the
/// shell (a call into Explorer).
fn ask_bars(monitor: windows::Win32::Graphics::Gdi::HMONITOR) -> titlebar::AutoHide {
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITORINFO};
    use windows::Win32::UI::Shell::{SHAppBarMessage, ABM_GETAUTOHIDEBAREX, APPBARDATA};
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: asks about a monitor handle Windows gave, into a local of
    // the size named.
    if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return titlebar::AutoHide::default();
    }
    titlebar::AutoHide::ask(|edge| {
        let mut data = APPBARDATA {
            cbSize: std::mem::size_of::<APPBARDATA>() as u32,
            uEdge: edge,
            rc: info.rcMonitor,
            ..Default::default()
        };
        // SAFETY: a question to the shell about a local APPBARDATA of the
        // size named; the answer is the hidden taskbar's window on that
        // edge, or 0.
        unsafe { SHAppBarMessage(ABM_GETAUTOHIDEBAREX, &mut data) != 0 }
    })
}

/// The taskbars may have changed (the work area, the display, a taskbar
/// made again): forget what the shell said, and ask again now about the
/// window's own monitor, so the frame that follows finds it known.
fn refresh_bars(hwnd: HWND) {
    let monitor = monitor_of(hwnd);
    BARS.with_borrow_mut(|c| {
        c.forget();
        c.get(monitor.0 as isize, || ask_bars(monitor));
    });
}

/// The edges of `hwnd`'s monitor with a taskbar that hides itself, for a
/// maximised window only: as the shell said (`BARS`), asked only for a
/// monitor not asked about since the taskbars last changed. Remembered for
/// the hit-test.
fn autohide_of(hwnd: HWND, maximized: bool) -> titlebar::AutoHide {
    let found = if maximized {
        let monitor = monitor_of(hwnd);
        BARS.with_borrow_mut(|c| c.get(monitor.0 as isize, || ask_bars(monitor)))
    } else {
        titlebar::AutoHide::default()
    };
    AUTOHIDE.set(found);
    found
}

/// The frame of `hwnd`, at its monitor's DPI.
fn frame_of(hwnd: HWND) -> titlebar::Frame {
    // SAFETY: asks only about this thread's own window.
    frame_at(unsafe { GetDpiForWindow(hwnd) })
}

/// The frame and the title bar, answered without `App`: Windows asks as
/// the window is made, before there is one, and may ask while it is
/// borrowed. `None` for any other message.
///
/// - `WM_NCCALCSIZE`: the client area is the whole window but the resize
///   frame at the sides and bottom, so the title bar is Kiln's to draw. A
///   maximised window's top frame is off the screen, and left out too.
/// - `WM_NCHITTEST`: the title bar's buttons, the bar itself and the
///   resize edges, as `titlebar::hit` says.
/// - `WM_NCACTIVATE`: Windows' answer, without painting a frame.
fn frame_message(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    match msg {
        WM_NCCALCSIZE => {
            // SAFETY: asks only about this thread's own window.
            let maximized = unsafe { IsZoomed(hwnd) }.as_bool();
            let (l, t, r, b) =
                titlebar::client_insets(frame_of(hwnd), maximized, autohide_of(hwnd, maximized));
            // SAFETY: with wparam TRUE, lparam is the system's
            // NCCALCSIZE_PARAMS, whose first rectangle is the window's new
            // place, given back as its client area; with FALSE it is that
            // rectangle alone. Either is valid and ours to change for the
            // length of the message.
            let rect = unsafe {
                if wparam.0 != 0 {
                    &mut system::lparam_mut::<NCCALCSIZE_PARAMS>(lparam)?.rgrc[0]
                } else {
                    system::lparam_mut::<RECT>(lparam)?
                }
            };
            rect.left += l;
            rect.top += t;
            rect.right -= r;
            rect.bottom -= b;
            Some(LRESULT(0))
        }
        WM_NCHITTEST => {
            let in_settings = AppCell::of(hwnd).is_some_and(|c| c.settings_open());
            Some(LRESULT(hit_test(hwnd, lparam, in_settings).code() as isize))
        }
        // lparam -1: Windows doesn't paint a frame that isn't there.
        // SAFETY: the message's own arguments, but for that one.
        WM_NCACTIVATE => Some(unsafe { DefWindowProcW(hwnd, msg, wparam, LPARAM(-1)) }),
        _ => None,
    }
}

/// What the screen point in `lparam` is on `hwnd` (`WM_NCHITTEST`), with
/// Back on the title bar while Settings is open.
fn hit_test(hwnd: HWND, lparam: LPARAM, in_settings: bool) -> titlebar::Hit {
    let mut rc = RECT::default();
    // SAFETY: asks only about this thread's own window; the rectangle is
    // a local.
    let (dpi, maximized) = unsafe {
        let _ = GetWindowRect(hwnd, &mut rc);
        (GetDpiForWindow(hwnd).max(96), IsZoomed(hwnd).as_bool())
    };
    let frame = frame_at(dpi);
    let scale = dpi as f32 / 96.0;
    let size = ((rc.right - rc.left) as f32, (rc.bottom - rc.top) as f32);
    let autohide = if maximized {
        AUTOHIDE.get()
    } else {
        titlebar::AutoHide::default()
    };
    let (l, _, r, _) = titlebar::client_insets(frame, maximized, autohide);
    let bar = titlebar::layout((size.0 - (l + r) as f32) / scale, in_settings);
    let at = ((lo(lparam) - rc.left) as f32, (hi(lparam) - rc.top) as f32);
    titlebar::hit(at, size, frame, scale, maximized, autohide, &bar)
}

/// The title bar button Windows' hit-test code `ht` is, if it is one of
/// its own: Minimise, Maximise or Close.
fn caption_part(ht: usize) -> Option<Part> {
    match ht as u32 {
        HTMINBUTTON => Some(Part::Min),
        HTMAXBUTTON => Some(Part::Max),
        HTCLOSE => Some(Part::Close),
        _ => None,
    }
}

/// The outer size of a window whose client area is `client` DIPs at
/// `dpi`, kept within the work area of the monitor it's on.
fn window_size(hwnd: HWND, dpi: f32, client: (f32, f32)) -> (i32, i32) {
    let scale = dpi / 96.0;
    let (mut w, mut h) = titlebar::outer_size(
        (
            (client.0 * scale).ceil() as i32,
            (client.1 * scale).ceil() as i32,
        ),
        frame_at(dpi as u32),
    );
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

/// The window resized, where it is, so its client area is `client` DIPs.
fn client_place(hwnd: HWND, dpi: f32, client: (f32, f32)) -> Place {
    let (w, h) = window_size(hwnd, dpi, client);
    Place {
        x: 0,
        y: 0,
        w,
        h,
        flags: SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
    }
}

/// The window's size for its monitor's scale and the Text size.
fn place_for_dpi(hwnd: HWND, dpi: f32, text_scale: f32) -> Place {
    client_place(hwnd, dpi, layout::default_window(text_scale))
}

fn lo(l: LPARAM) -> i32 {
    (l.0 & 0xFFFF) as i16 as i32
}

fn hi(l: LPARAM) -> i32 {
    ((l.0 >> 16) & 0xFFFF) as i16 as i32
}
#[cfg(test)]
mod tests {
    #[test]
    fn a_start_in_the_background_follows_the_drag_wheel() {
        use super::*;
        let on = Settings {
            drag_wheel: true,
            ..Settings::default()
        };
        let off = Settings::default();
        // Not in the background: the window, whatever the setting.
        assert_eq!(background_start(false, &on, false), Background::No);
        assert_eq!(background_start(false, &off, true), Background::No);
        // On: the notification area. Off: the entry goes and Kiln ends.
        assert_eq!(background_start(true, &on, false), Background::Hidden);
        assert_eq!(background_start(true, &off, false), Background::Quit);
        // An unreadable file says nothing about the setting: the window
        // shows, with the reason, and the entry is left alone.
        assert_eq!(background_start(true, &off, true), Background::Shown);
        assert_eq!(background_start(true, &on, true), Background::Shown);
    }

    #[test]
    fn a_failed_job_is_logged_with_what_it_was_doing() {
        use super::*;
        let failed = |details: &str| JobError {
            message: "This file is damaged.".to_owned(),
            details: details.to_owned(),
            engine: Some("FFmpeg"),
            cancelled: false,
            no_room: None,
            store_product: None,
        };
        let clip = [PathBuf::from(r"C:\in\clip.mov")];
        assert_eq!(
            failed_job_line(
                &clip,
                &Action::Convert("mp4".to_owned()),
                &failed("moov atom not found\nInvalid data")
            ),
            "job failed: C:\\in\\clip.mov -> MP4: This file is damaged. \
             | moov atom not found\nInvalid data"
        );
        // A merge names all its files; no details, no bar.
        let pdfs = [PathBuf::from(r"C:\in\a.pdf"), PathBuf::from(r"C:\in\b.pdf")];
        assert_eq!(
            failed_job_line(
                &pdfs,
                &Action::Tool(Tool::Merge, kiln_core::Kind::Document),
                &failed("")
            ),
            r"job failed: C:\in\a.pdf, C:\in\b.pdf -> merge: This file is damaged."
        );
    }

    #[test]
    fn windows_tells_its_build_and_where_kiln_is_loaded() {
        // major.minor.build.UBR, all numbers: "10.0.26300.1".
        let build = super::system::windows_build();
        let parts: Vec<&str> = build.split('.').collect();
        assert!(parts.len() == 4, "{build}");
        assert!(parts.iter().all(|p| p.parse::<u32>().is_ok()), "{build}");
        assert!(build.starts_with("10.0."), "{build}");
        // A module is loaded on a 64 KB boundary.
        let base = super::system::module_base();
        assert!(base != 0 && base % 0x10000 == 0, "{base:#x}");
    }

    /// What a wake that handled `events` asks of the tree, as `after`
    /// works it out.
    fn wake_sync(events: &[super::Event], said: bool) -> super::SyncKind {
        use super::*;
        events
            .iter()
            .map(event_sync)
            .fold(sync_kind(WM_KILN_WAKE, 0, said), SyncKind::max)
    }

    fn job_news() -> super::Event {
        super::Event::Job(super::JobNews {
            event: kiln_core::JobEvent::Notes(kiln_core::JobId(1), vec![]),
            sizes: vec![],
        })
    }

    fn a_preview() -> super::Event {
        use crate::previews::{Preview, PreviewKey, PreviewSize};
        let key = PreviewKey {
            path: "a.png".into(),
            modified: None,
            size: PreviewSize::Row,
            pixels: (32, 32),
        };
        super::Event::Preview(key, Preview::None)
    }

    fn a_request() -> super::Event {
        super::Event::A11y(crate::a11y::Request {
            id: crate::a11y::Id::Root,
            verb: crate::a11y::Verb::Invoke,
        })
    }

    #[test]
    fn a_status_set_reaches_ui_automation_at_once() {
        use super::*;
        // Job progress and animation frames wait their turn...
        assert_eq!(wake_sync(&[job_news()], false), SyncKind::Soon);
        assert_eq!(sync_kind(WM_KILN_FRAME, 0, false), SyncKind::Soon);
        // ...but not a job that set the status, such as a batch summary.
        assert_eq!(wake_sync(&[job_news()], true), SyncKind::Now);
        assert_eq!(sync_kind(WM_KILN_FRAME, 0, true), SyncKind::Now);
        // Input and the window's own news go at once.
        assert_eq!(sync_kind(WM_KEYDOWN, 0, false), SyncKind::Now);
        assert_eq!(wake_sync(&[a_request()], false), SyncKind::Now);
        // A preview changes nothing the tree shows.
        assert_eq!(wake_sync(&[a_preview()], false), SyncKind::Skip);
        assert_eq!(sync_kind(WM_SETFOCUS, 0, false), SyncKind::Now);
        // Painting never builds the tree; the flush timer builds its own.
        assert_eq!(sync_kind(WM_PAINT, 0, false), SyncKind::Skip);
        assert_eq!(sync_kind(WM_TIMER, A11Y_TIMER, false), SyncKind::Skip);
    }

    #[test]
    fn a_modal_inside_a_handler_keeps_its_events_tree_update() {
        use super::*;
        // As a handler that runs a modal loop: a nested message's `after`
        // takes `woke` while the handler runs.
        let mut woke = SyncKind::Skip;
        fold_after(
            &mut woke,
            SyncKind::Now,
            |w| w,
            |w| {
                let nested = std::mem::replace(w, SyncKind::Skip);
                assert_eq!(nested, SyncKind::Skip);
            },
        );
        assert_eq!(woke, SyncKind::Now, "the wake's own after still syncs");
        // A weaker event never lowers what an earlier one asked.
        fold_after(&mut woke, SyncKind::Soon, |w| w, |_| {});
        assert_eq!(woke, SyncKind::Now);
    }

    #[test]
    fn a_wake_asks_what_its_strongest_event_needs() {
        use super::*;
        // A wake with nothing in it changes nothing the tree shows.
        assert_eq!(wake_sync(&[], false), SyncKind::Skip);
        // Each event as its message was.
        assert_eq!(event_sync(&Event::TextScale), SyncKind::Soon);
        assert_eq!(event_sync(&job_news()), SyncKind::Soon);
        assert_eq!(event_sync(&a_request()), SyncKind::Now);
        assert_eq!(event_sync(&a_preview()), SyncKind::Skip);
        // A batch goes as its strongest.
        assert_eq!(wake_sync(&[a_preview(), job_news()], false), SyncKind::Soon);
        assert_eq!(
            wake_sync(&[job_news(), a_request(), a_preview()], false),
            SyncKind::Now
        );
        // A drop goes at once, the rest of a drag in its turn.
        assert_eq!(drag_sync(DragKind::Drop, false), SyncKind::Now);
        assert_eq!(drag_sync(DragKind::Over, false), SyncKind::Soon);
        assert_eq!(drag_sync(DragKind::Over, true), SyncKind::Now);
    }

    #[test]
    fn help_covers_the_list_like_any_panel() {
        use super::surfaces::Open;
        use super::Over;
        let over = |wheel, sheet, settings, details, help| {
            Over::of(
                &Open {
                    menu: false,
                    dialog: false,
                    tool: false,
                    sheet,
                    settings,
                    details,
                    help,
                    wheel,
                }
                .stack(),
            )
        };
        // Help alone, or over a fading wheel, is a panel: the list beneath
        // takes no clicks, links or tooltips, and a drag pours no wheel.
        for wheel in [false, true] {
            let help = over(wheel, false, false, false, true);
            assert_eq!(help, Over::Panel);
            assert!(!help.list_live());
        }
        for (sheet, settings, details) in [
            (true, false, false),
            (false, true, false),
            (false, false, true),
        ] {
            assert_eq!(over(false, sheet, settings, details, false), Over::Panel);
        }
        // A wheel keeps the list's clicks but lets a drag change it.
        let wheel = over(true, false, false, false, false);
        assert!(!wheel.list_live());
        let nothing = over(false, false, false, false, false);
        assert!(nothing.list_live());
    }

    use super::*;

    /// Every message Kiln posts itself has a number of its own: one shared
    /// would hand one message's plain values to another's handler. The wake
    /// takes none of the numbers that once carried boxes, so nothing posted
    /// for an older Kiln is taken for it.
    #[test]
    fn every_kiln_message_has_its_own_number() {
        let ids = [
            WM_KILN_WAKE,
            WM_KILN_NOTIFY,
            WM_KILN_STARTED,
            WM_KILN_HOOK,
            WM_KILN_TRAY,
            crash::WM_KILN_TEST_PANIC,
        ];
        let mut seen = HashSet::new();
        for id in ids {
            assert!(id >= WM_APP, "{id:#x} isn't an app message");
            assert!(seen.insert(id), "{id:#x} is taken twice");
        }
        // Drag, drop, job, update, probe, facts, exported, text scale,
        // UI Automation, preview, Media Foundation, floating wheel, cards,
        // extensions and recovered.
        let boxed = [1, 2, 3, 5, 6, 7, 8, 9, 10, 12, 13, 14, 17, 18, 19];
        assert!(boxed.iter().all(|n| WM_APP + n != WM_KILN_WAKE));
    }

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
                panic!("save worker test panic");
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
    fn engines_keep_the_latest_answer_and_none_before_the_first() {
        let mut engines = Engines::default();
        // Before the first answer nothing is found, and nothing is known.
        assert!(!engines.known());
        assert_eq!(engines.get(), &Availability::default());
        let first = engines.ask();
        let second = engines.ask();
        assert!(second > first);
        // The later look answers first: the earlier one's answer, which
        // comes after it, is stale and dropped.
        assert!(engines.found(second, Availability::all()));
        assert!(engines.known());
        assert!(!engines.found(first, Availability::default()));
        assert_eq!(engines.get(), &Availability::all());
        // The same answer again changes nothing; another one does.
        let third = engines.ask();
        assert!(!engines.found(third, Availability::all()));
        let fourth = engines.ask();
        assert!(engines.found(fourth, Availability::default()));
        assert_eq!(engines.get(), &Availability::default());
    }

    /// What a drop asked of Kiln, in order.
    #[derive(Debug, Clone, PartialEq)]
    enum Did {
        Add(Vec<PathBuf>, bool),
        Launch(Action, Vec<PathBuf>, Option<Quality>, Origin),
        ShowWindow,
        Sheet(String, Vec<PathBuf>),
        /// Started as the window starts: asking first.
        Start(Action, Vec<PathBuf>, Option<Quality>),
    }

    /// What a drop did, and whether its rows held unsaved results.
    #[derive(Default)]
    struct Record(Vec<Did>, bool);

    impl DropDoes for Record {
        fn add(&mut self, paths: Vec<PathBuf>, keys: bool) {
            self.0.push(Did::Add(paths, keys));
        }
        fn launch_as(
            &mut self,
            action: Action,
            paths: Vec<PathBuf>,
            quality: Option<Quality>,
            origin: Origin,
        ) {
            self.0.push(Did::Launch(action, paths, quality, origin));
        }
        fn open_window(&mut self) {
            self.0.push(Did::ShowWindow);
        }
        fn open_sheet_for(&mut self, ext: &str, paths: Vec<PathBuf>, _keyboard: bool) -> bool {
            self.0.push(Did::Sheet(ext.to_owned(), paths));
            true
        }
        fn holds(&self, _paths: &[PathBuf]) -> bool {
            self.1
        }
        fn start(&mut self, action: Action, paths: Vec<PathBuf>, quality: Option<Quality>) {
            self.0.push(Did::Start(action, paths, quality));
        }
    }

    fn format_slice(ext: &str) -> wheel::Item {
        wheel::Item {
            label: ext.to_ascii_uppercase(),
            is_code: true,
            action: Action::Convert(ext.to_owned()),
            kind: Some(kiln_core::Kind::Image),
        }
    }

    fn tool_slice(tool: Tool) -> wheel::Item {
        wheel::Item {
            label: tool.label(kiln_core::Kind::Audio).to_owned(),
            is_code: false,
            action: Action::Tool(tool, kiln_core::Kind::Audio),
            kind: None,
        }
    }

    /// The drop as `App` hears it: the item it landed on, picked as the
    /// drag-wheel picks.
    fn dropped(paths: &[PathBuf], item: &wheel::Item) -> Vec<Did> {
        let mut r = Record::default();
        drop_on_wheel(&mut r, paths.to_vec(), wheel::pick_item(item));
        r.0
    }

    #[test]
    fn a_drop_starts_one_job_with_the_quality_defaults() {
        let photo = vec![PathBuf::from(r"C:\pictures\photo.png")];
        // The files join the list, and the format starts at once with no
        // quality of its own: the defaults.
        assert_eq!(
            dropped(&photo, &format_slice("webp")),
            [
                Did::Add(photo.clone(), false),
                Did::Launch(
                    Action::Convert("webp".into()),
                    photo.clone(),
                    None,
                    Origin::Drag
                ),
            ]
        );
        // No quality runs one job with Settings > Quality's defaults for
        // the pair: here Small for pictures, not the built-in High.
        let kiln = Kiln::new();
        let mut settings = Settings::default();
        settings.quality.set(
            q::DefaultSetting::Images,
            Value::Preset(kiln_core::Preset::Small),
        );
        let action = Action::Convert("webp".into());
        let jobs = jobs_for(&kiln, &settings, &action, &photo, None, None);
        assert_eq!(jobs.len(), 1);
        let (files, kind, options) = &jobs[0];
        assert_eq!(files, &photo);
        assert_eq!(
            kind,
            &JobKind::Convert {
                input: photo[0].clone(),
                target: "webp".into()
            }
        );
        let reg = kiln.registry();
        let (png, webp) = (reg.format("png").unwrap(), reg.format("webp").unwrap());
        assert_eq!(options, &settings.options_for(png, webp));
        assert_eq!(options.quality.preset, kiln_core::Preset::Small);
        assert_ne!(options, &Settings::default().options_for(png, webp));
        // Three files dropped, three jobs, one each.
        let three: Vec<PathBuf> = ["a.png", "b.png", "c.png"].map(PathBuf::from).to_vec();
        assert_eq!(
            jobs_for(&kiln, &settings, &action, &three, None, None).len(),
            3
        );
    }

    #[test]
    fn a_tool_dialogs_options_reach_every_job() {
        let kiln = Kiln::new();
        let settings = Settings::default();
        let photos = vec![PathBuf::from("a.png"), PathBuf::from("b.png")];
        let resize = Action::Tool(Tool::Resize, kiln_core::Kind::Image);
        let chosen = ToolOptions {
            long_side: Some(1280),
            ..ToolOptions::default()
        };
        let jobs = jobs_for(&kiln, &settings, &resize, &photos, None, Some(&chosen));
        assert_eq!(jobs.len(), 2);
        assert!(jobs.iter().all(|j| j.2.tool == chosen));
        // Without a dialog's options, none are made up.
        let jobs = jobs_for(&kiln, &settings, &resize, &photos, None, None);
        assert_eq!(jobs[0].2.tool, ToolOptions::default());
    }

    #[test]
    fn a_drop_never_shows_the_window() {
        let files = vec![PathBuf::from(r"C:\clips\talk.wav")];
        // Neither a format nor a tool with nothing to set brings Kiln
        // forward: the result is told in a notification. Make smaller and
        // Resize have a size to set.
        let mut slices = vec![format_slice("mp3")];
        slices.extend(
            Tool::ALL
                .into_iter()
                .filter(|t| !matches!(t, Tool::Smaller | Tool::Resize))
                .map(tool_slice),
        );
        for item in &slices {
            let did = dropped(&files, item);
            assert!(!did.contains(&Did::ShowWindow), "{item:?}: {did:?}");
            assert!(did.iter().all(|d| !matches!(d, Did::Sheet(..))), "{item:?}");
            assert!(
                matches!(did.last(), Some(Did::Launch(_, _, None, Origin::Drag))),
                "{item:?}: {did:?}"
            );
        }
    }

    #[test]
    fn a_drop_on_make_smaller_or_resize_brings_kiln_forward_on_its_dialog() {
        let photo = vec![PathBuf::from(r"C:\pictures\photo.jpg")];
        for tool in [Tool::Smaller, Tool::Resize] {
            let action = Action::Tool(tool, kiln_core::Kind::Audio);
            // Started as the window starts, which opens the dialog.
            assert_eq!(
                dropped(&photo, &tool_slice(tool)),
                [
                    Did::Add(photo.clone(), false),
                    Did::ShowWindow,
                    Did::Start(action, photo.clone(), None),
                ]
            );
        }
    }

    #[test]
    fn a_drop_skips_the_replace_prompt() {
        let photo = vec![PathBuf::from(r"C:\pictures\photo.png")];
        let action = Action::Convert("jpg".into());
        // A drop is a new request: it adds and launches, and
        // nothing else, so nothing asks.
        assert_eq!(
            dropped(&photo, &format_slice("jpg")),
            [
                Did::Add(photo.clone(), false),
                Did::Launch(action, photo.clone(), None, Origin::Drag),
            ]
        );
        // Nothing dropped, nothing done; More is not a slice to drop on.
        let mut r = Record::default();
        drop_on_wheel(&mut r, vec![], Picked::Run(Action::Convert("jpg".into())));
        drop_on_wheel(&mut r, photo, Picked::More);
        assert_eq!(r.0, []);
    }

    #[test]
    fn a_drop_on_a_row_holding_an_unsaved_result_asks_first() {
        let photo = vec![PathBuf::from(r"C:\pictures\photo.png")];
        for item in [format_slice("webp"), tool_slice(Tool::CutInHalf)] {
            let Picked::Run(action) = wheel::pick_item(&item) else {
                panic!("{item:?} runs");
            };
            // The row still holds a result never saved: Kiln comes
            // forward, and starts as its window does, asking first, so
            // the result is never lost unseen.
            let mut r = Record(vec![], true);
            drop_on_wheel(&mut r, photo.clone(), Picked::Run(action.clone()));
            assert_eq!(
                r.0,
                [
                    Did::Add(photo.clone(), false),
                    Did::ShowWindow,
                    Did::Start(action.clone(), photo.clone(), None),
                ]
            );
            // Holding nothing, the same drop asks nothing.
            let mut r = Record::default();
            drop_on_wheel(&mut r, photo.clone(), Picked::Run(action.clone()));
            assert!(r
                .0
                .iter()
                .all(|d| !matches!(d, Did::Start(..) | Did::ShowWindow)));
        }
    }

    fn batch(origin: Origin, action: Action, finished: &[(&str, Option<&str>)]) -> Batch {
        Batch {
            action,
            origin,
            finished: finished
                .iter()
                .map(|(name, failed)| {
                    (
                        PathBuf::from(format!(r"C:\in\{name}")),
                        (*name).to_owned(),
                        failed.map(str::to_owned),
                    )
                })
                .collect(),
            drops: vec![],
            left: vec![],
            files: finished.len(),
            ok: 0,
            failed: 0,
            cancelled: 0,
            groups: finished.len(),
            pending: 0,
            pending_saves: HashSet::new(),
            saves: model::Saves::default(),
        }
    }

    #[test]
    fn window_batches_never_tell_as_the_drag_wheel_does() {
        let webp = Action::Convert("webp".into());
        let all: [&[(&str, Option<&str>)]; 3] = [
            &[("a.png", None)],
            &[("a.png", None), ("b.png", Some("It broke."))],
            &[("a.png", Some("It broke."))],
        ];
        for finished in all {
            // Chosen over Kiln's window: the window's own finish notice
            // tells, never the drag-wheel's.
            let window = batch(Origin::Window, webp.clone(), finished);
            assert_eq!(window.drag_notice(), None);
            // A drop: one notice for the batch, opening the row it names
            // or the first that failed.
            let drag = batch(Origin::Drag, webp.clone(), finished);
            let (_, _, row) = drag.drag_notice().expect("a drop tells");
            let want = finished.iter().position(|(_, f)| f.is_some()).unwrap_or(0);
            assert_eq!(row, drag.finished[want].0);
        }
        let drag = batch(Origin::Drag, webp, &[("a.png", None), ("b.png", None)]);
        assert_eq!(
            drag.drag_notice().map(|t| t.1).as_deref(),
            Some("Converted 2 files to WEBP.")
        );
    }

    #[test]
    fn a_drops_notice_says_where_its_results_went() {
        let webp = Action::Convert("webp".into());
        let told = |b: &Batch| b.drag_notice().map(|(title, text, _)| (title, text));
        // Saved next to the original: the drop's sentence heads where the
        // result went, and a click shows it there.
        let mut one = batch(Origin::Drag, webp.clone(), &[("photo.png", None)]);
        one.saves.saved.push("photo.webp".into());
        assert_eq!(
            told(&one),
            Some((
                "Converted photo.png to WEBP.".into(),
                "Saved photo.webp next to the original.".into()
            ))
        );
        assert!(one.all_saved());
        let mut two = batch(
            Origin::Drag,
            webp.clone(),
            &[("a.png", None), ("b.png", None)],
        );
        two.saves.saved = vec!["a.webp".into(), "b (1).webp".into()];
        assert_eq!(
            told(&two),
            Some((
                "Converted 2 files to WEBP.".into(),
                "Saved 2 files next to the originals.".into()
            ))
        );
        // Kept, as on a camera card: it says so, and a click opens Kiln.
        let mut card = batch(Origin::Drag, webp.clone(), &[("photo.png", None)]);
        card.saves.kept.push(("DCIM".into(), "photo.webp".into()));
        assert_eq!(
            told(&card),
            Some((
                "Converted photo.png to WEBP.".into(),
                "Couldn't save photo.webp. Kiln tries again the next time it starts.".into()
            ))
        );
        assert!(!card.all_saved());
        // One saved and one kept: the click opens the kept one's row, not
        // the first.
        let mut mixed = batch(
            Origin::Drag,
            webp.clone(),
            &[("a.png", None), ("b.png", None)],
        );
        mixed.saves.saved.push("a.webp".into());
        mixed.saves.kept.push(("DCIM".into(), "b.webp".into()));
        mixed.saves.first_kept = Some(mixed.finished[1].0.clone());
        let (_, text, row) = mixed.drag_notice().unwrap();
        assert_eq!(
            text,
            "Couldn't save b.webp. Kiln tries again the next time it starts."
        );
        assert_eq!(row, mixed.finished[1].0);
        // A failure reads as it did, saved or not, and opens Kiln on it.
        let mut part = batch(
            Origin::Drag,
            webp,
            &[("a.png", None), ("b.png", Some("It broke."))],
        );
        part.saves.saved.push("a.webp".into());
        assert_eq!(
            told(&part),
            Some(("Kiln".into(), "Converted 1 of 2 files to WEBP.".into()))
        );
        assert!(!part.all_saved());
        let (_, _, row) = part.drag_notice().unwrap();
        assert_eq!(row, part.finished[1].0);
    }

    #[test]
    fn a_drops_notice_tells_of_the_files_it_left_out() {
        let jpg = Action::Convert("jpg".into());
        let told = |b: &Batch| b.drag_notice().map(|(title, text, _)| (title, text));
        let mut one = batch(Origin::Drag, jpg.clone(), &[("b.png", None)]);
        one.left = vec!["a.jpg is already JPG.".into()];
        assert_eq!(
            told(&one),
            Some((
                "Kiln".into(),
                "Converted b.png to JPG. a.jpg is already JPG.".into()
            ))
        );
        one.saves.saved.push("b.jpg".into());
        assert_eq!(
            told(&one),
            Some((
                "Converted b.png to JPG.".into(),
                "Saved b.jpg next to the original. a.jpg is already JPG.".into()
            ))
        );
        // The window's own batches never tell.
        let mut window = batch(Origin::Window, jpg, &[("b.png", None)]);
        window.left = vec!["a.jpg is already JPG.".into()];
        assert_eq!(window.drag_notice(), None);
    }

    #[test]
    fn a_drop_with_nothing_to_convert_still_tells() {
        // Every file was left out: no job, but the drop says why, and a
        // click opens the first one's row.
        let split = wheel::Split {
            convert: vec![],
            already: vec![PathBuf::from(r"C:\in\a.jpg")],
            cant: vec![PathBuf::from(r"C:\in\c.wav")],
            needs: vec![],
        };
        let left = wheel::left_out_lines(&split, "jpg");
        assert_eq!(
            left_alone_notice(&split, &left),
            Some((
                "Kiln".to_owned(),
                "a.jpg is already JPG. c.wav can't become JPG.".to_owned(),
                PathBuf::from(r"C:\in\a.jpg")
            ))
        );
        // Over the window, rows say it: nothing to tell.
        assert_eq!(left_alone_notice(&split, &[]), None);
    }

    #[test]
    fn a_drops_notice_carries_what_its_files_lose() {
        let mp4 = Action::Convert("mp4".into());
        let two = kiln_core::first_of_tracks(2);
        let no_subtitles = kiln_core::NO_SUBTITLES.to_owned();
        let told = |b: &Batch| b.drag_notice().map(|(title, text, _)| (title, text));
        // Held, the drop's sentence heads what the file loses.
        let mut one = batch(Origin::Drag, mp4.clone(), &[("clip.mkv", None)]);
        one.drops = vec![vec![two.clone(), no_subtitles.clone()]];
        assert_eq!(
            told(&one),
            Some((
                "Kiln".into(),
                "Converted clip.mkv to MP4. Keeps the first of 2 sound tracks. Subtitles aren't kept."
                    .into()
            ))
        );
        // Saved next to the original, it follows where the result went and
        // the title is the drop's sentence alone, as a title is short.
        one.saves.saved.push("clip.mp4".into());
        assert_eq!(
            told(&one),
            Some((
                "Converted clip.mkv to MP4.".into(),
                "Saved clip.mp4 next to the original. Keeps the first of 2 sound tracks. Subtitles aren't kept."
                    .into()
            ))
        );
        // Several files: only what each of them loses.
        let mut two_files = batch(
            Origin::Drag,
            mp4.clone(),
            &[("a.mkv", None), ("b.mkv", None)],
        );
        two_files.drops = vec![
            vec![two.clone(), no_subtitles.clone()],
            vec![kiln_core::first_of_tracks(3), no_subtitles.clone()],
        ];
        assert_eq!(
            told(&two_files),
            Some((
                "Kiln".into(),
                "Converted 2 files to MP4. Subtitles aren't kept.".into()
            ))
        );
        // One that failed lost nothing: the one that converted says its own.
        let mut part = batch(
            Origin::Drag,
            mp4.clone(),
            &[("a.mkv", None), ("b.mkv", Some("It broke."))],
        );
        part.drops = vec![vec![two]];
        assert_eq!(
            told(&part),
            Some((
                "Kiln".into(),
                "Converted 1 of 2 files to MP4. Keeps the first of 2 sound tracks.".into()
            ))
        );
        // Nothing lost, nothing added.
        let none = batch(Origin::Drag, mp4, &[("a.mp4", None)]);
        assert_eq!(
            told(&none),
            Some(("Kiln".into(), "Converted a.mp4 to MP4.".into()))
        );
    }

    /// A finished drag-wheel conversion of `source` to JPG, its result
    /// `output`, saved into `saved` or held.
    fn finished_drop(source: &Path, output: PathBuf, saved: Option<PathBuf>) -> model::Entry {
        let kiln = Kiln::new();
        let mut e = model::entry_for(
            &kiln,
            &Availability::default(),
            source.to_path_buf(),
            1,
            None,
        );
        e.job = JobState::Done {
            action: Action::Convert("jpg".into()),
            outputs: vec![output],
            saved: saved.map(model::Saved::Folder),
        };
        e.from_drag = true;
        e.last = Some(Action::Convert("jpg".into()));
        e
    }

    fn converted(output: &Path) -> JobState {
        JobState::Done {
            action: Action::Convert("jpg".into()),
            outputs: vec![output.to_path_buf()],
            saved: None,
        }
    }

    fn broke() -> JobState {
        JobState::Failed {
            message: "It broke.".into(),
            details: String::new(),
            engine: None,
            store_product: None,
        }
    }

    /// The state the window keeps of its rows, for the steps a redo and a
    /// leave take through it: `a.png`, a finished drop holding `a.jpg`
    /// ("first") in the holding folder, and `b.png`, whose result was
    /// saved next to it.
    struct Rig {
        holding: tempfile::TempDir,
        files: FileList,
        notes: model::Notes,
        motion: Motion,
        redos: Redos,
    }

    impl Rig {
        fn new() -> Rig {
            let holding = tempfile::tempdir().unwrap();
            let held = holding.path().join("a.jpg");
            std::fs::write(&held, b"first").unwrap();
            let mut files = FileList::default();
            files.insert(0, finished_drop(Path::new(r"C:\in\a.png"), held, None));
            let saved = PathBuf::from(r"C:\in\b.jpg");
            files.insert(
                1,
                finished_drop(Path::new(r"C:\in\b.png"), saved, Some(r"C:\in".into())),
            );
            let mut motion = Motion::new(Instant::now(), 1.0);
            motion.rows_added(0, 2, Instant::now());
            Rig {
                holding,
                files,
                notes: model::Notes::default(),
                motion,
                redos: Redos::default(),
            }
        }

        fn kept_file(&self) -> PathBuf {
            self.holding.path().join("a.jpg")
        }

        fn row(&self, i: usize) -> &model::Entry {
            &self.files.entries()[i]
        }

        /// Row `i`'s redo starts as `App::redo` starts it: what the row is
        /// is kept, and `launch_as` sets it running on its job, its notes
        /// and failed save forgotten.
        fn redo(&mut self, i: usize) -> JobId {
            let id = JobId(40 + i as u64);
            let path = self.row(i).path.clone();
            let kept = Kept::of(self.row(i), &self.notes);
            let e = &mut self.files.entries_mut()[i];
            e.job = JobState::Running {
                action: Action::Convert("jpg".into()),
                fraction: None,
                started: None,
            };
            e.save_failed = false;
            e.job_note = None;
            self.notes.remove(&path);
            self.motion.rows[i].job = Some(id);
            self.redos.keep(id, path, kept);
            id
        }

        /// Job `id`'s news for row `i`'s file arrives, as `on_job` takes
        /// it.
        fn news(&mut self, id: JobId, i: usize, state: JobState) -> Option<(usize, bool)> {
            self.noted_news(id, i, state, None)
        }

        /// [`Rig::news`] from a job that sent `note` for its details.
        fn noted_news(
            &mut self,
            id: JobId,
            i: usize,
            state: JobState,
            note: Option<String>,
        ) -> Option<(usize, bool)> {
            let path = self.row(i).path.clone();
            job_ended(
                &mut self.files,
                &mut self.notes,
                &mut self.motion,
                &mut self.redos,
                (id, &path),
                (&state, note),
            )
        }

        /// What a leave of `rows` asks first, as `leave_after` asks it.
        fn ask(&self, rows: &Rows, verb: Leave) -> (Vec<PathBuf>, Option<model::Prompt>) {
            let mask = rows_mask(&self.files, rows);
            let picked = |i: usize| mask.get(i).copied().unwrap_or(false);
            let held = held_for_leave(&self.files, &self.redos, picked, &HashSet::new(), |p| {
                p.exists()
            });
            let running = running_of(&self.files, &mask).len();
            let prompt = leave_prompt(running, verb);
            (held, prompt)
        }

        /// Stop on a leave of `rows`: each redo of them gives its row its
        /// old result back now (`App::give_back`).
        fn give_back(&mut self, rows: &Rows) -> Vec<JobId> {
            let mask = rows_mask(&self.files, rows);
            give_back(
                &mut self.files,
                &mut self.notes,
                &mut self.motion,
                &mut self.redos,
                |i| mask.get(i).copied().unwrap_or(false),
            )
        }
    }

    #[test]
    fn a_redo_keeps_the_rows_job_note_with_its_old_result() {
        let cpu = Some(kiln_core::ON_THE_PROCESSOR.to_owned());
        // A redo that fails or is cancelled gives the row back its old
        // result with the note its job sent, as it gives back the
        // rest of the row.
        for end in [broke(), JobState::Cancelled] {
            let mut rig = Rig::new();
            rig.files.entries_mut()[0].job_note = cpu.clone();
            let id = rig.redo(0);
            assert_eq!(rig.noted_news(id, 0, end, None), Some((0, true)));
            assert_eq!(rig.row(0).job_note, cpu);
            assert!(rig.row(0).line().0.ends_with(kiln_core::ON_THE_PROCESSOR));
        }
        // So does a leave that stops it.
        let mut rig = Rig::new();
        rig.files.entries_mut()[0].job_note = cpu.clone();
        let id = rig.redo(0);
        assert_eq!(rig.give_back(&Rows::All), [id]);
        assert_eq!(rig.row(0).job_note, cpu);
        // A redo that succeeds says its own job's note, or none.
        for note in [None, cpu.clone()] {
            let mut rig = Rig::new();
            rig.files.entries_mut()[0].job_note = cpu.clone().xor(Some("Before.".to_owned()));
            let id = rig.redo(0);
            let new = rig.holding.path().join("a (2).jpg");
            std::fs::write(&new, b"second").unwrap();
            assert_eq!(
                rig.noted_news(id, 0, converted(&new), note.clone()),
                Some((0, false))
            );
            assert_eq!(rig.row(0).job_note, note);
        }
    }

    #[test]
    fn a_running_row_says_what_its_job_leaves_out_and_the_job_ends_it() {
        let drops = vec![kiln_core::first_of_tracks(2)];
        let note = model::job_note(&drops, None);
        let mut rig = Rig::new();
        let id = rig.redo(0);
        let paths = [rig.row(0).path.clone(), rig.row(1).path.clone()];
        // The rows waiting on the job say it after their line; a row that
        // isn't (row 1 has its result, from no job) is not touched.
        row_noted(&mut rig.files, &rig.motion, id, &paths, note.clone());
        assert_eq!(rig.row(0).job_note, note);
        assert!(
            rig.row(0)
                .line()
                .0
                .ends_with(" · Keeps the first of 2 sound tracks."),
            "{}",
            rig.row(0).line().0
        );
        assert_eq!(rig.row(1).job_note, None);
        // A job that was given a new row since leaves the row alone.
        let other = JobId(99);
        row_noted(
            &mut rig.files,
            &rig.motion,
            other,
            &paths,
            Some("Other.".into()),
        );
        assert_eq!(rig.row(0).job_note, note);
        // It converts: the row says what the job sent in all.
        let new = rig.holding.path().join("a (2).jpg");
        std::fs::write(&new, b"second").unwrap();
        let all = model::job_note(&drops, Some(kiln_core::ON_THE_PROCESSOR));
        assert_eq!(
            rig.noted_news(id, 0, converted(&new), all.clone()),
            Some((0, false))
        );
        assert_eq!(rig.row(0).job_note, all);
        // One that is stopped gives the row back its old result and its old
        // note, not the stopped job's.
        let mut rig = Rig::new();
        rig.files.entries_mut()[0].job_note = Some("Before.".into());
        let id = rig.redo(0);
        row_noted(&mut rig.files, &rig.motion, id, &paths, note);
        assert_eq!(
            rig.noted_news(id, 0, JobState::Cancelled, None),
            Some((0, true))
        );
        assert_eq!(rig.row(0).job_note.as_deref(), Some("Before."));
    }

    #[test]
    fn a_leave_mid_redo_names_the_kept_result_and_it_survives_stop() {
        let a = PathBuf::from(r"C:\in\a.png");
        for (verb, rows) in [
            (Leave::Close, Rows::All),
            (Leave::Clear, Rows::All),
            (Leave::Remove, Rows::Files(vec![a.clone()])),
        ] {
            let mut rig = Rig::new();
            let kept = rig.kept_file();
            let before = rig.row(0).job.clone();
            let id = rig.redo(0);
            // The prompt asks about the running redo only; the old result
            // it keeps is still the row's to deliver.
            let (held, prompt) = rig.ask(&rows, verb);
            assert_eq!(held, std::slice::from_ref(&kept), "{verb:?}");
            let prompt = prompt.unwrap();
            assert!(
                prompt.title.starts_with("1 file is still converting."),
                "{verb:?}"
            );
            assert!(prompt.buttons.contains(&PromptButton::Stop(verb)));
            // Stop gives the row its old result back at once, before the
            // job's news can come, and stops the job.
            assert_eq!(rig.give_back(&rows), [id], "{verb:?}");
            assert_eq!(rig.row(0).job, before, "{verb:?}");
            assert_eq!(rig.row(0).held(), Some(std::slice::from_ref(&kept)));
            assert_eq!(rig.motion.rows[0].job, None);
            // Looked at again, it is still there, and nothing more.
            let (now, _) = rig.ask(&rows, verb);
            assert_eq!(now, std::slice::from_ref(&kept), "{verb:?}");
            // The stopped job's news comes late and changes nothing.
            assert_eq!(rig.news(id, 0, JobState::Cancelled), None, "{verb:?}");
            assert_eq!(rig.row(0).job, before, "{verb:?}");
            assert_eq!(std::fs::read(&kept).unwrap(), b"first", "{verb:?}");
        }
    }

    #[test]
    fn a_hide_leaves_a_redo_running_with_its_kept_result() {
        // A hide stops no job, so its answers give no redo back; every
        // other leave stops its rows' jobs.
        assert!(!stops_jobs(&Then::Hide));
        for then in [Then::Close, Then::Clear, Then::Remove(vec![])] {
            assert!(stops_jobs(&then));
        }
        for k in 0..3 {
            let mut rig = Rig::new();
            let kept = rig.kept_file();
            let new = rig.holding.path().join("a (2).jpg");
            let end = match k {
                0 => broke(),
                1 => JobState::Cancelled,
                _ => {
                    std::fs::write(&new, b"second").unwrap();
                    converted(&new)
                }
            };
            let id = rig.redo(0);
            // The redo ends as it would have with the window shown: a redo
            // that failed or was cancelled gives the row its old result
            // back, whole. One that succeeded gives the row the new one,
            // and the old one stays where it was, for the next start to
            // deliver: nothing a row held is thrown away.
            let back = !matches!(end, JobState::Done { .. });
            assert_eq!(rig.news(id, 0, end), Some((0, back)), "{k}");
            if back {
                assert_eq!(rig.row(0).held(), Some(std::slice::from_ref(&kept)));
                assert_eq!(std::fs::read(&kept).unwrap(), b"first");
            } else {
                assert_eq!(rig.row(0).held(), Some(std::slice::from_ref(&new)));
                assert_eq!(std::fs::read(&kept).unwrap(), b"first");
            }
            assert!(rig.redos.0.is_empty());
        }
    }

    #[test]
    fn a_retried_drop_still_offers_options_once_done() {
        let jpg = Action::Convert("jpg".into());
        let source = Path::new(r"C:\in\a.png");
        let mut e = finished_drop(source, PathBuf::from(r"C:\held\a.jpg"), None);
        e.job = JobState::Cancelled;
        // Retry starts it from Kiln's window, as `launch_as` starts every
        // row of a job.
        assert!(e.retryable());
        start_row(&mut e, &jpg, false);
        assert!(matches!(e.job, JobState::Running { .. }));
        assert!(e.from_drag);
        assert_eq!(e.last, Some(jpg.clone()));
        e.job = converted(Path::new(r"C:\held\a 2.jpg"));
        assert_eq!(e.options_for(), Some("jpg"));
        // Something else started on it isn't the drop's.
        let mut other = finished_drop(source, PathBuf::from(r"C:\held\a.jpg"), None);
        other.job = JobState::Cancelled;
        start_row(&mut other, &Action::Convert("webp".into()), false);
        assert!(!other.from_drag);
        // Nor is a row converted over the window in the first place.
        let mut window = finished_drop(source, PathBuf::from(r"C:\held\a.jpg"), None);
        window.from_drag = false;
        window.job = JobState::Cancelled;
        start_row(&mut window, &jpg, false);
        assert!(!window.from_drag);
        // A drop is always the drop's.
        start_row(&mut window, &jpg, true);
        assert!(window.from_drag);
    }

    #[test]
    fn a_leave_names_only_the_kept_results_of_its_own_rows() {
        let mut rig = Rig::new();
        rig.redo(0);
        // Removing b, whose result is saved, asks nothing about a's.
        let b = Rows::Files(vec![PathBuf::from(r"C:\in\b.png")]);
        let (held, prompt) = rig.ask(&b, Leave::Remove);
        assert!(held.is_empty() && prompt.is_none());
        assert!(rig.give_back(&b).is_empty());
        assert!(matches!(rig.row(0).job, JobState::Running { .. }));
    }

    #[test]
    fn a_redo_that_succeeds_takes_the_new_result_and_never_removes_the_old() {
        let mut rig = Rig::new();
        let kept = rig.kept_file();
        let id = rig.redo(0);
        // Kept while the redo runs.
        assert_eq!(std::fs::read(&kept).unwrap(), b"first");
        // The new result arrives beside it under a free name.
        let new = rig.holding.path().join("a 2.jpg");
        std::fs::write(&new, b"second").unwrap();
        assert_eq!(rig.news(id, 0, converted(&new)), Some((0, false)));
        // The new result is the row's. The old one is never removed: it
        // stays in the holding folder, whole, and the next start delivers
        // it beside the new one under a free name.
        assert_eq!(rig.row(0).held(), Some(std::slice::from_ref(&new)));
        assert_eq!(std::fs::read(&kept).unwrap(), b"first");
        assert_eq!(rig.row(0).options_for(), Some("jpg"));
        assert!(rig.redos.0.is_empty());
    }

    #[test]
    fn a_redo_that_fails_or_is_cancelled_gives_the_row_back_as_it_was() {
        for end in [broke(), JobState::Cancelled] {
            let mut rig = Rig::new();
            let kept = rig.kept_file();
            // Saving it had failed, and the row said so.
            let note = ("Couldn't save to Pictures.".to_owned(), Tone::Crack);
            rig.files.entries_mut()[0].save_failed = true;
            rig.notes.insert(rig.row(0).path.clone(), note.clone());
            let before = rig.row(0).job.clone();
            let id = rig.redo(0);
            assert_eq!(rig.news(id, 0, end.clone()), Some((0, true)), "{end:?}");
            // Back to its old result, untouched, as it was: its note and
            // failed save with it.
            assert_eq!(rig.row(0).job, before, "{end:?}");
            assert!(rig.row(0).save_failed);
            assert_eq!(rig.notes.get(&rig.row(0).path), Some(&note));
            assert_eq!(std::fs::read(&kept).unwrap(), b"first", "{end:?}");
            // Closing Kiln asks nothing about it: it stays for the next
            // start to deliver.
            let (held, prompt) = rig.ask(&Rows::All, Leave::Close);
            assert_eq!(held, std::slice::from_ref(&kept));
            assert_eq!(prompt, None);
        }
    }

    #[test]
    fn a_result_that_leaves_the_holding_folder_is_cleared_from_its_sources() {
        let root = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let photos = out.path().join("Ana María");
        std::fs::create_dir(&photos).unwrap();
        let h = kiln_core::Holding::open(root.path()).unwrap();
        let dir = h.dir().to_path_buf();
        let hold = |name: &str| {
            let f = dir.join(name);
            std::fs::write(&f, name).unwrap();
            record_sources(
                h.dir(),
                &[photos.join("photo.png")],
                std::slice::from_ref(&f),
            );
            f
        };
        // Saved next to its original.
        let a = hold("a.jpg");
        let moved = move_out(vec![a.clone()], &Dest::Into(photos.clone()), &dir);
        assert_eq!(moved, [(a.clone(), Some(photos.join("a.jpg")))]);
        // Its folder gone: saved in Documents\Kiln results instead.
        let b = hold("b.jpg");
        let fallback = out.path().join("Kiln results");
        std::fs::create_dir(&fallback).unwrap();
        let (moved, cancelled) = move_out_with(
            vec![b.clone()],
            (&Dest::Into(out.path().join("gone")), Some(&fallback)),
            &mut |_, _, _| {},
            &CancelToken::new(),
            &|dir, f| left_holding(dir, f),
        );
        assert_eq!(moved, [(b, Some(fallback.join("b.jpg")))]);
        assert!(cancelled.is_empty());
        // A move that fails leaves it held, and noted as it was.
        let d = hold("d.jpg");
        let gone = out.path().join("gone");
        let moved = move_out(vec![d.clone()], &Dest::Into(gone), &dir);
        assert_eq!(moved, [(d.clone(), None)]);
        assert!(d.exists());

        let p = photos.display();
        assert_eq!(
            std::fs::read_to_string(dir.join(kiln_core::SOURCES_NAME)).unwrap(),
            format!("a.jpg\t{p}\na.jpg\t\nb.jpg\t{p}\nb.jpg\t\nd.jpg\t{p}\n")
        );
        // A new a.jpg that Kiln ended before noting isn't offered as made
        // from the old one's folder.
        std::fs::write(&a, "again").unwrap();
        drop(h);
        let runs = kiln_core::stale_runs(root.path());
        assert_eq!(runs.len(), 1);
        assert_eq!(
            runs[0].files,
            [
                kiln_core::Recovered {
                    path: a,
                    origin: None,
                },
                kiln_core::Recovered {
                    path: d,
                    origin: Some(photos.clone()),
                },
            ]
        );
    }

    #[test]
    fn a_held_result_on_another_drive_is_never_looked_at_on_the_windows_thread() {
        let dir = tempfile::tempdir().unwrap();
        let h = Held::open(
            kiln_core::Places::new(dir.path()),
            Box::new(kiln_core::SystemVolumes),
        )
        .unwrap();
        let mut gone = GoneLooks::default();
        // In the local holding folder: looked at.
        let here = h.local().join("a.jpg");
        assert!(!present_in(Some(&h), &gone)(&here));
        std::fs::write(&here, "a").unwrap();
        assert!(present_in(Some(&h), &gone)(&here));
        // Elsewhere, on a drive that isn't there at all: never looked at,
        // so there until the gone thread says otherwise.
        let far = PathBuf::from(r"Q:\.kiln-held\S-1\1-1\b.jpg");
        assert!(present_in(Some(&h), &gone)(&far));
        assert!(present_in(None, &gone)(&far));
        let ask = gone.ask();
        gone.found(ask, [far.clone()].into());
        assert!(!present_in(Some(&h), &gone)(&far));
        // The gone thread's word never stands for a local one: it is
        // looked at.
        let ask = gone.ask();
        gone.found(ask, [far.clone(), here.clone()].into());
        assert!(present_in(Some(&h), &gone)(&here));
        // A quit keeps the run folder that holds a result.
        let kept = h.close_empty(Duration::from_secs(10));
        assert!(here.exists());
        assert!(kept.iter().any(|d| here.starts_with(d)), "{kept:?}");
    }

    #[test]
    fn a_copy_says_how_far_a_few_times_a_second() {
        let t0 = Instant::now();
        let mut pace = CopyPace::default();
        // Nothing to copy says nothing.
        assert_eq!(pace.due(t0, 0, 0), None);
        assert_eq!(pace.due(t0, 0, 1000), Some(0.0));
        // Soon after, even with more done, nothing new is sent.
        let soon = t0 + Duration::from_millis(30);
        assert_eq!(pace.due(soon, 500, 1000), None);
        let later = t0 + COPY_PROGRESS_EVERY;
        assert_eq!(pace.due(later, 500, 1000), Some(0.5));
        // The same percent again isn't sent, however late.
        let much_later = later + Duration::from_secs(5);
        assert_eq!(pace.due(much_later, 501, 1000), None);
        // The end always is, at once.
        let end = much_later + Duration::from_millis(1);
        assert_eq!(pace.due(end, 1000, 1000), Some(1.0));
        assert_eq!(model::saving_line(None), "Saving\u{2026}");
        assert_eq!(model::saving_line(Some(0.426)), "Saving\u{2026} \u{b7} 42%");
    }

    #[test]
    fn a_cancelled_export_leaves_every_held_file_whole() {
        let root = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let h = kiln_core::Holding::open(root.path()).unwrap();
        let files: Vec<PathBuf> = ["a.jpg", "b.jpg"]
            .into_iter()
            .map(|n| {
                let f = h.dir().join(n);
                std::fs::write(&f, n).unwrap();
                f
            })
            .collect();
        let cancel = CancelToken::new();
        cancel.cancel();
        let dest = Dest::Into(out.path().to_path_buf());
        let (results, cancelled) = move_out_with(
            files.clone(),
            (&dest, None),
            &mut |_, _, _| {},
            &cancel,
            &|_, _| {},
        );
        assert_eq!(
            results,
            files.iter().map(|f| (f.clone(), None)).collect::<Vec<_>>()
        );
        assert_eq!(cancelled, files);
        for f in &files {
            assert_eq!(
                std::fs::read(f).unwrap(),
                f.file_name().unwrap().as_encoded_bytes()
            );
        }
        assert_eq!(std::fs::read_dir(out.path()).unwrap().count(), 0);
    }

    /// A drive other than this test's temp folder's: `E:` when it is
    /// there.
    fn other_drive() -> Option<PathBuf> {
        let temp = std::env::temp_dir();
        let e = PathBuf::from(r"E:\");
        let other = !temp
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("E:");
        (other && e.is_dir()).then_some(e)
    }

    #[test]
    fn a_save_to_another_drive_shows_progress_and_a_cancel_keeps_the_result() {
        let Some(e) = other_drive() else {
            eprintln!("skipped: no E: drive apart from the temp folder's");
            return;
        };
        // Only this folder is made on E:, and only it is removed, even if
        // an assertion fails.
        let dest = e.join(format!("kiln-phase2-test-{}", std::process::id()));
        std::fs::create_dir(&dest).unwrap();
        let _tidy = OwnFolder(dest.clone());
        let root = tempfile::tempdir().unwrap();
        let h = kiln_core::Holding::open(root.path()).unwrap();
        let held = h.dir().join("clip.mp4");
        let bytes: Vec<u8> = (0..24 * 1024 * 1024u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&held, &bytes).unwrap();

        // Cancelled midway: the held result is whole, and nothing is left
        // on E:, neither the temp copy nor the result.
        let cancel = CancelToken::new();
        let mut seen = vec![];
        let stop = cancel.clone();
        let (results, cancelled) = move_out_with(
            vec![held.clone()],
            (&Dest::Into(dest.clone()), None),
            &mut |f, done, total| {
                seen.push((f.to_path_buf(), done, total));
                if done > 0 {
                    stop.cancel();
                }
            },
            &cancel,
            &|_, _| {},
        );
        assert_eq!(results, [(held.clone(), None)]);
        assert_eq!(cancelled, std::slice::from_ref(&held));
        assert!(!seen.is_empty(), "the copy says how far it is");
        assert!(seen
            .iter()
            .all(|(f, _, t)| f == &held && *t == bytes.len() as u64));
        assert_eq!(std::fs::read(&held).unwrap(), bytes);
        let left: Vec<_> = std::fs::read_dir(&dest).unwrap().flatten().collect();
        assert!(left.is_empty(), "nothing is left on E: {left:?}");

        // Not cancelled: it gets there whole, and its progress ends at all
        // of it.
        let mut last = (0, 0);
        let (results, cancelled) = move_out_with(
            vec![held.clone()],
            (&Dest::Into(dest.clone()), None),
            &mut |_, done, total| last = (done, total),
            &CancelToken::new(),
            &|_, _| {},
        );
        let saved = dest.join("clip.mp4");
        assert_eq!(results, [(held.clone(), Some(saved.clone()))]);
        assert!(cancelled.is_empty());
        assert_eq!(last, (bytes.len() as u64, bytes.len() as u64));
        assert!(!held.exists());
        assert_eq!(std::fs::read(&saved).unwrap(), bytes);
    }

    /// The test's own folder on another drive: when it goes, the files the
    /// test makes there (`clip.mp4`, and its temp copies) go, then the
    /// folder, with a plain remove. Nothing else in it is touched, and a
    /// folder with anything else in it stays.
    struct OwnFolder(PathBuf);

    impl Drop for OwnFolder {
        fn drop(&mut self) {
            if let Ok(entries) = std::fs::read_dir(&self.0) {
                for e in entries.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    let ours = name == "clip.mp4"
                        || (name.starts_with("clip.mp4.")
                            && name.contains(".kiln-export")
                            && name.ends_with(".tmp"));
                    if ours && e.file_type().is_ok_and(|t| t.is_file()) {
                        let _ = std::fs::remove_file(e.path());
                    }
                }
            }
            let _ = std::fs::remove_dir(&self.0);
        }
    }

    #[test]
    fn a_card_taken_out_before_the_save_keeps_the_result_or_saves_it_in_kiln_results() {
        let holding = tempfile::tempdir().unwrap();
        let card = tempfile::tempdir().unwrap();
        let dcim = card.path().join("DCIM");
        std::fs::create_dir(&dcim).unwrap();
        let source = dcim.join("photo.png");
        std::fs::write(&source, b"photo").unwrap();
        let held = holding.path().join("photo.webp");
        std::fs::write(&held, b"result").unwrap();
        let mut list = FileList::default();
        list.insert(0, finished_drop(&source, held.clone(), None));
        let mut notes = model::Notes::default();
        // Where `save_beside` sends the result...
        let (files, dir) = beside(
            std::slice::from_ref(&source),
            std::slice::from_ref(&held),
            std::slice::from_ref(&held),
            None,
        )
        .unwrap();
        assert_eq!(dir, dcim);
        // ...is gone by the time it is moved: the card came out.
        std::fs::remove_file(&source).unwrap();
        std::fs::remove_dir(&dcim).unwrap();
        let results = move_out(files, &Dest::Into(dir.clone()), holding.path());
        // With nowhere else to go it couldn't be saved, so it is still
        // held, whole, and the folder wasn't made again.
        assert_eq!(results, vec![(held.clone(), None)]);
        assert_eq!(std::fs::read(&held).unwrap(), b"result");
        assert!(!dir.exists());
        // The row, told as `App::exported` tells it, says Kiln tries again
        // at the next start, and still holds the result.
        let failed = vec![held.clone()];
        let exists = |p: &Path| p.exists();
        record_export(&mut list, &mut notes, &dir, (&[], &failed), exists);
        assert_eq!(
            notes.get(&source),
            Some(&(model::KEPT_LINE.to_owned(), Tone::Crack))
        );
        let row = &list.entries()[0];
        assert_eq!(row.held(), Some(failed.as_slice()));
        assert_eq!(row.link(false), None);
        // With Documents\Kiln results there, it is saved in it, whole.
        let fallback = card.path().join("Kiln results");
        std::fs::create_dir(&fallback).unwrap();
        let (results, _) = move_out_with(
            vec![held.clone()],
            (&Dest::Into(dir.clone()), Some(&fallback)),
            &mut |_, _, _| {},
            &CancelToken::new(),
            &|dir, f| left_holding(dir, f),
        );
        let to = fallback.join("photo.webp");
        assert_eq!(results, vec![(held.clone(), Some(to.clone()))]);
        assert_eq!(std::fs::read(&to).unwrap(), b"result");
        assert!(!held.exists());
        assert!(!dir.exists());
        // The row says where it went, and why.
        record_export(&mut list, &mut notes, &dir, (&[(held, to)], &[]), exists);
        assert_eq!(
            notes.get(&source),
            Some(&(model::fallback_line("DCIM"), Tone::Muted))
        );
    }

    #[test]
    fn without_a_holding_folder_results_go_straight_to_the_chosen_folder() {
        let (held, chosen) = (Path::new(r"C:\held"), Path::new(r"D:\Chosen"));
        assert_eq!(
            written_into(Some(held), Some(chosen)).as_deref(),
            Some(held)
        );
        assert_eq!(written_into(Some(held), None).as_deref(), Some(held));
        assert_eq!(written_into(None, Some(chosen)).as_deref(), Some(chosen));
        assert_eq!(written_into(None, None), None);
    }

    #[test]
    fn a_delivery_tells_its_rows_what_became_of_each_result() {
        let holding = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let held = |name: &str| {
            let f = holding.path().join(name);
            std::fs::write(&f, b"result").unwrap();
            f
        };
        let (a, b) = (held("a.jpg"), held("b.jpg"));
        let mut list = FileList::default();
        list.insert(
            0,
            finished_drop(&folder.path().join("a.png"), a.clone(), None),
        );
        list.insert(
            1,
            finished_drop(&folder.path().join("b.png"), b.clone(), None),
        );
        let mut notes = model::Notes::default();
        // One saved next to its original, one kept for the next start: the
        // rows say so, and the footer is left to the batch's summary.
        let saved = [(a.clone(), folder.path().join("a.jpg"))];
        let failed = [b.clone()];
        record_export(
            &mut list,
            &mut notes,
            folder.path(),
            (&saved, &failed),
            |p: &Path| p.exists(),
        );
        assert_eq!(list.entries()[0].line().0, "Saved as a.jpg");
        assert_eq!(
            notes
                .get(&folder.path().join("b.png"))
                .map(|n| n.0.as_str()),
            Some(model::KEPT_LINE)
        );
        assert_eq!(list.entries()[1].held(), Some(std::slice::from_ref(&b)));
    }

    #[test]
    fn a_delivery_leaves_the_rows_of_other_deliveries_alone() {
        let holding = tempfile::tempdir().unwrap();
        let (pictures, videos) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let held = |name: &str| {
            let f = holding.path().join(name);
            std::fs::write(&f, b"result").unwrap();
            f
        };
        let (a, b) = (held("a.png"), held("b.mp3"));
        let (photo, clip) = (pictures.path().join("a.jpg"), videos.path().join("b.mp4"));
        let mut list = FileList::default();
        list.insert(0, finished_drop(&photo, a.clone(), None));
        list.insert(1, finished_drop(&clip, b.clone(), None));
        let mut notes = model::Notes::default();
        let present = |p: &Path| p.exists();
        // The photo is saved next to its original, then the clip next to
        // its own: the second delivery says nothing of the first row.
        let first = [(a, pictures.path().join("a.png"))];
        record_export(
            &mut list,
            &mut notes,
            pictures.path(),
            (&first, &[]),
            present,
        );
        let second = [(b, videos.path().join("b.mp3"))];
        record_export(
            &mut list,
            &mut notes,
            videos.path(),
            (&second, &[]),
            present,
        );
        assert_eq!(notes.get(&photo), None, "{:?}", notes.get(&photo));
        assert_eq!(notes.get(&clip), None);
        assert_eq!(list.entries()[0].line().0, "Saved as a.png");
    }

    #[test]
    fn a_redo_never_overwrites_a_result_already_saved() {
        let holding = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        // The first result was saved next to the original.
        let source = folder.path().join("photo.png");
        let saved = folder.path().join("photo.jpg");
        std::fs::write(&saved, b"first").unwrap();
        let mut e = finished_drop(&source, saved.clone(), Some(folder.path().to_path_buf()));
        assert_eq!(e.options_for(), Some("jpg"));
        let kept = Kept::of(&e, &model::Notes::default());
        // The redo's new result is held first...
        let redone = holding.path().join("photo.jpg");
        std::fs::write(&redone, b"second").unwrap();
        e.job = converted(&redone);
        let mut notes = model::Notes::default();
        assert!(!redo_ended(&mut e, &mut notes, kept));
        // ...takes nothing of the saved one away...
        assert_eq!(std::fs::read(&saved).unwrap(), b"first");
        // ...and goes beside the original as `save_beside` sends it: a
        // delivery into the folder, under a free name.
        assert_eq!(model::autosave_for(true), model::AutoSave::Beside);
        let held = e.held().unwrap().to_vec();
        let (files, dir) = beside(std::slice::from_ref(&source), &held, &held, None).unwrap();
        assert_eq!(dir, folder.path());
        let results = export_each(files, |f| {
            kiln_core::deliver_into_with(f, &dir, &mut |_, _| {}, &CancelToken::new()).ok()
        });
        let [(from, Some(to))] = results.as_slice() else {
            panic!("{results:?}");
        };
        assert_eq!(from, &redone);
        assert_ne!(to, &saved);
        assert_eq!(std::fs::read(&saved).unwrap(), b"first");
        assert_eq!(std::fs::read(to).unwrap(), b"second");
    }

    #[test]
    fn a_drop_on_a_row_being_saved_is_refused_as_options_is() {
        let mut files = FileList::default();
        let a = PathBuf::from(r"C:\in\a.png");
        let b = PathBuf::from(r"C:\in\b.png");
        files.insert(0, finished_drop(&a, PathBuf::from(r"C:\held\a.jpg"), None));
        files.insert(1, finished_drop(&b, PathBuf::from(r"C:\held\b.jpg"), None));
        let saving: HashSet<PathBuf> = [a.clone()].into();
        // a's result is being exported: it stays as it is until it's saved.
        assert_eq!(
            drop_refused(&files, &saving, std::slice::from_ref(&a)),
            Some(model::SAVING)
        );
        assert_eq!(
            drop_refused(&files, &saving, &[b.clone(), a.clone()]),
            Some(model::SAVING)
        );
        // b's isn't, and goes ahead.
        assert_eq!(
            drop_refused(&files, &saving, std::slice::from_ref(&b)),
            None
        );
        // A row still converting says so, as before.
        files.entries_mut()[1].job = JobState::Running {
            action: Action::Convert("jpg".into()),
            fraction: None,
            started: None,
        };
        assert_eq!(
            drop_refused(&files, &HashSet::new(), std::slice::from_ref(&b)),
            files.convert_busy(std::slice::from_ref(&b))
        );
        assert!(drop_refused(&files, &HashSet::new(), std::slice::from_ref(&b)).is_some());
    }

    #[test]
    fn a_drop_that_asked_first_still_offers_options_once_done() {
        // A row converted over Kiln's window, holding its result: a drop
        // on the drag-wheel takes the window's path and asks first,
        // then starts as the drop's (`Then::Start`'s flag reaches
        // `start_row`).
        let jpg = Action::Convert("jpg".into());
        let mut e = finished_drop(
            Path::new(r"C:\in\a.png"),
            PathBuf::from(r"C:\held\a.webp"),
            None,
        );
        e.from_drag = false;
        start_row(&mut e, &jpg, true);
        assert!(e.from_drag);
        e.job = converted(Path::new(r"C:\held\a.jpg"));
        assert_eq!(e.options_for(), Some("jpg"));
        // The same start from the window is not a drop's.
        let mut w = e.clone();
        w.from_drag = false;
        start_row(&mut w, &jpg, false);
        assert!(!w.from_drag);
    }

    #[test]
    fn the_floating_wheel_never_opens_over_kiln_or_its_dialogs() {
        // Over another app, with no dialog of Kiln's open: it opens,
        // whether Kiln's window shows or not.
        assert!(float_opens(false, true, false));
        assert!(float_opens(false, false, false));
        // Over Kiln's own window while it shows, where its in-window wheel
        // is.
        assert!(!float_opens(true, true, false));
        // Hidden, Kiln's window is under nothing.
        assert!(float_opens(true, false, false));
        // While a dialog, prompt or menu of Kiln's is open, anywhere.
        for (on_kiln, shown) in [(false, false), (false, true), (true, true)] {
            assert!(!float_opens(on_kiln, shown, true));
        }
    }

    #[test]
    fn a_tool_with_settings_opens_its_sheet() {
        let files = vec![PathBuf::from(r"C:\pictures\photo.png")];
        // A pick with settings shows Kiln with its sheet open for the
        // dropped files.
        let mut r = Record::default();
        drop_on_wheel(&mut r, files.clone(), Picked::Options("jpg".into()));
        assert_eq!(
            r.0,
            [
                Did::Add(files.clone(), false),
                Did::ShowWindow,
                Did::Sheet("jpg".into(), files.clone()),
            ]
        );
        // Kiln's other tools have nothing to set, so each runs as it
        // lands; Make smaller and Resize ask in their dialog (see
        // `a_drop_on_make_smaller_or_resize_brings_kiln_forward_on_its_dialog`).
        for tool in Tool::ALL
            .into_iter()
            .filter(|t| !matches!(t, Tool::Smaller | Tool::Resize))
        {
            assert_eq!(
                dropped(&files, &tool_slice(tool)),
                [
                    Did::Add(files.clone(), false),
                    Did::Launch(
                        Action::Tool(tool, kiln_core::Kind::Audio),
                        files.clone(),
                        None,
                        Origin::Drag
                    ),
                ]
            );
        }
    }
}
