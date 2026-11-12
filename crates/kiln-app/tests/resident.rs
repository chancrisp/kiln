//! Kiln stays in the notification area while the drag-wheel is on. Its
//! icon is there only while the window is hidden.
//!
//! Each test starts a Kiln of its own, with a scratch APPDATA and
//! LOCALAPPDATA, and drives it only by posting it window messages and
//! through UI Automation patterns. Nothing here moves the pointer or sends
//! input, and no test starts while another Kiln is running: Kiln is one
//! window per session, so a second would only hand over to the first.
//!
//! The tests that need the notification area say "skipped: no taskbar"
//! and return where there is none.

#![cfg(windows)]

mod common;

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use kiln_app::win::tray::{CMD_DRAG_WHEEL, CMD_OPEN, CMD_QUIT, TRAY_ID, WM_KILN_TRAY};
use tempfile::TempDir;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::Shell::{Shell_NotifyIconW, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW};
use windows::Win32::UI::WindowsAndMessaging::*;

/// One test Kiln at a time.
static ONE_KILN: Mutex<()> = Mutex::new(());

/// A debug build can take a while to show its first frame.
const START: Duration = Duration::from_secs(60);
/// How long a posted message may take to show its effect.
const SETTLE: Duration = Duration::from_secs(10);
/// How long a running conversion of a small picture may take.
const CONVERT: Duration = Duration::from_secs(60);

/// A left click on the icon, as NOTIFYICON_VERSION_4 reports it.
const NIN_SELECT: u32 = 0x400;
/// How long a test Kiln gets to quit by itself before it is ended.
const QUIT: Duration = Duration::from_secs(3);
/// The drag-wheel on, past its one-time turn-off: Kiln stays in the
/// notification area when its window closes. A new install has it off.
const ON: &str = "drag_wheel = true\ndrag_wheel_reset = true\n";

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

fn icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ID,
        ..Default::default()
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

/// A Kiln this test started. It is ended when dropped.
struct TestKiln {
    child: Child,
    hwnd: HWND,
    appdata: TempDir,
    local: TempDir,
    /// Stands in for Documents: Kiln results is made in it, never in the
    /// real one.
    docs: TempDir,
    _one: MutexGuard<'static, ()>,
}

