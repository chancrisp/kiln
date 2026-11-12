//! The footer row of More's list, "Options for JPG…", does something visible
//! whenever it is offered (owner report: it did nothing).
//!
//! The test starts a Kiln of its own, with a scratch APPDATA and
//! LOCALAPPDATA, and drives it only through UI Automation patterns (Invoke,
//! SetFocus and Window's Close). It sends no input and never moves the
//! pointer or takes focus. Set `KILN_TEST_PLACE` (`x,y,w,h`) to keep its
//! window on one monitor. No test starts while another Kiln is running: Kiln
//! is one window per session, so a second would only hand over to the first.
//!
//! `KILN_TEST_EXE` names the Kiln to start instead of the one built with the
//! tests; it must carry the test hooks.

#![cfg(windows)]

mod common;

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use tempfile::TempDir;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// A debug build can take a while to show its first frame.
const START: Duration = Duration::from_secs(60);
const SETTLE: Duration = Duration::from_secs(10);
const QUIT: Duration = Duration::from_secs(3);

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
        if pid_of(h) == pid && unsafe { IsWindowVisible(h) }.as_bool() {
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

/// The Kiln to start: `KILN_TEST_EXE`, or the one built with the tests.
fn exe() -> PathBuf {
    std::env::var_os("KILN_TEST_EXE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_kiln-app")))
}

struct TestKiln {
    child: Child,
    hwnd: HWND,
    _appdata: TempDir,
    _local: TempDir,
    _docs: TempDir,
}

impl TestKiln {
    fn start(files: &[PathBuf]) -> TestKiln {
        assert!(
            !a_kiln_runs(),
            "another Kiln is running. Close it, then run this test again."
        );
        let (appdata, local, docs) = (
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
        );
        let temp = local.path().join("Temp");
        std::fs::create_dir_all(&temp).unwrap();
        let exe = exe();
        common::refuse_unless_hooked(&exe);
        let child = Command::new(&exe)
            .args(files)
            .env("APPDATA", appdata.path())
            .env("LOCALAPPDATA", local.path())
            .env("KILN_TEST_DRIVES", "")
            .env("KILN_TEST_RUN_KEY", "")
            .env("KILN_TEST_DOCUMENTS", docs.path())
            .env("TEMP", &temp)
            .env("TMP", &temp)
            .spawn()
            .expect("kiln-app starts");
        let pid = child.id();
        let mut hwnd = None;
        let shown = wait_until(START, || {
            hwnd = window_of(pid);
            hwnd.is_some()
        });
        let k = TestKiln {
            child,
            hwnd: hwnd.unwrap_or_default(),
            _appdata: appdata,
            _local: local,
            _docs: docs,
        };
        assert!(shown, "Kiln's window never showed");
        k
    }
}

