//! A panic in the window code shows Kiln's crash message, keeps the held
//! results and ends Kiln, where it once ended it at once. The next start
//! offers what it kept.
//!
//! The test starts a Kiln of its own, with a scratch APPDATA and
//! LOCALAPPDATA, and drives it only by posting it window messages and
//! through UI Automation patterns, as `resident.rs` does. Nothing here
//! moves the pointer or sends input, and it doesn't start while another
//! Kiln is running.
//!
//! Only debug builds answer the message that makes the window panic, so
//! the test is in debug builds only.

#![cfg(all(windows, debug_assertions))]

mod common;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use kiln_app::win::crash::{TEST_PANIC_AFTER_QUIT, WM_KILN_TEST_PANIC};
use kiln_app::win::tray::CMD_QUIT;
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
/// How long a posted message may take to show its effect.
const SETTLE: Duration = Duration::from_secs(10);
/// How long a running conversion of a small picture may take.
const CONVERT: Duration = Duration::from_secs(60);
/// The id a message box gives its text.
const MESSAGE_TEXT: i32 = 0xFFFF;
/// What the test panic says.
const PANIC_TEXT: &str = "a test asked the window code to panic";

/// One test Kiln at a time.
static ONE_KILN: Mutex<()> = Mutex::new(());

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

/// A window's text, asked of it with `WM_GETTEXT`.
fn text_of(hwnd: HWND) -> String {
    unsafe {
        let len = SendMessageW(hwnd, WM_GETTEXTLENGTH, None, None).0 as usize;
        let mut buf = vec![0u16; len + 1];
        let got = SendMessageW(
            hwnd,
            WM_GETTEXT,
            Some(WPARAM(buf.len())),
            Some(LPARAM(buf.as_mut_ptr() as isize)),
        );
        String::from_utf16_lossy(&buf[..got.0 as usize])
    }
}

/// Press a message box's one button, as a click on it would: post the box
/// the `WM_COMMAND` the button sends. An OK-only box's OK button has the id
/// `IDCANCEL`, not `IDOK` (so Esc closes it too), and the box doesn't
/// answer a command from a button it doesn't have.
fn press_the_one_button(message: HWND) {
    unsafe {
        let button = FindWindowExW(Some(message), None, w!("Button"), PCWSTR::null())
            .expect("the message has a button");
        assert!(
            FindWindowExW(Some(message), Some(button), w!("Button"), PCWSTR::null()).is_err(),
            "the message has one button only"
        );
        let id = GetDlgCtrlID(button) as u16;
        let clicked = (u32::from(BN_CLICKED as u16) << 16) | u32::from(id);
        PostMessageW(
            Some(message),
            WM_COMMAND,
            WPARAM(clicked as usize),
            LPARAM(button.0 as isize),
        )
        .expect("the message hears its button");
    }
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

/// Every file under `dir`, at any depth.
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(files_under(&p));
        } else {
            out.push(p);
        }
    }
    out
}

/// The scratch APPDATA and LOCALAPPDATA test Kilns start on, with a TEMP
/// inside it.
struct Scratch {
    appdata: TempDir,
    local: TempDir,
    /// Stands in for Documents: Kiln results is made in it, never in the
    /// real one.
    docs: TempDir,
}

/// A Kiln this test started. It is ended when dropped, if it hasn't ended.
struct TestKiln {
    child: Child,
    hwnd: HWND,
    /// Kept until the last Kiln on it has ended.
    scratch: Option<Scratch>,
    local: PathBuf,
    /// Reads what Kiln writes to stderr, so the panic it reports is checked
    /// here rather than shown in a green run.
    stderr: Option<JoinHandle<String>>,
    _one: MutexGuard<'static, ()>,
}

