//! Results a crashed or killed Kiln left are delivered at the next start,
//! with no question: saved next to their originals, never over a file
//! there, else in Documents\Kiln results, or kept for the start after.
//!
//! Each test makes the run folder such a Kiln leaves in a scratch
//! LOCALAPPDATA, starts a Kiln of its own on it, with a scratch APPDATA and
//! a scratch Documents, and reads it only through UI Automation, as
//! `resident.rs` does. Nothing here moves the pointer or sends input, and
//! no test starts while another Kiln is running.

#![cfg(windows)]

mod common;

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::{Mutex, MutexGuard};
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

/// One test Kiln at a time.
static ONE_KILN: Mutex<()> = Mutex::new(());

/// A debug build can take a while to show its first frame.
const START: Duration = Duration::from_secs(60);
/// How long a posted message may take to show its effect.
const SETTLE: Duration = Duration::from_secs(10);
/// First-run help is dismissed (under its old key), closing the window
/// quits, and the retired hold setting still loads.
const SETTINGS: &str = "save_next_to_original = false\nwelcome_shown = true\ndrag_wheel = false\n";
/// What the original's folder already holds under the result's name.
const OLD: &[u8] = b"the original folder's own a.png";
/// The size of the result a slowed copy is ended part-way through.
const BIG: usize = 32 * 1024 * 1024;

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

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

/// The names in `dir`, sorted, each with its bytes.
fn contents(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            let name = e.file_name().to_string_lossy().into_owned();
            (name, std::fs::read(e.path()).unwrap())
        })
        .collect();
    v.sort();
    v
}

/// The names in `dir`, sorted, without opening anything: a live run's
/// lock can't be opened.
fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

/// A scratch APPDATA and LOCALAPPDATA, with a TEMP inside it, which test
/// Kilns are started on, one after another.
struct Scratch {
    appdata: TempDir,
    local: TempDir,
    /// Stands in for Documents: Kiln results is made in it, never in the
    /// real one.
    docs: TempDir,
    /// The folder the original was in: not ASCII, with a file of the
    /// result's name already in it.
    origin: PathBuf,
    /// The run folder a killed Kiln left.
    run: PathBuf,
    /// The result in it, as it was.
    result: Vec<u8>,
}

impl Scratch {
    /// The run folder a Kiln killed while it held one result, `a.png`, left
    /// behind: its lock closed, the result's original folder noted, and
    /// the empty placeholder of a second result Kiln was killed before
    /// placing, which is no result and is never offered.
    fn with_a_left_run() -> Scratch {
        Scratch::left_holding(std::fs::read(fixture("photo.png")).unwrap())
    }

    /// [`Scratch::with_a_left_run`], with a result big enough that a slowed
    /// copy of it takes seconds.
    fn with_a_big_left_run() -> Scratch {
        let big = (0..BIG).map(|i| (i % 251) as u8).collect();
        Scratch::left_holding(big)
    }

    /// The left run folder, holding `result` as `a.png`.
    fn left_holding(result: Vec<u8>) -> Scratch {
        let appdata = tempfile::tempdir().unwrap();
        let local = tempfile::tempdir().unwrap();
        let docs = tempfile::tempdir().unwrap();
        let dir = appdata.path().join("Kiln");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("settings.toml"), SETTINGS).unwrap();
        std::fs::create_dir_all(local.path().join("Temp")).unwrap();

        let origin = local.path().join("Ana María").join("fotos");
        std::fs::create_dir_all(&origin).unwrap();
        std::fs::write(origin.join("a.png"), OLD).unwrap();

