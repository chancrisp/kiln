//! The menu of the icon in the notification area, drawn by Kiln itself in a
//! small window of its own: the same Menu flyout the right-click menu has,
//! in the theme, at the scale and at the Text size the window has, so it is
//! not a white Windows menu on a dark desktop.
//!
//! It opens where the icon was clicked, inside the screen's work area, and
//! closes when it loses the focus, on a click anywhere else, on Escape and
//! once an item is chosen. The keys are Up, Down, Home, End, Enter, Space,
//! Escape and each item's access key. UI Automation sees a Menu of menu
//! items (`a11y::tray_menu`). Choosing an item posts the window it was
//! opened for a `WM_COMMAND` with the item's command, as a menu does; the
//! window answers that as it always has.
//!
//! With the test placement switch on it is shown without being activated
//! and never takes the focus, so it stays open until it is closed or an item
//! is chosen: a test drives it through UI Automation.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::OnceLock;

use windows::core::{w, Error, Result, HRESULT, PCWSTR};
use windows::Win32::Foundation::{E_FAIL, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, ValidateRect, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Accessibility::UiaRootObjectId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetCapture, SetCapture, SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VIRTUAL_KEY,
    VK_APPS, VK_CONTROL, VK_DOWN, VK_END, VK_ESCAPE, VK_HOME, VK_LEFT, VK_MENU, VK_RETURN,
    VK_RIGHT, VK_SHIFT, VK_SPACE, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::a11y::{self, Id, Request, TrayLineIn, Verb};
use crate::controls::{Key, MenuEvent, MenuState, Press};
use crate::layout::{self, kit::MenuLayout, kit::MenuRow, Rect};
use crate::theme::fluent::{glyph, FluentPalette};

use super::channel::{self, Inbox, WM_KILN_WAKE};
use super::crash;
use super::float_wheel::monitor_dpi;
use super::placement;
use super::render::{MenuLine, Renderer};
use super::tray::{menu_items, TrayCommand};
use super::uia;

/// `WM_MOUSELEAVE`, which asking for it (`TrackMouseEvent`) brings once.
const WM_MOUSELEAVE: u32 = 0x02A3;

/// The menu's window class.
const CLASS: PCWSTR = w!("KilnTrayMenu");

/// The window's name, for UI Automation and the task switcher's lists.
pub const TITLE: &str = "Kiln menu";

/// Room round the menu for its shadow, in layout units: the flyout shadow
/// reaches 8 units down and 8 round (`FluentPalette::flyout_shadow`).
const SHADOW: f32 = 20.0;

/// One line of the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    Item {
        command: TrayCommand,
        /// What it says, without the `&` of its access key.
        label: String,
        /// The key that chooses it from the keyboard, in capitals.
        access: Option<char>,
        /// Whether it is checked: `None` for an item that is not a switch.
        checked: Option<bool>,
    },
    Separator,
}

/// The menu's lines, from `tray::menu_items`, for the drag-wheel being on
/// or off.
pub fn lines(drag_wheel_on: bool) -> Vec<Line> {
    menu_items(drag_wheel_on)
        .into_iter()
        .map(|item| match item {
            Some((command, words, checked)) => {
                let (label, access) = split_access(words);
                // Only the drag-wheel's item is a switch.
                let checked = (command == TrayCommand::ToggleDragWheel).then_some(checked);
                Line::Item {
                    command,
                    label,
                    access,
                    checked,
                }
            }
            None => Line::Separator,
        })
        .collect()
}

/// `words` as Windows menus write them, with `&` before the access key:
/// what it says, and the key in capitals.
fn split_access(words: &str) -> (String, Option<char>) {
    let mut label = String::new();
    let mut access = None;
    let mut chars = words.chars();
    while let Some(c) = chars.next() {
        if c == '&' {
            match chars.next() {
                Some('&') => label.push('&'),
                Some(k) => {
                    access.get_or_insert(k.to_ascii_uppercase());
                    label.push(k);
                }
                None => {}
            }
        } else {
            label.push(c);
        }
    }
    (label, access)
}

/// What an input did to the open menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Did {
    /// Nothing to draw again.
    Nothing,
    /// The lit item changed.
    Lit,
    /// Do this command; the menu closes.
    Chose(TrayCommand),
    /// The menu closes with nothing chosen.
    Close,
}

/// The menu's state: what it holds, where it is (in its window's layout
/// units), and which item is lit. No window: what a key or the pointer
/// does to it.
pub struct Menu {
    pub lines: Vec<Line>,
    pub layout: MenuLayout,
    state: MenuState,
}

/// A key as the menu's keys read it.
fn press_of(key: VIRTUAL_KEY) -> Option<Press> {
    Some(Press::key(match key {
        VK_UP => Key::Up,
        VK_DOWN => Key::Down,
        VK_HOME => Key::Home,
        VK_END => Key::End,
        VK_RETURN => Key::Enter,
        VK_SPACE => Key::Space,
        VK_ESCAPE => Key::Esc,
        _ => return None,
    }))
}

