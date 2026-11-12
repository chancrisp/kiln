//! What Kiln does when its window code panics.
//!
//! Windows calls the window procedures and the drag-wheel's hook
//! procedures through `extern "system"` functions, and a panic that reaches
//! one of those ends the process at once, with no word to anyone. So each
//! runs its work in a [`Guard`], which catches the panic. The panic hook
//! (`diag`) has written its crash file by then.
//!
//! - **The windows**, the main one and the floating wheel, share
//!   [`WINDOW`]. Once it has tripped, every window procedure hands its
//!   messages straight to `DefWindowProcW` and never touches `App` again,
//!   so there is never a second `&mut App`, even while the message box
//!   pumps messages. [`close_after_panic`] says Kiln has to close, then
//!   ends it without dropping `App` and without discarding the holding
//!   folder, so what it holds is there at the next start. The settings
//!   saved before the panic are written first ([`flush_settings_with`]).
//! - **The hooks** have [`HOOKS`]. Once it has tripped, they only hand
//!   each event on: the drag-wheel stops, and Kiln keeps running.

use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use windows::core::{w, HSTRING};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::ValidateRect;
use windows::Win32::UI::WindowsAndMessaging::{
    IsWindow, MessageBoxW, PeekMessageW, MB_APPLMODAL, MB_ICONERROR, MB_OK, MB_SETFOREGROUND,
    MB_TASKMODAL, MB_TOPMOST, MESSAGEBOX_STYLE, MSG, PM_REMOVE, WM_APP, WM_PAINT, WM_QUIT,
};

use crate::settings;

/// Runs work so that a panic in it is caught, and runs nothing once one
/// has been.
#[derive(Default)]
pub struct Guard {
    tripped: AtomicBool,
}

impl Guard {
    pub const fn new() -> Self {
        Guard {
            tripped: AtomicBool::new(false),
        }
    }

    /// Whether a panic has reached [`run`](Self::run), or one is unwinding
    /// on this thread on its way there. While it unwinds, what it drops
    /// can call a window procedure again, and that call must find the
    /// guard tripped too.
    pub fn tripped(&self) -> bool {
        self.tripped.load(Ordering::Relaxed) || std::thread::panicking()
    }

    /// `f`'s answer, or `None` when it panics, which trips the guard. Once
    /// the guard has tripped, `f` isn't run and the answer is `None`.
    pub fn run<R>(&self, f: impl FnOnce() -> R) -> Option<R> {
        if self.tripped() {
            return None;
        }
        // Whatever `f` left half done is never used again: from here the
        // guard runs nothing.
        match panic::catch_unwind(AssertUnwindSafe(f)) {
            Ok(answer) => Some(answer),
            Err(payload) => {
                self.tripped.store(true, Ordering::Relaxed);
                // Dropping the payload runs its own code, which could panic
                // again with no guard to catch it. It is let go instead.
                std::mem::forget(payload);
                None
            }
        }
    }
}

/// The main window's and the floating wheel's. Both run on the UI thread,
/// and a panic in either closes Kiln.
pub static WINDOW: Guard = Guard::new();

/// The drag-wheel's hook procedures', on the hooks' own thread. A panic
/// there stops the drag-wheel only.
pub static HOOKS: Guard = Guard::new();

/// What writes the settings saved so far, once the window has a store.
static SETTINGS: OnceLock<settings::Flusher> = OnceLock::new();

/// How long a crash waits for the settings to be written.
const SETTINGS_STOP: Duration = Duration::from_secs(2);

/// Have [`close_after_panic`] write what `flusher`'s store has saved
/// before Kiln ends. The first store given is the one used.
pub(super) fn flush_settings_with(flusher: settings::Flusher) {
    let _ = SETTINGS.set(flusher);
}

/// Say Kiln has to close, over `owner` while it is still a window, and end
/// Kiln once the message is closed. Nothing is dropped on the way out: not
/// `App`, and not the holding folder, so the results in it stay for the
/// next start. Child processes end with Kiln's job object.
pub fn close_after_panic(owner: HWND) -> ! {
    let logs = settings::logs_dir().unwrap_or_else(|| PathBuf::from(r"%LOCALAPPDATA%\Kiln\logs"));
    let text = HSTRING::from(crash_text(&logs));
    // The settings writer is a thread of its own, untouched by the panic:
    // what was saved is written before the message waits on anyone.
    if let Some(f) = SETTINGS.get() {
        let _ = f.flush_timeout(SETTINGS_STOP);
    }
    let owner = unsafe { IsWindow(Some(owner)) }.as_bool().then_some(owner);
    // With no owner, the message turns off all of this thread's windows
    // itself. Topmost, it shows above the drag-wheel when that is open.
    let modal = if owner.is_some() {
        MB_APPLMODAL
    } else {
        MB_TASKMODAL
    };
    // The test placement switch never takes the foreground.
    let foreground = if super::placement::quiet_on() {
        MESSAGEBOX_STYLE(0)
    } else {
        MB_SETFOREGROUND
    };
    unsafe {
        take_quit();
        MessageBoxW(
            owner,
            &text,
            w!("Kiln"),
            MB_OK | MB_ICONERROR | foreground | MB_TOPMOST | modal,
        );
    }
    // A graphics device still held as the process ends fails its exit, as
    // `win::run` knows.
    super::render::gpu::forget_shared_device();
    std::process::exit(1)
}