impl TestKiln {
    /// Start Kiln, with `settings` as its settings.toml and `env` set, to
    /// list `file`, and wait for its window. Its temp folder is in the
    /// scratch LOCALAPPDATA too.
    fn start(settings: &str, env: (&str, &str), file: &Path) -> TestKiln {
        let appdata = tempfile::tempdir().unwrap();
        let local = tempfile::tempdir().unwrap();
        let dir = appdata.path().join("Kiln");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("settings.toml"), settings).unwrap();
        std::fs::create_dir_all(local.path().join("Temp")).unwrap();
        let docs = tempfile::tempdir().unwrap();
        TestKiln::launch(
            Scratch {
                appdata,
                local,
                docs,
            },
            Some(env),
            Some(file),
        )
    }

    /// Start another Kiln, listing nothing, on this one's scratch folders,
    /// once this one has ended.
    fn start_again(mut self) -> TestKiln {
        let scratch = self.scratch.take().expect("the scratch folders");
        drop(self);
        TestKiln::launch(scratch, None, None)
    }

    fn launch(scratch: Scratch, env: Option<(&str, &str)>, file: Option<&Path>) -> TestKiln {
        let one = ONE_KILN.lock().unwrap_or_else(|e| e.into_inner());
        assert!(
            !a_kiln_runs(),
            "another Kiln is running. Close it, then run this test again."
        );
        let local = scratch.local.path().to_path_buf();
        let temp = local.join("Temp");
        // Only a build that honours the test switches: one without them looks in
        // the real drives' holding folders and saves into the real Documents.
        common::refuse_without_hooks();
        let mut command = Command::new(env!("CARGO_BIN_EXE_kiln-app"));
        command
            .args(file)
            .env("APPDATA", scratch.appdata.path())
            .env("LOCALAPPDATA", &local)
            // Recovery looks only on stand-in drives (none), never in a
            // real drive's .kiln-held.
            .env("KILN_TEST_DRIVES", "")
            // Debug builds leave the start-up entry alone: none is read
            // or written.
            .env("KILN_TEST_RUN_KEY", "")
            .env("KILN_TEST_DOCUMENTS", scratch.docs.path())
            .env("TEMP", &temp)
            .env("TMP", &temp)
            .stderr(Stdio::piped());
        if let Some((key, value)) = env {
            command.env(key, value);
        }
        let mut child = command.spawn().expect("kiln-app starts");
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
            scratch: Some(scratch),
            local,
            stderr: Some(stderr),
            _one: one,
        };
        assert!(shown, "Kiln's window never showed");
        k
    }

    fn post(&self, msg: u32, wparam: usize) {
        unsafe { PostMessageW(Some(self.hwnd), msg, WPARAM(wparam), LPARAM(0)) }
            .expect("the message is posted");
    }

    fn ended(&mut self) -> Option<ExitStatus> {
        self.child.try_wait().unwrap()
    }

    /// Everything Kiln wrote to stderr, once it has ended.
    fn stderr_text(&mut self) -> String {
        self.stderr
            .take()
            .map(|t| t.join().unwrap_or_default())
            .unwrap_or_default()
    }

    /// Kiln's message, once it shows. Fails if Kiln ends without one.
    fn message_opens(&mut self) -> HWND {
        let pid = self.child.id();
        let mut message = None;
        let mut status = None;
        wait_until(SETTLE, || {
            message = shown_windows_of(pid, w!("#32770")).into_iter().next();
            status = self.ended();
            message.is_some() || status.is_some()
        });
        match (message, status) {
            (Some(m), _) => m,
            (None, Some(s)) => panic!(
                "Kiln ended ({s}) without a word. Its stderr: {}",
                self.stderr_text()
            ),
            (None, None) => panic!("Kiln showed no message"),
        }
    }

    /// Press the message's OK and wait for Kiln to end, with 1, having
    /// reported the panic on stderr.
    fn ok_ends_kiln(&mut self, message: HWND) {
        press_the_one_button(message);
        let mut status = None;
        wait_until(SETTLE, || {
            status = self.ended();
            status.is_some()
        });
        let Some(status) = status else {
            let still = unsafe { IsWindow(Some(message)) }.as_bool();
            panic!("Kiln didn't end once OK was pressed (its message still there: {still})");
        };
        assert_eq!(status.code(), Some(1), "Kiln closed itself: {status}");
        let stderr = self.stderr_text();
        assert!(stderr.contains(PANIC_TEXT), "stderr: {stderr}");
    }

    /// Quit, as the notification area's menu would, and wait for Kiln to
    /// end. With nothing held it asks nothing.
    fn quit(&mut self) {
        self.post(WM_COMMAND, usize::from(CMD_QUIT));
        let mut status = None;
        wait_until(SETTLE, || {
            status = self.ended();
            status.is_some()
        });
        assert!(
            status.is_some_and(|s| s.success()),
            "Quit ends Kiln: {status:?}"
        );
    }

    fn kiln_local(&self) -> PathBuf {
        self.local.join("Kiln")
    }

    /// The results held, without each run folder's lock and its list of
    /// where they came from.
    fn held(&self) -> Vec<PathBuf> {
        files_under(&self.kiln_local().join("held"))
            .into_iter()
            .filter(|f| {
                let name = f.file_name();
                name != Some("lock".as_ref()) && name != Some(kiln_core::SOURCES_NAME.as_ref())
            })
            .collect()
    }

    /// The results held and their sizes, sorted.
    fn held_sizes(&self) -> Vec<(PathBuf, u64)> {
        let mut sizes: Vec<(PathBuf, u64)> = self
            .held()
            .into_iter()
            .map(|f| {
                let size = std::fs::metadata(&f).map(|m| m.len()).unwrap_or(0);
                (f, size)
            })
            .collect();
        sizes.sort();
        sizes
    }

    /// Convert the listed file to its suggested format, through UI
    /// Automation, as Narrator would (the footer's Convert, in a window
    /// wide enough for the side pane, converts at once), and wait for the
    /// result to be held: for a result that can't be saved anywhere, which
    /// Kiln keeps.
    fn convert_and_keep(&self) {
        let ui = Ui::new(self.hwnd);
        let row = ui.wait_for_row().expect("the file is listed");
        ui.select(&row);
        // Pressed again until a result is there: the first press can come
        // before the file's formats are known.
        let ok = wait_until(SETTLE, || {
            if !self.held().is_empty() {
                return true;
            }
            if let Some(e) = ui.find("ConvertButton") {
                ui.invoke(&e);
            }
            std::thread::sleep(Duration::from_millis(600));
            !self.held().is_empty()
        });
        assert!(ok, "Convert never made a result");
        // Whole, not just begun: `job::place` makes an empty placeholder
        // before the result takes its place. So every held file has its
        // data, and two looks in a row find the same.
        let mut last = None;
        let whole = wait_until(CONVERT, || {
            let now = self.held_sizes();
            let full = !now.is_empty() && now.iter().all(|(_, size)| *size > 0);
            let same = last.as_ref() == Some(&now);
            last = Some(now);
            full && same
        });
        assert!(whole, "the result was never held whole: {last:?}");
    }
}