impl Menu {
    /// A menu of `lines` laid out as `layout` has them.
    pub fn new(lines: Vec<Line>, layout: MenuLayout) -> Menu {
        let enabled = lines.iter().map(|l| *l != Line::Separator).collect();
        Menu {
            lines,
            layout,
            state: MenuState::new(enabled, false),
        }
    }

    /// Whether `(x, y)` is on the menu itself, not its shadow.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.layout.rect.contains(x, y)
    }

    /// The lit item.
    pub fn lit(&self) -> Option<usize> {
        self.state.highlight
    }

    /// Light item `k`, if it is one.
    pub fn light(&mut self, k: usize) -> Did {
        if !matches!(self.lines.get(k), Some(Line::Item { .. })) || self.lit() == Some(k) {
            return Did::Nothing;
        }
        self.state.highlight = Some(k);
        Did::Lit
    }

    /// The command of item `k`.
    pub fn command(&self, k: usize) -> Option<TrayCommand> {
        match self.lines.get(k)? {
            Line::Item { command, .. } => Some(*command),
            Line::Separator => None,
        }
    }

    /// The item under `(x, y)`: never a separator.
    pub fn item_at(&self, x: f32, y: f32) -> Option<usize> {
        self.layout
            .rows
            .iter()
            .position(|r| r.contains(x, y))
            .filter(|&k| self.lines[k] != Line::Separator)
    }

    /// The pointer moved to `(x, y)`: the item under it is lit, and off the
    /// items nothing is.
    pub fn hover(&mut self, x: f32, y: f32) -> Did {
        let under = self.item_at(x, y);
        if under == self.state.highlight {
            return Did::Nothing;
        }
        self.state.highlight = under;
        Did::Lit
    }

    /// A button came up at `(x, y)`: the item under it is chosen. Off the
    /// menu it closes; on a separator or the menu's edge nothing happens.
    pub fn release(&self, x: f32, y: f32) -> Did {
        match self.item_at(x, y) {
            Some(k) => self.command(k).map_or(Did::Nothing, Did::Chose),
            None if self.contains(x, y) => Did::Nothing,
            None => Did::Close,
        }
    }

    /// A button went down at `(x, y)`: off the menu it closes.
    pub fn press(&self, x: f32, y: f32) -> Did {
        if self.contains(x, y) {
            Did::Nothing
        } else {
            Did::Close
        }
    }

    /// A key: Up and Down move round the items, Home and End go to the
    /// ends, Enter and Space choose the lit one, Escape closes, and an
    /// item's access key chooses it. Left and Right do nothing, as there
    /// are no submenus, and neither does a letter that is no access key.
    /// Any other key closes the menu, as a Windows menu does with a key it
    /// has no use for, except Shift, Ctrl and Alt alone.
    pub fn key(&mut self, key: VIRTUAL_KEY) -> Did {
        if matches!(key, VK_SHIFT | VK_CONTROL | VK_MENU | VK_LEFT | VK_RIGHT) {
            return Did::Nothing;
        }
        if let Some(letter) = letter_of(key) {
            let chosen = self.lines.iter().find_map(|l| match l {
                Line::Item {
                    command,
                    access: Some(a),
                    ..
                } if *a == letter => Some(*command),
                _ => None,
            });
            return chosen.map_or(Did::Nothing, Did::Chose);
        }
        let Some(p) = press_of(key) else {
            return Did::Close;
        };
        match self.state.key(p) {
            MenuEvent::Ignored => Did::Nothing,
            MenuEvent::Moved(_) => Did::Lit,
            MenuEvent::Act(k) => self.command(k).map_or(Did::Nothing, Did::Chose),
            MenuEvent::Close => Did::Close,
        }
    }

    /// The lines as the flyout draws them: a checked item has a check
    /// mark in front of it.
    fn drawn(&self) -> Vec<MenuLine<'_>> {
        self.lines
            .iter()
            .map(|l| match l {
                Line::Separator => MenuLine::Separator,
                Line::Item { label, checked, .. } => MenuLine::Item {
                    label,
                    glyph: (*checked == Some(true)).then_some(glyph::CHECKMARK),
                    shortcut: None,
                    enabled: true,
                },
            })
            .collect()
    }

    /// The lines as the tree reads them.
    fn tree_lines(&self) -> Vec<TrayLineIn<'_>> {
        self.lines
            .iter()
            .map(|l| match l {
                Line::Separator => None,
                Line::Item { label, checked, .. } => Some((label.as_str(), *checked)),
            })
            .collect()
    }

    /// The tree for UI Automation.
    fn tree(&self) -> a11y::Node {
        a11y::tray_menu(&self.tree_lines(), &self.layout, self.lit())
    }
}