        let run = held_root(&local).join("1-1");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(run.join("lock"), "").unwrap();
        std::fs::write(run.join("a.png"), &result).unwrap();
        std::fs::write(run.join("b.png"), "").unwrap();
        let line = format!("a.png\t{}\n", origin.display());
        std::fs::write(run.join(kiln_core::SOURCES_NAME), line).unwrap();
        Scratch {
            appdata,
            local,
            docs,
            origin,
            run,
            result,
        }
    }

    fn held_root(&self) -> PathBuf {
        held_root(&self.local)
    }

    /// What the test Kilns have written to their log, `kiln.log` in the
    /// scratch LOCALAPPDATA, or nothing.
    fn log(&self) -> String {
        let log = self.local.path().join("Kiln").join("logs").join("kiln.log");
        std::fs::read_to_string(log).unwrap_or_default()
    }

    /// What Documents\Kiln results holds, by name, with the bytes.
    fn in_kiln_results(&self) -> Vec<(String, Vec<u8>)> {
        let dir = self.docs.path().join("Kiln results");
        if dir.is_dir() {
            contents(&dir)
        } else {
            vec![]
        }
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

fn held_root(local: &TempDir) -> PathBuf {
    local.path().join("Kiln").join("held")
}

/// A Kiln this test started. It is ended when dropped.
struct TestKiln {
    child: Child,
    hwnd: HWND,
    _one: MutexGuard<'static, ()>,
}

impl TestKiln {
    /// Start Kiln on `scratch`, and wait for its window.
    fn start(scratch: &Scratch) -> TestKiln {
        TestKiln::start_with(scratch, &[])
    }

    /// Start Kiln on `scratch` with the extra environment `vars`, and wait
    /// for its window.
    fn start_with(scratch: &Scratch, vars: &[(&str, &std::ffi::OsStr)]) -> TestKiln {
        let one = ONE_KILN.lock().unwrap_or_else(|e| e.into_inner());
        assert!(
            !a_kiln_runs(),
            "another Kiln is running. Close it, then run these tests again."
        );
        let temp = scratch.local.path().join("Temp");
        // Only a build that honours the test switches: one without them looks in
        // the real drives' holding folders and saves into the real Documents.
        common::refuse_without_hooks();
        let mut command = Command::new(env!("CARGO_BIN_EXE_kiln-app"));
        command
            .env("APPDATA", scratch.appdata.path())
            .env("LOCALAPPDATA", scratch.local.path())
            // Recovery looks only on stand-in drives (none), never in a
            // real drive's .kiln-held.
            .env("KILN_TEST_DRIVES", "")
            // Debug builds leave the start-up entry alone: none is read
            // or written.
            .env("KILN_TEST_RUN_KEY", "")
            .env("KILN_TEST_DOCUMENTS", scratch.docs.path())
            .env("TEMP", &temp)
            .env("TMP", &temp);
        for (name, value) in vars {
            command.env(name, value);
        }
        let child = command.spawn().expect("kiln-app starts");
        let pid = child.id();
        let mut hwnd = None;
        let shown = wait_until(START, || {
            hwnd = shown_windows_of(pid, w!("KilnWindow")).into_iter().next();
            hwnd.is_some()
        });
        let k = TestKiln {
            child,
            hwnd: hwnd.unwrap_or_default(),
            _one: one,
        };
        assert!(shown, "Kiln's window never showed");
        k
    }

    fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Ask the window to close, which quits this Kiln, and look at nothing.
    fn close(&self) {
        unsafe { PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) }.unwrap();
    }

    /// Whether this Kiln ends within `limit`.
    fn ends_within(&mut self, limit: Duration) -> bool {
        let child = &mut self.child;
        wait_until(limit, || !matches!(child.try_wait(), Ok(None)))
    }

    /// Whether Kiln lets the session end, asked as Windows asks before
    /// signing out. Nothing ends: no WM_ENDSESSION follows.
    fn lets_the_session_end(&self) -> bool {
        let mut answer = 0usize;
        let sent = unsafe {
            SendMessageTimeoutW(
                self.hwnd,
                WM_QUERYENDSESSION,
                WPARAM(0),
                LPARAM(ENDSESSION_LOGOFF as isize),
                SMTO_ABORTIFHUNG,
                SETTLE.as_millis() as u32,
                Some(&mut answer),
            )
        };
        assert_ne!(sent.0, 0, "Kiln answers");
        answer != 0
    }

    /// Close the window, which quits this Kiln, and wait for it to end.
    fn quit(&mut self) {
        unsafe { PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) }.unwrap();
        let child = &mut self.child;
        let ended = wait_until(SETTLE, || !matches!(child.try_wait(), Ok(None)));
        assert!(ended, "closing the window quits Kiln");
    }
}

