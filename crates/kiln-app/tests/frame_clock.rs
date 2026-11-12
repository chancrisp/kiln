//! Frames come from the display, and nothing runs while Kiln is hidden or
//! minimised.
//!
//! Each test starts a Kiln of its own, with a scratch APPDATA and
//! LOCALAPPDATA and `KILN_FRAME_LOG` set, and drives it only by showing and
//! minimising its window without activating it (`ShowWindow`, with
//! SW_SHOWMINNOACTIVE and SW_SHOWNOACTIVATE) and through UI Automation
//! patterns. Nothing here moves the pointer or sends input, and no test
//! starts while another Kiln is running.
//!
//! The frame log is written through a buffer that is flushed by a later
//! frame and when Kiln quits, so each test quits Kiln before it reads the
//! log, or reads it while frames are still coming. It holds `main <µs>`
//! for each frame and `clock <µs>` for each tick of the inspector's clock,
//! the microseconds counted from Kiln's start.
//!
//! The job test makes its long video with FFmpeg, which Kiln converts
//! with, and answers Kiln's prompt, inside its window, through UI
//! Automation.

#![cfg(windows)]

mod common;

use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use kiln_app::win::tray::CMD_QUIT;
use tempfile::TempDir;
use windows::core::{w, BOOL, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, EnumDisplaySettingsW, GetMonitorInfoW, MonitorFromWindow, DEVMODEW,
    ENUM_CURRENT_SETTINGS, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{GetProcessTimes, OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::*;

/// One test Kiln at a time.
static ONE_KILN: Mutex<()> = Mutex::new(());

/// Why this is skipped on a CI runner (`CI` set), if it is: it times what
/// the display and the compositor do, which a shared runner, with no
/// steady compositor or real monitor, can't show. It runs in the local
/// gate.
fn on_ci() -> Option<&'static str> {
    std::env::var_os("CI")
        .is_some()
        .then_some("CI is set; display timing runs in the local gate only")
}

/// Skip the test, saying why, when it runs on CI ([`on_ci`]).
macro_rules! skip_on_ci {
    () => {
        if let Some(why) = on_ci() {
            println!("skipped: {why}");
            return;
        }
    };
}

const START: Duration = Duration::from_secs(60);
const SETTLE: Duration = Duration::from_secs(10);
const QUIT: Duration = Duration::from_secs(10);

fn wait_until(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    loop {
        if done() {
            return true;
        }
        if start.elapsed() > limit {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn a_kiln_runs() -> bool {
    unsafe {
        if let Ok(h) = OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, w!("Local\\KilnWindow")) {
            let _ = CloseHandle(h);
            return true;
        }
        FindWindowW(w!("KilnWindow"), PCWSTR::null()).is_ok_and(|h| !h.is_invalid())
    }
}

fn pid_of(hwnd: HWND) -> u32 {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

fn window_of(pid: u32) -> Option<HWND> {
    let mut after: Option<HWND> = None;
    while let Ok(h) = unsafe { FindWindowExW(None, after, w!("KilnWindow"), PCWSTR::null()) } {
        if h.is_invalid() {
            break;
        }
        if pid_of(h) == pid {
            return Some(h);
        }
        after = Some(h);
    }
    None
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

/// A Kiln this test started. It is quit when dropped.
struct TestKiln {
    child: Child,
    hwnd: HWND,
    log: PathBuf,
    /// When the process was started, and when its window was first seen
    /// shown: the frame log's microseconds count from just before the
    /// second.
    spawned: Instant,
    seen: Instant,
    _dirs: (TempDir, TempDir, TempDir),
    _one: MutexGuard<'static, ()>,
}

impl TestKiln {
    fn start(settings: Option<&str>, files: &[PathBuf]) -> TestKiln {
        let one = ONE_KILN.lock().unwrap_or_else(|e| e.into_inner());
        assert!(
            !a_kiln_runs(),
            "another Kiln is running. Close it, then run these tests again."
        );
        let appdata = tempfile::tempdir().unwrap();
        let local = tempfile::tempdir().unwrap();
        let logs = tempfile::tempdir().unwrap();
        if let Some(text) = settings {
            let dir = appdata.path().join("Kiln");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("settings.toml"), text).unwrap();
        }
        let temp = local.path().join("Temp");
        std::fs::create_dir_all(&temp).unwrap();
        let log = logs.path().join("frames.log");
        let spawned = Instant::now();
        // The measuring tests can run another build (`KILN_COST_EXE`) and
        // leave the frame log off (`KILN_COST_NOLOG`), so the log's own
        // work isn't counted.
        // Only a build that honours the test switches is started: one
        // without them would look in the real drives' `.kiln-held` at
        // start and save every result it found there at once, into the
        // folders it was meant for or the real Documents. A build given as
        // `KILN_COST_EXE` must carry the test hooks' mark (a release build
        // made with `--features test-hooks`); with these switches it then
        // looks on no drive and has no Documents.
        let exe: PathBuf = match std::env::var_os("KILN_COST_EXE") {
            Some(exe) => {
                let exe = PathBuf::from(exe);
                common::refuse_unless_hooked(&exe);
                exe
            }
            None => {
                common::refuse_without_hooks();
                env!("CARGO_BIN_EXE_kiln-app").into()
            }
        };
        let mut command = Command::new(exe);
        command
            .args(files)
            .env("APPDATA", appdata.path())
            .env("LOCALAPPDATA", local.path())
            // Recovery looks only on stand-in drives (none), never in a
            // real drive's .kiln-held.
            .env("KILN_TEST_DRIVES", "")
            // Debug builds leave the start-up entry alone: none is read
            // or written.
            .env("KILN_TEST_RUN_KEY", "")
            .env("TEMP", &temp)
            .env("TMP", &temp);
        if std::env::var_os("KILN_COST_NOLOG").is_none() {
            command.env("KILN_FRAME_LOG", &log);
        }
        let child = command.spawn().expect("kiln-app starts");
        let pid = child.id();
        let mut hwnd = None;
        let shown = wait_until(START, || {
            hwnd = window_of(pid).filter(|&h| unsafe { IsWindowVisible(h) }.as_bool());
            hwnd.is_some()
        });
        assert!(shown, "Kiln's window never showed");
        TestKiln {
            child,
            hwnd: hwnd.unwrap_or_default(),
            log,
            spawned,
            seen: Instant::now(),
            _dirs: (appdata, local, logs),
            _one: one,
        }
    }

    /// Show the window as `how`, without activating it.
    fn show(&self, how: SHOW_WINDOW_CMD) {
        // SAFETY: a plain call on the test Kiln's window.
        let _ = unsafe { ShowWindow(self.hwnd, how) };
    }

    fn minimised(&self) -> bool {
        unsafe { IsIconic(self.hwnd) }.as_bool()
    }

    /// The processor time Kiln has used so far, kernel and user.
    fn cpu(&self) -> Duration {
        let (mut created, mut exited, mut kernel, mut user) = Default::default();
        // SAFETY: the child's own process handle, open while `self` is.
        unsafe {
            GetProcessTimes(
                HANDLE(self.child.as_raw_handle()),
                &mut created,
                &mut exited,
                &mut kernel,
                &mut user,
            )
            .expect("process times");
        }
        let ticks = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
        Duration::from_nanos((ticks(kernel) + ticks(user)) * 100)
    }

    /// Quit Kiln as the tray menu would, so it flushes its frame log, and
    /// read the log: each frame's microseconds from Kiln's start.
    fn quit_and_read_log(&mut self) -> Vec<u64> {
        // SAFETY: a plain post to the test Kiln's window.
        unsafe {
            let quit = WPARAM(usize::from(CMD_QUIT));
            let _ = PostMessageW(Some(self.hwnd), WM_COMMAND, quit, LPARAM(0));
        }
        let child = &mut self.child;
        let ended = wait_until(QUIT, || !matches!(child.try_wait(), Ok(None)));
        assert!(ended, "Kiln didn't quit");
        lines_of(
            &std::fs::read_to_string(&self.log).unwrap_or_default(),
            "main",
        )
    }

    /// The lines of the frame log that start with `what`, while Kiln runs:
    /// what has been flushed so far, each line's microseconds.
    fn logged(&self, what: &str) -> Vec<u64> {
        lines_of(
            &std::fs::read_to_string(&self.log).unwrap_or_default(),
            what,
        )
    }

    /// Kiln's prompt, inside its window, once it opens.
    fn prompt_opens(&self) -> IUIAutomationElement {
        let mut prompt = None;
        wait_until(START, || {
            prompt = find(self.hwnd, "Ask");
            prompt.is_some()
        });
        assert!(
            shown_windows_of(self.child.id(), w!("#32770")).is_empty(),
            "Kiln asks inside its window"
        );
        prompt.expect("Kiln asks")
    }

    /// Microseconds on the frame log's clock for `at`, a test-side instant.
    fn log_clock(&self, at: Instant) -> i64 {
        let offset = self.seen.duration_since(self.spawned);
        at.duration_since(self.spawned).as_micros() as i64 - offset.as_micros() as i64
    }
}

impl Drop for TestKiln {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            // SAFETY: a plain post to the test Kiln's window.
            unsafe {
                let quit = WPARAM(usize::from(CMD_QUIT));
                let _ = PostMessageW(Some(self.hwnd), WM_COMMAND, quit, LPARAM(0));
            }
            let child = &mut self.child;
            wait_until(QUIT, || !matches!(child.try_wait(), Ok(None)));
        }
        if matches!(self.child.try_wait(), Ok(None)) {
            eprintln!("the test Kiln didn't quit; ending it");
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        wait_until(SETTLE, || !a_kiln_runs());
    }
}

/// The microseconds of the frame log's lines that start with `what`.
fn lines_of(text: &str, what: &str) -> Vec<u64> {
    text.lines()
        .filter_map(|l| l.strip_prefix(what)?.strip_prefix(' '))
        .filter_map(|n| n.trim().parse().ok())
        .collect()
}

/// The windows of class `class` that process `pid` shows.
fn shown_windows_of(pid: u32, class: PCWSTR) -> Vec<HWND> {
    let mut out = vec![];
    let mut after: Option<HWND> = None;
    while let Ok(h) = unsafe { FindWindowExW(None, after, class, PCWSTR::null()) } {
        if h.is_invalid() {
            break;
        }
        if pid_of(h) == pid && unsafe { IsWindowVisible(h) }.as_bool() {
            out.push(h);
        }
        after = Some(h);
    }
    out
}

/// How many of `times` fall in `from..to`.
fn count_in(times: &[u64], from: i64, to: i64) -> usize {
    times
        .iter()
        .filter(|&&t| (t as i64) >= from && (t as i64) < to)
        .count()
}

/// The intervals between frames, in milliseconds.
fn intervals(frames: &[u64]) -> Vec<f64> {
    frames
        .windows(2)
        .map(|w| (w[1] - w[0]) as f64 / 1000.0)
        .collect()
}

/// The `p` quantile of `v` (nearest rank).
fn quantile(v: &[f64], p: f64) -> f64 {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    s[((s.len() as f64 * p).ceil() as usize).clamp(1, s.len()) - 1]
}

/// The refresh interval of the display Kiln's window is on, in
/// milliseconds, as that display's current mode says
/// (`EnumDisplaySettings`; the compositor's own timing can't be asked
/// about another process's window).
fn refresh_ms(hwnd: HWND) -> f64 {
    // SAFETY: plain queries, with the structures' sizes set.
    unsafe {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        assert!(GetMonitorInfoW(monitor, &mut info.monitorInfo).as_bool());
        let mut mode = DEVMODEW {
            dmSize: std::mem::size_of::<DEVMODEW>() as u16,
            ..Default::default()
        };
        assert!(EnumDisplaySettingsW(
            PCWSTR(info.szDevice.as_ptr()),
            ENUM_CURRENT_SETTINGS,
            &mut mode
        )
        .as_bool());
        assert!(mode.dmDisplayFrequency > 1, "a refresh rate");
        1000.0 / f64::from(mode.dmDisplayFrequency)
    }
}

#[test]
fn nothing_runs_while_the_window_is_minimised() {
    skip_on_ci!();
    // A fresh profile: the empty window, which draws its first frames and
    // then nothing until something changes.
    let mut k = TestKiln::start(Some("drag_wheel = false\n"), &[]);
    std::thread::sleep(Duration::from_secs(2));
    k.show(SW_SHOWMINNOACTIVE);
    assert!(wait_until(SETTLE, || k.minimised()), "the window minimises");
    // What the minimising itself draws is over after this.
    std::thread::sleep(Duration::from_secs(1));
    let minimised_at = Instant::now();
    let before = k.cpu();
    std::thread::sleep(Duration::from_secs(5));
    let used = k.cpu() - before;
    assert!(k.minimised());
    assert!(
        used < Duration::from_millis(50),
        "a minimised Kiln used {used:?} of processor time in 5 s"
    );
    // Restored without activating it, it is drawn again.
    k.show(SW_SHOWNOACTIVATE);
    assert!(wait_until(SETTLE, || !k.minimised()), "the window restores");
    std::thread::sleep(Duration::from_secs(2));
    let restored_at = Instant::now();
    let frames = k.quit_and_read_log();

    // The log has a quiet stretch, with no frame in it, that covers the
    // five seconds sampled, and frames come again after it.
    let gap = frames
        .windows(2)
        .map(|w| (w[0], w[1]))
        .max_by_key(|(a, b)| b - a)
        .expect("frames were logged");
    let quiet = Duration::from_micros(gap.1 - gap.0);
    assert!(
        quiet >= Duration::from_secs(5),
        "frames were drawn while minimised: the longest quiet stretch in {} frames is {quiet:?}",
        frames.len()
    );
    // It lies within the time the window was minimised.
    let (start, end) = (k.log_clock(minimised_at), k.log_clock(restored_at));
    assert!(
        (gap.0 as i64) < start + 500_000 && (gap.1 as i64) > start && (gap.1 as i64) < end,
        "the quiet stretch {gap:?} is outside {start}..{end}"
    );
    assert!(
        frames.iter().any(|&t| t >= gap.1),
        "the window is drawn again when it is restored"
    );
}

#[test]
fn nothing_runs_while_the_window_is_hidden() {
    skip_on_ci!();
    let mut k = TestKiln::start(Some("drag_wheel = false\n"), &[]);
    std::thread::sleep(Duration::from_secs(2));
    k.show(SW_HIDE);
    assert!(wait_until(SETTLE, || !unsafe { IsWindowVisible(k.hwnd) }.as_bool()));
    std::thread::sleep(Duration::from_secs(1));
    let before = k.cpu();
    std::thread::sleep(Duration::from_secs(5));
    let used = k.cpu() - before;
    assert!(
        used < Duration::from_millis(50),
        "a hidden Kiln used {used:?} of processor time in 5 s"
    );
    let shown_at = Instant::now();
    k.show(SW_SHOWNOACTIVATE);
    std::thread::sleep(Duration::from_secs(2));
    let frames = k.quit_and_read_log();
    let gap = frames
        .windows(2)
        .map(|w| w[1] - w[0])
        .max()
        .expect("frames were logged");
    assert!(
        gap >= 5_000_000,
        "frames were drawn while hidden: the longest quiet stretch is {gap} us"
    );
    // A fresh profile's empty window has nothing animating (the welcome
    // card's demos are gone), so shown again it is drawn, then rests.
    let shown = k.log_clock(shown_at);
    assert!(
        frames.iter().any(|&t| t as i64 >= shown),
        "the window is drawn again when it is shown"
    );
}

/// The element with `automation_id` in Kiln's window, once it is there.
fn find(hwnd: HWND, automation_id: &str) -> Option<IUIAutomationElement> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let auto: IUIAutomation =
            CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).unwrap();
        let root = auto.ElementFromHandle(hwnd).ok()?;
        let cond = auto
            .CreatePropertyCondition(UIA_AutomationIdPropertyId, &VARIANT::from(automation_id))
            .ok()?;
        root.FindFirst(TreeScope_Descendants, &cond).ok()
    }
}

