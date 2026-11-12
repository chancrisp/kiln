//! The drag-wheel in a running Kiln: the drag watch's verdicts open and
//! close the floating wheel, and the setting starts and stops the hooks.
//!
//! What a drop on the wheel does is tested in-process, in `win` (with the
//! wheel's own drop target, in `float_wheel.rs`): a drop from another
//! process needs a real drag. Here each test starts a Kiln of its own, with
//! a scratch APPDATA and LOCALAPPDATA, and drives it only by posting it
//! window messages and through UI Automation patterns. A verdict is posted
//! as the hooks would post it. Nothing here moves the pointer or sends
//! input, and no test starts while another Kiln is running.

#![cfg(windows)]

mod common;

use std::process::{Child, Command};
use std::sync::{Mutex, MutexGuard, Once};
use std::time::{Duration, Instant};

use kiln_app::drag_watch::Verdict;
use kiln_app::settings::{DRAG_WHEEL_LABEL, DRAG_WHEEL_NOTE};
use kiln_app::win::hooks::{encode, WM_KILN_HOOK};
use kiln_app::win::tray::{CMD_DRAG_WHEEL, CMD_QUIT};
use tempfile::TempDir;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows::Win32::System::Threading::{
    GetThreadDescription, OpenMutexW, OpenThread, SYNCHRONIZATION_SYNCHRONIZE,
    THREAD_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::*;

/// One test Kiln at a time.
static ONE_KILN: Mutex<()> = Mutex::new(());

/// A debug build can take a while to show its first frame.
const START: Duration = Duration::from_secs(60);
/// How long a posted message may take to show its effect.
const SETTLE: Duration = Duration::from_secs(10);
/// The drag-wheel on, past its one-time turn-off. A new install has it
/// off.
const ON: &str = "drag_wheel = true\ndrag_wheel_reset = true\n";
/// How long a test Kiln gets to quit by itself before it is ended.
const QUIT: Duration = Duration::from_secs(3);
/// The name the hooks' thread runs under.
const HOOK_THREAD: &str = "kiln-drag-hooks";

fn wait_until(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    loop {
        if done() {
            return true;
        }
        if start.elapsed() > limit {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Like `wait_until`, but looks every 5 ms, and says when `done` first
/// held: often enough to time a close, and light enough that the looking
/// doesn't slow Kiln down.
fn poll_until(limit: Duration, mut done: impl FnMut() -> bool, at: &mut Option<Instant>) -> bool {
    let start = Instant::now();
    loop {
        if done() {
            *at = Some(Instant::now());
            return true;
        }
        if start.elapsed() > limit {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The foreground window once it has stayed the same for 200 ms, or after
/// `SETTLE` whatever it is.
fn settled_foreground() -> HWND {
    let mut last = unsafe { GetForegroundWindow() };
    let mut since = Instant::now();
    wait_until(SETTLE, || {
        let now = unsafe { GetForegroundWindow() };
        if now != last {
            last = now;
            since = Instant::now();
        }
        since.elapsed() >= Duration::from_millis(200)
    });
    last
}

/// Whether any Kiln is running in this session.
fn a_kiln_runs() -> bool {
    unsafe {
        if let Ok(h) = OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, w!("Local\\KilnWindow")) {
            let _ = CloseHandle(h);
            return true;
        }
        FindWindowW(w!("KilnWindow"), PCWSTR::null()).is_ok_and(|h| !h.is_invalid())
    }
}

fn have_taskbar() -> bool {
    unsafe { FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()) }.is_ok_and(|h| !h.is_invalid())
}

fn pid_of(hwnd: HWND) -> u32 {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

/// The top-level windows of class `class` that process `pid` owns,
/// hidden ones too.
fn windows_of(pid: u32, class: PCWSTR) -> Vec<HWND> {
    let mut found = vec![];
    let mut after: Option<HWND> = None;
    while let Ok(h) = unsafe { FindWindowExW(None, after, class, PCWSTR::null()) } {
        if h.is_invalid() {
            break;
        }
        if pid_of(h) == pid {
            found.push(h);
        }
        after = Some(h);
    }
    found
}

/// The names of process `pid`'s threads that have one.
fn thread_names(pid: u32) -> Vec<String> {
    let mut names = vec![];
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) else {
            return names;
        };
        let mut entry = THREADENTRY32 {
            dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        let mut more = Thread32First(snap, &mut entry).is_ok();
        while more {
            if entry.th32OwnerProcessID == pid {
                if let Ok(t) =
                    OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, entry.th32ThreadID)
                {
                    if let Ok(text) = GetThreadDescription(t) {
                        if let Ok(s) = text.to_string() {
                            if !s.is_empty() {
                                names.push(s);
                            }
                        }
                        let _ = LocalFree(Some(HLOCAL(text.0 as *mut _)));
                    }
                    let _ = CloseHandle(t);
                }
            }
            more = Thread32Next(snap, &mut entry).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    names
}

/// A point well inside the primary monitor's work area, away from its
/// middle, where the invisible wheel opens for a moment. In screen pixels,
/// as Kiln and its hooks see them.
fn a_quiet_point() -> (i32, i32) {
    static AWARE: Once = Once::new();
    AWARE.call_once(|| unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    });
    // With `KILN_TEST_PLACE` (`win/placement.rs`) set, a point in that area.
    let placed: Option<Vec<i32>> = std::env::var("KILN_TEST_PLACE").ok().and_then(|t| {
        t.split(',')
            .map(|p| p.trim().parse().ok())
            .collect::<Option<_>>()
    });
    if let Some([x, y, w, h]) = placed.as_deref() {
        if *w > 400 && *h > 400 {
            return (x + 400, y + 400);
        }
    }
    let monitor = unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    assert!(unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool());
    let r = info.rcWork;
    (r.left + 400, r.top + 400)
}

/// A Kiln this test started. It is ended when dropped.
struct TestKiln {
    child: Child,
    hwnd: HWND,
    appdata: TempDir,
    _local: TempDir,
    _one: MutexGuard<'static, ()>,
}

impl TestKiln {
    /// Start Kiln with `settings` as its settings.toml, if given, and wait
    /// for its window.
    fn start(settings: Option<&str>) -> TestKiln {
        let one = ONE_KILN.lock().unwrap_or_else(|e| e.into_inner());
        assert!(
            !a_kiln_runs(),
            "another Kiln is running. Close it, then run these tests again."
        );
        let appdata = tempfile::tempdir().unwrap();
        let local = tempfile::tempdir().unwrap();
        if let Some(text) = settings {
            let dir = appdata.path().join("Kiln");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("settings.toml"), text).unwrap();
        }
        // Only a build that honours the test switches: one without them looks in
        // the real drives' holding folders and saves into the real Documents.
        common::refuse_without_hooks();
        let child = Command::new(env!("CARGO_BIN_EXE_kiln-app"))
            .env("APPDATA", appdata.path())
            .env("LOCALAPPDATA", local.path())
            // Recovery looks only on stand-in drives (none), never in a
            // real drive's .kiln-held.
            .env("KILN_TEST_DRIVES", "")
            // Debug builds leave the start-up entry alone: none is read
            // or written.
            .env("KILN_TEST_RUN_KEY", "")
            .spawn()
            .expect("kiln-app starts");
        let pid = child.id();
        let mut hwnd = None;
        let shown = wait_until(START, || {
            hwnd = windows_of(pid, w!("KilnWindow"))
                .into_iter()
                .find(|&h| unsafe { IsWindowVisible(h) }.as_bool());
            hwnd.is_some()
        });
        let mut k = TestKiln {
            child,
            hwnd: hwnd.unwrap_or_default(),
            appdata,
            _local: local,
            _one: one,
        };
        assert!(shown, "Kiln's window never showed");
        assert!(k.running());
        k
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn post(&self, msg: u32, wparam: WPARAM, lparam: LPARAM) {
        unsafe { PostMessageW(Some(self.hwnd), msg, wparam, lparam) }
            .expect("the message is posted");
    }

    /// A verdict, as the hooks post it.
    fn verdict(&self, v: Verdict) {
        let (wparam, lparam) = encode(v);
        self.post(WM_KILN_HOOK, wparam, lparam);
    }

    /// The tray menu's Drag-wheel, which turns the setting over.
    fn toggle_drag_wheel(&self) {
        self.post(WM_COMMAND, WPARAM(usize::from(CMD_DRAG_WHEEL)), LPARAM(0));
    }

    fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn visible(&self) -> bool {
        unsafe { IsWindowVisible(self.hwnd) }.as_bool()
    }

    fn hooked(&self) -> bool {
        thread_names(self.pid()).iter().any(|n| n == HOOK_THREAD)
    }

    /// The floating wheel's window, while it is up.
    fn wheel(&self) -> Option<HWND> {
        windows_of(self.pid(), w!("KilnFloatWheel"))
            .into_iter()
            .next()
    }

    fn settings_text(&self) -> String {
        let file = self.appdata.path().join("Kiln").join("settings.toml");
        std::fs::read_to_string(file).unwrap_or_default()
    }
}

impl Drop for TestKiln {
    fn drop(&mut self) {
        if self.running() {
            self.post(WM_COMMAND, WPARAM(usize::from(CMD_QUIT)), LPARAM(0));
            let child = &mut self.child;
            wait_until(QUIT, || !matches!(child.try_wait(), Ok(None)));
        }
        if self.running() {
            eprintln!("the test Kiln didn't quit; ending it");
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        wait_until(SETTLE, || !a_kiln_runs());
    }
}

/// A UI Automation client on Kiln's window.
struct Ui {
    auto: IUIAutomation,
    root: IUIAutomationElement,
}

impl Ui {
    fn new(hwnd: HWND) -> Ui {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let auto: IUIAutomation =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).unwrap();
            let root = auto.ElementFromHandle(hwnd).unwrap();
            Ui { auto, root }
        }
    }

    fn find(&self, automation_id: &str) -> Option<IUIAutomationElement> {
        unsafe {
            let value = VARIANT::from(automation_id);
            let cond = self
                .auto
                .CreatePropertyCondition(UIA_AutomationIdPropertyId, &value)
                .ok()?;
            self.root.FindFirst(TreeScope_Descendants, &cond).ok()
        }
    }

    fn wait_for(&self, automation_id: &str) -> IUIAutomationElement {
        let mut found = None;
        wait_until(SETTLE, || {
            found = self.find(automation_id);
            found.is_some()
        });
        found.unwrap_or_else(|| panic!("{automation_id} never appeared"))
    }

    /// Flip a ToggleSwitch, as Narrator would.
    fn toggle(&self, e: &IUIAutomationElement) {
        unsafe {
            let p: IUIAutomationTogglePattern = e
                .GetCurrentPatternAs(UIA_TogglePatternId)
                .expect("it can be toggled");
            p.Toggle().unwrap();
        }
    }

    /// A ToggleSwitch's state, as UI Automation reads it.
    fn toggled(&self, e: &IUIAutomationElement) -> Option<bool> {
        unsafe {
            let p: IUIAutomationTogglePattern = e.GetCurrentPatternAs(UIA_TogglePatternId).ok()?;
            let s = p.CurrentToggleState().ok()?;
            Some(s == ToggleState_On)
        }
    }

    /// Press a button, or choose a tab or a segment, as Narrator would.
    fn press(&self, e: &IUIAutomationElement) {
        unsafe {
            if let Ok(p) = e.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
            {
                p.Invoke().unwrap();
                return;
            }
            let p: IUIAutomationSelectionItemPattern = e
                .GetCurrentPatternAs(UIA_SelectionItemPatternId)
                .expect("it can be invoked or selected");
            p.Select().unwrap();
        }
    }
}

#[test]
fn turning_the_setting_off_stops_the_hooks() {
    let mut k = TestKiln::start(Some(ON));
    // On: the hooks run.
    assert!(wait_until(SETTLE, || k.hooked()), "the hooks never started");

    // The tray menu's Drag-wheel turns it off, and the hooks stop.
    k.toggle_drag_wheel();
    assert!(wait_until(SETTLE, || !k.hooked()), "the hooks kept running");
    // Settings are written off the window's thread, a moment after.
    assert!(wait_until(SETTLE, || k
        .settings_text()
        .contains("drag_wheel = false")));
    // A verdict that comes now opens nothing.
    k.verdict(Verdict::Open {
        x: a_quiet_point().0,
        y: a_quiet_point().1,
        tools: false,
    });
    assert!(
        !wait_until(Duration::from_millis(500), || k.wheel().is_some()),
        "a wheel opened with the setting off"
    );

    // Settings' Drag-wheel card turns it back on, as Narrator would: the
    // card reads its name and note, and its ToggleSwitch starts the hooks
    // again.
    let ui = Ui::new(k.hwnd);
    ui.press(&ui.wait_for("SettingsButton"));
    let card = ui.wait_for("SettingsCard(DragWheel)");
    unsafe {
        assert_eq!(card.CurrentName().unwrap().to_string(), DRAG_WHEEL_LABEL);
        // Shift is the key the note names, tapped.
        assert_eq!(card.CurrentHelpText().unwrap().to_string(), DRAG_WHEEL_NOTE);
        assert!(DRAG_WHEEL_NOTE.starts_with("Tap Shift while dragging"));
    }
    let toggle = ui.wait_for("Toggle(DragWheel)");
    assert_eq!(ui.toggled(&toggle), Some(false));
    ui.toggle(&toggle);
    assert!(
        wait_until(SETTLE, || k.hooked()),
        "the hooks didn't restart"
    );
    assert!(wait_until(SETTLE, || k
        .settings_text()
        .contains("drag_wheel = true")));
    assert!(wait_until(SETTLE, || ui.toggled(&toggle) == Some(true)));
    // And off stops them.
    ui.toggle(&toggle);
    assert!(wait_until(SETTLE, || !k.hooked()), "Off left the hooks on");
    assert!(k.running());
}

#[test]
fn a_verdict_opens_the_wheel_without_showing_kiln() {
    if !have_taskbar() {
        eprintln!("skipped: no taskbar");
        return;
    }
    let k = TestKiln::start(Some(ON));
    assert!(wait_until(SETTLE, || k.hooked()));
    // Kiln hidden in the notification area, as it is while the
    // drag-wheel waits.
    k.post(WM_CLOSE, WPARAM(0), LPARAM(0));
    assert!(wait_until(SETTLE, || !k.visible()), "Kiln didn't hide");
    // Windows moves the activation on once Kiln hides.
    let front = settled_foreground();

    let (x, y) = a_quiet_point();
    let open = Verdict::Open { x, y, tools: false };
    k.verdict(open);
    assert!(
        wait_until(SETTLE, || k.wheel().is_some()),
        "no wheel opened"
    );
    let fw = k.wheel().unwrap();
    // Centred on the point, over every other window, and Kiln itself
    // stays hidden and out of the way.
    let mut r = RECT::default();
    unsafe { GetWindowRect(fw, &mut r).unwrap() };
    assert!(
        r.left < x && x < r.right && r.top < y && y < r.bottom,
        "{r:?} for {x}, {y}"
    );
    let ex = unsafe { GetWindowLongPtrW(fw, GWL_EXSTYLE) } as u32;
    assert_ne!(ex & WS_EX_TOPMOST.0, 0);
    assert_ne!(ex & WS_EX_NOACTIVATE.0, 0);
    assert!(!k.visible());
    assert_eq!(unsafe { GetForegroundWindow() }, front);

    // No drag with files reaches it, so it closes unseen; the next
    // verdict opens it again.
    assert!(wait_until(SETTLE, || k.wheel().is_none()), "it stayed open");
    // Asked for now, the wheel opens no sooner, so it could close itself
    // for want of files no sooner than NO_FILES_MS (250 ms) from here.
    let asked = Instant::now();
    k.verdict(Verdict::Open { x, y, tools: true });
    let mut seen = None;
    assert!(poll_until(SETTLE, || k.wheel().is_some(), &mut seen));
    // A flip changes the ring, and a close closes it at once (it has drawn
    // nothing yet, so there is nothing to fade): well inside those 250 ms,
    // with 80 ms to spare, so this is the close's doing.
    k.verdict(Verdict::Flip);
    k.verdict(Verdict::Close);
    let mut gone = None;
    assert!(poll_until(SETTLE, || k.wheel().is_none(), &mut gone));
    let gone = gone.unwrap();
    assert!(
        gone - asked < Duration::from_millis(170),
        "closed {:?} after it was asked for (seen after {:?})",
        gone - asked,
        seen.unwrap() - asked
    );
    assert!(!k.visible());
}