impl TestKiln {
    /// Start Kiln with `settings` as its settings.toml, if given, and
    /// `files` to list, and wait for its window. Its temp folder is in the
    /// scratch LOCALAPPDATA too, and its Documents is a scratch folder.
    fn start(settings: Option<&str>, files: &[PathBuf]) -> TestKiln {
        let docs = tempfile::tempdir().unwrap();
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
        let temp = local.path().join("Temp");
        std::fs::create_dir_all(&temp).unwrap();
        // Only a build that honours the test switches: one without them looks in
        // the real drives' holding folders and saves into the real Documents.
        common::refuse_without_hooks();
        let child = Command::new(env!("CARGO_BIN_EXE_kiln-app"))
            .args(files)
            .env("APPDATA", appdata.path())
            .env("LOCALAPPDATA", local.path())
            // Recovery looks only on stand-in drives (none), never in a
            // real drive's .kiln-held.
            .env("KILN_TEST_DRIVES", "")
            // Debug builds leave the start-up entry alone: none is read
            // or written.
            .env("KILN_TEST_RUN_KEY", "")
            .env("KILN_TEST_DOCUMENTS", docs.path())
            .env("TEMP", &temp)
            .env("TMP", &temp)
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
            local,
            docs,
            _one: one,
        };
        assert!(shown, "Kiln's window never showed");
        assert!(k.running());
        k
    }

    fn command(&self) -> Command {
        let temp = self.local.path().join("Temp");
        // Only a build that honours the test switches: one without them looks in
        // the real drives' holding folders and saves into the real Documents.
        common::refuse_without_hooks();
        let mut c = Command::new(env!("CARGO_BIN_EXE_kiln-app"));
        c.env("APPDATA", self.appdata.path())
            .env("LOCALAPPDATA", self.local.path())
            // Recovery looks only on stand-in drives (none), never in a
            // real drive's .kiln-held.
            .env("KILN_TEST_DRIVES", "")
            // Debug builds leave the start-up entry alone: none is read
            // or written.
            .env("KILN_TEST_RUN_KEY", "")
            .env("KILN_TEST_DOCUMENTS", self.docs.path())
            .env("TEMP", &temp)
            .env("TMP", &temp);
        c
    }

    fn post(&self, msg: u32, wparam: usize, lparam: isize) {
        unsafe { PostMessageW(Some(self.hwnd), msg, WPARAM(wparam), LPARAM(lparam)) }
            .expect("the message is posted");
    }

    fn close(&self) {
        self.post(WM_CLOSE, 0, 0);
    }

    /// A tray menu command, as the menu would give it.
    fn tray_command(&self, id: u16) {
        self.post(WM_COMMAND, usize::from(id), 0);
    }

    /// A left click on the tray icon.
    fn click_icon(&self) {
        self.post(WM_KILN_TRAY, 0, ((TRAY_ID << 16) | NIN_SELECT) as isize);
    }

    fn visible(&self) -> bool {
        unsafe { IsWindowVisible(self.hwnd) }.as_bool()
    }

    fn hides(&self) -> bool {
        wait_until(SETTLE, || !self.visible())
    }

    fn shows(&self) -> bool {
        wait_until(SETTLE, || self.visible())
    }

    fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Whether it is still running a second from now.
    fn lives_on(&mut self) -> bool {
        std::thread::sleep(Duration::from_secs(1));
        self.running()
    }

    fn ends(&mut self) -> bool {
        wait_until(SETTLE, || !matches!(self.child.try_wait(), Ok(None)))
    }

    /// Whether Explorer has Kiln's tray icon: changing nothing on it
    /// succeeds only if it is there.
    fn icon_there(&self) -> bool {
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &icon_data(self.hwnd)) }.as_bool()
    }

    fn icon_comes(&self) -> bool {
        wait_until(SETTLE, || self.icon_there())
    }

    fn icon_goes(&self) -> bool {
        wait_until(SETTLE, || !self.icon_there())
    }

    fn settings_file(&self) -> PathBuf {
        self.appdata.path().join("Kiln").join("settings.toml")
    }

    fn settings_text(&self) -> String {
        std::fs::read_to_string(self.settings_file()).unwrap_or_default()
    }

    fn held(&self) -> Vec<PathBuf> {
        files_under(&self.local.path().join("Kiln").join("held"))
    }

    /// The results held, without each run folder's lock and its list of
    /// where they came from, with their sizes, sorted.
    fn results(&self) -> Vec<(PathBuf, u64)> {
        let mut sizes: Vec<(PathBuf, u64)> = self
            .held()
            .into_iter()
            .filter(|f| {
                let name = f.file_name();
                name != Some("lock".as_ref()) && name != Some(kiln_core::SOURCES_NAME.as_ref())
            })
            .map(|f| {
                let size = std::fs::metadata(&f).map(|m| m.len()).unwrap_or(0);
                (f, size)
            })
            .collect();
        sizes.sort();
        sizes
    }

    /// Convert the one listed file to its suggested format, through UI
    /// Automation, as Narrator would, and wait for the result to be held
    /// whole: for a result that can't be saved anywhere, which Kiln keeps.
    fn convert_and_keep(&self) {
        self.start_converting();
        // Whole, not just begun: `job::place` makes an empty placeholder
        // before the result takes its place. So every held file has its
        // data, and two looks in a row find the same.
        let mut last = None;
        let whole = wait_until(CONVERT, || {
            let now = self.results();
            let full = !now.is_empty() && now.iter().all(|(_, size)| *size > 0);
            let same = last.as_ref() == Some(&now);
            last = Some(now);
            full && same
        });
        assert!(whole, "the result was never held whole: {last:?}");
    }

    /// Convert the one listed file to its suggested format, through UI
    /// Automation, as Narrator would: the footer's Convert, in a window wide
    /// enough for the side pane, converts at once.
    fn start_converting(&self) -> Ui {
        let ui = Ui::new(self.hwnd);
        let row = ui.wait_for_row().expect("the file is listed");
        ui.select(&row);
        // Pressed again until the job has started: the first press can come
        // before the file's formats are known.
        let started = || {
            ui.row_text().is_some_and(|t| {
                [
                    "Converting",
                    "Saving",
                    "Saved",
                    "Couldn't save",
                    "Kiln can't",
                ]
                .iter()
                .any(|w| t.contains(w))
            })
        };
        let ok = wait_until(SETTLE, || {
            if started() {
                return true;
            }
            if let Some(e) = ui.find("ConvertButton") {
                ui.invoke(&e);
            }
            std::thread::sleep(Duration::from_millis(600));
            started()
        });
        assert!(ok, "Convert never started a job: {:?}", ui.row_text());
        ui
    }

    /// The results saved in Documents\Kiln results, with their sizes.
    fn in_kiln_results(&self) -> Vec<(PathBuf, u64)> {
        others_in(&self.docs.path().join("Kiln results"), Path::new(""))
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

/// Each file in `dir` but `except`, with its size, sorted.
fn others_in(dir: &Path, except: &Path) -> Vec<(PathBuf, u64)> {
    let mut found: Vec<(PathBuf, u64)> = files_under(dir)
        .into_iter()
        .filter(|f| f != except)
        .map(|f| {
            let size = std::fs::metadata(&f).map(|m| m.len()).unwrap_or(0);
            (f, size)
        })
        .collect();
    found.sort();
    found
}

impl Drop for TestKiln {
    fn drop(&mut self) {
        // Quit as the menu would, so its icon goes with it. One still
        // converting asks instead, and is ended.
        if self.running() {
            unsafe {
                let quit = WPARAM(usize::from(CMD_QUIT));
                let _ = PostMessageW(Some(self.hwnd), WM_COMMAND, quit, LPARAM(0));
            }
            let child = &mut self.child;
            wait_until(QUIT, || !matches!(child.try_wait(), Ok(None)));
        }
        if self.running() {
            eprintln!("the test Kiln didn't quit; ending it");
            let _ = self.child.kill();
            // An ended Kiln can't take its icon away: it is taken here, so
            // no icon is left behind in the notification area.
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &icon_data(self.hwnd));
            }
        }
        let _ = self.child.wait();
        // Its mutex goes with it; the next test checks for it.
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

    fn wait_for(&self, automation_id: &str) -> Option<IUIAutomationElement> {
        let mut found = None;
        wait_until(START, || {
            found = self.find(automation_id);
            found.is_some()
        });
        found
    }

    /// The first row's name, as a screen reader reads it: the file's name,
    /// then its line.
    fn row_text(&self) -> Option<String> {
        let row = self.first(
            UIA_ControlTypePropertyId,
            VARIANT::from(UIA_ListItemControlTypeId.0),
        )?;
        unsafe { row.CurrentName().ok().map(|n| n.to_string()) }
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
fn a_new_install_saves_next_to_the_original() {
    let dir = tempfile::tempdir().unwrap();
    let photo = dir.path().join("photo.png");
    std::fs::copy(fixture("photo.png"), &photo).unwrap();
    // No settings file, as a new install has none.
    let k = TestKiln::start(None, std::slice::from_ref(&photo));
    let ui = k.start_converting();
    // The result lands next to the photo, whole (the delivery claims its
    // name with an empty file first), and nothing is left held.
    let mut last = None;
    let saved = wait_until(CONVERT, || {
        let now = others_in(dir.path(), &photo);
        let whole = now.len() == 1 && now[0].1 > 0;
        let same = last.as_ref() == Some(&now);
        last = Some(now);
        whole && same && k.results().is_empty()
    });
    assert!(
        saved,
        "the result was never saved next to the photo: {last:?}, held {:?}",
        k.results()
    );
    let result = last.unwrap()[0].0.clone();
    assert_eq!(result.file_stem(), Some("photo".as_ref()));
    // The row names the result it saved and how its size changed, and
    // there is nothing to save.
    let line = format!(
        "photo.png. Saved as {}. ",
        result.file_name().unwrap().to_string_lossy()
    );
    assert!(
        wait_until(SETTLE, || ui
            .row_text()
            .is_some_and(|t| t.starts_with(&line) && t.ends_with("smaller"))),
        "the row reads {:?}",
        ui.row_text()
    );
    // Nothing went to Documents\Kiln results, and the retired setting is
    // never written.
    assert!(k.in_kiln_results().is_empty());
    std::thread::sleep(SETTLE);
    assert!(
        !k.settings_text().contains("save_next_to_original"),
        "{}",
        k.settings_text()
    );
    drop(k);
    drop(dir);
}

#[test]
fn a_read_only_folder_sends_the_result_to_kiln_results() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("Card");
    std::fs::create_dir(&card).unwrap();
    let photo = card.join("photo.png");
    std::fs::copy(fixture("photo.png"), &photo).unwrap();
    let locked = Locked::new(&card);
    let k = TestKiln::start(None, std::slice::from_ref(&photo));
    let ui = k.start_converting();
    // It can't be saved next to the photo, so it goes to Documents\Kiln
    // results, whole, and the row says so.
    let line = "photo.png. Saved in Documents\\Kiln results. Kiln can't save in Card.";
    assert!(
        wait_until(CONVERT, || ui.row_text().as_deref() == Some(line)),
        "the row reads {:?}",
        ui.row_text()
    );
    let saved = k.in_kiln_results();
    assert_eq!(saved.len(), 1, "one result in Kiln results: {saved:?}");
    assert!(saved[0].1 > 0);
    assert_eq!(saved[0].0.file_stem(), Some("photo".as_ref()));
    assert!(k.results().is_empty(), "nothing is left held");
    assert!(
        others_in(&card, &photo).is_empty(),
        "nothing was left in the folder"
    );
    drop(k);
    drop(locked);
    drop(dir);
}

#[test]
fn a_result_that_cant_be_saved_anywhere_is_kept_and_a_close_asks_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("Card");
    std::fs::create_dir(&card).unwrap();
    let photo = card.join("photo.png");
    std::fs::copy(fixture("photo.png"), &photo).unwrap();
    let locked = Locked::new(&card);
    let mut k = TestKiln::start(None, std::slice::from_ref(&photo));
    // No Documents at all: Kiln results can't be made.
    std::fs::remove_dir(k.docs.path()).unwrap();
    k.convert_and_keep();
    let ui = Ui::new(k.hwnd);
    let line = "photo.png. Couldn't save this. Kiln tries again the next time it starts.";
    assert!(
        wait_until(SETTLE, || ui.row_text().as_deref() == Some(line)),
        "the row reads {:?}",
        ui.row_text()
    );
    let held = k.results();
    assert_eq!(held.len(), 1, "the result is still held: {held:?}");
    assert!(!k.docs.path().exists(), "Documents is never made");
    // Closing asks nothing about it (the drag-wheel is off): Kiln quits,
    // and the result stays, whole, for the next start to deliver.
    k.close();
    assert!(k.ends(), "Kiln quits without asking");
    assert_eq!(k.results(), held, "the result is kept");
    drop(k);
    drop(locked);
    drop(dir);
}