/// The monitors' work areas, in this process's coordinates.
fn monitors() -> Vec<RECT> {
    unsafe extern "system" fn each(m: HMONITOR, _: HDC, _: *mut RECT, out: LPARAM) -> BOOL {
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: `out` is the vector `monitors` passed, alive for the
        // enumeration; the structure's size is set.
        unsafe {
            if GetMonitorInfoW(m, &mut info).as_bool() {
                (*(out.0 as *mut Vec<RECT>)).push(info.rcWork);
            }
        }
        true.into()
    }
    let mut out: Vec<RECT> = vec![];
    // SAFETY: the callback writes only into `out`, which outlives the call.
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(each), LPARAM(&mut out as *mut _ as isize));
    }
    out
}

extern "system" fn plain_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: the message's own arguments, handed on unchanged.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// What a plain window took from the compositor while it ran.
struct Probe {
    /// The intervals between its frames, in milliseconds.
    gaps: Vec<f64>,
    /// Waits on its waitable that timed out (100 ms).
    timeouts: u32,
}

/// How often the compositor takes a window's frames on the monitor that
/// holds `area`, measured for `run` by a plain window of this test's own,
/// shown there without activation, presenting a cleared flip-model swap
/// chain as fast as its frame-latency waitable allows. A display's refresh
/// rate is not always the rate its windows are composed at: with displays
/// of mixed rates, Windows may pace them all at the primary's. Run on a
/// thread of its own, beside what it is compared with, it sees the same
/// load.
fn probe_compositor(area: RECT, run: Duration) -> Probe {
    use windows::core::Interface;
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
    use windows::Win32::Graphics::Direct3D11::*;
    use windows::Win32::Graphics::Dxgi::Common::*;
    use windows::Win32::Graphics::Dxgi::*;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::WaitForSingleObjectEx;
    // SAFETY: a window, device and swap chain of this thread's own, used
    // and dropped here; every out-pointer is a local that outlives its call.
    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None).unwrap().into();
        let class = w!("KilnComposedProbe");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(plain_proc),
            hInstance: instance,
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            class,
            w!("composition probe"),
            WS_POPUP,
            area.left + 40,
            area.top + 40,
            320,
            200,
            None,
            None,
            Some(instance),
            None,
        )
        .unwrap();
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let (mut device, mut context) = (None, None);
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
        .unwrap();
        let (device, context) = (device.unwrap(), context.unwrap());
        let factory: IDXGIFactory2 = device
            .cast::<IDXGIDevice>()
            .unwrap()
            .GetAdapter()
            .unwrap()
            .GetParent()
            .unwrap();
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: 320,
            Height: 200,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            Flags: DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as u32,
            ..Default::default()
        };
        let swap: IDXGISwapChain2 = factory
            .CreateSwapChainForHwnd(&device, hwnd, &desc, None, None)
            .unwrap()
            .cast()
            .unwrap();
        swap.SetMaximumFrameLatency(1).unwrap();
        let waitable = swap.GetFrameLatencyWaitableObject();
        let (mut times, mut timeouts) = (vec![], 0);
        let start = Instant::now();
        let mut n = 0u32;
        while start.elapsed() < run {
            n += 1;
            if WaitForSingleObjectEx(waitable, 100, false) != WAIT_OBJECT_0 {
                timeouts += 1;
            }
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                DispatchMessageW(&msg);
            }
            let back: ID3D11Texture2D = swap.GetBuffer(0).unwrap();
            let mut view = None;
            device
                .CreateRenderTargetView(&back, None, Some(&mut view))
                .unwrap();
            let shade = (n % 30) as f32 / 30.0;
            context.ClearRenderTargetView(view.as_ref().unwrap(), &[shade, 0.2, 0.3, 1.0]);
            let _ = swap.Present(1, DXGI_PRESENT(0));
            times.push(Instant::now());
        }
        let _ = CloseHandle(waitable);
        let _ = DestroyWindow(hwnd);
        // The first frames settle in; the rest are the compositor's pace.
        let gaps = times
            .windows(2)
            .skip(10)
            .map(|w| w[1].duration_since(w[0]).as_secs_f64() * 1000.0)
            .collect();
        Probe { gaps, timeouts }
    }
}

