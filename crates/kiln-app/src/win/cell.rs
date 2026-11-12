//! The window's `App`, in a cell.
//!
//! Every `&mut App` is made by [`AppCell::with`], which takes a
//! `RefCell` borrow with `try_borrow_mut`: while one is live a second
//! can't be made, so a message that comes in meanwhile (from a call that
//! sends straight back to the window) never touches `App`. It is answered
//! without it (`nested` in `win`), and the few flags it needs to leave
//! behind live here, outside the borrow.
//!
//! The window reaches its cell through `GWLP_USERDATA`, set, read and
//! cleared only here.

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::UI::Accessibility::IRawElementProviderSimple;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongPtrW, PostMessageW, SetWindowLongPtrW, GWLP_USERDATA,
};

use super::held::NotesFlusher;
use super::{log, App};
use crate::settings::Flusher;

/// How long the end of a session waits for settings to be written, and,
/// at the same time, for the holding folders' notes.
pub(super) const SETTINGS_STOP: Duration = Duration::from_secs(5);

pub(super) struct AppCell {
    hwnd: HWND,
    app: RefCell<App>,
    /// A paint came while `App` was borrowed: Windows' default answer
    /// validated the window, so it is painted again once the borrow ends.
    repaint_after: Cell<bool>,
    /// How many effects are running now with `App` let go, the outermost
    /// first: a runner entered from inside one runs only what it may
    /// (`effects::run_effects`).
    depth: Cell<u32>,
    /// The session ended while `App` was borrowed: Windows' default answer
    /// can't keep the holding folder, so the window does once it can.
    ending: Cell<bool>,

    /// The messages the log was told came in while `App` was borrowed,
    /// so each says so once a borrow.
    logged: RefCell<Vec<u32>>,
    /// Posted messages that came while `App` was borrowed, each once, to
    /// be posted again when the borrow ends: posted again at once, a loop
    /// that delivers them while `App` is borrowed would only spin.
    later: RefCell<Vec<(u32, WPARAM, LPARAM)>>,
    /// The UI Automation root, from the start until the window goes, to
    /// answer a client that asks while `App` is borrowed. Its provider
    /// reads the tree's own shared copy, never `App`.
    root: RefCell<Option<IRawElementProviderSimple>>,
    /// A client was given the root while `App` was borrowed: the next
    /// borrow does what the first client's root does (`App::uia_root`).
    client_asked: Cell<bool>,
    /// Whether Settings was open when `App` was last let go: the title
    /// bar has Back then, which a hit-test answers without `App`.
    settings_open: Cell<bool>,
    /// Writes the settings saved so far, while `App` is borrowed too: at
    /// the end of a session, which can come during a dialog.
    flusher: Flusher,
    /// Waits for the holding folders' notes asked so far, as `flusher`
    /// does for the settings.
    notes: NotesFlusher,
    /// Debug builds: how many messages found `App` borrowed. In-process
    /// tests check it stays at zero.
    #[cfg(debug_assertions)]
    nested: Cell<u32>,
}

impl AppCell {
    pub(super) fn new(hwnd: HWND, app: App) -> Self {
        // A client that asks before `App` has answered one still gets the
        // tree.
        let root = app.uia.provider();
        let flusher = app.prefs.store.flusher();
        let notes = app.held_notes.flusher();
        AppCell {
            hwnd,
            app: RefCell::new(app),
            repaint_after: Cell::new(false),
            depth: Cell::new(0),
            ending: Cell::new(false),

            logged: RefCell::new(Vec::new()),
            later: RefCell::new(Vec::new()),
            root: RefCell::new(Some(root)),
            client_asked: Cell::new(false),
            settings_open: Cell::new(false),
            flusher,
            notes,
            #[cfg(debug_assertions)]
            nested: Cell::new(0),
        }
    }

    /// Run `f` on `App`. `None`, with `f` not run, while `App` is already
    /// borrowed. Once the borrow ends, a paint that came meanwhile is
    /// asked for again, and posted messages are posted again.
    pub(super) fn with<R>(&self, f: impl FnOnce(&mut App) -> R) -> Option<R> {
        let answer = {
            let mut app = self.app.try_borrow_mut().ok()?;
            f(&mut app)
        };
        self.logged.borrow_mut().clear();
        if self.repaint_after.take() {
            // SAFETY: plain call on this thread's own window; it sends
            // nothing, and a destroyed window makes it fail harmlessly.
            unsafe {
                let _ = InvalidateRect(Some(self.hwnd), None, false);
            }
        }
        for (msg, wparam, lparam) in self.later.take() {
            // SAFETY: plain call on this thread's own window, with the
            // message's own arguments; nothing they point at is read.
            unsafe {
                let _ = PostMessageW(Some(self.hwnd), msg, wparam, lparam);
            }
        }
        Some(answer)
    }

    pub(super) fn repaint_after(&self) {
        self.repaint_after.set(true);
    }

    pub(super) fn depth(&self) -> u32 {
        self.depth.get()
    }