impl Drop for TestKiln {
    fn drop(&mut self) {
        if self.running() {
            eprintln!("the test Kiln didn't quit; ending it");
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        // Its mutex goes with it; the next test checks for it.
        wait_until(SETTLE, || !a_kiln_runs());
    }
}

/// A UI Automation client on one of Kiln's windows.
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

    /// The element with `automation_id`, if it is there.
    fn find(&self, automation_id: &str) -> Option<IUIAutomationElement> {
        self.first(UIA_AutomationIdPropertyId, VARIANT::from(automation_id))
    }

    /// The name of the element with `automation_id`, if it is there.
    fn name_of(&self, automation_id: &str) -> Option<String> {
        let e = self.first(UIA_AutomationIdPropertyId, VARIANT::from(automation_id))?;
        unsafe { e.CurrentName().ok().map(|n| n.to_string()) }
    }
}

/// What the InfoBar at the top of the window says, once it says `start`.
fn info_says(k: &TestKiln, start: &str) -> Option<String> {
    let ui = Ui::new(k.hwnd);
    let mut said = None;
    wait_until(START, || {
        said = ui.name_of("TopInfo");
        said.as_deref().is_some_and(|s| s.contains(start))
    });
    said
}

/// Whether Kiln asks nothing, inside its window or out, for `limit`.
fn asks_nothing_for(k: &TestKiln, limit: Duration) -> bool {
    let ui = Ui::new(k.hwnd);
    let pid = k.child.id();
    let start = Instant::now();
    while start.elapsed() < limit {
        if ui.find("Ask").is_some() || !shown_windows_of(pid, w!("#32770")).is_empty() {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    true
}

#[test]
fn recovery_delivers_next_to_the_original_without_overwriting() {
    let scratch = Scratch::with_a_left_run();
    // A Kiln that is still running holds this one: it is never delivered
    // or touched.
    let live = kiln_core::Holding::open(&scratch.held_root()).unwrap();
    std::fs::write(live.dir().join("live.png"), "live").unwrap();

    let mut k = TestKiln::start(&scratch);
    let saved = scratch.origin.join("a (1).png");
    assert!(
        wait_until(START, || saved.is_file() && !scratch.run.exists()),
        "the result is saved beside its original, and its run folder goes: {:?}",
        names(&scratch.origin)
    );
    assert_eq!(
        contents(&scratch.origin),
        [
            ("a (1).png".to_owned(), scratch.result.clone()),
            ("a.png".to_owned(), OLD.to_vec()),
        ],
        "the original folder's own a.png is untouched"
    );
    // It asks nothing, and says what it did once, at the top of the window.
    let said = info_says(&k, "from last time");
    assert_eq!(
        said.as_deref(),
        Some("Saved 1 converted file from last time in fotos."),
    );
    assert!(asks_nothing_for(&k, Duration::from_secs(2)));
    assert!(scratch.in_kiln_results().is_empty());

    // The live run is as it was, and still its own.
    assert_eq!(
        names(live.dir()),
        ["live.png", "lock"],
        "the live run is untouched"
    );
    assert!(k.running());
    k.quit();
    drop(k);
    let live_dir = live.dir().to_path_buf();
    std::fs::remove_file(live_dir.join("live.png")).unwrap();
    assert!(live.close_if_empty());
    assert!(!live_dir.exists());
}

#[test]
fn a_result_that_cant_be_delivered_is_kept_and_the_next_start_delivers_it() {
    let scratch = Scratch::with_a_left_run();
    let left = contents(&scratch.run);
    // Its folder refuses it, and there is no Documents for Kiln results.
    let locked = Locked::new(&scratch.origin);
    std::fs::remove_dir(scratch.docs.path()).unwrap();

    let mut k = TestKiln::start(&scratch);
    let said = info_says(&k, "couldn't be saved");
    assert_eq!(
        said.as_deref(),
        Some(
            "1 converted file from last time couldn't be saved. \
             Kiln tries again the next time it starts."
        ),
    );
    assert!(asks_nothing_for(&k, Duration::from_secs(1)));
    assert_eq!(contents(&scratch.run), left, "the run folder is as it was");
    assert_eq!(names(&scratch.origin), ["a.png"]);
    assert!(!scratch.docs.path().exists(), "Documents is never made");
    // And no longer locked: the next start can deliver it.
    let runs = kiln_core::stale_runs(&scratch.held_root());
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].dir, scratch.run);
    drop(runs);
    k.quit();
    drop(k);
    assert_eq!(contents(&scratch.run), left, "quitting leaves it too");

    // The folder takes files again: the next start delivers it, asking
    // nothing.
    drop(locked);
    let mut k = TestKiln::start(&scratch);
    let saved = scratch.origin.join("a (1).png");
    assert!(
        wait_until(START, || saved.is_file() && !scratch.run.exists()),
        "the next start delivers it: {:?}",
        names(&scratch.origin)
    );
    assert_eq!(std::fs::read(&saved).unwrap(), scratch.result);
    assert_eq!(std::fs::read(scratch.origin.join("a.png")).unwrap(), OLD);
    assert!(asks_nothing_for(&k, Duration::from_secs(1)));
    k.quit();
}