/// A bloom's frame intervals on the monitor that holds `area`, in
/// milliseconds, by a Kiln of its own (the window moved there without
/// activating it, the side pane's "More formats and tools" invoked on the
/// selected row through UI Automation), with what a probe window took from the compositor there
/// meanwhile (`probe_compositor`), and that display's refresh interval.
fn bloom_on(area: RECT, photo: &Path) -> (Vec<f64>, Probe, f64) {
    let mut k = TestKiln::start(
        Some("welcome_shown = true\ndrag_wheel = false\n"),
        &[photo.to_path_buf()],
    );
    unsafe {
        SetWindowPos(
            k.hwnd,
            None,
            area.left + 60,
            area.top + 60,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
        .unwrap();
    }
    // A file given at start opens its wheel by itself (R19), over the
    // footer: Escape closes it, posted to the window (no real input), so
    // the bloom measured is the one the pane's More opens (the footer's
    // Convert now converts at once, or, with the pane folded, opens the
    // same list).
    assert!(
        wait_until(SETTLE, || find(k.hwnd, "Wheel").is_some()),
        "the start-up wheel opens"
    );
    unsafe {
        let _ = PostMessageW(Some(k.hwnd), WM_KEYDOWN, WPARAM(0x1B), LPARAM(0x0001_0001));
        let _ = PostMessageW(Some(k.hwnd), WM_KEYUP, WPARAM(0x1B), LPARAM(0xC001_0001));
    }
    assert!(
        wait_until(SETTLE, || find(k.hwnd, "Wheel").is_none()),
        "Escape closes the start-up wheel"
    );
    // Select the row, then wait for everything it started to settle.
    let row = {
        let mut row = None;
        wait_until(START, || {
            row = list_item(k.hwnd);
            row.is_some()
        });
        row.expect("the file is listed")
    };
    unsafe {
        let p: IUIAutomationSelectionItemPattern = row
            .GetCurrentPatternAs(UIA_SelectionItemPatternId)
            .expect("a row can be selected");
        p.Select().unwrap();
    }
    std::thread::sleep(Duration::from_millis(2500));
    let more = {
        let mut e = None;
        wait_until(START, || {
            e = find(k.hwnd, "InspectorControl(More)");
            e.is_some()
        });
        e.expect("the side pane has More formats and tools")
    };
    // The probe runs from a little before the bloom to well after it.
    let probe = std::thread::spawn(move || probe_compositor(area, Duration::from_millis(1500)));
    std::thread::sleep(Duration::from_millis(300));
    let trigger = Instant::now();
    unsafe {
        let p: IUIAutomationInvokePattern = more
            .GetCurrentPatternAs(UIA_InvokePatternId)
            .expect("More formats and tools can be invoked");
        p.Invoke().unwrap();
    }
    let probe = probe.join().expect("the probe ran");
    std::thread::sleep(Duration::from_millis(1000));
    let refresh = refresh_ms(k.hwnd);
    let frames = k.quit_and_read_log();
    // The bloom: the frames from the trigger on, until the first quiet
    // stretch. The first interval is the gap before the first frame.
    let from = k.log_clock(trigger) - 100_000;
    let mut bloom: Vec<u64> = frames
        .iter()
        .copied()
        .filter(|&t| t as i64 >= from)
        .collect();
    let burst_end = bloom
        .windows(2)
        .position(|w| w[1] - w[0] > 100_000)
        .map_or(bloom.len(), |i| i + 1);
    bloom.truncate(burst_end);
    assert!(
        bloom.len() >= 8,
        "the bloom drew {} frames: {bloom:?}",
        bloom.len()
    );
    (intervals(&bloom[1..]), probe, refresh)
}

/// Tries at a bloom on one monitor while the probe beside it shows the
/// machine too busy to keep a plain window at the compositor's pace.
const BLOOM_TRIES: usize = 3;

#[test]
fn a_bloom_is_drawn_at_the_displays_pace() {
    skip_on_ci!();
    let dir = tempfile::tempdir().unwrap();
    let photo = dir.path().join("photo.png");
    std::fs::copy(fixture("photo.png"), &photo).unwrap();
    for area in monitors() {
        let mut tries = vec![];
        let (gaps, tick, refresh) = loop {
            let (gaps, probe, refresh) = bloom_on(area, &photo);
            // The interval the compositor takes this monitor's windows at,
            // measured beside the bloom, under the same load.
            let tick = quantile(&probe.gaps, 0.5);
            let steady = probe.timeouts == 0 && quantile(&probe.gaps, 0.95) <= 1.25 * tick;
            tries.push(format!(
                "probe p50 {tick:.2} ms, p95 {:.2} ms, {} timeouts",
                quantile(&probe.gaps, 0.95),
                probe.timeouts
            ));
            if steady {
                break (gaps, tick, refresh);
            }
            assert!(
                tries.len() < BLOOM_TRIES,
                "the machine was too busy for a plain window to keep the \
                 compositor's pace in {BLOOM_TRIES} tries: {tries:?}"
            );
        };
        let (p50, p95) = (quantile(&gaps, 0.5), quantile(&gaps, 0.95));
        eprintln!(
            "bloom on the monitor at ({}, {}): {} intervals, p50 {p50:.2} ms, p95 {p95:.2} ms, \
             max {:.2} ms; refresh interval {refresh:.2} ms, windows composed every {tick:.2} ms; \
             tries: {tries:?}",
            area.left,
            area.top,
            gaps.len(),
            quantile(&gaps, 1.0)
        );
        // Paced by the display: a frame on most of the compositor's ticks.
        assert!(
            p50 <= 1.25 * tick,
            "p50 interval {p50:.2} ms is over 1.25 x the compositor's tick there, \
             {tick:.2} ms: {gaps:?}"
        );
        // And the slow frames no slower than 1.5 ticks, or than 1.5 x a
        // 60 Hz refresh where the tick is shorter: on a faster display a
        // frame of a test build may miss a tick, which is still paced.
        let bound = 1.5 * tick.max(1000.0 / 60.0);
        assert!(
            p95 <= bound,
            "p95 interval {p95:.2} ms is over {bound:.2} ms (the compositor's tick there \
             {tick:.2} ms): {gaps:?}"
        );
    }
}

/// A video `seconds` long, made with FFmpeg in `dir`.
fn long_video(dir: &Path, seconds: u32) -> PathBuf {
    // Made while no other test's Kiln runs: FFmpeg takes every core for
    // a few seconds, which would load a Kiln being measured.
    let _one = ONE_KILN.lock().unwrap_or_else(|e| e.into_inner());
    let out = dir.join("long.avi");
    let made = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg("testsrc2=size=1920x1080:rate=30")
        .args(["-t", &seconds.to_string(), "-c:v", "mpeg4", "-q:v", "31"])
        .arg(&out)
        .status()
        .expect("FFmpeg runs");
    assert!(made.success(), "FFmpeg made the video");
    out
}

/// Whether the first row of Kiln's list says its file is converting.
fn converting(hwnd: HWND) -> bool {
    list_item(hwnd)
        .and_then(|e| unsafe { e.CurrentName() }.ok())
        .is_some_and(|n| n.to_string().contains("Converting"))
}

/// The first row of Kiln's list, if it has one.
fn list_item(hwnd: HWND) -> Option<IUIAutomationElement> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let auto: IUIAutomation =
            CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).unwrap();
        let root = auto.ElementFromHandle(hwnd).ok()?;
        let cond = auto
            .CreatePropertyCondition(
                UIA_ControlTypePropertyId,
                &VARIANT::from(UIA_ListItemControlTypeId.0),
            )
            .ok()?;
        root.FindFirst(TreeScope_Descendants, &cond).ok()
    }
}

