//! The drag-wheel's input hooks.
//!
//! `WH_MOUSE_LL` and `WH_KEYBOARD_LL` run on a thread of their own with a
//! message loop, since Windows calls a low-level hook on the thread that
//! installed it, through that thread's message queue. The thread owns the
//! `Watch`. Each callback copies its event into the watch, posts any verdict
//! to the window that asked for them, and hands the event on with
//! `CallNextHookEx`. A callback allocates nothing, waits on nothing, draws
//! nothing and writes nothing, and no input is ever swallowed.
//!
//! Injected input is fed like any other, since accessibility tools inject.
//!
//! **A panic** in a callback is caught (`crash::HOOKS`), since one that
//! reached Windows would end Kiln. The drag-wheel stops for the rest of the
//! run, the log says so, every event after it is only handed on, and Kiln
//! keeps running.
//!
//! **A verdict is one posted message.** `wparam` says which: `Close`,
//! `Flip`, or `Open` on the Convert or the Tools ring. For an `Open`,
//! `lparam` holds the screen point, x in its low 32 bits and y in its high
//! 32 bits, each a whole `i32`. `LPARAM` is 64 bits wide on every target
//! Kiln builds for, so every coordinate, including the negative ones of a
//! monitor left of or above the primary, comes back exactly. `decode`
//! undoes `encode`.

use std::cell::RefCell;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use windows::core::HRESULT;
use windows::Win32::Foundation::{E_FAIL, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    GetCurrentThread, GetCurrentThreadId, SetThreadPriority, THREAD_PRIORITY_HIGHEST,
};
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, GetSystemMetrics, PeekMessageW, PostMessageW, PostThreadMessageW,
    SetWindowsHookExW, UnhookWindowsHookEx, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT,
    PM_NOREMOVE, SM_CXDRAG, SM_CYDRAG, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_APP, WM_KEYDOWN, WM_KEYUP,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_USER,
};

use crate::drag_watch::{
    Input, Verdict, Watch, VK_CONTROL, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_MENU, VK_RCONTROL,
    VK_RMENU, VK_RSHIFT, VK_SHIFT,
};

use super::crash::HOOKS;

/// The message a verdict can travel in, for callers that have no id of
/// their own: `Hooks::start(hwnd, WM_KILN_HOOK, ..)`, read with `decode`.
/// On the hook thread's own queue, the same id is `Hooks::wheel_closed`.
/// The verdict is two plain numbers (`encode`): no `lparam` is
/// dereferenced.
pub const WM_KILN_HOOK: u32 = WM_APP + 15;

/// What the setting shows when Windows refuses a hook.
pub const HOOK_REFUSED: &str = crate::settings::DRAG_WHEEL_REFUSED;

/// Windows refused a hook, or the hook thread couldn't start. It reads as
/// `HOOK_REFUSED`; `code` is Windows' reason, for the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookError {
    pub code: HRESULT,
}

impl std::fmt::Display for HookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(HOOK_REFUSED)
    }
}

impl std::error::Error for HookError {}

/// The low-level hooks, running on their thread until `stop` or drop.
pub struct Hooks {
    thread: Option<JoinHandle<()>>,
    thread_id: u32,
}