#[test]
fn a_fresh_kiln_shows_first_run_help_until_it_is_dismissed() {
    let k = TestKiln::start(None, &[]);
    let ui = Ui::new(k.hwnd);
    // A fresh settings file: the empty window holds the card, which
    // covers nothing.
    let dismiss = ui
        .wait_for("FirstRunDismiss")
        .expect("the first-run card shows");
    assert!(ui.find("FirstRunCard").is_some());
    assert!(ui.find("HelpButton").is_some(), "the card covers nothing");
    assert!(ui.find("AddFiles").is_some());
    assert!(!k.settings_text().contains("first_run_dismissed"));
    // Dismiss takes it away, and that is remembered.
    ui.invoke(&dismiss);
    assert!(
        wait_until(SETTLE, || k
            .settings_text()
            .contains("first_run_dismissed = true")),
        "dismissing the card is remembered"
    );
    assert!(wait_until(SETTLE, || ui.find("FirstRunCard").is_none()));

    // Help's Show first-run help brings it back, and that is saved too.
    ui.invoke(&ui.wait_for("HelpButton").expect("the window's tree"));
    ui.invoke(
        &ui.wait_for("HelpGuide")
            .expect("Help has Show first-run help"),
    );
    let again = ui.wait_for("FirstRunDismiss").expect("the card is back");
    assert!(ui.find("HelpClose").is_none(), "Help closed for it");
    assert!(
        wait_until(SETTLE, || !k
            .settings_text()
            .contains("first_run_dismissed")),
        "showing it again is remembered"
    );
    ui.invoke(&again);
    assert!(wait_until(SETTLE, || k
        .settings_text()
        .contains("first_run_dismissed = true")));
    drop(ui);
    drop(k);

    // A Kiln whose settings say the welcome card was closed, the old key,
    // opens without the card.
    let k = TestKiln::start(
        Some(
            "welcome_shown = true
",
        ),
        &[],
    );
    let ui = Ui::new(k.hwnd);
    assert!(ui.wait_for("AddFiles").is_some());
    std::thread::sleep(Duration::from_secs(1));
    assert!(ui.find("FirstRunCard").is_none(), "dismissed is dismissed");
}