/// Convert the listed file to the first format its wheel offers, through
/// UI Automation as Narrator would, and leave its row selected, so the
/// inspector shows it converting, with its time left.
fn start_converting(hwnd: HWND) {
    let row = {
        let mut row = None;
        wait_until(START, || {
            row = list_item(hwnd);
            row.is_some()
        });
        row.expect("the file is listed")
    };
    unsafe {
        let p: IUIAutomationSelectionItemPattern = row
            .GetCurrentPatternAs(UIA_SelectionItemPatternId)
            .expect("a row can be selected");
        p.Select().unwrap();
    }
    // Each is pressed again until what it opens is there: the first press
    // can come before the file's formats are known.
    let press_until = |id: &str, done: &dyn Fn() -> bool| {
        let ok = wait_until(SETTLE, || {
            if done() {
                return true;
            }
            if let Some(e) = find(hwnd, id) {
                unsafe {
                    if let Ok(p) =
                        e.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
                    {
                        let _ = p.Invoke();
                    }
                }
            }
            // Longer than the wheel's opening and its pulse.
            std::thread::sleep(Duration::from_millis(600));
            false
        });
        assert!(ok, "pressing {id} didn't lead on");
    };
    // A file given at start opens its wheel by itself: picking its
    // first format converts at once. Every caller converts a video,
    // whose ring starts with MP4 (a photo's starts with Make smaller). The
    // next item is MP3, which fails on the test video: it has no sound.
    // Where that wheel never opens, the footer's Convert converts to the
    // format the pane suggests, which is the first one too.
    if wait_until(SETTLE, || find(hwnd, "WheelItem(Inner(0))").is_some()) {
        press_until("WheelItem(Inner(0))", &|| {
            find(hwnd, "WheelItem(Inner(0))").is_none()
        });
    } else {
        let convert = find(hwnd, "ConvertButton").expect("the footer has Convert");
        unsafe {
            convert
                .GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
                .expect("Convert can be invoked")
                .Invoke()
                .unwrap();
        }
    }
}