    /// Count an effect running with `App` let go, until the guard drops.
    pub(super) fn running(&self) -> Running<'_> {
        self.depth.set(self.depth.get() + 1);
        Running(self)
    }

    pub(super) fn session_ended(&self) {
        self.ending.set(true);
    }

    /// Whether the session ended while `App` was borrowed, once: the
    /// window acts on it now.
    pub(super) fn take_ending(&self) -> bool {
        self.ending.take()
    }

    /// Whether the session ended while `App` was borrowed and the window
    /// never got to act on it: what's held is kept.
    pub(super) fn ended(&self) -> bool {
        self.ending.get()
    }

    /// Note that `msg` found `App` borrowed. True when the log should be
    /// told: the first time each borrow for each message.
    pub(super) fn note_nested(&self, msg: u32) -> bool {
        #[cfg(debug_assertions)]
        self.nested.set(self.nested.get() + 1);
        let mut logged = self.logged.borrow_mut();
        let first = !logged.contains(&msg);
        if first {
            logged.push(msg);
        }
        first
    }

    /// Post `msg` again once the borrow ends; the same message twice is
    /// posted once.
    pub(super) fn post_later(&self, msg: u32, wparam: WPARAM, lparam: LPARAM) {
        let mut later = self.later.borrow_mut();
        if !later.contains(&(msg, wparam, lparam)) {
            later.push((msg, wparam, lparam));
        }
    }

    /// Write the settings saved so far, and the holding folders' notes
    /// asked so far (where each held result came from), waiting at most
    /// `SETTINGS_STOP` for both together: the session is ending, and
    /// Windows ends Kiln once its message is answered. Works whether or
    /// not `App` is borrowed.
    pub(super) fn flush_at_session_end(&self) {
        let until = Instant::now() + SETTINGS_STOP;
        // Marked first, so the notes worker writes while the settings do.
        let notes = self.notes.mark();
        // Waited too long (`None`): the writer logged it and writes on.
        if let Some(Err(e)) = self.flusher.flush_timeout(SETTINGS_STOP) {
            log(&e);
        }
        notes.wait(until);
    }

    /// Keep `root`, the UI Automation root just given a client.
    pub(super) fn keep_root(&self, root: &IRawElementProviderSimple) {
        *self.root.borrow_mut() = Some(root.clone());
    }

    /// The UI Automation root, until the window goes.
    pub(super) fn root(&self) -> Option<IRawElementProviderSimple> {
        self.root.borrow().clone()
    }

    /// A client was given the root while `App` was borrowed.
    pub(super) fn client_asked(&self) {
        self.client_asked.set(true);
    }

    /// Whether a client was given the root while `App` was borrowed,
    /// once.
    pub(super) fn take_client_asked(&self) -> bool {
        self.client_asked.take()
    }

    /// Whether Settings is open, as `App` was last let go.
    pub(super) fn set_settings_open(&self, open: bool) {
        self.settings_open.set(open);
    }

    /// Whether Settings was open when `App` was last let go.
    pub(super) fn settings_open(&self) -> bool {
        self.settings_open.get()
    }

    /// The window has gone: no client is given its root again.
    pub(super) fn drop_root(&self) {
        self.root.borrow_mut().take();
    }

    /// Debug builds: how many messages found `App` borrowed so far.
    #[cfg(all(test, debug_assertions))]
    pub(super) fn nested(&self) -> u32 {
        self.nested.get()
    }

    pub(super) fn into_app(self) -> App {
        self.app.into_inner()
    }

    /// The cell `install` gave `hwnd`, while it has one. Only for the
    /// length of one call of the window's procedure or the drop target's.
    pub(super) fn of<'a>(hwnd: HWND) -> Option<&'a AppCell> {
        // SAFETY: reading a window's user data has no preconditions.
        let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const AppCell;
        // SAFETY: when set, the user data is the box `install` leaked, and
        // it is read only on this, the window's, thread. `Installed::finish`
        // clears it before it frees the box, and does that only once the
        // message loop has ended, when no window procedure is running that
        // could still hold the reference.
        unsafe { ptr.as_ref() }
    }

    /// The window has gone (`WM_NCDESTROY`): it forgets its cell, which
    /// lives on until the message loop ends.
    pub(super) fn forget(hwnd: HWND) {
        // SAFETY: plain call on this thread's own window. Nothing is freed
        // here.
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        }
    }
}

/// Counts one effect running with `App` let go (`AppCell::running`).
pub(super) struct Running<'a>(&'a AppCell);

impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.0.depth.set(self.0.depth.get().saturating_sub(1));
    }
}

/// The window's cell, given to it by [`install`]: the only owner of the
/// box, which [`Installed::finish`] takes back.
pub(super) struct Installed {
    hwnd: HWND,
    cell: *mut AppCell,
}

/// Give `hwnd` its cell. From here every message reaches `App` through it.
pub(super) fn install(hwnd: HWND, cell: AppCell) -> Installed {
    let cell = Box::into_raw(Box::new(cell));
    // SAFETY: plain call on this thread's own window. The box stays alive
    // until `Installed::finish`, which clears the user data first.
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, cell as isize);
    }
    Installed { hwnd, cell }
}

impl Installed {
    /// The cell, for the set-up code beside the message loop.
    pub(super) fn cell(&self) -> &AppCell {
        // SAFETY: the box lives until `finish` takes `self`.
        unsafe { &*self.cell }
    }

    /// Take the cell back once the message loop has ended, and with it
    /// `App`. The window no longer reaches it from here.
    pub(super) fn finish(self) -> AppCell {
        if AppCell::of(self.hwnd).is_some_and(|c| std::ptr::eq(c, self.cell)) {
            AppCell::forget(self.hwnd);
        }
        // SAFETY: `install` leaked this box and nothing else frees it. The
        // message loop has ended, so no window procedure holds a reference
        // from `of`, and the user data no longer points at it, so none is
        // made later.
        *unsafe { Box::from_raw(self.cell) }
    }
}
