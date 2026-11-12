//! The test placement switch: started with `KILN_TEST_PLACE=x,y,w,h`, Kiln
//! opens inside that rectangle and never becomes the foreground window.
//!
//! The rectangle is a monitor's work area in physical pixels, and comes
//! from `KILN_TEST_PLACE` in this test's own environment (the launcher
//! finds the 3840x2160 monitor with `find-4k.ps1`). Without it the test
//! prints why and returns.
//!
//! The test starts one Kiln of its own, with a scratch APPDATA and
//! LOCALAPPDATA, only when no other `kiln-app.exe` is running, and drives
//! it by one posted message (Quit) alone. Nothing here moves the pointer or
//! sends input.

#![cfg(windows)]
// Without the switch compiled in, Kiln ignores KILN_TEST_PLACE and would
// show and take focus as usual, so this test starts no Kiln at all then.
#![cfg(any(debug_assertions, feature = "test-hooks"))]

mod common;

use std::process::{Child, Command};
use std::time::{Duration, Instant};

use kiln_app::win::tray::CMD_QUIT;
use windows::core::BOOL;
use windows::Win32::Foundation::*;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::*;

/// A debug build can take a while to show its first frame.
const START: Duration = Duration::from_secs(60);
/// How long Kiln keeps being watched after its window shows: its first
/// frame, its engine look and its settings load all happen then.
const WATCH: Duration = Duration::from_secs(3);
/// How long Kiln gets to quit by itself before it is ended.
const QUIT: Duration = Duration::from_secs(10);

/// `x,y,w,h`, four whole numbers, as `KILN_TEST_PLACE` says.
fn parse(text: &str) -> Option<RECT> {
    let n: Vec<i32> = text
        .split(',')
        .map(|p| p.trim().parse().ok())
        .collect::<Option<_>>()?;
    let [x, y, w, h] = n[..] else {
        return None;
    };
    (w > 0 && h > 0).then_some(RECT {
        left: x,
        top: y,
        right: x + w,
        bottom: y + h,
    })
}

fn inside(r: RECT, area: RECT) -> bool {
    r.left >= area.left && r.top >= area.top && r.right <= area.right && r.bottom <= area.bottom
}

/// Whether any process called `kiln-app.exe` is running, for any user
/// session or started by anyone.
fn a_kiln_process_runs() -> bool {
    // SAFETY: a snapshot handle, read through the documented entry calls
    // and closed.
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            // Can't tell: treat it as running, and don't start one.
            return true;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut found = false;
        let mut more = Process32FirstW(snap, &mut entry).is_ok();
        while more {
            let len = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
            if name.eq_ignore_ascii_case("kiln-app.exe") {
                found = true;
                break;
            }
            more = Process32NextW(snap, &mut entry).is_ok();
        }
        let _ = CloseHandle(snap);
        found
    }
}