/// The capital letter a key types, for access keys.
fn letter_of(key: VIRTUAL_KEY) -> Option<char> {
    (0x41..=0x5A)
        .contains(&key.0)
        .then(|| char::from(key.0 as u8))
}

/// Each line as the flyout lays it out, its words measured by `width` (in
/// Body, `false`, or Caption, `true`). A check mark's column is there when
/// an item is checked.
pub fn rows(lines: &[Line], width: &dyn Fn(&str, bool) -> f32) -> Vec<MenuRow> {
    let checked = lines.iter().any(|l| {
        matches!(
            l,
            Line::Item {
                checked: Some(true),
                ..
            }
        )
    });
    lines
        .iter()
        .map(|l| match l {
            Line::Separator => MenuRow::Separator,
            Line::Item { label, .. } => MenuRow::Item {
                label: width(label, false),
                accel: 0.0,
                glyph: checked,
            },
        })
        .collect()
}

/// Where the menu goes for a click at `at` (screen pixels) in `work` (the
/// screen's work area, pixels) at `k` pixels a layout unit: the window's
/// rectangle in screen pixels, with room round the menu for its shadow,
/// and the menu's layout inside it, in the window's units. The menu opens
/// down and right from the click, and up or left where the work area has
/// no room, as a Windows menu does.
pub fn plan(work: RECT, at: POINT, k: f32, rows: &[MenuRow]) -> (RECT, MenuLayout) {
    let bounds = Rect::new(
        0.0,
        0.0,
        (work.right - work.left) as f32 / k,
        (work.bottom - work.top) as f32 / k,
    );
    let click = ((at.x - work.left) as f32 / k, (at.y - work.top) as f32 / k);
    let on_screen = layout::kit::menu_flyout(click, rows, bounds);
    let size = (
        on_screen.rect.w + 2.0 * SHADOW,
        on_screen.rect.h + 2.0 * SHADOW,
    );
    let left = work.left + ((on_screen.rect.x - SHADOW) * k).round() as i32;
    let top = work.top + ((on_screen.rect.y - SHADOW) * k).round() as i32;
    let window = RECT {
        left,
        top,
        right: left + (size.0 * k).round() as i32,
        bottom: top + (size.1 * k).round() as i32,
    };
    let inside =
        layout::kit::menu_flyout((SHADOW, SHADOW), rows, Rect::new(0.0, 0.0, size.0, size.1));
    (window, inside)
}

/// What the menu is opened with.
pub struct Open {
    /// The window that hears the chosen command (`WM_COMMAND`).
    pub owner: HWND,
    /// The click, in screen pixels.
    pub at: POINT,
    pub drag_wheel_on: bool,
    pub palette: FluentPalette,
    /// Windows' Text size, 1.0 to 2.25.
    pub text_scale: f32,
}

/// What lives with the window: freed at its last message.
struct State {
    alive: Rc<Cell<bool>>,
    owner: HWND,
    menu: Menu,
    palette: FluentPalette,
    renderer: Renderer,
    host: uia::Host,
    inbox: Inbox<Request>,
    /// Pixels a layout unit.
    px: f32,
    tracking: bool,
    /// It holds the mouse capture because Windows would not give it the
    /// foreground: a click anywhere then reaches it, and closes it.
    captured: bool,
}

/// The open menu. Its window closes when this is dropped.
pub struct TrayMenu {
    hwnd: HWND,
    alive: Rc<Cell<bool>>,
}

fn register_class() -> Result<()> {
    static REGISTERED: OnceLock<std::result::Result<(), HRESULT>> = OnceLock::new();
    let done = REGISTERED.get_or_init(|| unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None).map_err(|e| e.code())?.into();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            return Err(Error::from_thread().code());
        }
        Ok(())
    });
    done.map_err(Error::from)
}