#[test]
fn close_hides_while_the_drag_wheel_is_on() {
    if !have_taskbar() {
        eprintln!("skipped: no taskbar");
        return;
    }
    let mut k = TestKiln::start(Some(ON), &[]);
    // While the window is open, no icon.
    std::thread::sleep(Duration::from_millis(500));
    assert!(!k.icon_there(), "no icon while the window is open");

    k.close();
    // Hidden, the window has no taskbar button either.
    assert!(k.hides(), "closing hides the window");
    assert!(k.lives_on(), "Kiln is still running");
    assert!(k.icon_comes(), "the icon comes as the window hides");

    // Quit Kiln, with nothing held, ends it at once, and its icon with it.
    k.tray_command(CMD_QUIT);
    assert!(k.ends(), "Quit Kiln ends Kiln");
    assert!(!k.icon_there());
}

#[test]
fn close_quits_while_it_is_off() {
    let mut k = TestKiln::start(Some("drag_wheel = false\n"), &[]);
    if have_taskbar() {
        // Nothing in the notification area while it's off.
        std::thread::sleep(Duration::from_millis(500));
        assert!(!k.icon_there());
    }
    k.close();
    assert!(k.ends(), "closing quits, as it always did");
}

#[test]
fn the_first_hide_tells_once() {
    if !have_taskbar() {
        eprintln!("skipped: no taskbar");
        return;
    }
    let k = TestKiln::start(Some(ON), &[]);
    assert!(!k.settings_text().contains("told_about_tray"));

    k.close();
    assert!(k.hides());
    // Told, and kept: the next time, and the next run, say nothing.
    assert!(
        wait_until(SETTLE, || k
            .settings_text()
            .contains("told_about_tray = true")),
        "the first hide tells, and remembers it did"
    );
    let told = std::fs::metadata(k.settings_file()).unwrap().modified();
    let told: SystemTime = told.unwrap();

    k.click_icon();
    assert!(k.shows());
    k.close();
    assert!(k.hides());
    std::thread::sleep(Duration::from_secs(1));
    let after = std::fs::metadata(k.settings_file())
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(after, told, "the second hide tells nothing");
}