impl Drop for TestKiln {
    fn drop(&mut self) {
        unsafe {
            let quit = WPARAM(usize::from(kiln_app::win::tray::CMD_QUIT));
            let _ = PostMessageW(Some(self.hwnd), WM_COMMAND, quit, LPARAM(0));
        }
        let child = &mut self.child;
        wait_until(QUIT, || !matches!(child.try_wait(), Ok(None)));
        if matches!(self.child.try_wait(), Ok(None)) {
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

    fn wait_for(&self, automation_id: &str, limit: Duration) -> Option<IUIAutomationElement> {
        let mut found = None;
        wait_until(limit, || {
            found = self.find(automation_id);
            found.is_some()
        });
        found
    }

    fn wait_gone(&self, automation_id: &str, limit: Duration) -> bool {
        wait_until(limit, || self.find(automation_id).is_none())
    }

    /// Every element in the window with its automation id and name.
    fn all(&self) -> Vec<(String, String, IUIAutomationElement)> {
        let mut out = vec![];
        unsafe {
            let Ok(cond) = self.auto.CreateTrueCondition() else {
                return out;
            };
            let Ok(found) = self.root.FindAll(TreeScope_Descendants, &cond) else {
                return out;
            };
            for i in 0..found.Length().unwrap_or(0) {
                if let Ok(e) = found.GetElement(i) {
                    let id = e.CurrentAutomationId().map(|s| s.to_string());
                    let name = e.CurrentName().map(|s| s.to_string());
                    out.push((id.unwrap_or_default(), name.unwrap_or_default(), e));
                }
            }
        }
        out
    }

    fn first_named(&self, prefix: &str) -> Option<IUIAutomationElement> {
        self.all()
            .into_iter()
            .find(|(_, name, _)| name.starts_with(prefix))
            .map(|(.., e)| e)
    }

    fn invoke(&self, e: &IUIAutomationElement) {
        unsafe {
            if let Ok(p) = e.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
            {
                let _ = p.Invoke();
            }
        }
    }

    fn select(&self, e: &IUIAutomationElement) {
        unsafe {
            let p: IUIAutomationSelectionItemPattern = e
                .GetCurrentPatternAs(UIA_SelectionItemPatternId)
                .expect("a row can be selected");
            p.Select().unwrap();
        }
    }

    /// The dialog's Window pattern Close: Cancel.
    fn close(&self, e: &IUIAutomationElement) {
        unsafe {
            if let Ok(p) = e.GetCurrentPatternAs::<IUIAutomationWindowPattern>(UIA_WindowPatternId)
            {
                let _ = p.Close();
            }
        }
    }
}

fn name_of(e: &IUIAutomationElement) -> String {
    unsafe { e.CurrentName().map(|s| s.to_string()).unwrap_or_default() }
}

/// How More's list is opened.
#[derive(Clone, Copy, Debug)]
enum Way {
    /// The side pane's "More formats and tools".
    PaneMore,
    /// The footer's Convert, in a window too narrow for the pane.
    FooterConvert,
}

/// Open More's list on the selected row by `way`.
fn open_list(ui: &Ui, way: Way) -> bool {
    let list = |ui: &Ui| ui.find("Flyout").is_some();
    let ok = wait_until(SETTLE, || {
        if list(ui) {
            return true;
        }
        let button = match way {
            Way::PaneMore => ui.first_named("More formats for"),
            Way::FooterConvert => ui.find("ConvertButton"),
        };
        if let Some(b) = button {
            ui.invoke(&b);
        }
        std::thread::sleep(Duration::from_millis(500));
        list(ui)
    });
    if !ok {
        // What the window shows instead, for the report.
        let seen: Vec<String> = ui
            .all()
            .into_iter()
            .filter(|(id, ..)| {
                id.starts_with("WheelItem(Inner(") || id == "ConvertButton" || id == "Flyout"
            })
            .map(|(id, name, e)| {
                let on = unsafe { e.CurrentIsEnabled().map(|b| b.as_bool()).unwrap_or(false) };
                format!("{id}={name:?}{}", if on { "" } else { " (disabled)" })
            })
            .collect();
        let more = ui.first_named("More formats for").map(|e| name_of(&e));
        let every: Vec<String> = ui
            .all()
            .into_iter()
            .filter(|(id, name, _)| !id.is_empty() || !name.is_empty())
            .map(|(id, name, _)| format!("{id}={name:?}"))
            .collect();
        eprintln!(
            "{way:?}: More's list never opened; pane link {more:?}; seen {seen:?}; every {every:?}"
        );
    }
    ok
}

/// Narrow the window until the pane folds, keeping its corner: under 760
/// DIPs the footer's Convert opens the list.
fn narrow(hwnd: HWND) {
    unsafe {
        let mut r = RECT::default();
        GetWindowRect(hwnd, &mut r).unwrap();
        // 700 pixels is at most 700 DIPs.
        let width = 700;
        SetWindowPos(
            hwnd,
            None,
            r.left,
            r.top,
            width,
            r.bottom - r.top,
            SWP_NOZORDER | SWP_NOACTIVATE,
        )
        .unwrap();
    }
}

#[test]
fn the_options_row_opens_the_options_dialog_whenever_it_is_offered() {
    let dir = tempfile::tempdir().unwrap();
    let photo = dir.path().join("photo.png");
    std::fs::copy(fixture("photo.png"), &photo).unwrap();
    let k = TestKiln::start(std::slice::from_ref(&photo));
    let ui = Ui::new(k.hwnd);
    let mut report = vec![];
    // A file given at start opens the ring a moment after the window
    // shows. Its More is the first way in, with nothing lit; once the
    // dialog it opens is closed, the ring is gone and the pane is steady.
    let first = wheel_more_on(&ui, None);
    let opened = first.ends_with("dialog=true");
    report.push(first);
    if !opened {
        // The ring is still up: light a format and try again, so the
        // ring goes and the pane can be reached.
        report.push(wheel_more_on(&ui, Some(1)));
    }
    for way in [Way::PaneMore, Way::FooterConvert] {
        if matches!(way, Way::FooterConvert) {
            narrow(k.hwnd);
        }
        // Row's own Select, then the list.
        let Some(row) = wait_row(&ui) else {
            report.push(format!("{way:?}: the file's row never showed"));
            continue;
        };
        ui.select(&row);
        // The line to light each time: none, then each of the first and
        // last few lines of the list.
        let mut tries: Vec<Option<String>> = vec![None];
        if !open_list(&ui, way) {
            report.push(format!("{way:?}: More's list never opened"));
            continue;
        }
        let lines: Vec<String> = ui
            .all()
            .into_iter()
            .filter(|(id, ..)| id.starts_with("WheelItem(Flyout("))
            .map(|(id, ..)| id)
            .collect();
        assert!(!lines.is_empty(), "{way:?}: the list has lines");
        let n = lines.len();
        for (i, id) in lines.iter().enumerate() {
            if i < 3 || i + 3 >= n {
                tries.push(Some(id.clone()));
            }
        }
        // Close the list again with Escape-equivalent: the Window pattern
        // of the wheel's menu is not there, so each try starts from the
        // list as it is, or reopens it.
        for lit in tries {
            if !open_list(&ui, way) {
                report.push(format!("{way:?} lit={lit:?}: More's list never opened"));
                break;
            }
            if let Some(id) = &lit {
                let line = ui.find(id).expect("the line");
                unsafe { line.SetFocus().unwrap() };
            }
            let Some(footer) = ui.wait_for("WheelItem(Options)", SETTLE) else {
                report.push(format!("{way:?} lit={lit:?}: no footer row"));
                continue;
            };
            let name = name_of(&footer);
            let enabled = unsafe { footer.CurrentIsEnabled().unwrap().as_bool() };
            // A disabled row refuses Invoke: what a screen reader meets.
            let invoked = unsafe {
                footer
                    .GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
                    .and_then(|p| p.Invoke())
                    .is_ok()
            };
            let sheet = ui.wait_for(
                "Sheet",
                if invoked {
                    SETTLE
                } else {
                    Duration::from_secs(1)
                },
            );
            report.push(format!(
                "{way:?} lit={lit:?} row={name:?} enabled={enabled} invoked={invoked} dialog={}",
                sheet.is_some()
            ));
            if let Some(s) = sheet {
                ui.close(&s);
                assert!(ui.wait_gone("Sheet", SETTLE), "the dialog closes");
            }
        }
    }
    drop(ui);
    drop(k);
    // The wheel's own More: the ring a file given at start opens. Each try
    // needs a new wheel, so a new Kiln: nothing lit, the first line, the
    // last line.
    for pick in [Some(0usize), Some(usize::MAX)] {
        report.push(wheel_more_try(&photo, pick));
    }
    eprintln!("{}", report.join("\n"));
    assert!(
        report
            .iter()
            .all(|l| l.contains("row=\"Options for ") && l.ends_with("dialog=true")),
        "the options row opened nothing: {report:#?}"
    );
}

/// One try on the ring's More: start Kiln with `photo`, open More from the
/// ring, light line `pick` of the list (`usize::MAX`: the last; `None`:
/// nothing), then invoke the footer row. The report line, as the other
/// ways write it.
fn wheel_more_try(photo: &Path, pick: Option<usize>) -> String {
    let k = TestKiln::start(&[photo.to_path_buf()]);
    let ui = Ui::new(k.hwnd);
    wheel_more_on(&ui, pick)
}

/// The try on the ring a file given at start opened, in the Kiln `ui` is on.
fn wheel_more_on(ui: &Ui, pick: Option<usize>) -> String {
    if ui.wait_for("WheelItem(Inner(0))", START).is_none() {
        return format!("WheelMore pick={pick:?}: the ring never opened");
    }
    let more = ui
        .all()
        .into_iter()
        .find(|(id, name, _)| id.starts_with("WheelItem(Inner(") && name == "More")
        .map(|(.., e)| e);
    let Some(more) = more else {
        return format!("WheelMore pick={pick:?}: the ring has no More");
    };
    let opened = wait_until(SETTLE, || {
        if ui.find("Flyout").is_some() {
            return true;
        }
        ui.invoke(&more);
        std::thread::sleep(Duration::from_millis(500));
        ui.find("Flyout").is_some()
    });
    if !opened {
        return format!("WheelMore pick={pick:?}: More's list never opened");
    }
    let lines: Vec<String> = ui
        .all()
        .into_iter()
        .filter(|(id, ..)| id.starts_with("WheelItem(Flyout("))
        .map(|(id, ..)| id)
        .collect();
    let lit = pick.and_then(|p| lines.get(p.min(lines.len().saturating_sub(1))).cloned());
    if let Some(id) = &lit {
        let line = ui.find(id).expect("the line");
        unsafe { line.SetFocus().unwrap() };
    }
    let Some(footer) = ui.wait_for("WheelItem(Options)", SETTLE) else {
        return format!("WheelMore lit={lit:?}: no footer row");
    };
    let name = name_of(&footer);
    let enabled = unsafe { footer.CurrentIsEnabled().unwrap().as_bool() };
    let invoked = unsafe {
        footer
            .GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
            .and_then(|p| p.Invoke())
            .is_ok()
    };
    let sheet = ui.wait_for(
        "Sheet",
        if invoked {
            SETTLE
        } else {
            Duration::from_secs(1)
        },
    );
    if let Some(s) = &sheet {
        ui.close(s);
        ui.wait_gone("Sheet", SETTLE);
    }
    format!(
        "WheelMore lit={lit:?} row={name:?} enabled={enabled} invoked={invoked} dialog={}",
        sheet.is_some()
    )
}

/// What Kiln shows on its own after a start with a file, second by second,
/// with nothing done to it.
#[test]
#[ignore = "a probe: run it by name"]
fn probe_start() {
    let dir = tempfile::tempdir().unwrap();
    let photo = dir.path().join("photo.png");
    std::fs::copy(fixture("photo.png"), &photo).unwrap();
    let k = TestKiln::start(std::slice::from_ref(&photo));
    let ui = Ui::new(k.hwnd);
    for s in 0..8 {
        let seen: Vec<String> = ui
            .all()
            .into_iter()
            .filter(|(id, ..)| {
                id.starts_with("Row.")
                    || id.starts_with("WheelItem(Inner(")
                    || id == "Flyout"
                    || id.starts_with("InspectorControl")
            })
            .map(|(id, name, _)| format!("{id}={name:?}"))
            .collect();
        eprintln!("t={s}s {seen:?}");
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn wait_row(ui: &Ui) -> Option<IUIAutomationElement> {
    let mut row = None;
    wait_until(START, || {
        row = ui
            .all()
            .into_iter()
            .find_map(|(id, _, e)| id.starts_with("Row.").then_some(e));
        row.is_some()
    });
    row
}