/// A folder nothing new can be made in, as on a locked camera card: its
/// files can still be read. Everyone is denied adding files and folders to
/// it until this is dropped.
struct Locked(PathBuf);

/// Everyone, by its well-known SID, so the deny holds whatever the
/// account's name is.
const EVERYONE: &str = "*S-1-1-0";

impl Locked {
    fn new(folder: &Path) -> Locked {
        let done = Command::new("icacls")
            .arg(folder)
            .args(["/deny", &format!("{EVERYONE}:(WD,AD)")])
            .output()
            .expect("icacls runs");
        assert!(done.status.success(), "icacls: {done:?}");
        // Made so it is undone, even if what follows fails.
        let locked = Locked(folder.to_path_buf());
        assert!(
            std::fs::write(folder.join("probe.txt"), b"").is_err(),
            "the folder still takes new files"
        );
        locked
    }
}

impl Drop for Locked {
    fn drop(&mut self) {
        let _ = Command::new("icacls")
            .arg(&self.0)
            .args(["/remove:d", EVERYONE])
            .output();
    }
}

impl Drop for TestKiln {
    fn drop(&mut self) {
        if self.ended().is_none() {
            eprintln!("the test Kiln didn't end; ending it");
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        // Its mutex goes with it.
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

    fn first(&self, property: UIA_PROPERTY_ID, value: VARIANT) -> Option<IUIAutomationElement> {
        unsafe {
            let cond = self.auto.CreatePropertyCondition(property, &value).ok()?;
            self.root.FindFirst(TreeScope_Descendants, &cond).ok()
        }
    }

    fn find(&self, automation_id: &str) -> Option<IUIAutomationElement> {
        self.first(UIA_AutomationIdPropertyId, VARIANT::from(automation_id))
    }

    fn wait_for_row(&self) -> Option<IUIAutomationElement> {
        let mut row = None;
        wait_until(START, || {
            row = self.first(
                UIA_ControlTypePropertyId,
                VARIANT::from(UIA_ListItemControlTypeId.0),
            );
            row.is_some()
        });
        row
    }

    fn select(&self, e: &IUIAutomationElement) {
        unsafe {
            let p: IUIAutomationSelectionItemPattern = e
                .GetCurrentPatternAs(UIA_SelectionItemPatternId)
                .expect("a row can be selected");
            p.Select().unwrap();
        }
    }

    fn invoke(&self, e: &IUIAutomationElement) {
        unsafe {
            if let Ok(p) = e.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
            {
                let _ = p.Invoke();
            }
        }
    }
}

#[test]
fn a_ui_panic_keeps_the_held_results() {
    let dir = tempfile::tempdir().unwrap();
    let photo = dir.path().join("photo.png");
    std::fs::copy(fixture("photo.png"), &photo).unwrap();
    // First-run help is dismissed (under its old key), and the retired
    // hold setting still loads. The photo's folder refuses the result and
    // there is no Documents, so the result is held.
    let settings = "save_next_to_original = false\nwelcome_shown = true\n";
    let locked = Locked::new(dir.path());
    let mut k = TestKiln::start(settings, ("KILN_TEST_PANIC", "1"), &photo);
    std::fs::remove_dir(k.scratch.as_ref().unwrap().docs.path()).unwrap();
    k.convert_and_keep();
    let held: Vec<(PathBuf, u64)> = k
        .held()
        .into_iter()
        .map(|f| {
            let size = std::fs::metadata(&f).unwrap().len();
            (f, size)
        })
        .collect();

    // The window code panics, with `App` borrowed.
    k.post(WM_KILN_TEST_PANIC, 0);
    let pid = k.child.id();
    let message = k.message_opens();
    let logs = k.kiln_local().join("logs");
    let text = unsafe { GetDlgItem(Some(message), MESSAGE_TEXT) }.expect("the message has text");
    assert_eq!(
        text_of(text),
        format!(
            "Kiln ran into a problem and has to close. Your converted files \
             are kept, and Kiln saves them the next time it starts. A report \
             was saved in {}.",
            logs.display()
        )
    );
    assert!(k.ended().is_none(), "Kiln waits on its message");

    // OK closes it, and Kiln with it.
    k.ok_ends_kiln(message);

    // The panic left its report, and what was held is still held, in this
    // run's folder.
    let crashes: Vec<PathBuf> = files_under(&logs)
        .into_iter()
        .filter(|f| {
            let name = f.file_name().unwrap().to_string_lossy();
            name.starts_with("crash-") && name.ends_with(".txt")
        })
        .collect();
    assert_eq!(crashes.len(), 1, "one report: {crashes:?}");
    let report = std::fs::read_to_string(&crashes[0]).unwrap();
    assert!(report.contains(PANIC_TEXT), "{report}");
    assert!(!held.is_empty());
    for (file, size) in &held {
        let run = file
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy();
        assert!(run.starts_with(&format!("{pid}-")), "{}", file.display());
        assert_eq!(
            std::fs::metadata(file).map(|m| m.len()).ok(),
            Some(*size),
            "{} is still held",
            file.display()
        );
    }
    let run = held[0].0.parent().unwrap().to_path_buf();
    assert!(run.join("lock").is_file(), "the run folder is kept whole");
    // Each result's original folder was noted as it finished.
    let sources = std::fs::read_to_string(run.join(kiln_core::SOURCES_NAME)).unwrap();
    for (file, _) in &held {
        let name = file.file_name().unwrap().to_string_lossy();
        let line = format!("{name}\t{}\n", dir.path().display());
        assert!(sources.contains(&line), "{sources:?} names {line:?}");
    }

    // The next start, with the folder writable again, delivers what was
    // kept beside the photo, asking nothing, clears the run folder, and
    // says so once.
    drop(locked);
    let mut k = k.start_again();
    let ui = Ui::new(k.hwnd);
    let beside = || -> Vec<u64> {
        let mut sizes: Vec<u64> = files_under(dir.path())
            .into_iter()
            .filter(|f| f != &photo)
            .map(|f| std::fs::metadata(f).unwrap().len())
            .collect();
        sizes.sort();
        sizes
    };
    let mut sizes: Vec<u64> = held.iter().map(|(_, size)| *size).collect();
    sizes.sort();
    assert!(
        wait_until(START, || !run.exists() && beside() == sizes),
        "saved beside the photo: {:?}",
        files_under(dir.path())
    );
    assert!(ui.find("Ask").is_none(), "nothing is asked");
    let said = wait_until(SETTLE, || {
        ui.find("TopInfo")
            .and_then(|e| unsafe { e.CurrentName() }.ok())
            .is_some_and(|n| {
                n.to_string()
                    .starts_with("Saved 1 converted file from last time")
            })
    });
    assert!(said, "the start says what it saved");
    assert_eq!(
        std::fs::read(&photo).unwrap(),
        std::fs::read(fixture("photo.png")).unwrap(),
        "the photo is untouched"
    );
    k.quit();
    drop(k);
    drop(dir);
}

#[test]
fn a_panic_after_the_quit_was_asked_still_waits_on_its_message() {
    let dir = tempfile::tempdir().unwrap();
    let photo = dir.path().join("photo.png");
    std::fs::copy(fixture("photo.png"), &photo).unwrap();
    let settings = "welcome_shown = true\n";
    let mut k = TestKiln::start(settings, ("KILN_TEST_PANIC", "1"), &photo);

    // As a panic in WM_DESTROY, which has already asked the message loop
    // to end: that WM_QUIT must not close the message at once.
    k.post(WM_KILN_TEST_PANIC, TEST_PANIC_AFTER_QUIT);
    let message = k.message_opens();
    std::thread::sleep(Duration::from_secs(1));
    assert!(
        unsafe { IsWindow(Some(message)) }.as_bool(),
        "the message stays until OK"
    );
    assert!(k.ended().is_none(), "Kiln waits on its message");
    k.ok_ends_kiln(message);
    drop(k);
    drop(dir);
}