#[test]
fn tray_open_shows_the_window() {
    if !have_taskbar() {
        eprintln!("skipped: no taskbar");
        return;
    }
    let mut k = TestKiln::start(Some(ON), &[]);
    k.close();
    assert!(k.hides());

    // A click on the icon.
    k.click_icon();
    assert!(k.shows(), "a click on the icon opens Kiln");
    assert!(k.icon_goes(), "the icon goes as the window shows");

    // Open Kiln, from the menu.
    k.close();
    assert!(k.hides());
    assert!(k.icon_comes(), "and comes back as it hides again");
    k.tray_command(CMD_OPEN);
    assert!(k.shows(), "Open Kiln opens Kiln");
    assert!(k.icon_goes());
    assert!(k.running());
}

#[test]
fn signing_out_keeps_the_held_results() {
    let dir = tempfile::tempdir().unwrap();
    let photo = dir.path().join("photo.png");
    std::fs::copy(fixture("photo.png"), &photo).unwrap();
    // Its folder refuses the result and there is no Documents: it is held.
    let locked = Locked::new(dir.path());
    let mut k = TestKiln::start(None, std::slice::from_ref(&photo));
    std::fs::remove_dir(k.docs.path()).unwrap();
    k.convert_and_keep();
    let held = k.results();
    assert_eq!(held.len(), 1, "one result is held: {held:?}");
    let result = held[0].0.clone();
    let run = result.parent().unwrap().to_path_buf();

    // Windows asks first, Kiln never stands in the way, and the session
    // ends.
    k.post(WM_QUERYENDSESSION, 0, ENDSESSION_LOGOFF as isize);
    k.post(WM_ENDSESSION, 1, ENDSESSION_LOGOFF as isize);
    assert!(k.ends(), "Kiln ends with the session");

    // The run folder keeps the result, whole, and where it came from, so
    // the next start delivers it next to its original.
    assert_eq!(k.results(), held, "the result is kept");
    let sources = std::fs::read_to_string(run.join(kiln_core::SOURCES_NAME))
        .expect("the run folder keeps its sources.tsv");
    let runs = kiln_core::stale_runs(run.parent().unwrap());
    assert_eq!(runs.len(), 1, "the next start finds the run: {sources:?}");
    assert_eq!(runs[0].dir, run);
    let original = std::fs::canonicalize(dir.path()).unwrap();
    let offered: Vec<(PathBuf, Option<PathBuf>)> = runs[0]
        .files
        .iter()
        .map(|f| (f.path.clone(), f.origin.clone()))
        .collect();
    assert!(
        matches!(
            offered.as_slice(),
            [(path, Some(origin))] if *path == result
                && std::fs::canonicalize(origin).is_ok_and(|o| o == original)
        ),
        "sources.tsv names the result and its original's folder: {offered:?}, {sources:?}"
    );
    drop(runs);
    drop(k);
    drop(locked);
    drop(dir);
}