#[test]
fn a_result_its_folder_refuses_goes_to_kiln_results() {
    let scratch = Scratch::with_a_left_run();
    let locked = Locked::new(&scratch.origin);

    let mut k = TestKiln::start(&scratch);
    let said = info_says(&k, "from last time");
    assert_eq!(
        said.as_deref(),
        Some("Saved 1 converted file from last time in Documents\\Kiln results."),
    );
    let saved = scratch.in_kiln_results();
    assert_eq!(
        saved,
        [("a.png".to_owned(), scratch.result.clone())],
        "saved whole in Kiln results"
    );
    assert!(wait_until(SETTLE, || !scratch.run.exists()));
    assert_eq!(names(&scratch.origin), ["a.png"]);
    k.quit();
    drop(k);
    drop(locked);
}

/// How far the slowed copies the log tells of got: bytes copied and the
/// size, from each `slowed copy: <done> of <total>` line.
fn slowed_copies(log: &str) -> Vec<(u64, u64)> {
    log.lines()
        .filter_map(|l| l.split_once("slowed copy: "))
        .filter_map(|(_, rest)| {
            let (done, total) = rest.trim().split_once(" of ")?;
            Some((done.parse().ok()?, total.parse().ok()?))
        })
        .collect()
}

/// A line in the log saying a delivery failed on its own, not stopped by
/// a quit.
fn delivery_failures(log: &str) -> Vec<&str> {
    log.lines()
        .filter(|l| {
            l.contains("couldn't be saved")
                || l.contains("saving the results from last time failed")
                || l.contains("looking for results from last time failed")
        })
        .collect()
}

/// Start Kiln on `scratch`'s big result with every delivery a slowed
/// copy, and wait until it is part-way through copying it next to the
/// original: its temp copy is there, and the copy has copied more than
/// nothing and less than the whole result. (The temp's own length can't
/// tell: the copy sets it to the whole size before it copies a byte.)
fn delivering(scratch: &Scratch) -> TestKiln {
    let k = TestKiln::start_with(scratch, &[("KILN_TEST_SLOW_COPY", "1".as_ref())]);
    let temp = || {
        names(&scratch.origin)
            .iter()
            .any(|n| n.ends_with(".tmp") && n.contains(".kiln-export"))
    };
    let part_way = || {
        temp()
            && slowed_copies(&scratch.log())
                .iter()
                .any(|&(done, total)| total == BIG as u64 && done > 0 && done < total)
    };
    assert!(
        wait_until(START, part_way),
        "the delivery never got part-way: {:?}",
        slowed_copies(&scratch.log())
    );
    assert!(delivery_failures(&scratch.log()).is_empty());
    k
}