fn pid_of(hwnd: HWND) -> u32 {
    let mut pid = 0;
    // SAFETY: a plain query into a local.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

/// The visible top-level windows that process `pid` owns.
fn visible_windows_of(pid: u32) -> Vec<HWND> {
    extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` is the `&mut (u32, Vec<HWND>)` passed below,
        // alive for the whole enumeration.
        let (pid, found) = unsafe { &mut *(lparam.0 as *mut (u32, Vec<HWND>)) };
        if pid_of(hwnd) == *pid && unsafe { IsWindowVisible(hwnd) }.as_bool() {
            found.push(hwnd);
        }
        true.into()
    }
    let mut state = (pid, Vec::new());
    // SAFETY: the callback only reads and fills `state`, which outlives it.
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut state as *mut _ as isize));
    }
    state.1
}

fn rect_of(hwnd: HWND) -> RECT {
    let mut r = RECT::default();
    // SAFETY: a plain query into a local.
    let _ = unsafe { GetWindowRect(hwnd, &mut r) };
    r
}

fn class_of(hwnd: HWND) -> String {
    let mut text = [0u16; 64];
    // SAFETY: a plain query into a local buffer.
    let n = unsafe { GetClassNameW(hwnd, &mut text) };
    String::from_utf16_lossy(&text[..n.max(0) as usize])
}

fn title_of(hwnd: HWND) -> String {
    let mut text = [0u16; 128];
    // SAFETY: a plain query into a local buffer.
    let n = unsafe { GetWindowTextW(hwnd, &mut text) };
    String::from_utf16_lossy(&text[..n.max(0) as usize])
}

/// A Kiln this test started; ended if it is still there when dropped.
struct Started(Child);

impl Drop for Started {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            eprintln!("the test Kiln didn't quit; ending it");
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

#[test]
fn kiln_opens_inside_the_given_area_and_never_takes_the_foreground() {
    let Ok(place) = std::env::var("KILN_TEST_PLACE") else {
        eprintln!("skipped: KILN_TEST_PLACE is not set (x,y,w,h in physical pixels)");
        return;
    };
    let Some(area) = parse(&place) else {
        panic!("KILN_TEST_PLACE is not x,y,w,h: {place:?}");
    };
    // SAFETY: the process asks for physical coordinates, before it makes
    // any window; if it already has an awareness the call fails and the
    // coordinates below are checked as they are.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    assert!(
        !a_kiln_process_runs(),
        "another kiln-app.exe is running. Close it, then run this test again."
    );

    let appdata = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    let temp = local.path().join("Temp");
    std::fs::create_dir_all(&temp).unwrap();

    // SAFETY: a plain query.
    let before = unsafe { GetForegroundWindow() };
    // Only a build that honours the test switches: one without them looks in
    // the real drives' holding folders and saves into the real Documents.
    common::refuse_without_hooks();
    let child = Command::new(env!("CARGO_BIN_EXE_kiln-app"))
        .env("APPDATA", appdata.path())
        .env("LOCALAPPDATA", local.path())
        .env("KILN_TEST_PLACE", &place)
        // Recovery looks only on stand-in drives (none), never in a real
        // drive's .kiln-held.
        .env("KILN_TEST_DRIVES", "")
        // Debug builds leave the start-up entry alone.
        .env("KILN_TEST_RUN_KEY", "")
        .env("TEMP", &temp)
        .env("TMP", &temp)
        .spawn()
        .expect("kiln-app starts");
    let pid = child.id();
    let mut kiln = Started(child);

    // Watched from the start until a few seconds after its window shows.
    let began = Instant::now();
    let mut shown_at = None;
    let mut ever_foreground = None;
    let mut window = None;
    loop {
        // SAFETY: plain queries.
        let now = unsafe { GetForegroundWindow() };
        if !now.is_invalid() && pid_of(now) == pid && ever_foreground.is_none() {
            ever_foreground = Some((now, title_of(now)));
        }
        if window.is_none() {
            window = visible_windows_of(pid)
                .into_iter()
                .find(|&h| class_of(h) == "KilnWindow");
            if window.is_some() {
                shown_at = Some(Instant::now());
            }
        }
        match shown_at {
            Some(at) if at.elapsed() >= WATCH => break,
            None if began.elapsed() > START => break,
            _ => {}
        }
        if matches!(kiln.0.try_wait(), Ok(Some(_))) {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    let window = window.expect("Kiln's window never showed");
    let after = unsafe { GetForegroundWindow() };
    let rects: Vec<(String, RECT)> = visible_windows_of(pid)
        .into_iter()
        .map(|h| (title_of(h), rect_of(h)))
        .collect();
    let main = rect_of(window);

    // Quit as the tray menu would, and wait for it.
    // SAFETY: a posted message to Kiln's own window.
    unsafe {
        let quit = WPARAM(usize::from(CMD_QUIT));
        let _ = PostMessageW(Some(window), WM_COMMAND, quit, LPARAM(0));
    }
    let quit_began = Instant::now();
    while matches!(kiln.0.try_wait(), Ok(None)) && quit_began.elapsed() < QUIT {
        std::thread::sleep(Duration::from_millis(50));
    }
    let quit_cleanly = matches!(kiln.0.try_wait(), Ok(Some(_)));
    drop(kiln);

    assert!(
        ever_foreground.is_none(),
        "Kiln became the foreground window while it started: {ever_foreground:?}"
    );
    assert_eq!(
        after, before,
        "the foreground window changed while Kiln started (it was {before:?}, it is {after:?}). \
         If you switched windows yourself meanwhile, run the test again."
    );
    assert!(
        inside(main, area),
        "Kiln's window {main:?} is not inside the area {area:?}"
    );
    for (title, r) in &rects {
        assert!(
            inside(*r, area),
            "Kiln's window {title:?} {r:?} is not inside the area {area:?}"
        );
    }
    assert!(quit_cleanly, "Kiln didn't quit when asked to");
}

#[test]
fn a_bad_area_is_no_area_in_this_test_too() {
    assert!(parse("1,2,3").is_none());
    assert!(parse("0,0,0,5").is_none());
    assert!(parse("1,2,3,x").is_none());
    let r = parse("3840,0,3840,2120").unwrap();
    assert_eq!((r.left, r.top, r.right, r.bottom), (3840, 0, 7680, 2120));
}