#[test]
fn turning_the_drag_wheel_off_while_hidden_makes_the_next_close_quit() {
    if !have_taskbar() {
        eprintln!("skipped: no taskbar");
        return;
    }
    let mut k = TestKiln::start(Some(ON), &[]);
    k.close();
    assert!(k.hides());
    assert!(k.icon_comes());

    // Off, from the menu: Kiln stays hidden, and its icon with it.
    k.tray_command(CMD_DRAG_WHEEL);
    assert!(
        wait_until(SETTLE, || k.settings_text().contains("drag_wheel = false")),
        "the setting is kept"
    );
    assert!(k.lives_on());
    assert!(!k.visible());
    assert!(k.icon_there(), "the hidden Kiln can still be found");

    // Opened again, the icon goes, and the next close quits.
    k.click_icon();
    assert!(k.shows());
    assert!(k.icon_goes());
    k.close();
    assert!(k.ends(), "with the drag-wheel off, closing quits");
}

#[test]
fn ending_the_session_ends_a_hidden_kiln() {
    if !have_taskbar() {
        eprintln!("skipped: no taskbar");
        return;
    }
    let mut k = TestKiln::start(Some(ON), &[]);
    k.close();
    assert!(k.hides());
    assert!(k.icon_comes());

    // As the Restart Manager closes an app for an update. Nothing is held,
    // so nothing asks.
    k.post(WM_QUERYENDSESSION, 0, ENDSESSION_CLOSEAPP as isize);
    k.post(WM_ENDSESSION, 1, ENDSESSION_CLOSEAPP as isize);
    assert!(k.ends(), "Kiln ends with the session");
    assert!(!k.icon_there(), "and its icon goes");
}

