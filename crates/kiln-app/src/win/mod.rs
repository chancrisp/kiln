//! The Win32 window: creation, the message loop, input and animation.

mod drop;
mod render;
mod text;

use std::path::PathBuf;
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

use kiln_core::Kiln;

use crate::layout::{self, ROW_HEIGHT};
use crate::model::{status_line, FileList};
use crate::theme::{self, motion, Palette};
use drop::{DragEvent, DropTarget, DRAG_ENTER, DRAG_LEAVE, DRAG_OVER, WM_KILN_DRAG, WM_KILN_DROP};
use render::{Renderer, View};

const ANIM_TIMER: usize = 1;
const FRAME_MS: u32 = 16;

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

struct App {
    hwnd: HWND,
    kiln: Kiln,
    palette: &'static Palette,
    renderer: Renderer,
    files: FileList,
    added_at: Vec<Instant>,
    scroll: f32,
    hover_row: Option<usize>,
    drag: Tween,
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
        self.drag.running()
            || self
                .added_at
                .iter()
                .any(|t| t.elapsed() < Duration::from_millis(motion::SETTLE_MS as u64))
    }

    fn row_alpha(&self, i: usize) -> f32 {
        let Some(t) = self.added_at.get(i) else {
            return 1.0;
        };
        motion::ease_out(t.elapsed().as_secs_f32() * 1000.0 / motion::SETTLE_MS as f32)
    }

    fn add(&mut self, paths: Vec<PathBuf>) {
        let before = self.files.len();
        let r = self.files.add(&self.kiln, paths);
        let now = Instant::now();
        self.added_at.resize(self.files.len(), now);
        for t in &mut self.added_at[before..] {
            *t = now;
        }
        self.status = status_line(self.files.len(), r.added, r.duplicates, r.skipped);
        if r.added > 0 {
            self.files.select(Some(self.files.len() - 1));
            self.scroll_to_selection();
        }
        self.animate();
    }

    fn remove_selected(&mut self) {
        if let Some(i) = self.files.selected() {
            self.files.remove_selected();
            self.added_at.remove(i);
            self.status = status_line(self.files.len(), 0, 0, 0);
            self.clamp_scroll();
            self.invalidate();
        }
    }

    fn clear(&mut self) {
        self.files.clear();
        self.added_at.clear();
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

    fn paint(&mut self) {
        let alphas: Vec<f32> = (0..self.files.len()).map(|i| self.row_alpha(i)).collect();
        let alpha = |i: usize| alphas.get(i).copied().unwrap_or(1.0);
        let view = View {
            palette: self.palette,
            entries: self.files.entries(),
            selected: self.files.selected(),
            hover_row: self.hover_row,
            scroll: self.scroll,
            drag: self.drag.value().clamp(0.0, 1.0),
            row_alpha: &alpha,
            status: &self.status,
            clear_hover: self.clear_hover,
            clear_pressed: self.clear_pressed,
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

    let app = Box::new(App {
        hwnd,
        kiln: Kiln::new(),
        palette,
        renderer: Renderer::new(hwnd, dpi)?,
        files: FileList::default(),
        added_at: vec![],
        scroll: 0.0,
        hover_row: None,
        drag: Tween::new(0.0),
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
            let l = app.layout();
            let row = layout::row_at(&l, x, y, app.scroll);
            let clear = l.clear_button.is_some_and(|b| b.contains(x, y));
            if row != app.hover_row || clear != app.clear_hover {
                app.hover_row = row;
                app.clear_hover = clear;
                app.invalidate();
            }
            Some(LRESULT(0))
        }
        WM_MOUSELEAVE => {
            app.tracking_mouse = false;
            app.hover_row = None;
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
            let l = app.layout();
            if l.clear_button.is_some_and(|b| b.contains(x, y)) {
                app.clear_pressed = true;
                unsafe { SetCapture(hwnd) };
            } else {
                app.files.select(layout::row_at(&l, x, y, app.scroll));
            }
            app.invalidate();
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
            let delta = ((wparam.0 >> 16) & 0xFFFF) as i16 as f32 / WHEEL_DELTA as f32;
            app.scroll -= delta * ROW_HEIGHT;
            app.clamp_scroll();
            app.invalidate();
            Some(LRESULT(0))
        }
        WM_KEYDOWN => {
            let key = VIRTUAL_KEY(wparam.0 as u16);
            match key {
                VK_UP => app.files.step(-1),
                VK_DOWN => app.files.step(1),
                VK_HOME => app.files.select(Some(0)),
                VK_END => app.files.select(app.files.len().checked_sub(1)),
                VK_DELETE | VK_BACK => app.remove_selected(),
                VK_ESCAPE => app.files.select(None),
                _ => return None,
            }
            app.scroll_to_selection();
            app.invalidate();
            Some(LRESULT(0))
        }
        WM_KILN_DRAG => {
            let to = if wparam.0 == DRAG_LEAVE { 0.0 } else { 1.0 };
            if wparam.0 == DRAG_ENTER || wparam.0 == DRAG_LEAVE {
                app.drag.set(to, motion::QUICK_MS);
                app.animate();
            }
            let _ = DRAG_OVER;
            Some(LRESULT(0))
        }
        WM_KILN_DROP => {
            let ev = unsafe { &*(lparam.0 as *const DragEvent) };
            let mut pt = POINT { x: ev.x, y: ev.y };
            unsafe {
                let _ = ScreenToClient(hwnd, &mut pt);
            }
            app.drag.set(0.0, motion::QUICK_MS);
            app.add(ev.paths.clone());
            unsafe {
                let _ = SetForegroundWindow(hwnd);
            }
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