/// Most queued messages [`take_quit`] looks through.
const QUEUE_LIMIT: usize = 10_000;

/// Take a quit the window code asked for before it panicked, as
/// `WM_DESTROY` does, so the message's own loop doesn't find it and close
/// at once. Windows gives out `WM_QUIT` only once the posted messages
/// ahead of it are gone, so those go too, unanswered: after a panic no
/// message is acted on. A paint stays queued until its window is
/// validated, so it is validated, unpainted.
unsafe fn take_quit() {
    let mut msg = MSG::default();
    for _ in 0..QUEUE_LIMIT {
        if !PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            return;
        }
        match msg.message {
            WM_QUIT => return,
            WM_PAINT => {
                let _ = ValidateRect(Some(msg.hwnd), None);
            }
            _ => {}
        }
    }
}

/// The crash message, naming the folder the report is in.
fn crash_text(logs: &Path) -> String {
    format!(
        "Kiln ran into a problem and has to close. Your converted files are \
         kept, and Kiln saves them the next time it starts. A report was \
         saved in {}.",
        logs.display()
    )
}

/// A message that makes the window code panic, for tests. Only debug
/// builds answer it, and only when Kiln started with `KILN_TEST_PANIC=1`.
/// Its `wparam` is read as a number only, and its `lparam` never.
pub const WM_KILN_TEST_PANIC: u32 = WM_APP + 0x3F0;

/// [`WM_KILN_TEST_PANIC`]'s wparam for a panic after the message loop was
/// asked to end, as a panic in `WM_DESTROY` or `WM_NCDESTROY` would be.
pub const TEST_PANIC_AFTER_QUIT: usize = 1;

/// Whether Kiln started with `KILN_TEST_PANIC=1`.
#[cfg(debug_assertions)]
static TEST_PANIC: AtomicBool = AtomicBool::new(false);

/// Debug builds: read `KILN_TEST_PANIC` once, at start.
#[cfg(debug_assertions)]
pub fn arm_test_panic() {
    let on = std::env::var_os("KILN_TEST_PANIC").is_some_and(|v| v == "1");
    TEST_PANIC.store(on, Ordering::Relaxed);
}

/// Debug builds: whether the window answers [`WM_KILN_TEST_PANIC`].
#[cfg(debug_assertions)]
pub fn test_panic_armed() -> bool {
    TEST_PANIC.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn a_panic_is_caught_and_trips_the_guard() {
        let guard = Guard::new();
        assert!(!guard.tripped());
        assert_eq!(guard.run(|| panic!("the wheel came off")), None::<()>);
        assert!(guard.tripped());
    }

    #[test]
    fn a_tripped_guard_runs_nothing() {
        let guard = Guard::new();
        guard.run(|| panic!("the wheel came off"));
        let flag = Cell::new(false);
        assert_eq!(guard.run(|| flag.set(true)), None);
        assert!(!flag.get(), "nothing ran");
    }

    #[test]
    fn an_untripped_guard_returns_the_answer() {
        let guard = Guard::new();
        assert_eq!(guard.run(|| 7), Some(7));
        assert!(!guard.tripped());
    }

    #[test]
    fn a_guard_runs_nothing_while_its_panic_unwinds() {
        // As a window procedure called again by something dropped on the
        // way out of a panic, before the guard has caught it.
        struct Again<'a> {
            guard: &'a Guard,
            tripped: &'a Cell<bool>,
            ran: &'a Cell<bool>,
        }
        impl Drop for Again<'_> {
            fn drop(&mut self) {
                self.tripped.set(self.guard.tripped());
                self.guard.run(|| self.ran.set(true));
            }
        }
        let guard = Guard::new();
        let (tripped, ran) = (Cell::new(false), Cell::new(false));
        let caught = guard.run(|| {
            let _again = Again {
                guard: &guard,
                tripped: &tripped,
                ran: &ran,
            };
            panic!("the wheel came off");
        });
        assert_eq!(caught, None::<()>);
        assert!(tripped.get(), "tripped already, as it unwinds");
        assert!(!ran.get(), "and ran nothing");
    }

    #[test]
    fn the_crash_message_names_the_logs_folder() {
        let logs = Path::new(r"C:\Users\Ana María\AppData\Local\Kiln\logs");
        assert_eq!(
            crash_text(logs),
            "Kiln ran into a problem and has to close. Your converted files \
             are kept, and Kiln saves them the next time it starts. A report \
             was saved in C:\\Users\\Ana María\\AppData\\Local\\Kiln\\logs."
        );
    }
}