#[test]
fn taskbar_created_readds_the_icon() {
    if !have_taskbar() {
        eprintln!("skipped: no taskbar");
        return;
    }
    let k = TestKiln::start(Some(ON), &[]);
    k.close();
    assert!(k.hides());
    assert!(k.icon_comes());

    // As Explorer restarting takes it away...
    unsafe {
        assert!(Shell_NotifyIconW(NIM_DELETE, &icon_data(k.hwnd)).as_bool());
    }
    assert!(!k.icon_there());
    // ...and then says the taskbar is back.
    let restarted = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
    k.post(restarted, 0, 0);
    assert!(
        k.icon_comes(),
        "the icon comes back while the window is hidden"
    );

    // With the window open there's no icon, and a restart adds none.
    k.click_icon();
    assert!(k.shows());
    assert!(k.icon_goes());
    k.post(restarted, 0, 0);
    std::thread::sleep(Duration::from_secs(1));
    assert!(!k.icon_there(), "no icon comes while the window is open");
}

#[test]
fn a_second_launch_shows_the_hidden_window() {
    if !have_taskbar() {
        eprintln!("skipped: no taskbar");
        return;
    }
    let mut k = TestKiln::start(Some(ON), &[]);
    k.close();
    assert!(k.hides());

    // Opened again, as from the Start menu: it hands over and exits.
    let mut second = k.command().spawn().expect("a second kiln-app starts");
    let mut status = None;
    let exited = wait_until(START, || {
        status = second.try_wait().unwrap();
        status.is_some()
    });
    if !exited {
        let _ = second.kill();
        let _ = second.wait();
    }
    assert!(exited, "the second launch hands over and exits");
    assert!(status.unwrap().success());
    assert!(k.shows(), "the hidden window shows");
    assert!(k.icon_goes(), "and the icon goes");
    assert!(k.running());
}

/// A scratch start-up key, `HKCU\Software\KilnTests\<random>\Run`, that a
/// Kiln started with it as `KILN_TEST_RUN_KEY` uses in place of Windows'
/// Run key. Dropped, its value goes, then the key, the random key and
/// `KilnTests`, each only if empty: nothing else is ever removed.
struct ScratchRun {
    parent: String,
    run: String,
}

impl ScratchRun {
    fn new() -> ScratchRun {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let parent = format!(r"Software\KilnTests\{}-{nanos:x}", std::process::id());
        let run = format!(r"{parent}\Run");
        ScratchRun { parent, run }
    }

    fn key(&self) -> kiln_app::autostart::RunKey {
        kiln_app::autostart::RunKey::at(&self.run)
    }

    fn value(&self) -> Option<String> {
        use kiln_app::autostart::RunStore;
        self.key().read().unwrap()
    }
}

impl Drop for ScratchRun {
    fn drop(&mut self) {
        use kiln_app::autostart::RunStore;
        use windows::core::HSTRING;
        use windows::Win32::System::Registry::{RegDeleteKeyW, HKEY_CURRENT_USER};
        let _ = self.key().delete();
        for k in [
            self.run.as_str(),
            self.parent.as_str(),
            r"Software\KilnTests",
        ] {
            let k = HSTRING::from(k);
            // Only an empty key is removed.
            let _ = unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(k.as_ptr())) };
        }
    }
}