/// The work area of the monitor `at` is on, in screen pixels.
fn work_area(at: POINT) -> RECT {
    let monitor = unsafe { MonitorFromPoint(at, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        info.rcWork
    } else {
        RECT {
            left: at.x - 1000,
            top: at.y - 1000,
            right: at.x + 1000,
            bottom: at.y + 1000,
        }
    }
}

/// Open the menu where `a.at` is. It shows nothing until its first frame
/// is drawn. The thread must run a message loop, as the window's does.
pub fn open(a: Open) -> Result<TrayMenu> {
    register_class()?;
    // The test placement switch brings the click into its area.
    let area = placement::area();
    let at = area.map_or((a.at.x, a.at.y), |r| {
        placement::point_in(r, (a.at.x, a.at.y))
    });
    let at = POINT { x: at.0, y: at.1 };
    let monitor = unsafe { MonitorFromPoint(at, MONITOR_DEFAULTTONEAREST) };
    let dpi = monitor_dpi(monitor);
    let work = area.map_or_else(
        || work_area(at),
        |r| RECT {
            left: r.x,
            top: r.y,
            right: r.right(),
            bottom: r.bottom(),
        },
    );
    let zoom = layout::text_scale(a.text_scale);
    let quiet = placement::quiet_on();
    let instance: HINSTANCE = unsafe { GetModuleHandleW(None)? }.into();
    let mut ex = WS_EX_LAYERED | WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOPMOST | WS_EX_TOOLWINDOW;
    if quiet {
        ex |= WS_EX_NOACTIVATE;
    }
    // No owner: the window it is for is hidden while the icon is there,
    // and an owned window hides with its owner.
    let hwnd = unsafe {
        CreateWindowExW(
            ex,
            CLASS,
            &windows::core::HSTRING::from(TITLE),
            WS_POPUP,
            // On the monitor it is for, so it is made at that monitor's
            // scale and moving it there changes none.
            work.left,
            work.top,
            1,
            1,
            None,
            None,
            Some(instance),
            None,
        )?
    };
    let alive = Rc::new(Cell::new(true));
    let menu = TrayMenu {
        hwnd,
        alive: alive.clone(),
    };
    // From here a failure drops `menu`, which destroys the window.
    let mut renderer = Renderer::new(hwnd, dpi * zoom)?;
    renderer.set_zoom(zoom);
    let px = layout::px_per_unit(dpi, zoom);
    let mut items = lines(a.drag_wheel_on);
    let rows = rows(&items, &|s, caption| renderer.menu_label_width(s, caption));
    let (window, laid) = plan(work, at, px, &rows);
    let (wake, inbox) = channel::channel::<Request>(hwnd);
    let host = uia::Host::with_post(hwnd, move |r| wake.send(r));
    let state = State {
        alive,
        owner: a.owner,
        menu: Menu::new(std::mem::take(&mut items), laid),
        palette: a.palette,
        renderer,
        host,
        inbox,
        px,
        tracking: false,
        captured: false,
    };
    let raw = Box::into_raw(Box::new(RefCell::new(state)));
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw as isize);
        SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            window.left,
            window.top,
            window.right - window.left,
            window.bottom - window.top,
            SWP_NOACTIVATE,
        )?;
        // A layered window shows nothing until it has its attributes:
        // whole, so what the composition shows is what is drawn.
        SetLayeredWindowAttributes(
            hwnd,
            windows::Win32::Foundation::COLORREF(0),
            255,
            LWA_ALPHA,
        )?;
    }
    // The first frame, and the tree: the menu appears to a screen reader
    // as it is shown.
    let events = {
        let cell = unsafe { &*raw };
        let mut s = cell.try_borrow_mut().map_err(|_| Error::from(E_FAIL))?;
        s.paint()?;
        s.announce_open()
    };
    unsafe {
        let _ = ShowWindow(hwnd, placement::show_cmd(SW_SHOW));
        if !quiet {
            // A menu that isn't in front can't tell a click elsewhere.
            let _ = SetForegroundWindow(hwnd);
            let _ = SetFocus(Some(hwnd));
            // Windows may refuse the foreground (a foreground lock, or a
            // delayed message). Then it never loses the activation, so it
            // takes the mouse instead: a click off the menu reaches it
            // and closes it, as a Windows menu's own capture does.
            if needs_capture(GetForegroundWindow() == hwnd) {
                let _ = SetCapture(hwnd);
                if GetCapture() == hwnd {
                    if let Ok(mut s) = (&*raw).try_borrow_mut() {
                        s.captured = true;
                    }
                }
            }
        }
    }
    uia::raise(events);
    Ok(menu)
}

/// Whether the menu must hold the mouse to hear a click elsewhere: it
/// does when it is not the foreground window, where losing the activation
/// would have told it.
fn needs_capture(is_foreground: bool) -> bool {
    !is_foreground
}

impl TrayMenu {
    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// Whether its window is still there: it closes itself.
    pub fn is_open(&self) -> bool {
        self.alive.get()
    }

    /// The lit item, as a line's number.
    pub fn lit(&self) -> Option<usize> {
        with_state(self.hwnd, |s| s.menu.lit()).flatten()
    }