/// What a delivery cut short leaves: the result held whole in its run
/// folder, for the next start, and nothing of it beside the original,
/// not even Kiln's temp copy. The next start delivers it, whole.
fn kept_whole_then_delivered(scratch: &Scratch) {
    let log = scratch.log();
    let failures = delivery_failures(&log);
    assert!(
        failures.is_empty(),
        "cut short by the end, not failed: {failures:?}"
    );
    let copied = slowed_copies(&log);
    assert!(
        copied.iter().all(|&(done, total)| done < total),
        "the copy never finished: {copied:?}"
    );
    assert_eq!(
        contents(&scratch.origin),
        [("a.png".to_owned(), OLD.to_vec())],
        "nothing of the copy is left beside the original"
    );
    assert_eq!(
        std::fs::read(scratch.run.join("a.png")).unwrap(),
        scratch.result,
        "kept whole for the next start"
    );
    assert_eq!(kiln_core::stale_runs(&scratch.held_root()).len(), 1);
    let mut k = TestKiln::start(scratch);
    let saved = scratch.origin.join("a (1).png");
    assert!(
        wait_until(START, || !scratch.run.exists() && saved.is_file()),
        "the next start delivers it"
    );
    assert_eq!(std::fs::read(&saved).unwrap(), scratch.result);
    k.quit();
}

#[test]
fn a_close_while_delivering_keeps_the_rest_for_next_time() {
    let scratch = Scratch::with_a_big_left_run();

    let mut k = delivering(&scratch);
    // Closed part-way through the copy: Kiln stops it, asks nothing, and
    // quits; the result it hadn't delivered is kept whole.
    k.close();
    assert!(k.ends_within(SETTLE), "Kiln quits");
    drop(k);
    kept_whole_then_delivered(&scratch);
}

#[test]
fn signing_out_while_delivering_keeps_the_rest() {
    let scratch = Scratch::with_a_big_left_run();

    let mut k = delivering(&scratch);
    // Windows asks first: Kiln never stands in the way of the session's
    // end, and ends with it.
    assert!(k.lets_the_session_end(), "Kiln lets the session end");
    unsafe {
        PostMessageW(
            Some(k.hwnd),
            WM_ENDSESSION,
            WPARAM(1),
            LPARAM(ENDSESSION_LOGOFF as isize),
        )
    }
    .unwrap();
    assert!(k.ends_within(SETTLE), "Kiln ends with the session");
    drop(k);
    kept_whole_then_delivered(&scratch);
}

#[test]
fn the_kiln_these_tests_start_honours_the_test_switches() {
    let exe = Path::new(env!("CARGO_BIN_EXE_kiln-app"));
    assert!(common::has_hooks(exe).unwrap(), "{}", exe.display());
}

#[cfg(any(debug_assertions, feature = "test-hooks"))]
#[test]
fn the_tests_look_for_the_mark_a_hooked_kiln_carries() {
    assert_eq!(common::mark(), kiln_app::test_build::MARK);
}

#[test]
fn a_file_without_the_mark_is_a_build_without_the_hooks() {
    let dir = tempfile::tempdir().unwrap();
    let plain = dir.path().join("plain.exe");
    std::fs::write(&plain, vec![0x4du8; 3 << 20]).unwrap();
    assert!(!common::has_hooks(&plain).unwrap());
    // The mark across a read's edge is still found.
    let mut bytes = vec![0u8; (1 << 20) - 5];
    bytes.extend_from_slice(&common::mark());
    bytes.extend_from_slice(&[0u8; 100]);
    let marked = dir.path().join("marked.exe");
    std::fs::write(&marked, bytes).unwrap();
    assert!(common::has_hooks(&marked).unwrap());
    assert!(common::has_hooks(&dir.path().join("none.exe")).is_err());
}

/// This user's SID, which names their folder in a drive's `.kiln-held`.
fn user_sid() -> String {
    use kiln_core::Volumes;
    kiln_core::SystemVolumes
        .user_sid()
        .expect("this process's SID")
}

/// A run folder a killed Kiln left in the holding root `root`: its lock
/// closed, one result `name` made from a file in `origin`, noted.
fn left_run(root: &Path, run: &str, name: &str, origin: &Path) -> PathBuf {
    let dir = root.join(run);
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("lock"), "").unwrap();
    std::fs::copy(fixture("photo.png"), dir.join(name)).unwrap();
    let line = format!("{name}\t{}\n", origin.display());
    std::fs::write(dir.join(kiln_core::SOURCES_NAME), line).unwrap();
    dir
}