impl TestKiln {
    /// Start Kiln as its start-up entry does, `--background`, with
    /// `settings` and the scratch start-up key `run`, and wait for its
    /// window to be made (not shown), or for it to end.
    fn start_background(settings: &str, run: &ScratchRun) -> TestKiln {
        let one = ONE_KILN.lock().unwrap_or_else(|e| e.into_inner());
        assert!(
            !a_kiln_runs(),
            "another Kiln is running. Close it, then run these tests again."
        );
        let appdata = tempfile::tempdir().unwrap();
        let local = tempfile::tempdir().unwrap();
        let docs = tempfile::tempdir().unwrap();
        let dir = appdata.path().join("Kiln");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("settings.toml"), settings).unwrap();
        let temp = local.path().join("Temp");
        std::fs::create_dir_all(&temp).unwrap();
        // Only a build that honours the test switches: one without them looks in
        // the real drives' holding folders and saves into the real Documents.
        common::refuse_without_hooks();
        let mut child = Command::new(env!("CARGO_BIN_EXE_kiln-app"))
            .arg("--background")
            .env("APPDATA", appdata.path())
            .env("LOCALAPPDATA", local.path())
            .env("KILN_TEST_DRIVES", "")
            .env("KILN_TEST_RUN_KEY", &run.run)
            .env("KILN_TEST_DOCUMENTS", docs.path())
            .env("TEMP", &temp)
            .env("TMP", &temp)
            .spawn()
            .expect("kiln-app starts");
        let pid = child.id();
        let mut hwnd = None;
        // Its window, or its end: with the drag-wheel off it makes none.
        wait_until(START, || {
            hwnd = windows_of(pid, w!("KilnWindow")).into_iter().next();
            hwnd.is_some() || !matches!(child.try_wait(), Ok(None))
        });
        TestKiln {
            child,
            hwnd: hwnd.unwrap_or_default(),
            appdata,
            local,
            docs,
            _one: one,
        }
    }
}

fn this_exe() -> PathBuf {
    std::fs::canonicalize(env!("CARGO_BIN_EXE_kiln-app"))
        .map(|p| {
            // `canonicalize` gives a `\\?\` path; Kiln's own is plain.
            let s = p.to_string_lossy().into_owned();
            PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s))
        })
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_BIN_EXE_kiln-app")))
}

#[test]
fn a_background_start_shows_no_window_and_puts_up_the_icon() {
    if !have_taskbar() {
        eprintln!("skipped: no taskbar");
        return;
    }
    let run = ScratchRun::new();
    let mut k = TestKiln::start_background(ON, &run);
    assert!(!k.hwnd.is_invalid(), "Kiln made its window");
    assert!(k.icon_comes(), "the icon is in the notification area");
    // No window shows, now or a moment later.
    std::thread::sleep(Duration::from_secs(2));
    assert!(!k.visible(), "the window stays hidden");
    assert!(k.running());
    // The drag-wheel is on: the start-up entry is made sure of.
    assert!(
        wait_until(SETTLE, || run
            .value()
            .is_some_and(|v| v.ends_with("--background"))),
        "the start-up entry is written: {:?}",
        run.value()
    );

    // A second launch, as from the Start menu, opens the window.
    let mut second = k.command().spawn().expect("a second kiln-app starts");
    let exited = wait_until(START, || second.try_wait().unwrap().is_some());
    if !exited {
        let _ = second.kill();
        let _ = second.wait();
    }
    assert!(exited, "the second launch hands over and exits");
    assert!(k.shows(), "the window shows");
    assert!(k.icon_goes(), "and the icon goes");

    // Quit Kiln ends it.
    k.tray_command(CMD_QUIT);
    assert!(k.ends(), "Quit Kiln ends Kiln");
}

#[test]
fn a_background_start_with_the_drag_wheel_off_removes_its_entry_and_ends() {
    let run = ScratchRun::new();
    {
        use kiln_app::autostart::{run_value, RunStore};
        run.key().write(&run_value(&this_exe())).unwrap();
    }
    let mut k = TestKiln::start_background("drag_wheel = false\ndrag_wheel_reset = true\n", &run);
    assert!(k.ends(), "Kiln ends at once");
    assert!(k.hwnd.is_invalid(), "no window was made");
    assert_eq!(run.value(), None, "its own start-up entry is gone");

    // Another program's entry under the name is left alone.
    let theirs = r#""C:\Tools\other.exe" --background"#;
    {
        use kiln_app::autostart::RunStore;
        run.key().write(theirs).unwrap();
    }
    drop(k);
    let mut k = TestKiln::start_background("drag_wheel = false\ndrag_wheel_reset = true\n", &run);
    assert!(k.ends());
    assert_eq!(run.value().as_deref(), Some(theirs));
}