impl Hooks {
    /// Installs both hooks on a new thread. Verdicts arrive at `post_to` as
    /// `msg`, encoded as `encode` does. Shift is the key tapped to convert
    /// and Ctrl the one that opens tools.
    ///
    /// Returns once both hooks are in, or once one has failed; then neither
    /// stays installed and the thread has ended.
    pub fn start(post_to: HWND, msg: u32) -> Result<Hooks, HookError> {
        debug_assert!(!post_to.0.is_null(), "the hooks need a window to post to");
        // HWND is a pointer and not Send; the thread rebuilds it.
        let to = post_to.0 as isize;
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("kiln-drag-hooks".into())
            .spawn(move || {
                let post_to = HWND(to as *mut core::ffi::c_void);
                run(post_to, msg, ready_tx);
            })
            .map_err(|_| HookError { code: E_FAIL })?;
        match ready_rx.recv() {
            Ok(Ok(thread_id)) => Ok(Hooks {
                thread: Some(thread),
                thread_id,
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            // The thread ended without a word: it panicked.
            Err(_) => {
                let _ = thread.join();
                Err(HookError { code: E_FAIL })
            }
        }
    }

    /// The window closed the wheel (a drop, the drag leaving, no files).
    /// The watch goes back to the drag, so the next tap opens it again.
    pub fn wheel_closed(&self) {
        let _ = unsafe { PostThreadMessageW(self.thread_id, WM_KILN_HOOK, WPARAM(0), LPARAM(0)) };
    }

    /// Removes both hooks and ends their thread.
    pub fn stop(mut self) {
        self.end();
    }

    fn end(&mut self) {
        if let Some(thread) = self.thread.take() {
            let quit = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
            // Without the quit the thread would never end, and a join would
            // hang the UI thread. Let it go instead.
            if quit.is_ok() {
                let _ = thread.join();
            }
        }
    }
}

impl Drop for Hooks {
    fn drop(&mut self) {
        self.end();
    }
}

/// `wparam` values. Anything else decodes as `Close`, the safe answer.
const CLOSE: usize = 0;
const FLIP: usize = 1;
const OPEN_CONVERT: usize = 2;
const OPEN_TOOLS: usize = 3;

// Two whole i32 coordinates fill a 64-bit LPARAM.
const _: () = assert!(
    size_of::<isize>() == 8,
    "the hooks pack two i32 coordinates into a 64-bit LPARAM"
);

/// A verdict as the message's `wparam` and `lparam`.
pub fn encode(v: Verdict) -> (WPARAM, LPARAM) {
    match v {
        Verdict::Close => (WPARAM(CLOSE), LPARAM(0)),
        Verdict::Flip => (WPARAM(FLIP), LPARAM(0)),
        Verdict::Open { x, y, tools } => (
            WPARAM(if tools { OPEN_TOOLS } else { OPEN_CONVERT }),
            LPARAM(pack(x, y)),
        ),
    }
}

/// The verdict a posted message carries.
pub fn decode(wparam: WPARAM, lparam: LPARAM) -> Verdict {
    match wparam.0 {
        FLIP => Verdict::Flip,
        OPEN_CONVERT | OPEN_TOOLS => {
            let (x, y) = unpack(lparam.0);
            Verdict::Open {
                x,
                y,
                tools: wparam.0 == OPEN_TOOLS,
            }
        }
        _ => Verdict::Close,
    }
}

fn pack(x: i32, y: i32) -> isize {
    let bits = u64::from(x as u32) | (u64::from(y as u32) << 32);
    bits as i64 as isize
}

fn unpack(l: isize) -> (i32, i32) {
    let bits = l as i64 as u64;
    (bits as u32 as i32, (bits >> 32) as u32 as i32)
}

/// A mouse hook event as the watch's input: the left button and moves.
///
/// The left button here is the primary one, the button that drags: with
/// the buttons swapped in Windows' settings the low-level hook already
/// hears the swapped message (Windows swaps the button before it posts
/// the input event the hook is called for), so the physical right button
/// arrives as `WM_LBUTTONDOWN` and nothing is mapped here. Only
/// `GetAsyncKeyState` reports physical buttons, and the watch reads no
/// button through it.
fn mouse_input(msg: u32, x: i32, y: i32, t: u32) -> Option<Input> {
    match msg {
        WM_LBUTTONDOWN => Some(Input::ButtonDown { x, y, t }),
        WM_LBUTTONUP => Some(Input::ButtonUp { t }),
        WM_MOUSEMOVE => Some(Input::Move { x, y, t }),
        _ => None,
    }
}

/// A keyboard hook event as the watch's input. Alt and F10 arrive as the
/// SYS forms.
fn key_input(msg: u32, vk: u32, t: u32) -> Option<Input> {
    let vk = u16::try_from(vk).ok()?;
    match msg {
        WM_KEYDOWN | WM_SYSKEYDOWN => Some(Input::KeyDown { vk, t }),
        WM_KEYUP | WM_SYSKEYUP => Some(Input::KeyUp { vk, t }),
        _ => None,
    }
}

/// The modifiers as the keyboard hook reports them, side by side.
const SIDED: [u16; 6] = [
    VK_LSHIFT,
    VK_RSHIFT,
    VK_LCONTROL,
    VK_RCONTROL,
    VK_LMENU,
    VK_RMENU,
];

fn key_is_down(vk: u16) -> bool {
    (unsafe { GetAsyncKeyState(i32::from(vk)) } as u16) & 0x8000 != 0
}

/// The watch, and where its verdicts go. Only the hook thread touches it.
struct Live {
    watch: Watch,
    post_to: HWND,
    msg: u32,
}

thread_local! {
    static LIVE: RefCell<Option<Live>> = const { RefCell::new(None) };
}

/// Feeds one event to the watch and posts any verdict. O(1): a borrow, a
/// few key-state reads on a button press, and a post.
fn feed(input: Input) {
    LIVE.with(|live| {
        // A callback re-entered from inside this one: let it pass.
        let Ok(mut live) = live.try_borrow_mut() else {
            return;
        };
        let Some(live) = live.as_mut() else {
            return;
        };
        if let Input::ButtonDown { .. } = input {
            // A guard against a key-up the hook never saw.
            for vk in SIDED {
                if key_is_down(vk) {
                    live.watch.seed_held(vk);
                } else {
                    live.watch.seed_released(vk);
                }
            }
        }
        if let Some(v) = live.watch.feed(input) {
            let (wparam, lparam) = encode(v);
            let _ = unsafe { PostMessageW(Some(live.post_to), live.msg, wparam, lparam) };
        }
    });
}

/// Run a callback's work inside `crash::HOOKS`. A panic in it stops the
/// drag-wheel for the rest of the run, with a line in the log, and Kiln
/// keeps running; from then on the callbacks only hand events on.
fn guarded(work: impl FnOnce()) {
    if HOOKS.tripped() {
        return;
    }
    if HOOKS.run(work).is_none() {
        crate::diag::log("the drag-wheel stopped after a problem");
    }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        guarded(|| {
            // SAFETY: for HC_ACTION, lparam points at the event's
            // MSLLHOOKSTRUCT for the length of the call.
            let Some(e) = (unsafe { super::system::lparam_ref::<MSLLHOOKSTRUCT>(lparam) }) else {
                return;
            };
            if let Some(input) = mouse_input(wparam.0 as u32, e.pt.x, e.pt.y, e.time) {
                feed(input);
            }
        });
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn key_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        guarded(|| {
            // SAFETY: for HC_ACTION, lparam points at the event's
            // KBDLLHOOKSTRUCT for the length of the call.
            let Some(e) = (unsafe { super::system::lparam_ref::<KBDLLHOOKSTRUCT>(lparam) }) else {
                return;
            };
            if let Some(input) = key_input(wparam.0 as u32, e.vkCode, e.time) {
                feed(input);
            }
        });
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Installs both hooks, or neither.
fn install() -> Result<(HHOOK, HHOOK), HookError> {
    let refused = |e: windows::core::Error| HookError { code: e.code() };
    let module: HINSTANCE = unsafe { GetModuleHandleW(None) }.map_err(refused)?.into();
    let mouse = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), Some(module), 0) }
        .map_err(refused)?;
    match unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(key_proc), Some(module), 0) } {
        Ok(keys) => Ok((mouse, keys)),
        Err(e) => {
            let _ = unsafe { UnhookWindowsHookEx(mouse) };
            Err(refused(e))
        }
    }
}

