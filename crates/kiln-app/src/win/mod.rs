//! The Win32 window: creation, the message loop, input and animation.

mod drop;
mod render;
mod text;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows::core::{w, Result, BOOL, HSTRING, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWINDOWATTRIBUTE};
use windows::Win32::Graphics::Gdi::{
    CreateSolidBrush, InvalidateRect, ScreenToClient, UpdateWindow, ValidateRect,
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

use kiln_core::{JobEvent, JobId, JobKind, Kiln, Options, Progress, Runner, Tool};

use crate::layout::{self, ROW_HEIGHT};
use crate::model::{batch_summary, status_line, FileList, JobState};
use crate::theme::{self, motion, Palette};
use crate::wheel::{self, Action, Wheel, WheelKind};
use drop::{DragEvent, DropTarget, DRAG_ENTER, DRAG_LEAVE, DRAG_OVER, WM_KILN_DRAG, WM_KILN_DROP};
use render::{Renderer, RowAnim, View, WheelView};

const ANIM_TIMER: usize = 1;
const FRAME_MS: u32 = 16;

/// Posted by the worker threads; lparam is a `Box<JobEvent>`.
const WM_KILN_JOB: u32 = WM_APP + 3;

/// Drag modifier flags (MK_SHIFT, MK_CONTROL).
const MK_SHIFT_FLAG: u32 = 0x0004;
const MK_CONTROL_FLAG: u32 = 0x0008;

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

/// An HWND the worker threads can post to.
#[derive(Clone, Copy)]
struct PostTarget(usize);

impl PostTarget {
    fn post(self, ev: JobEvent) {
        let raw = Box::into_raw(Box::new(ev));
        let hwnd = HWND(self.0 as *mut core::ffi::c_void);
        let posted =
            unsafe { PostMessageW(Some(hwnd), WM_KILN_JOB, WPARAM(0), LPARAM(raw as isize)) };
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
    hover: Option<usize>,
    opened: Instant,
    closing: Option<Instant>,
    /// The chosen item and when it was chosen; its job starts once the
    /// pulse has played.
    pulse: Option<(usize, Instant)>,
    /// Opened from the keyboard rather than during a drag.
    keyboard: bool,
}

impl WheelUi {
    fn is_open(&self) -> bool {
        self.closing.is_none() && self.pulse.is_none()
    }
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
}

struct App {
    hwnd: HWND,
    kiln: Arc<Kiln>,
    runner: Runner,
    jobs: HashMap<JobId, JobInfo>,
    batches: HashMap<u64, Batch>,
    next_batch: u64,
    palette: &'static Palette,
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
    /// Engine output shown in the details dialog.
    details: Option<String>,
    dialog_close_hover: bool,
    status: String,
    clear_hover: bool,
    clear_pressed: bool,
    tracking_mouse: bool,
    dpi: f32,
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
        let rows = self.rows.iter().any(|r| {
            within(r.added, motion::SETTLE_MS)
                || r.settled.is_some_and(|t| within(t, motion::CONFIRM_MS))
        });
        let wheel = self
            .wheel
            .as_ref()
            .is_some_and(|w| within(w.opened, OPEN_MS) || w.closing.is_some() || w.pulse.is_some());
        self.drag.running() || rows || wheel
    }

    fn row_anim(&self, i: usize) -> RowAnim {
        match self.rows.get(i) {
            Some(r) => RowAnim {
                appear: eased(r.added, motion::SETTLE_MS),
                settle: r.settled.map_or(1.0, |t| eased(t, motion::CONFIRM_MS)),
            },
            None => RowAnim {
                appear: 1.0,
                settle: 1.0,
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
        if r.added > 0 {
            self.files.select(Some(self.files.len() - 1));
            self.scroll_to_selection();
        }
        self.animate();
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
        self.cancel_jobs_for(&path);
        self.files.remove_selected();
        self.rows.remove(i);
        self.status = status_line(self.files.len(), 0, 0, 0);
        self.clamp_scroll();
        self.invalidate();
    }

    fn clear(&mut self) {
        // Stop what's running first, and forget it: results for files
        // that are no longer listed have nowhere to go.
        self.runner.cancel_all();
        self.jobs.clear();
        self.batches.clear();
        self.files.clear();
        self.rows.clear();
        self.scroll = 0.0;
        self.status.clear();
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
        let items = wheel::items_for(kind, &self.kiln, &paths);
        let hover = (keyboard && !items.is_empty()).then_some(0);
        self.wheel = Some(WheelUi {
            wheel: Wheel {
                kind,
                center: wheel::clamp_center(at.0, at.1, w, h),
                source_code: wheel::source_code(&paths),
                items,
            },
            paths,
            hover,
            opened: Instant::now(),
            closing: None,
            pulse: None,
            keyboard,
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

    fn close_wheel(&mut self) {
        if let Some(w) = &mut self.wheel {
            if w.closing.is_none() {
                w.closing = Some(Instant::now());
                self.animate();
            }
        }
    }

    /// Pulse item `i`; its job starts when the pulse ends.
    fn choose(&mut self, i: usize) {
        if let Some(w) = &mut self.wheel {
            if w.is_open() && i < w.wheel.items.len() {
                w.pulse = Some((i, Instant::now()));
                w.hover = Some(i);
                self.animate();
            }
        }
    }

    /// Advance the wheel's animations: start a pulsed job, drop a faded
    /// wheel.
    fn tick_wheel(&mut self) {
        let Some(w) = &mut self.wheel else {
            return;
        };
        if let Some((i, t)) = w.pulse {
            if !within(t, PULSE_MS) {
                w.pulse = None;
                w.closing = Some(Instant::now());
                let action = w.wheel.items[i].action.clone();
                let paths = w.paths.clone();
                self.start(action, paths);
                return;
            }
        }
        if w.closing.is_some_and(|t| !within(t, motion::QUICK_MS)) {
            self.wheel = None;
        }
    }

    fn wheel_view(w: Option<&WheelUi>) -> Option<WheelView<'_>> {
        let w = w?;
        Some(WheelView {
            wheel: &w.wheel,
            hover: w.hover,
            open: eased(w.opened, OPEN_MS),
            fade: w.closing.map_or(1.0, |t| 1.0 - eased(t, motion::QUICK_MS)),
            pulse: w
                .pulse
                .map(|(i, t)| (i, t.elapsed().as_secs_f32() * 1000.0 / PULSE_MS as f32)),
        })
    }

    // Jobs.

    fn start(&mut self, action: Action, paths: Vec<PathBuf>) {
        // Only files that made it into the list.
        let paths: Vec<PathBuf> = paths
            .into_iter()
            .filter(|p| self.files.index_of(p).is_some())
            .collect();
        if paths.is_empty() {
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
            };
            let id = self.runner.submit(kind, Options::default());
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
        self.invalidate();
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
                    },
                    Err(e) if e.cancelled => JobState::Cancelled,
                    Err(e) => JobState::Failed {
                        // One plain line in the row; the rest is in the
                        // details.
                        message: e.message.lines().next().unwrap_or_default().to_owned(),
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
                        Ok(_) => b.ok += 1,
                        Err(e) if e.cancelled => b.cancelled += 1,
                        Err(_) => b.failed += 1,
                    }
                    b.pending -= 1;
                    if b.pending == 0 {
                        self.status =
                            batch_summary(&b.action, b.files, b.ok, b.failed, b.cancelled);
                        self.batches.remove(&job.batch);
                    }
                }
                self.animate();
            }
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

    // Input.

    /// The row under a point and whether the point is on its cancel cross
    /// or "Show details" link.
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
        let link = matches!(&e.job, JobState::Failed { details, .. } if !details.is_empty())
            && layout::row_link(&r).contains(x, y);
        (Some(i), cancel, link)
    }

    fn mouse_move(&mut self, x: f32, y: f32) {
        if self.details.is_some() {
            let (w, h) = self.renderer.size_dips();
            let hot = layout::dialog(w, h).close.contains(x, y);
            if hot != self.dialog_close_hover {
                self.dialog_close_hover = hot;
                self.invalidate();
            }
            return;
        }
        if let Some(w) = &mut self.wheel {
            if w.is_open() {
                let hover = wheel::hit(&w.wheel, x, y);
                if hover != w.hover {
                    w.hover = hover;
                    self.invalidate();
                }
            }
            return;
        }
        let (row, cancel, link) = self.hit_row(x, y);
        let clear = self.layout().clear_button.is_some_and(|b| b.contains(x, y));
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

    fn mouse_down(&mut self, x: f32, y: f32) {
        if self.details.is_some() {
            let (w, h) = self.renderer.size_dips();
            if layout::dialog(w, h).close.contains(x, y) {
                self.details = None;
            }
            self.invalidate();
            return;
        }
        if let Some(w) = &self.wheel {
            if w.is_open() {
                match wheel::hit(&w.wheel, x, y) {
                    Some(i) => self.choose(i),
                    None => self.close_wheel(),
                }
            }
            return;
        }
        let l = self.layout();
        if l.clear_button.is_some_and(|b| b.contains(x, y)) {
            self.clear_pressed = true;
            unsafe { SetCapture(self.hwnd) };
        } else {
            let (row, cancel, link) = self.hit_row(x, y);
            self.files.select(row);
            if let Some(i) = row {
                if cancel {
                    self.cancel_row(i);
                } else if link {
                    self.show_details(i);
                }
            }
        }
        self.invalidate();
    }

    fn key(&mut self, key: VIRTUAL_KEY) -> bool {
        let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
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
        if let Some(w) = self.wheel.as_mut().filter(|w| w.is_open()) {
            let n = w.wheel.items.len();
            match key {
                VK_RIGHT | VK_DOWN | VK_TAB => w.hover = wheel::step(w.hover, 1, n),
                VK_LEFT | VK_UP => w.hover = wheel::step(w.hover, -1, n),
                VK_RETURN | VK_SPACE => {
                    if let Some(i) = w.hover {
                        self.choose(i);
                    }
                }
                VK_ESCAPE => self.close_wheel(),
                _ => return true,
            }
            self.invalidate();
            return true;
        }
        match key {
            VK_UP => self.files.step(-1),
            VK_DOWN => self.files.step(1),
            VK_HOME => self.files.select(Some(0)),
            VK_END => self.files.select(self.files.len().checked_sub(1)),
            VK_DELETE | VK_BACK => self.remove_selected(),
            VK_ESCAPE => self.files.select(None),
            VK_RETURN if ctrl => self.open_wheel_for_selection(WheelKind::Tools),
            VK_RETURN => self.open_wheel_for_selection(WheelKind::Convert),
            // Space follows a failed row's "Show details" link.
            VK_SPACE => {
                if let Some(i) = self.files.selected() {
                    self.show_details(i);
                }
            }
            _ => return false,
        }
        self.scroll_to_selection();
        self.invalidate();
        true
    }

    /// While a drag is over the window: Shift pours the convert wheel,
    /// Ctrl the tools wheel; letting go of the key lets it fade.
    fn drag_over(&mut self, ev: &DragEvent) {
        let at = self.screen_dips(ev.x, ev.y);
        let keys = ev.keys.0;
        let want = if keys & MK_SHIFT_FLAG != 0 {
            Some(WheelKind::Convert)
        } else if keys & MK_CONTROL_FLAG != 0 {
            Some(WheelKind::Tools)
        } else {
            None
        };
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
        if let Some(w) = &mut self.wheel {
            if w.is_open() {
                let hover = wheel::hit(&w.wheel, at.0, at.1);
                if hover != w.hover {
                    w.hover = hover;
                    self.invalidate();
                }
            }
        }
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
            Some(i) => self.choose(i),
            None => self.close_wheel(),
        }
        self.drag_paths.clear();
    }

    fn paint(&mut self) {
        let anims: Vec<RowAnim> = (0..self.files.len()).map(|i| self.row_anim(i)).collect();
        let anim = |i: usize| {
            anims.get(i).copied().unwrap_or(RowAnim {
                appear: 1.0,
                settle: 1.0,
            })
        };
        let view = View {
            palette: self.palette,
            entries: self.files.entries(),
            selected: self.files.selected(),
            hover_row: self.hover_row,
            hover_cancel: self.hover_cancel,
            hover_link: self.hover_link,
            scroll: self.scroll,
            drag: self.drag.value().clamp(0.0, 1.0),
            row_anim: &anim,
            status: &self.status,
            clear_hover: self.clear_hover,
            clear_pressed: self.clear_pressed,
            wheel: Self::wheel_view(self.wheel.as_ref()),
            details: self.details.as_deref(),
            dialog_close_hover: self.dialog_close_hover,
        };
        if let Err(e) = self.renderer.draw(&view) {
            log(&format!("draw failed: {e}"));
        }
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
        OleInitialize(None)?;
    }
    let result = run_window();
    unsafe { OleUninitialize() };
    result
}

fn run_window() -> Result<()> {
    let palette = &theme::DARK;
    let instance: HINSTANCE = unsafe { GetModuleHandleW(None)? }.into();
    let class = w!("KilnWindow");
    let icon = unsafe { LoadIconW(Some(instance), PCWSTR(1 as _)) }.unwrap_or_default();
    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
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

    let kiln = Arc::new(Kiln::new());
    let target = PostTarget(hwnd.0 as usize);
    let runner = Runner::new(kiln.clone(), Runner::default_workers(), move |ev| {
        target.post(ev)
    });

    let app = Box::new(App {
        hwnd,
        kiln,
        runner,
        jobs: HashMap::new(),
        batches: HashMap::new(),
        next_batch: 1,
        palette,
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
        details: None,
        dialog_close_hover: false,
        status: String::new(),
        clear_hover: false,
        clear_pressed: false,
        tracking_mouse: false,
        dpi,
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
    }

    // Files dropped on the app icon arrive as arguments.
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    if !args.is_empty() {
        unsafe { (*app).add(args) };
    }

    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        // Dropping the app stops the workers; events they post now find
        // no window and are freed on their side.
        drop(Box::from_raw(app));
    }
    Ok(())
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
        WM_TIMER if wparam.0 == ANIM_TIMER => {
            app.tick_wheel();
            if !app.animating() {
                unsafe {
                    let _ = KillTimer(Some(hwnd), ANIM_TIMER);
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
            let (x, y) = app.dips(lo(lparam), hi(lparam));
            app.mouse_move(x, y);
            Some(LRESULT(0))
        }
        WM_MOUSELEAVE => {
            app.tracking_mouse = false;
            app.hover_row = None;
            app.hover_cancel = false;
            app.hover_link = false;
            app.clear_hover = false;
            app.clear_pressed = false;
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
        WM_LBUTTONUP => {
            if app.clear_pressed {
                app.clear_pressed = false;
                unsafe {
                    let _ = ReleaseCapture();
                }
                let (x, y) = app.dips(lo(lparam), hi(lparam));
                if app.layout().clear_button.is_some_and(|b| b.contains(x, y)) {
                    app.clear();
                }
                app.invalidate();
            }
            Some(LRESULT(0))
        }
        WM_MOUSEWHEEL => {
            if app.wheel.is_none() && app.details.is_none() {
                let delta = ((wparam.0 >> 16) & 0xFFFF) as i16 as f32 / WHEEL_DELTA as f32;
                app.scroll -= delta * ROW_HEIGHT;
                app.clamp_scroll();
                app.invalidate();
            }
            Some(LRESULT(0))
        }
        // The Apps key, Shift+F10 or a right click: the convert wheel for
        // the selected file.
        WM_CONTEXTMENU => {
            if app.wheel.is_some() || app.details.is_some() {
                return Some(LRESULT(0));
            }
            if lparam.0 == -1 {
                app.open_wheel_for_selection(WheelKind::Convert);
            } else {
                let at = app.screen_dips(lo(lparam), hi(lparam));
                let (row, _, _) = app.hit_row(at.0, at.1);
                if let Some(i) = row {
                    app.files.select(Some(i));
                    let paths = vec![app.files.entries()[i].path.clone()];
                    app.open_wheel(WheelKind::Convert, paths, at, false);
                }
            }
            Some(LRESULT(0))
        }
        WM_KEYDOWN => app.key(VIRTUAL_KEY(wparam.0 as u16)).then_some(LRESULT(0)),
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
        WM_DESTROY => {
            unsafe {
                let _ = RevokeDragDrop(hwnd);
                PostQuitMessage(0);
            }
            Some(LRESULT(0))
        }
        _ => None,
    }
}