    /// Close it with nothing chosen. The menu is asked to close rather than
    /// destroyed here: `App` owns it, so this runs with `App` borrowed, and
    /// destroying the active window would send the main window its
    /// activation from inside that borrow. The menu's own window closes on
    /// the posted message, with nothing borrowed.
    pub fn close(&self) {
        if self.alive.get() {
            unsafe {
                let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
    }
}

impl Drop for TrayMenu {
    fn drop(&mut self) {
        self.close();
    }
}

/// The window's state, for the length of one call. `None` when there is
/// none, or it is in use further up the stack: a message sent while it was
/// being used, such as one `ShowWindow` makes.
fn with_state<R>(hwnd: HWND, f: impl FnOnce(&mut State) -> R) -> Option<R> {
    // SAFETY: the user data is null or the pointer `open` stored from a
    // `Box`, freed only at the window's last message, after which no call
    // reads it again; all on the window's thread.
    let raw = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const RefCell<State>;
    let cell = unsafe { raw.as_ref() }?;
    let mut s = cell.try_borrow_mut().ok()?;
    Some(f(&mut s))
}

impl State {
    /// Draw the menu and show it.
    fn paint(&mut self) -> Result<()> {
        let State {
            renderer,
            menu,
            palette,
            ..
        } = self;
        let lines = menu.drawn();
        renderer.draw_popup_menu(&lines, &menu.layout, menu.lit(), palette)
    }

    /// The menu appearing, told to UI Automation: first an empty window,
    /// then the menu in it, so clients are told a menu opened and where
    /// the keyboard is.
    fn announce_open(&mut self) -> Vec<uia::Event> {
        let empty = a11y::Node::new(Id::Root, a11y::Role::Window, "Kiln", Rect::default());
        self.host.update(empty, self.px, false, false);
        self.host
            .update(self.menu.tree(), self.px, !placement::quiet_on(), true)
    }

    /// The tree again, after the lit item moved.
    fn tell(&mut self) -> Vec<uia::Event> {
        self.host
            .update(self.menu.tree(), self.px, !placement::quiet_on(), true)
    }

    /// `(x, y)` in pixels, in layout units.
    fn units(&self, x: i32, y: i32) -> (f32, f32) {
        (x as f32 / self.px, y as f32 / self.px)
    }
}

/// What the window does after a message, once its state is let go.
#[derive(Debug, PartialEq)]
enum Then {
    Nothing,
    Draw,
    Do(TrayCommand),
    Close,
}

impl From<Did> for Then {
    fn from(d: Did) -> Then {
        match d {
            Did::Nothing => Then::Nothing,
            Did::Lit => Then::Draw,
            Did::Chose(c) => Then::Do(c),
            Did::Close => Then::Close,
        }
    }
}

/// Carry out `then` with the state let go: a command is posted to the
/// window the menu is for, and the menu closes; a lit item moved is drawn
/// and told.
fn finish(hwnd: HWND, then: Then) {
    match then {
        Then::Nothing => {}
        Then::Draw => {
            let events = with_state(hwnd, |s| {
                if let Err(e) = s.paint() {
                    crate::diag::log(&format!("the tray menu couldn't be drawn: {e}"));
                }
                s.tell()
            });
            uia::raise(events.unwrap_or_default());
        }
        Then::Do(command) => {
            if let Some(owner) = with_state(hwnd, |s| s.owner) {
                // SAFETY: posts a number to a window that may be gone, in
                // which case the post fails.
                unsafe {
                    let _ = PostMessageW(
                        Some(owner),
                        WM_COMMAND,
                        WPARAM(usize::from(command.id())),
                        LPARAM(0),
                    );
                }
            }
            close(hwnd);
        }
        Then::Close => close(hwnd),
    }
}

fn close(hwnd: HWND) {
    // Its own end of the capture is not a reason to close again.
    let _ = with_state(hwnd, |s| s.captured = false);
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

/// The window's procedure. Its work runs inside `crash::WINDOW`, the main
/// window's guard: after a panic in either, neither touches its state
/// again, and Kiln says it has to close and ends.
extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if crash::WINDOW.tripped() {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    crash::WINDOW
        .run(|| answer(hwnd, msg, wparam, lparam))
        .unwrap_or_else(|| crash::close_after_panic(HWND::default()))
}

fn point_of(lparam: LPARAM) -> (i32, i32) {
    (
        i32::from(lparam.0 as u16 as i16),
        i32::from((lparam.0 >> 16) as u16 as i16),
    )
}

fn answer(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // Clicked or not, it is never activated while a test drives it.
        WM_MOUSEACTIVATE if placement::quiet_on() => return LRESULT(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => return LRESULT(1),
        WM_PAINT => {
            unsafe {
                let _ = ValidateRect(Some(hwnd), None);
            }
            with_state(hwnd, |s| {
                if let Err(e) = s.paint() {
                    crate::diag::log(&format!("the tray menu couldn't be drawn: {e}"));
                }
            });
            return LRESULT(0);
        }
        WM_GETOBJECT if lparam.0 as i32 == UiaRootObjectId => {
            if let Some(root) = with_state(hwnd, |s| s.host.root()) {
                return uia::answer(hwnd, wparam, lparam, root);
            }
        }
        WM_MOUSEMOVE => {
            let then = with_state(hwnd, |s| {
                if !s.tracking {
                    s.tracking = true;
                    let mut track = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    // SAFETY: asks for one WM_MOUSELEAVE for this window.
                    unsafe {
                        let _ = TrackMouseEvent(&mut track);
                    }
                }
                let (x, y) = point_of(lparam);
                let (x, y) = s.units(x, y);
                s.menu.hover(x, y)
            });
            finish(hwnd, then.map_or(Then::Nothing, Then::from));
            return LRESULT(0);
        }
        WM_MOUSELEAVE => {
            let then = with_state(hwnd, |s| {
                s.tracking = false;
                s.menu.hover(-1.0, -1.0)
            });
            finish(hwnd, then.map_or(Then::Nothing, Then::from));
            return LRESULT(0);
        }
        WM_LBUTTONDOWN | WM_RBUTTONDOWN => {
            let then = with_state(hwnd, |s| {
                let (x, y) = point_of(lparam);
                let (x, y) = s.units(x, y);
                s.menu.press(x, y)
            });
            finish(hwnd, then.map_or(Then::Nothing, Then::from));
            return LRESULT(0);
        }
        WM_LBUTTONUP | WM_RBUTTONUP => {
            let then = with_state(hwnd, |s| {
                let (x, y) = point_of(lparam);
                let (x, y) = s.units(x, y);
                s.menu.release(x, y)
            });
            finish(hwnd, then.map_or(Then::Nothing, Then::from));
            return LRESULT(0);
        }
        WM_KEYDOWN => {
            let key = VIRTUAL_KEY(wparam.0 as u16);
            let then = with_state(hwnd, |s| s.menu.key(key));
            finish(hwnd, then.map_or(Then::Nothing, Then::from));
            return LRESULT(0);
        }
        // The menu key opens the menu on the icon and closes this one.
        WM_SYSKEYDOWN if wparam.0 as u16 == VK_APPS.0 => {
            finish(hwnd, Then::Close);
            return LRESULT(0);
        }
        // A client asked for something: it is done as a click or a key
        // would, if the item is still there.
        WM_KILN_WAKE => {
            let then = with_state(hwnd, |s| {
                let mut then = Then::Nothing;
                for Request { id, verb } in s.inbox.woken() {
                    let Id::TrayMenuItem(k) = id else {
                        continue;
                    };
                    match verb {
                        Verb::Invoke | Verb::Toggle => {
                            if let Some(c) = s.menu.command(k) {
                                then = Then::Do(c);
                            }
                        }
                        Verb::Focus | Verb::Select => {
                            let moved = s.menu.light(k) == Did::Lit;
                            if moved && then == Then::Nothing {
                                then = Then::Draw;
                            }
                        }
                        _ => {}
                    }
                }
                then
            });
            finish(hwnd, then.unwrap_or(Then::Nothing));
            return LRESULT(0);
        }
        // Losing the focus closes it, as does the screen changing under it.
        WM_ACTIVATE if (wparam.0 & 0xFFFF) as u32 == WA_INACTIVE => {
            finish(hwnd, Then::Close);
            return LRESULT(0);
        }
        // The capture it took for want of the foreground went to another
        // window: nothing tells it a click elsewhere any more.
        WM_CAPTURECHANGED => {
            let held = with_state(hwnd, |s| std::mem::take(&mut s.captured)).unwrap_or(false);
            if held && lparam.0 != hwnd.0 as isize {
                finish(hwnd, Then::Close);
            }
            return LRESULT(0);
        }
        WM_DISPLAYCHANGE => {
            finish(hwnd, Then::Close);
            return LRESULT(0);
        }
        WM_DESTROY => {
            with_state(hwnd, |s| s.host.shutdown());
            uia::disconnect_window(hwnd);
        }
        WM_NCDESTROY => {
            // SAFETY: the user data is cleared first, so the box is freed
            // once, at the window's last message, and nothing reads it
            // after.
            let raw = unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) } as *mut RefCell<State>;
            if !raw.is_null() {
                let state = unsafe { Box::from_raw(raw) };
                state.borrow().alive.set(false);
                drop(state);
            }
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::fluent::ALL_BUILT_IN;

    fn width(s: &str, _caption: bool) -> f32 {
        s.chars().count() as f32 * 7.0
    }

    const WORK: RECT = RECT {
        left: 100,
        top: 50,
        right: 1100,
        bottom: 850,
    };

    fn menu(drag_wheel_on: bool) -> Menu {
        let items = lines(drag_wheel_on);
        let rows = rows(&items, &width);
        let (_, laid) = plan(WORK, POINT { x: 500, y: 400 }, 1.0, &rows);
        Menu::new(items, laid)
    }

    fn centre(m: &Menu, k: usize) -> (f32, f32) {
        let r = m.layout.rows[k];
        (r.x + r.w / 2.0, r.y + r.h / 2.0)
    }

    #[test]
    fn the_menu_reads_open_drag_wheel_then_quit_as_it_always_did() {
        let l = lines(true);
        let words: Vec<_> = l
            .iter()
            .map(|l| match l {
                Line::Item {
                    command,
                    label,
                    access,
                    checked,
                } => Some((*command, label.as_str(), *access, *checked)),
                Line::Separator => None,
            })
            .collect();
        assert_eq!(
            words,
            [
                Some((TrayCommand::Open, "Open Kiln", Some('O'), None)),
                Some((
                    TrayCommand::ToggleDragWheel,
                    "Drag-wheel",
                    Some('D'),
                    Some(true)
                )),
                None,
                Some((TrayCommand::Quit, "Quit Kiln", Some('Q'), None)),
            ]
        );
        // Off, the drag-wheel's item is there, unchecked.
        assert!(matches!(
            lines(false)[1],
            Line::Item {
                checked: Some(false),
                ..
            }
        ));
    }

    #[test]
    fn an_access_key_is_taken_out_of_the_words() {
        assert_eq!(
            split_access("&Open Kiln"),
            ("Open Kiln".to_owned(), Some('O'))
        );
        assert_eq!(
            split_access("Save && close"),
            ("Save & close".to_owned(), None)
        );
        assert_eq!(split_access("Quit"), ("Quit".to_owned(), None));
    }

    #[test]
    fn the_keys_move_round_the_items_and_skip_the_separator() {
        let mut m = menu(true);
        assert_eq!(m.lit(), None);
        assert_eq!(m.key(VK_DOWN), Did::Lit);
        assert_eq!(m.lit(), Some(0));
        assert_eq!(m.key(VK_DOWN), Did::Lit);
        assert_eq!(m.key(VK_DOWN), Did::Lit);
        // Past the separator, on Quit.
        assert_eq!(m.lit(), Some(3));
        // Down wraps to the first, Up to the last.
        assert_eq!(m.key(VK_DOWN), Did::Lit);
        assert_eq!(m.lit(), Some(0));
        assert_eq!(m.key(VK_UP), Did::Lit);
        assert_eq!(m.lit(), Some(3));
        assert_eq!(m.key(VK_HOME), Did::Lit);
        assert_eq!(m.lit(), Some(0));
        assert_eq!(m.key(VK_END), Did::Lit);
        assert_eq!(m.key(VK_RETURN), Did::Chose(TrayCommand::Quit));
        assert_eq!(m.key(VK_HOME), Did::Lit);
        assert_eq!(m.key(VK_SPACE), Did::Chose(TrayCommand::Open));
    }

    #[test]
    fn enter_with_nothing_lit_does_nothing_and_escape_closes() {
        let mut m = menu(false);
        assert_eq!(m.key(VK_RETURN), Did::Nothing);
        assert_eq!(m.key(VK_ESCAPE), Did::Close);
        // Left, Right, Shift, Ctrl and Alt do nothing; another key closes
        // it as a Windows menu does.
        for k in [VK_LEFT, VK_RIGHT, VK_SHIFT, VK_CONTROL, VK_MENU] {
            assert_eq!(m.key(k), Did::Nothing);
        }
        assert_eq!(m.key(VIRTUAL_KEY(0x09)), Did::Close);
        assert_eq!(m.key(VK_APPS), Did::Close);
    }

    #[test]
    fn an_access_key_chooses_its_item_and_another_letter_does_nothing() {
        let mut m = menu(true);
        assert_eq!(
            m.key(VIRTUAL_KEY(b'O' as u16)),
            Did::Chose(TrayCommand::Open)
        );
        assert_eq!(
            m.key(VIRTUAL_KEY(b'D' as u16)),
            Did::Chose(TrayCommand::ToggleDragWheel)
        );
        assert_eq!(
            m.key(VIRTUAL_KEY(b'Q' as u16)),
            Did::Chose(TrayCommand::Quit)
        );
        assert_eq!(m.key(VIRTUAL_KEY(b'Z' as u16)), Did::Nothing);
    }

    #[test]
    fn the_pointer_lights_and_chooses_an_item_and_a_click_off_the_menu_closes_it() {
        let mut m = menu(true);
        let (x, y) = centre(&m, 1);
        assert_eq!(m.hover(x, y), Did::Lit);
        assert_eq!(m.lit(), Some(1));
        assert_eq!(m.hover(x + 1.0, y), Did::Nothing);
        assert_eq!(m.release(x, y), Did::Chose(TrayCommand::ToggleDragWheel));
        // A separator is no item.
        let (sx, sy) = centre(&m, 2);
        assert_eq!(m.item_at(sx, sy), None);
        assert_eq!(m.release(sx, sy), Did::Nothing);
        assert_eq!(m.hover(sx, sy), Did::Lit);
        assert_eq!(m.lit(), None);
        // Off the menu, in its shadow: a press closes it.
        assert_eq!(m.press(2.0, 2.0), Did::Close);
        assert_eq!(m.release(2.0, 2.0), Did::Close);
        assert_eq!(m.press(x, y), Did::Nothing);
    }

    #[test]
    fn a_menu_without_the_foreground_holds_the_mouse_so_a_click_elsewhere_closes_it() {
        assert!(needs_capture(false));
        assert!(!needs_capture(true));
        // With the capture a click anywhere on the screen arrives as a
        // press in the menu's own coordinates, off the menu and often
        // negative: it closes the menu.
        let m = menu(true);
        let (x, y) = point_of(LPARAM(
            ((-40i16 as u16 as isize) << 16) | (-300i16 as u16 as isize),
        ));
        assert_eq!((x, y), (-300, -40));
        assert_eq!(m.press(x as f32, y as f32), Did::Close);
        let (x, y) = centre(&m, 1);
        assert_eq!(m.press(x, y), Did::Nothing);
    }

    #[test]
    fn the_menu_opens_down_and_right_of_the_click_inside_the_work_area() {
        let rows = rows(&lines(true), &width);
        let (win, laid) = plan(WORK, POINT { x: 500, y: 400 }, 1.0, &rows);
        // The menu starts at the click; its window has room for the shadow.
        assert_eq!(
            (win.left, win.top),
            (500 - SHADOW as i32, 400 - SHADOW as i32)
        );
        let r = laid.rect;
        assert_eq!((r.x, r.y), (SHADOW, SHADOW));
        assert_eq!(win.right - win.left, (r.w + 2.0 * SHADOW).round() as i32);
        assert_eq!(win.bottom - win.top, (r.h + 2.0 * SHADOW).round() as i32);
        assert_eq!(laid.rows.len(), 4);
    }

    #[test]
    fn a_click_in_a_taskbar_at_the_bottom_opens_the_menu_upward_in_the_work_area() {
        let rows = rows(&lines(true), &width);
        // Below the work area, in the taskbar, at the right.
        let at = POINT { x: 1090, y: 860 };
        let (win, laid) = plan(WORK, at, 1.0, &rows);
        let menu_right = win.left as f32 + SHADOW + laid.rect.w;
        let menu_bottom = win.top as f32 + SHADOW + laid.rect.h;
        assert!(menu_right <= WORK.right as f32, "{menu_right}");
        assert!(menu_bottom <= WORK.bottom as f32, "{menu_bottom}");
        assert!(win.left as f32 + SHADOW >= WORK.left as f32);
        assert!(win.top as f32 + SHADOW >= WORK.top as f32);
    }

    #[test]
    fn the_menu_scales_with_the_screen_and_the_text() {
        let rows = rows(&lines(true), &width);
        let at = POINT { x: 500, y: 400 };
        let (one, laid_one) = plan(WORK, at, 1.0, &rows);
        let (two, laid_two) = plan(WORK, at, 2.0, &rows);
        // The same layout, in units, twice as many pixels.
        assert_eq!(laid_one, laid_two);
        assert_eq!(two.right - two.left, 2 * (one.right - one.left));
        assert_eq!(two.bottom - two.top, 2 * (one.bottom - one.top));
    }

    #[test]
    fn a_checked_item_has_a_column_for_its_check_mark() {
        let on = rows(&lines(true), &width);
        let off = rows(&lines(false), &width);
        assert!(on
            .iter()
            .all(|r| !matches!(r, MenuRow::Item { glyph: false, .. })));
        assert!(off
            .iter()
            .all(|r| !matches!(r, MenuRow::Item { glyph: true, .. })));
        let m = menu(true);
        let drawn = m.drawn();
        assert!(matches!(
            drawn[1],
            MenuLine::Item {
                glyph: Some(glyph::CHECKMARK),
                ..
            }
        ));
        assert!(matches!(drawn[0], MenuLine::Item { glyph: None, .. }));
        assert!(matches!(drawn[2], MenuLine::Separator));
    }

    #[test]
    fn the_tree_is_a_menu_of_menu_items_with_the_drag_wheel_a_toggle() {
        let mut m = menu(true);
        m.light(1);
        let tree = m.tree();
        let menu = tree.find(&Id::TrayMenu).expect("a menu");
        assert_eq!(menu.role, a11y::Role::Menu);
        // The separator is left out; each item keeps its line's number.
        let names: Vec<_> = menu
            .children
            .iter()
            .map(|n| (n.id.clone(), n.name.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                (Id::TrayMenuItem(0), "Open Kiln"),
                (Id::TrayMenuItem(1), "Drag-wheel"),
                (Id::TrayMenuItem(3), "Quit Kiln"),
            ]
        );
        assert!(menu.children.iter().all(|n| n.role == a11y::Role::MenuItem));
        let drag = tree.find(&Id::TrayMenuItem(1)).unwrap();
        assert_eq!(drag.states.toggled, Some(true));
        assert!(drag.states.focused);
        assert!(drag.patterns().toggle && !drag.patterns().invoke);
        let open = tree.find(&Id::TrayMenuItem(0)).unwrap();
        assert!(open.patterns().invoke);
        assert_eq!(open.states.toggled, None);
        assert_eq!(
            tree.focused().map(|n| n.id.clone()),
            Some(Id::TrayMenuItem(1))
        );
    }

    #[test]
    fn every_theme_has_room_for_the_menu_shadow() {
        for p in ALL_BUILT_IN {
            for s in p.flyout_shadow {
                assert!(s.y + s.blur / 2.0 <= SHADOW, "{}", p.name);
            }
        }
    }
}