#[test]
fn recovery_finds_what_was_left_on_every_drive_and_says_so_in_one_infobar() {
    // The local root holds one result, as phase 1's tests have it.
    let scratch = Scratch::with_a_left_run();
    let sid = user_sid();
    let record = scratch
        .local
        .path()
        .join("Kiln")
        .join(kiln_core::ROOTS_NAME);

    // A drive root Kiln made and noted: its user folder holds a run.
    let noted = tempfile::tempdir().unwrap();
    let noted_root = noted.path().join(kiln_core::HELD_DIR_NAME).join(&sid);
    let made = kiln_core::prepare_held_dir(&noted_root).unwrap();
    assert!(made.made && made.made_parent);
    kiln_core::note_held_root(&record, &noted_root, true).unwrap();
    let origin_b = noted.path().join("Bilder ü");
    std::fs::create_dir(&origin_b).unwrap();
    let run_b = left_run(&noted_root, "2-2", "c.png", &origin_b);

    // A mounted drive, not in the record, whose user folder holds a run
    // and a live Kiln's run, which is never delivered or touched.
    let mounted = tempfile::tempdir().unwrap();
    let mounted_root = mounted.path().join(kiln_core::HELD_DIR_NAME).join(&sid);
    kiln_core::prepare_held_dir(&mounted_root).unwrap();
    let origin_c = mounted.path().join("photos");
    std::fs::create_dir(&origin_c).unwrap();
    let run_c = left_run(&mounted_root, "3-3", "d.png", &origin_c);
    let live = kiln_core::Holding::open(&mounted_root).unwrap();
    std::fs::write(live.dir().join("live.png"), "live").unwrap();

    // A root on a drive that isn't plugged in stays in the record.
    let unplugged = scratch
        .local
        .path()
        .join("unplugged")
        .join(kiln_core::HELD_DIR_NAME)
        .join(&sid);
    kiln_core::note_held_root(&record, &unplugged, true).unwrap();

    // Only the stand-in drives are looked on, never a real one. The noted
    // root's drive is one of them too: a root both recorded and mounted is
    // delivered from once.
    let drives = std::env::join_paths([mounted.path(), noted.path()]).unwrap();
    let mut k = TestKiln::start_with(&scratch, &[("KILN_TEST_DRIVES", drives.as_os_str())]);
    let saved = [
        scratch.origin.join("a (1).png"),
        origin_b.join("c.png"),
        origin_c.join("d.png"),
    ];
    assert!(
        wait_until(START, || saved.iter().all(|f| f.is_file())
            && !scratch.run.exists()
            && !run_b.exists()
            && !run_c.exists()),
        "each is saved beside its original, and its run folder goes"
    );
    let photo = std::fs::read(fixture("photo.png")).unwrap();
    for f in &saved {
        assert_eq!(std::fs::read(f).unwrap(), photo, "{}", f.display());
    }
    assert_eq!(std::fs::read(scratch.origin.join("a.png")).unwrap(), OLD);

    // One InfoBar says it all, and nothing is asked.
    let said = info_says(&k, "from last time");
    assert_eq!(
        said.as_deref(),
        Some("Saved 3 converted files from last time in 3 folders."),
    );
    assert!(asks_nothing_for(&k, Duration::from_secs(2)), "no prompt");

    // The noted root, left empty, goes with its .kiln-held, and leaves the
    // record; the unplugged one stays in it.
    assert!(
        wait_until(SETTLE, || !noted
            .path()
            .join(kiln_core::HELD_DIR_NAME)
            .exists()),
        "the empty root Kiln made goes"
    );
    let listed = kiln_core::read_held_roots(&record);
    assert_eq!(listed, std::slice::from_ref(&unplugged), "{listed:?}");
    // The live run is as it was, and its root with it.
    assert_eq!(names(live.dir()), ["live.png", "lock"]);
    assert!(k.running());
    k.quit();
    drop(k);
    let live_dir = live.dir().to_path_buf();
    std::fs::remove_file(live_dir.join("live.png")).unwrap();
    assert!(live.close_if_empty());
    assert!(!live_dir.exists());
    assert!(mounted_root.is_dir(), "a root Kiln didn't note stays");
}