/// The hook thread: install, say so, pump until `WM_QUIT`, uninstall.
fn run(post_to: HWND, msg: u32, ready: mpsc::SyncSender<Result<u32, HookError>>) {
    // A busy machine is less likely to time a slow hook out, which Windows
    // does by removing it without a word.
    let _ = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST) };

    // Make this thread's queue now, so a stop posted the moment `start`
    // returns is not lost.
    let mut m = MSG::default();
    let _ = unsafe { PeekMessageW(&mut m, None, WM_USER, WM_USER, PM_NOREMOVE) };

    let (cx, cy) = unsafe { (GetSystemMetrics(SM_CXDRAG), GetSystemMetrics(SM_CYDRAG)) };
    let mut watch = Watch::new(cx, cy);

    let (mouse, keys) = match install() {
        Ok(hooks) => hooks,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };

    // Keys already down are held, not pressed. The callbacks run only while
    // this thread pumps, so nothing is fed until the loop below.
    for vk in 0x08..=0xFE_u16 {
        // The sideless forms never come from the hook; their sides do.
        if !matches!(vk, VK_SHIFT | VK_CONTROL | VK_MENU) && key_is_down(vk) {
            watch.seed_held(vk);
        }
    }
    LIVE.with(|live| {
        *live.borrow_mut() = Some(Live {
            watch,
            post_to,
            msg,
        })
    });
    let _ = ready.send(Ok(unsafe { GetCurrentThreadId() }));

    loop {
        let got = unsafe { GetMessageW(&mut m, None, 0, 0) };
        // 0 is WM_QUIT; -1 is an error, and nothing more will come.
        if got.0 == 0 || got.0 == -1 {
            break;
        }
        // The thread has no windows, so only thread messages arrive.
        if m.hwnd.0.is_null() && m.message == WM_KILN_HOOK {
            LIVE.with(|live| {
                if let Some(live) = live.borrow_mut().as_mut() {
                    live.watch.wheel_closed();
                }
            });
        }
    }

    unsafe {
        let _ = UnhookWindowsHookEx(keys);
        let _ = UnhookWindowsHookEx(mouse);
    }
    LIVE.with(|live| live.borrow_mut().take());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drag_watch::VK_ESCAPE;

    #[test]
    fn decode_round_trips_every_verdict() {
        let mut every = vec![Verdict::Close, Verdict::Flip];
        let points = [
            (0, 0),
            (1920, 1080),
            // Monitors left of and above the primary.
            (-1920, -1080),
            (-1, 1),
            (2559, -1440),
            (-32768, 32767),
            (-40000, 70000),
            (i32::MIN, i32::MAX),
            (i32::MAX, i32::MIN),
            (-1, -1),
        ];
        for (x, y) in points {
            for tools in [false, true] {
                every.push(Verdict::Open { x, y, tools });
            }
        }
        for v in every {
            let (wparam, lparam) = encode(v);
            assert_eq!(decode(wparam, lparam), v, "{v:?}");
        }
    }

    #[test]
    fn each_verdict_has_its_own_wparam() {
        let open = |tools| Verdict::Open { x: 5, y: 7, tools };
        let ws: Vec<usize> = [Verdict::Close, Verdict::Flip, open(false), open(true)]
            .into_iter()
            .map(|v| encode(v).0 .0)
            .collect();
        assert_eq!(ws, [CLOSE, FLIP, OPEN_CONVERT, OPEN_TOOLS]);
        // Close and Flip carry no point.
        assert_eq!(encode(Verdict::Close).1 .0, 0);
        assert_eq!(encode(Verdict::Flip).1 .0, 0);
    }

    #[test]
    fn the_point_packs_x_low_and_y_high() {
        assert_eq!(pack(1, 2), 0x0000_0002_0000_0001);
        assert_eq!(pack(-1, 0), 0x0000_0000_FFFF_FFFF);
        assert_eq!(pack(0, -1) as u64, 0xFFFF_FFFF_0000_0000);
        assert_eq!(unpack(pack(-1920, 1080)), (-1920, 1080));
        assert_eq!(unpack(pack(1080, -1920)), (1080, -1920));
    }

    #[test]
    fn an_unknown_wparam_decodes_as_close() {
        assert_eq!(decode(WPARAM(4), LPARAM(0)), Verdict::Close);
        assert_eq!(decode(WPARAM(usize::MAX), LPARAM(-1)), Verdict::Close);
    }

    #[test]
    fn mouse_events_map_to_inputs() {
        assert_eq!(
            mouse_input(WM_LBUTTONDOWN, -5, 9, 100),
            Some(Input::ButtonDown {
                x: -5,
                y: 9,
                t: 100
            })
        );
        assert_eq!(
            mouse_input(WM_LBUTTONUP, -5, 9, 101),
            Some(Input::ButtonUp { t: 101 })
        );
        assert_eq!(
            mouse_input(WM_MOUSEMOVE, -2560, -10, 102),
            Some(Input::Move {
                x: -2560,
                y: -10,
                t: 102
            })
        );
        use windows::Win32::UI::WindowsAndMessaging::{
            WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_RBUTTONUP,
        };
        for other in [WM_RBUTTONDOWN, WM_RBUTTONUP, WM_MOUSEWHEEL] {
            assert_eq!(mouse_input(other, 0, 0, 0), None);
        }
    }

    #[test]
    fn key_events_map_to_inputs() {
        let shift = u32::from(VK_LSHIFT);
        assert_eq!(
            key_input(WM_KEYDOWN, shift, 7),
            Some(Input::KeyDown {
                vk: VK_LSHIFT,
                t: 7
            })
        );
        assert_eq!(
            key_input(WM_KEYUP, shift, 8),
            Some(Input::KeyUp {
                vk: VK_LSHIFT,
                t: 8
            })
        );
        // Alt arrives as the SYS forms.
        let alt = u32::from(VK_RMENU);
        assert_eq!(
            key_input(WM_SYSKEYDOWN, alt, 9),
            Some(Input::KeyDown { vk: VK_RMENU, t: 9 })
        );
        assert_eq!(
            key_input(WM_SYSKEYUP, alt, 10),
            Some(Input::KeyUp {
                vk: VK_RMENU,
                t: 10
            })
        );
        assert_eq!(
            key_input(WM_KEYDOWN, u32::from(VK_ESCAPE), 11),
            Some(Input::KeyDown {
                vk: VK_ESCAPE,
                t: 11
            })
        );
        use windows::Win32::UI::WindowsAndMessaging::WM_CHAR;
        assert_eq!(key_input(WM_CHAR, shift, 12), None);
        // Not a virtual key.
        assert_eq!(key_input(WM_KEYDOWN, 0x1_0000, 13), None);
    }

    #[test]
    fn a_refused_hook_reads_as_the_settings_copy() {
        let e = HookError { code: E_FAIL };
        assert_eq!(
            e.to_string(),
            "Windows didn't allow the drag-wheel to watch the mouse and keys."
        );
    }
}
