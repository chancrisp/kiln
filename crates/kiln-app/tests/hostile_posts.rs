//! Another program can post Kiln's window any `WM_APP` number with any
//! `lparam`. Kiln once took some of those numbers' `lparam` for a pointer
//! to a box its own threads had posted, so a stray post made it read
//! whatever the number pointed at and crash. Now no `WM_APP` message is
//! read as a pointer: the window keeps running and ends cleanly.
//!
//! The test starts a Kiln of its own, with a scratch APPDATA and
//! LOCALAPPDATA, and drives it only by posting it window messages, as
//! `resident.rs` does. Nothing here moves the pointer or sends input, and
//! it doesn't start while another Kiln is running.

#![cfg(windows)]

mod common;

use std::io::Read;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use kiln_app::win::tray::CMD_QUIT;
use tempfile::TempDir;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::System::Threading::{OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};
use windows::Win32::UI::WindowsAndMessaging::*;

/// A debug build can take a while to show its first frame.
const START: Duration = Duration::from_secs(60);
/// How long a posted message may take to show its effect.
const SETTLE: Duration = Duration::from_secs(10);

/// Every `WM_APP + n` Kiln used before its workers reached the window
/// through one channel: the drop target's (1, 2), the workers' boxed
/// results (3, 5 to 10, 12 to 14, 17 to 19), the notice's (11), the hooks'
/// (15), the tray's (16), the start's (20), and the test panic's (0x3F0),
/// which this Kiln, started without `KILN_TEST_PANIC`, doesn't answer.
/// 4 and 21 to 32 are posted too, for any number in between.
fn old_numbers() -> Vec<u32> {
    (1..=32).chain([0x3F0]).map(|n| WM_APP + n).collect()
}

fn wait_until(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    loop {
        if done() {
            return true;
        }
        if start.elapsed() > limit {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Whether any Kiln is running in this session: every Kiln holds this
/// mutex while it runs, and registers this window class.
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

/// The visible top-level windows of class `class` that process `pid` owns.
fn shown_windows_of(pid: u32, class: PCWSTR) -> Vec<HWND> {
    let mut found = vec![];
    let mut after: Option<HWND> = None;
    while let Ok(h) = unsafe { FindWindowExW(None, after, class, PCWSTR::null()) } {
        if h.is_invalid() {
            break;
        }
        if pid_of(h) == pid && unsafe { IsWindowVisible(h) }.as_bool() {
            found.push(h);
        }
        after = Some(h);
    }
    found
}

/// A Kiln this test started. It is ended when dropped, if it hasn't ended.
struct TestKiln {
    child: Child,
    hwnd: HWND,
    _appdata: TempDir,
    _local: TempDir,
    stderr: Option<JoinHandle<String>>,
}

impl TestKiln {
    fn start() -> TestKiln {
        assert!(
            !a_kiln_runs(),
            "another Kiln is running. Close it, then run this test again."
        );
        let appdata = tempfile::tempdir().unwrap();
        let local = tempfile::tempdir().unwrap();
        let temp = local.path().join("Temp");
        std::fs::create_dir_all(&temp).unwrap();
        // Only a build that honours the test switches: one without them looks in
        // the real drives' holding folders and saves into the real Documents.
        common::refuse_without_hooks();
        let mut child = Command::new(env!("CARGO_BIN_EXE_kiln-app"))
            .env("APPDATA", appdata.path())
            .env("LOCALAPPDATA", local.path())
            // Recovery looks only on stand-in drives (none), never in a
            // real drive's .kiln-held.
            .env("KILN_TEST_DRIVES", "")
            // Debug builds leave the start-up entry alone: none is read
            // or written.
            .env("KILN_TEST_RUN_KEY", "")
            .env("TEMP", &temp)
            .env("TMP", &temp)
            .env_remove("KILN_TEST_PANIC")
            .stderr(Stdio::piped())
            .spawn()
            .expect("kiln-app starts");
        // Read as it comes: a full pipe would stop Kiln mid-report.
        let mut pipe = child.stderr.take().expect("stderr is piped");
        let stderr = std::thread::spawn(move || {
            let mut text = String::new();
            let _ = pipe.read_to_string(&mut text);
            text
        });
        let pid = child.id();
        let mut hwnd = None;
        let shown = wait_until(START, || {
            hwnd = shown_windows_of(pid, w!("KilnWindow")).into_iter().next();
            hwnd.is_some()
        });
        let k = TestKiln {
            child,
            hwnd: hwnd.unwrap_or_default(),
            _appdata: appdata,
            _local: local,
            stderr: Some(stderr),
        };
        assert!(shown, "Kiln's window never showed");
        k
    }

    fn ended(&mut self) -> Option<ExitStatus> {
        self.child.try_wait().unwrap()
    }

    fn stderr_text(&mut self) -> String {
        self.stderr
            .take()
            .map(|t| t.join().unwrap_or_default())
            .unwrap_or_default()
    }

    /// Whether the window answers a sent message within a few seconds.
    fn answers(&self) -> bool {
        let mut result = 0usize;
        unsafe {
            SendMessageTimeoutW(
                self.hwnd,
                WM_NULL,
                WPARAM(0),
                LPARAM(0),
                SMTO_ABORTIFHUNG,
                5000,
                Some(&mut result),
            )
        }
        .0 != 0
    }
}

impl Drop for TestKiln {
    fn drop(&mut self) {
        if self.ended().is_none() {
            eprintln!("the test Kiln didn't end; ending it");
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        wait_until(SETTLE, || !a_kiln_runs());
    }
}

#[test]
fn a_posted_app_message_with_a_garbage_lparam_reads_no_pointer() {
    let mut k = TestKiln::start();
    for msg in old_numbers() {
        for lparam in [1usize, usize::MAX] {
            unsafe { PostMessageW(Some(k.hwnd), msg, WPARAM(0), LPARAM(lparam as isize)) }
                .unwrap_or_else(|e| panic!("{msg:#x} is posted: {e}"));
        }
    }
    // Posted messages are all answered well within this; a crash ends
    // Kiln with an access violation.
    let crashed = wait_until(Duration::from_secs(3), || k.ended().is_some());
    assert!(
        !crashed,
        "Kiln ended ({:?}) on a posted message. Its stderr: {}",
        k.ended(),
        k.stderr_text()
    );
    assert!(k.answers(), "the window still answers");
    assert!(
        unsafe { IsWindowVisible(k.hwnd) }.as_bool(),
        "the window is still shown"
    );

    // It still quits as it always did, with nothing held to ask about.
    unsafe {
        PostMessageW(
            Some(k.hwnd),
            WM_COMMAND,
            WPARAM(usize::from(CMD_QUIT)),
            LPARAM(0),
        )
    }
    .expect("Quit is posted");
    let mut status = None;
    wait_until(SETTLE, || {
        status = k.ended();
        status.is_some()
    });
    let stderr = k.stderr_text();
    assert!(
        status.is_some_and(|s| s.success()),
        "Quit ends Kiln cleanly: {status:?}. Its stderr: {stderr}"
    );
    assert!(!stderr.contains("panicked"), "stderr: {stderr}");
}