#[test]
fn a_running_jobs_heat_and_clock_stop_while_the_window_is_away_and_go_on_under_a_prompt() {
    skip_on_ci!();
    let dir = tempfile::tempdir().unwrap();
    let video = long_video(dir.path(), 300);
    let k = TestKiln::start(
        Some("welcome_shown = true\ndrag_wheel = false\n"),
        std::slice::from_ref(&video),
    );
    // Wide enough for the inspector beside the list, without activating
    // the window.
    unsafe {
        let dpi = GetDpiForWindow(k.hwnd) as i32;
        SetWindowPos(
            k.hwnd,
            None,
            0,
            0,
            1100 * dpi / 96,
            720 * dpi / 96,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
        .unwrap();
    }
    start_converting(k.hwnd);
    // The job runs: its line breathes, and the inspector's clock ticks.
    assert!(
        wait_until(SETTLE, || k.logged("clock").len() >= 2),
        "the inspector's clock ticks while the file converts"
    );
    let shown = || unsafe { IsWindowVisible(k.hwnd) }.as_bool();
    for away in [SW_SHOWMINNOACTIVE, SW_HIDE] {
        k.show(away);
        assert!(
            wait_until(SETTLE, || k.minimised() || !shown()),
            "the window goes away"
        );
        // What going away itself draws is over after this.
        std::thread::sleep(Duration::from_secs(1));
        let (from, before) = (Instant::now(), k.cpu());
        std::thread::sleep(Duration::from_secs(5));
        let (to, used) = (Instant::now(), k.cpu() - before);
        k.show(SW_SHOWNOACTIVATE);
        assert!(
            wait_until(SETTLE, || !k.minimised() && shown()),
            "the window comes back"
        );
        std::thread::sleep(Duration::from_millis(2500));
        let back = Instant::now();
        assert!(converting(k.hwnd), "{away:?}: the file is still converting");
        let (clock, frames) = (k.logged("clock"), k.logged("main"));
        let (from, to) = (k.log_clock(from), k.log_clock(to));
        let (since, now) = (k.log_clock(back) - 2_000_000, k.log_clock(back));
        eprintln!(
            "{away:?}: {used:?} of processor time in 5 s away; frames {} away, {} in 2 s back; \
             clock ticks {} away, {} in 2 s back",
            count_in(&frames, from, to),
            count_in(&frames, since, now),
            count_in(&clock, from, to),
            count_in(&clock, since, now),
        );
        assert_eq!(
            count_in(&frames, from, to),
            0,
            "{away:?}: frames were drawn"
        );
        assert_eq!(count_in(&clock, from, to), 0, "{away:?}: the clock ticked");
        // The job's progress reports still come, and are read, but draw
        // nothing.
        assert!(
            used < Duration::from_millis(100),
            "{away:?}: Kiln used {used:?} of processor time in 5 s"
        );
        // Back, the file still converting: the heat's frames come every
        // 50 ms, and the clock ticks every second.
        assert!(
            count_in(&frames, since, now) >= 20,
            "{away:?}: the heat's frames came back"
        );
        assert!(
            count_in(&clock, since, now) >= 1,
            "{away:?}: the clock came back"
        );
    }
    // Closing asks first about the file still converting, inside the
    // window, and the heat goes on under it.
    unsafe { PostMessageW(Some(k.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) }.unwrap();
    let prompt = k.prompt_opens();
    std::thread::sleep(Duration::from_secs(1));
    let from = Instant::now();
    std::thread::sleep(Duration::from_secs(2));
    let to = Instant::now();
    // Keep converting (Cancel), as closing the dialog is: the window
    // stays, the file converting.
    unsafe {
        let w: IUIAutomationWindowPattern =
            prompt.GetCurrentPatternAs(UIA_WindowPatternId).unwrap();
        w.Close().unwrap();
    }
    assert!(
        wait_until(SETTLE, || find(k.hwnd, "Ask").is_none()),
        "the prompt closes"
    );
    std::thread::sleep(Duration::from_millis(500));
    assert!(converting(k.hwnd), "the file is still converting");
    let frames = k.logged("main");
    let under = count_in(&frames, k.log_clock(from), k.log_clock(to));
    eprintln!("under the prompt: {under} frames in 2 s");
    // A frame every 50 ms; at the least, every other.
    assert!(
        under >= 20,
        "the heat under the prompt drew {under} frames in 2 s"
    );
}

/// What `quit_and_read_log` leaves out: how long each frame's drawing
/// took, from the `draw <µs>` lines (a whole window) and the `part <µs>`
/// lines (only the rows that changed).
fn draw_times(k: &TestKiln, what: &str) -> Vec<f64> {
    k.logged(what).iter().map(|&us| us as f64).collect()
}

/// Report a stretch's drawing cost and processor use. Nothing is asserted:
/// this is for comparing builds, and runs with `--ignored`, best on a
/// release build.
fn report_cost(what: &str, whole: &[f64], part: &[f64], used: Duration, over: Duration) {
    let frames = whole.len() + part.len();
    assert!(frames >= 20, "{what}: only {frames} frames");
    let stats = |v: &[f64]| {
        if v.is_empty() {
            "none".to_owned()
        } else {
            format!(
                "p50 {:.0} us, p95 {:.0} us, mean {:.0} us",
                quantile(v, 0.5),
                quantile(v, 0.95),
                v.iter().sum::<f64>() / v.len() as f64
            )
        }
    };
    eprintln!(
        "COST {what}: {frames} frames in {:.1} s, {:.2}% of one core;          whole window {} frames ({}); rows only {} frames ({})",
        over.as_secs_f64(),
        100.0 * used.as_secs_f64() / over.as_secs_f64(),
        whole.len(),
        stats(whole),
        part.len(),
        stats(part),
    );
}

#[test]
#[ignore = "measures only; run it with --ignored, on a release build"]
fn cost_of_a_running_jobs_frames() {
    let dir = tempfile::tempdir().unwrap();
    let video = long_video(dir.path(), 300);
    let k = TestKiln::start(
        Some(
            "welcome_shown = true
drag_wheel = false
",
        ),
        std::slice::from_ref(&video),
    );
    unsafe {
        let dpi = GetDpiForWindow(k.hwnd) as i32;
        SetWindowPos(
            k.hwnd,
            None,
            0,
            0,
            1100 * dpi / 96,
            720 * dpi / 96,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
        .unwrap();
    }
    start_converting(k.hwnd);
    std::thread::sleep(Duration::from_secs(3));
    let (from, before) = (Instant::now(), k.cpu());
    let skip = (draw_times(&k, "draw").len(), draw_times(&k, "part").len());
    std::thread::sleep(Duration::from_secs(10));
    let (over, used) = (from.elapsed(), k.cpu() - before);
    let (whole, part) = (draw_times(&k, "draw"), draw_times(&k, "part"));
    report_cost("running job", &whole[skip.0..], &part[skip.1..], used, over);
    // Quitting asks about the job; the drop ends Kiln if it isn't answered.
    drop(k);
}

#[test]
#[ignore = "measures only; set KILN_COST_EXE and KILN_COST_NOLOG, run with --ignored"]
fn cpu_of_a_running_job() {
    let dir = tempfile::tempdir().unwrap();
    let video = long_video(dir.path(), 300);
    let k = TestKiln::start(
        Some(
            "welcome_shown = true
drag_wheel = false
",
        ),
        std::slice::from_ref(&video),
    );
    unsafe {
        let dpi = GetDpiForWindow(k.hwnd) as i32;
        SetWindowPos(
            k.hwnd,
            None,
            0,
            0,
            1100 * dpi / 96,
            720 * dpi / 96,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
        .unwrap();
    }
    start_converting(k.hwnd);
    std::thread::sleep(Duration::from_secs(3));
    let (from, before) = (Instant::now(), k.cpu());
    std::thread::sleep(Duration::from_secs(20));
    let (over, used) = (from.elapsed(), k.cpu() - before);
    eprintln!("CPU {:.3}", 100.0 * used.as_secs_f64() / over.as_secs_f64());
    drop(k);
}
