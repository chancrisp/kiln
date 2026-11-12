//! Kiln's log and crash files, in `%LOCALAPPDATA%\Kiln\logs`.
//!
//! `kiln.log` gets a line for each thing worth knowing later, such as a
//! failed job. When a write would take it past [`LOG_MAX`], it becomes
//! `kiln.1.log`, replacing the one before, and a new `kiln.log` starts: two
//! files at most, 2 MB in all.
//!
//! A panic on any thread writes `crash-YYYYMMDD-HHMMSS-<pid>.txt` beside
//! them ([`install_panic_hook`]), and the newest [`KEEP_CRASHES`] are kept.
//!
//! Nothing here is Windows. The time, the debugger output, the Windows
//! version and where Kiln is loaded come from the [`Platform`] the app sets
//! at start, so the rolling and the reports are tested anywhere.

use std::backtrace::Backtrace;
use std::borrow::Cow;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// How big `kiln.log` may grow before it rolls to `kiln.1.log`: 1 MB.
pub const LOG_MAX: u64 = 1 << 20;

/// The longest line the log takes, in bytes: an eighth of [`LOG_MAX`].
/// Longer ones are cut to it, ending in "…".
pub const LINE_MAX: usize = (LOG_MAX / 8) as usize;

/// How many crash files are kept.
pub const KEEP_CRASHES: usize = 10;

const LOG_NAME: &str = "kiln.log";
const ROLLED_NAME: &str = "kiln.1.log";

/// Local time to the second: year, month, day, hour, minute, second.
pub type LocalTime = (u16, u8, u8, u8, u8, u8);

/// What only the platform can tell this module. The app sets it first
/// thing. Until then, and in tests, the time is fixed, nothing goes to a
/// debugger, and the Windows version and load address are unknown.
#[derive(Clone, Copy)]
pub struct Platform {
    /// The local time (`GetLocalTime`).
    pub now: fn() -> LocalTime,
    /// A message for an attached debugger (`OutputDebugStringW`).
    pub debug: fn(&str),
    /// The Windows version and build, "10.0.26300.1".
    pub windows: fn() -> String,
    /// The address `kiln-app.exe` is loaded at.
    pub module_base: fn() -> usize,
}

const FIXED: Platform = Platform {
    now: || (2000, 1, 1, 0, 0, 0),
    debug: |_| {},
    windows: || "unknown".to_owned(),
    module_base: || 0,
};

static PLATFORM: OnceLock<Platform> = OnceLock::new();

/// Say what the platform tells this module. Only the first call counts.
pub fn set_platform(p: Platform) {
    let _ = PLATFORM.set(p);
}

fn platform() -> Platform {
    PLATFORM.get().copied().unwrap_or(FIXED)
}

/// `kiln.log` in a folder, and the `kiln.1.log` it rolls to.
pub struct LogFile {
    dir: PathBuf,
    max: u64,
}

impl LogFile {
    pub fn new(dir: PathBuf, max: u64) -> Self {
        LogFile { dir, max }
    }

    /// Add `line` and a line end to `kiln.log`, making the folder if it's
    /// gone. When that would take the file past `max`, the file so far
    /// first becomes `kiln.1.log`, replacing the old one. A line longer
    /// than [`LINE_MAX`] is cut to that length, so one line never fills the
    /// log. When `max` is smaller than that, a line longer than `max` goes
    /// in a file of its own.
    pub fn append(&self, line: &str) -> io::Result<()> {
        let line = clipped(line, LINE_MAX);
        fs::create_dir_all(&self.dir)?;
        let path = self.dir.join(LOG_NAME);
        let had = fs::metadata(&path).map_or(0, |m| m.len());
        let adding = line.len() as u64 + 1;
        if had > 0 && had + adding > self.max {
            fs::rename(&path, self.dir.join(ROLLED_NAME))?;
        }
        let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
        file.write_all(format!("{line}\n").as_bytes())
    }
}

/// `line` if it is at most `max` bytes, else as much of it as fits in
/// `max` with "…" at the end. It is never cut inside a letter.
fn clipped(line: &str, max: usize) -> Cow<'_, str> {
    if line.len() <= max {
        return Cow::Borrowed(line);
    }
    const MORE: &str = "…";
    let mut end = max.saturating_sub(MORE.len());
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    Cow::Owned(format!("{}{MORE}", &line[..end]))
}

/// The app's log, once [`init`] has said where. Nothing done while it is
/// locked can panic, so the panic hook's own line never waits on a lock
/// its thread already holds.
static LOG: Mutex<Option<LogFile>> = Mutex::new(None);

/// Log to `kiln.log` in `dir` from now on.
pub fn init(dir: PathBuf) {
    *LOG.lock().unwrap_or_else(|e| e.into_inner()) = Some(LogFile::new(dir, LOG_MAX));
}

/// Log `msg`: a line in `kiln.log` with the local time and this thread's
/// name, and the message for an attached debugger. A line that can't be
/// written is let go: logging never stops Kiln.
pub fn log(msg: &str) {
    let p = platform();
    (p.debug)(msg);
    let line = line_at((p.now)(), &thread_name(), msg);
    if let Some(file) = LOG.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        let _ = file.append(&line);
    }
}

/// One entry of the log, `YYYY-MM-DD HH:MM:SS [thread] msg`. Further lines
/// of `msg`, such as an engine's own words, are indented under it, so each
/// entry starts with its time.
fn line_at(at: LocalTime, thread: &str, msg: &str) -> String {
    let (y, mo, d, h, mi, s) = at;
    let msg = msg.lines().collect::<Vec<_>>().join("\n    ");
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02} [{thread}] {msg}")
}

/// This thread's name, as the log and crash files give it.
fn thread_name() -> String {
    std::thread::current()
        .name()
        .unwrap_or("unnamed")
        .to_owned()
}

/// What a crash file says about one panic.
pub struct CrashInfo {
    message: String,
    location: String,
    thread: String,
    backtrace: String,
    version: &'static str,
    commit: &'static str,
    windows: String,
    module_base: usize,
}

impl CrashInfo {
    /// The panic `info` tells of, on this thread.
    fn of(info: &PanicHookInfo) -> Self {
        let p = platform();
        let payload = info.payload();
        let message = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "a panic with no message".to_owned());
        CrashInfo {
            message,
            location: info.location().map_or_else(
                || "an unknown place".to_owned(),
                |l| format!("{}:{}:{}", l.file(), l.line(), l.column()),
            ),
            thread: thread_name(),
            // Every frame with its address. Release builds have no
            // symbols, so the addresses, the load address and the commit
            // are what decode it later, with that build's symbols.
            backtrace: format!("{:#}", Backtrace::force_capture()),
            version: env!("CARGO_PKG_VERSION"),
            commit: env!("KILN_COMMIT"),
            windows: (p.windows)(),
            module_base: (p.module_base)(),
        }
    }

    /// The crash file's text.
    pub fn report(&self) -> String {
        let CrashInfo {
            message,
            location,
            thread,
            backtrace,
            version,
            commit,
            windows,
            module_base,
        } = self;
        format!(
            "Kiln {version} ({commit})\n\
             Windows {windows}\n\
             kiln-app.exe loaded at {module_base:#x}\n\
             \n\
             Panic on thread {thread}: {message}\n\
             at {location}\n\
             \n\
             Backtrace:\n\
             {backtrace}\n"
        )
    }
}

/// Write `info`'s report to `crash-{stamp}-{pid}.txt` in `dir`, making the
/// folder if need be. A second panic in the same second adds its report to
/// that file, after the first.
pub fn write_crash(dir: &Path, info: &CrashInfo, stamp: &str, pid: u32) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let path = dir.join(format!("crash-{stamp}-{pid}.txt"));
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    if file.metadata()?.len() > 0 {
        file.write_all(b"\n")?;
    }
    file.write_all(info.report().as_bytes())?;
    Ok(path)
}

/// Remove all but the newest `keep` crash files in `dir`. Only plain files
/// named `crash-*.txt` count, and their names say their age, so the oldest
/// name goes first. Nothing else in the folder is touched, and a file that
/// can't be removed stays.
pub fn prune_crashes(dir: &Path, keep: usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut crashes: Vec<(std::ffi::OsString, PathBuf)> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.starts_with("crash-") && n.ends_with(".txt"))
        })
        .map(|e| (e.file_name(), e.path()))
        .collect();
    crashes.sort();
    let old = crashes.len().saturating_sub(keep);
    for (_, path) in crashes.iter().take(old) {
        let _ = fs::remove_file(path);
    }
}

/// Have every panic leave a crash file and a line in the log, then do what
/// Rust does by default, so a debugger still sees it. Call it first thing
/// in `main`.
///
/// A panic inside a panic hook aborts the process, and `catch_unwind`
/// can't stop that, so the hook is written never to panic: it unwraps
/// nothing, and every failure to write is let go.
pub fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        on_panic(info, crate::settings::logs_dir());
        default(info);
    }));
}

/// Leave a crash file in `dir` for the panic `info` tells of, keep the
/// newest, and say so in the log.
fn on_panic(info: &PanicHookInfo, dir: Option<PathBuf>) {
    let crash = CrashInfo::of(info);
    let saved = dir.map(|dir| {
        // Pruned to one fewer first, so the new file is always one of
        // those kept, even when the clock has gone back.
        prune_crashes(&dir, KEEP_CRASHES - 1);
        let (y, mo, d, h, mi, s) = (platform().now)();
        let stamp = format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}");
        write_crash(&dir, &crash, &stamp, std::process::id())
    });
    let report = match saved {
        Some(Ok(path)) => format!("the report is in {}", path.display()),
        Some(Err(e)) => format!("the report couldn't be saved: {e}"),
        None => "LOCALAPPDATA isn't set, so no report was saved".to_owned(),
    };
    log(&format!(
        "panic at {}: {}; {report}",
        crash.location, crash.message
    ));
}

/// How many lines of the log Copy diagnostics takes.
pub const DIAGNOSTIC_LINES: usize = 200;

/// What Copy diagnostics says of the machine, ahead of the log.
pub struct Environment {
    /// "1.2.3 (abc1234)".
    pub kiln: String,
    /// The Windows version and build, "10.0.26300.1".
    pub windows: String,
    /// "Windows 11 Pro", when Windows says.
    pub edition: Option<String>,
    /// Where `kiln-app.exe` is running from.
    pub runs_from: Option<PathBuf>,
    /// Each helper app and what Kiln found of it, in the order shown in
    /// Settings.
    pub helpers: Vec<(String, String)>,
}

/// What Kiln found of a helper app, in a line.
pub fn helper_state(st: &crate::settings::EngineStatus) -> String {
    use crate::settings::EngineStatus as S;
    match st {
        S::Included(p) => format!("Included, {}", p.display()),
        S::OnPath(p) | S::Installed(p) => format!("Found, {}", p.display()),
        S::Chosen { path, exists: true } => format!("Chosen, {}", path.display()),
        S::Chosen { path, .. } => format!("Chosen, not found at {}", path.display()),
        S::Missing => "Not found".to_owned(),
    }
}

/// Windows' product name made true for Windows 11: the registry still
/// calls it "Windows 10 ..." there, and `build` says which it is.
pub fn edition_name(product: &str, build: u32) -> String {
    match product.strip_prefix("Windows 10") {
        Some(rest) if build >= 22000 => format!("Windows 11{rest}"),
        _ => product.to_owned(),
    }
}

/// The longest a log line is in the copied text, in characters.
pub const DIAGNOSTIC_LINE_CHARS: usize = 1_000;

/// The most the copied text's log part holds, in characters, so the whole
/// stays well inside what a GitHub issue takes (65,536).
pub const DIAGNOSTIC_LOG_CHARS: usize = 32_000;

/// How much of the end of each log file Copy diagnostics reads.
const TAIL_READ_BYTES: u64 = 64 * 1024;

/// `text` with the start of every path under `profile` (the user's
/// profile folder) said as `%USERPROFILE%`. Letters match whatever their
/// case, and only a whole folder name does: `C:\Users\me` is found in
/// `C:\Users\me\x`, not in `C:\Users\mel`.
pub fn hide_profile(text: &str, profile: Option<&str>) -> String {
    let profile = profile.map_or("", |p| p.trim_end_matches(['\\', '/']));
    if profile.is_empty() {
        return text.to_owned();
    }
    let want = profile.to_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while at < text.len() {
        let end = at + profile.len();
        let hit = text.get(at..end).is_some_and(|w| w.to_lowercase() == want)
            && text[end..].chars().next().is_none_or(|c| {
                c.is_whitespace() || matches!(c, '\\' | '/' | '"' | '\'' | ',' | ';' | ')' | ']')
            });
        if hit {
            out.push_str("%USERPROFILE%");
            at = end;
        } else {
            let c = text[at..].chars().next().unwrap();
            out.push(c);
            at += c.len_utf8();
        }
    }
    out
}

/// `line` cut to `max` characters, the last of them "…" when it was cut.
fn clip_chars(line: &str, max: usize) -> Cow<'_, str> {
    if line.chars().nth(max).is_none() {
        return Cow::Borrowed(line);
    }
    let keep = line
        .char_indices()
        .nth(max - 1)
        .map_or(line.len(), |(i, _)| i);
    Cow::Owned(format!("{}…", &line[..keep]))
}

/// The text Copy diagnostics puts on the clipboard: the environment, then
/// `log`, the log's last lines, oldest first. Each log line is clipped to
/// [`DIAGNOSTIC_LINE_CHARS`], and the oldest lines go first when the log
/// part would pass [`DIAGNOSTIC_LOG_CHARS`]. `profile` is the user's
/// profile folder, which no line names.
pub fn diagnostics_text(env: &Environment, log: &[String], profile: Option<&str>) -> String {
    let hide = |t: &str| hide_profile(t, profile);
    let mut out = format!("Kiln {}\n", env.kiln);
    match &env.edition {
        Some(e) => out.push_str(&format!("Windows {} ({e})\n", env.windows)),
        None => out.push_str(&format!("Windows {}\n", env.windows)),
    }
    match &env.runs_from {
        Some(p) => out.push_str(&format!("Runs from {}\n", hide(&p.display().to_string()))),
        None => out.push_str("Runs from an unknown place\n"),
    }
    out.push_str("\nHelper apps\n");
    for (name, state) in &env.helpers {
        out.push_str(&format!("  {name}: {}\n", hide(state)));
    }
    // From the newest line back, while they fit.
    let mut kept: Vec<String> = vec![];
    let mut used = 0;
    for line in log.iter().rev() {
        let line = clip_chars(&hide(line), DIAGNOSTIC_LINE_CHARS).into_owned();
        let cost = line.chars().count() + 1;
        if used + cost > DIAGNOSTIC_LOG_CHARS {
            break;
        }
        used += cost;
        kept.push(line);
    }
    if kept.is_empty() {
        out.push_str("\nLog: empty\n");
    } else {
        out.push_str(&format!(
            "\nLog, last {} lines (can name files and folders)\n",
            kept.len()
        ));
        for line in kept.iter().rev() {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// The last `n` lines of the log, oldest first: those of `current`
/// (`kiln.log`), and when it has fewer than `n`, the end of `rolled`
/// (`kiln.1.log`) before them.
pub fn log_tail(rolled: Option<&str>, current: Option<&str>, n: usize) -> Vec<String> {
    let mut lines: Vec<&str> = rolled.into_iter().flat_map(str::lines).collect();
    lines.extend(current.into_iter().flat_map(str::lines));
    let from = lines.len().saturating_sub(n);
    lines[from..].iter().map(|l| (*l).to_owned()).collect()
}

/// `%LOCALAPPDATA%\Kiln\logs`, made if it isn't there, or `None` when it
/// can't be. For Open log folder, on the press that opens it.
pub fn make_logs_dir() -> Option<PathBuf> {
    let dir = crate::settings::logs_dir()?;
    fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// The end of the file at `path`, at most [`TAIL_READ_BYTES`] of it, from
/// the start of a line. `None` if it can't be read.
fn read_end(path: &Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let cut = len > TAIL_READ_BYTES;
    if cut {
        f.seek(SeekFrom::Start(len - TAIL_READ_BYTES)).ok()?;
    }
    let mut bytes = vec![];
    f.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    if !cut {
        return Some(text);
    }
    // The first line is most likely cut through.
    Some(
        text.split_once('\n')
            .map_or(String::new(), |(_, rest)| rest.to_owned()),
    )
}

/// [`log_tail`] of the log files in `dir`: what can't be read is left out,
/// and only the end of each file is read.
pub fn read_log_tail(dir: &Path, n: usize) -> Vec<String> {
    let read = |name: &str| read_end(&dir.join(name));
    log_tail(read(ROLLED_NAME).as_deref(), read(LOG_NAME).as_deref(), n)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The names in `dir`, sorted.
    fn names_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn the_log_rolls_at_its_limit() {
        let tmp = tempfile::tempdir().unwrap();
        // The first line makes the folder.
        let dir = tmp.path().join("logs");
        let log = LogFile::new(dir.clone(), 100);
        let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap();
        // 39 letters and the line end: 40 bytes each.
        let line = |c: char| c.to_string().repeat(39);
        let lines = |cs: &str| {
            cs.chars()
                .map(|c| format!("{}\n", line(c)))
                .collect::<String>()
        };

        log.append(&line('a')).unwrap();
        log.append(&line('b')).unwrap();
        assert_eq!(names_in(&dir), ["kiln.log"]);
        assert_eq!(read("kiln.log"), lines("ab"));

        // 120 bytes would pass 100: the first two move to kiln.1.log.
        log.append(&line('c')).unwrap();
        assert_eq!(names_in(&dir), ["kiln.1.log", "kiln.log"]);
        assert_eq!(read("kiln.1.log"), lines("ab"));
        assert_eq!(read("kiln.log"), lines("c"));

        // The next roll replaces kiln.1.log; there are still two files.
        log.append(&line('d')).unwrap();
        assert_eq!(names_in(&dir), ["kiln.1.log", "kiln.log"]);
        log.append(&line('e')).unwrap();
        assert_eq!(names_in(&dir), ["kiln.1.log", "kiln.log"]);
        assert_eq!(read("kiln.1.log"), lines("cd"));
        assert_eq!(read("kiln.log"), lines("e"));
    }

    #[test]
    fn a_huge_line_is_cut_short() {
        // Kept whole up to the limit, and cut to it past it, ending in "…".
        assert_eq!(clipped("abcdefghij", 10), "abcdefghij");
        assert_eq!(clipped("abcdefghijk", 10), "abcdefg…");
        // Never inside a letter: "é" is two bytes, so seven is too many.
        assert_eq!(clipped(&"é".repeat(6), 10), "ééé…");

        // The app's log cuts at an eighth of its size.
        assert_eq!(LINE_MAX as u64, LOG_MAX / 8);
        let tmp = tempfile::tempdir().unwrap();
        let log = LogFile::new(tmp.path().to_owned(), LOG_MAX);
        let huge = "x".repeat(LOG_MAX as usize * 2);
        log.append(&huge).unwrap();
        let text = std::fs::read_to_string(tmp.path().join("kiln.log")).unwrap();
        assert_eq!(text.len(), LINE_MAX + 1, "the line and its end");
        assert!(text.ends_with("x…\n"));
        assert!(huge.starts_with(text.trim_end_matches("…\n")));
    }

    #[test]
    fn a_log_line_has_the_time_and_the_thread() {
        let at = (2026, 3, 4, 5, 6, 7);
        assert_eq!(
            line_at(at, "kiln-thumbs", "cleared the folder"),
            "2026-03-04 05:06:07 [kiln-thumbs] cleared the folder"
        );
        // An engine's many lines stay under the one that has the time.
        assert_eq!(
            line_at(at, "main", "job failed: x\r\nline 2\nline 3\n"),
            "2026-03-04 05:06:07 [main] job failed: x\n    line 2\n    line 3"
        );
    }

    fn info() -> CrashInfo {
        CrashInfo {
            message: "the wheel came off".to_owned(),
            location: "src/x.rs:1:2".to_owned(),
            thread: "kiln-thumbs".to_owned(),
            backtrace: "   0:     0x7ff6a1b2c3d4 - <unknown>".to_owned(),
            version: "0.1.0",
            commit: "abc1234",
            windows: "10.0.26300.1".to_owned(),
            module_base: 0x7ff6_a1b2_0000,
        }
    }

    #[test]
    fn a_crash_report_names_everything() {
        let text = info().report();
        for part in [
            "the wheel came off",
            "at src/x.rs:1:2",
            "kiln-thumbs",
            "Kiln 0.1.0 (abc1234)",
            "Windows 10.0.26300.1",
            "0x7ff6a1b20000",
            "0x7ff6a1b2c3d4 - <unknown>",
        ] {
            assert!(text.contains(part), "{part:?} is missing from:\n{text}");
        }
    }

    #[test]
    fn a_crash_file_is_named_for_its_time_and_process() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("logs");
        let path = write_crash(&dir, &info(), "20261026-142233", 4120).unwrap();
        assert_eq!(path, dir.join("crash-20261026-142233-4120.txt"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), info().report());
        // A second panic in the same second keeps the first report.
        write_crash(&dir, &info(), "20261026-142233", 4120).unwrap();
        let both = std::fs::read_to_string(&path).unwrap();
        assert_eq!(both.matches("the wheel came off").count(), 2, "{both}");
    }

    #[test]
    fn a_panic_leaves_a_crash_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("logs");
        // The hook is the process's, so it's put back at once.
        let previous = std::panic::take_hook();
        let into = dir.clone();
        std::panic::set_hook(Box::new(move |info| on_panic(info, Some(into.clone()))));
        let ended = std::thread::Builder::new()
            .name("kiln-test-panic".to_owned())
            .spawn(|| panic!("the wheel came off: {}", 7))
            .unwrap()
            .join();
        std::panic::set_hook(previous);
        assert!(ended.is_err());

        // The platform isn't set in tests: the time is the fixed one.
        let name = format!("crash-20000101-000000-{}.txt", std::process::id());
        assert_eq!(names_in(&dir), std::slice::from_ref(&name));
        let text = std::fs::read_to_string(dir.join(&name)).unwrap();
        for part in [
            "Panic on thread kiln-test-panic: the wheel came off: 7\nat ",
            "diag.rs:",
            concat!("Kiln ", env!("CARGO_PKG_VERSION"), " ("),
            "Windows unknown",
            "loaded at 0x0",
            "Backtrace:\n",
        ] {
            assert!(text.contains(part), "{part:?} is missing from:\n{text}");
        }
    }

    #[test]
    fn pruning_keeps_the_newest_crash_files_only() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let crash = |day: u32| format!("crash-202610{day:02}-120000-77.txt");
        // Written out of order: the name, not the write, says which is old.
        for day in (1..=12).rev() {
            std::fs::write(dir.join(crash(day)), "report").unwrap();
        }
        std::fs::write(dir.join("kiln.log"), "log").unwrap();
        std::fs::write(dir.join("other.txt"), "other").unwrap();

        prune_crashes(dir, 10);

        let mut kept: Vec<String> = (3..=12).map(crash).collect();
        kept.push("kiln.log".to_owned());
        kept.push("other.txt".to_owned());
        kept.sort();
        assert_eq!(names_in(dir), kept);
        let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap();
        assert_eq!(read("kiln.log"), "log");
        assert_eq!(read("other.txt"), "other");
    }

    #[test]
    fn diagnostics_text_has_the_environment_then_the_log() {
        let env = Environment {
            kiln: "1.2.3 (abc1234)".into(),
            windows: "10.0.26300.1".into(),
            edition: Some("Windows 11 Pro".into()),
            runs_from: Some(PathBuf::from(r"C:\Kiln\kiln-app.exe")),
            helpers: vec![
                ("FFmpeg".into(), r"Included, C:\Kiln\ffmpeg.exe".into()),
                ("Pandoc".into(), "Not found".into()),
            ],
        };
        let log = vec!["one".to_owned(), "two".to_owned()];
        assert_eq!(
            diagnostics_text(&env, &log, None),
            "Kiln 1.2.3 (abc1234)\n\
             Windows 10.0.26300.1 (Windows 11 Pro)\n\
             Runs from C:\\Kiln\\kiln-app.exe\n\
             \n\
             Helper apps\n  \
             FFmpeg: Included, C:\\Kiln\\ffmpeg.exe\n  \
             Pandoc: Not found\n\
             \n\
             Log, last 2 lines (can name files and folders)\n\
             one\n\
             two\n"
        );
        let bare = Environment {
            edition: None,
            runs_from: None,
            helpers: vec![],
            ..env
        };
        let text = diagnostics_text(&bare, &[], None);
        assert!(text.contains("Windows 10.0.26300.1\n"));
        assert!(text.contains("Runs from an unknown place"));
        assert!(text.ends_with("Log: empty\n"));
    }

    #[test]
    fn a_helper_is_told_by_where_it_was_found() {
        use crate::settings::EngineStatus as S;
        let p = || PathBuf::from(r"C:\Tools\x.exe");
        assert_eq!(helper_state(&S::Included(p())), r"Included, C:\Tools\x.exe");
        assert_eq!(helper_state(&S::OnPath(p())), r"Found, C:\Tools\x.exe");
        assert_eq!(helper_state(&S::Installed(p())), r"Found, C:\Tools\x.exe");
        assert_eq!(
            helper_state(&S::Chosen {
                path: p(),
                exists: true
            }),
            r"Chosen, C:\Tools\x.exe"
        );
        assert_eq!(
            helper_state(&S::Chosen {
                path: p(),
                exists: false
            }),
            r"Chosen, not found at C:\Tools\x.exe"
        );
        assert_eq!(helper_state(&S::Missing), "Not found");
    }

    #[test]
    fn windows_11_is_called_that_whatever_the_registry_says() {
        assert_eq!(edition_name("Windows 10 Pro", 26300), "Windows 11 Pro");
        assert_eq!(edition_name("Windows 10 Pro", 19045), "Windows 10 Pro");
        assert_eq!(edition_name("Windows 11 Home", 26100), "Windows 11 Home");
    }

    #[test]
    fn the_log_tail_is_the_last_lines_and_reaches_into_the_rolled_file() {
        let many: String = (1..=300).map(|i| format!("line {i}\n")).collect();
        let tail = log_tail(Some("old 1\nold 2\n"), Some(&many), 200);
        assert_eq!(tail.len(), 200);
        assert_eq!(tail[0], "line 101");
        assert_eq!(tail[199], "line 300");
        // Fewer than asked for: the rolled file's end comes first.
        let tail = log_tail(Some("old 1\nold 2\nold 3\n"), Some("a\nb\n"), 4);
        assert_eq!(tail, ["old 2", "old 3", "a", "b"]);
        assert_eq!(log_tail(None, None, 200), Vec::<String>::new());
        assert_eq!(log_tail(None, Some("only\n"), 200), ["only"]);
    }

    #[test]
    fn the_log_tail_is_read_from_the_two_files() {
        let dir = std::env::temp_dir().join(format!("kiln-tail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(read_log_tail(&dir, 5).is_empty());
        std::fs::write(dir.join("kiln.1.log"), "r1\nr2\n").unwrap();
        std::fs::write(dir.join("kiln.log"), "c1\n").unwrap();
        assert_eq!(read_log_tail(&dir, 5), ["r1", "r2", "c1"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn bare_env() -> Environment {
        Environment {
            kiln: "1".into(),
            windows: "10".into(),
            edition: None,
            runs_from: None,
            helpers: vec![],
        }
    }

    #[test]
    fn a_log_line_is_clipped_and_the_oldest_go_first() {
        let env = bare_env();
        assert_eq!(clip_chars("abc", 3), "abc");
        assert_eq!(clip_chars("abcd", 3), "ab…");
        assert_eq!(clip_chars(&"é".repeat(1001), 1000).chars().count(), 1000);
        // One huge line is clipped to 1,000 characters.
        let text = diagnostics_text(&env, &["x".repeat(130_000)], None);
        let line = text.lines().last().unwrap();
        assert_eq!(line.chars().count(), DIAGNOSTIC_LINE_CHARS);
        assert!(line.ends_with("x…"));
        // 200 long lines pass the budget: the newest stay, in order.
        let log: Vec<String> = (0..200)
            .map(|i| format!("{i:03}{}", "y".repeat(997)))
            .collect();
        let text = diagnostics_text(&env, &log, None);
        let part = text.split_once("Log, last ").unwrap().1;
        let lines: Vec<&str> = part.lines().skip(1).collect();
        assert!(lines.len() < 200 && lines.len() >= 30);
        assert!(lines.last().unwrap().starts_with("199"));
        assert!(part.starts_with(&format!("{} lines", lines.len())));
        let first: usize = lines[0][..3].parse().unwrap();
        assert_eq!(first + lines.len(), 200);
        assert!(text.chars().count() < 33_000);
    }

    #[test]
    fn the_profile_folder_is_said_as_the_variable() {
        let me = Some(r"C:\Users\Mo Ann");
        assert_eq!(
            hide_profile(r"C:\users\mo ann\AppData\x.exe", me),
            r"%USERPROFILE%\AppData\x.exe"
        );
        assert_eq!(
            hide_profile(r"opened C:\Users\Mo Ann and (C:\Users\Mo Ann)", me),
            r"opened %USERPROFILE% and (%USERPROFILE%)"
        );
        // Not a longer folder name, and not another folder.
        for same in [r"C:\Users\Mo Annie\x", r"D:\Users\Mo Ann2", r"C:\Kiln\x"] {
            assert_eq!(hide_profile(same, me), same);
        }
        assert_eq!(
            hide_profile(r"C:\Users\Mo Ann\x", None),
            r"C:\Users\Mo Ann\x"
        );
        assert_eq!(hide_profile("abc", Some("")), "abc");
        // It reaches the path, the helpers and the log in the copied text.
        let env = Environment {
            runs_from: Some(PathBuf::from(r"C:\Users\Mo Ann\Kiln\kiln-app.exe")),
            helpers: vec![(
                "Pandoc".into(),
                r"Found, C:\Users\Mo Ann\p\pandoc.exe".into(),
            )],
            ..bare_env()
        };
        let log = [r"failed on C:\Users\Mo Ann\a.docx".to_owned()];
        let text = diagnostics_text(&env, &log, me);
        assert!(!text.contains("Mo Ann"), "{text}");
        assert_eq!(text.matches("%USERPROFILE%").count(), 3);
    }

    #[test]
    fn only_the_end_of_a_big_log_file_is_read() {
        let dir = std::env::temp_dir().join(format!("kiln-tail-big-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let big: String = (0..20_000).map(|i| format!("line {i}\n")).collect();
        assert!(big.len() as u64 > TAIL_READ_BYTES);
        std::fs::write(dir.join("kiln.log"), &big).unwrap();
        let tail = read_log_tail(&dir, 100_000);
        assert!(tail.len() < 20_000);
        assert_eq!(tail.last().unwrap(), "line 19999");
        // The first line is whole: the cut one was dropped.
        let n: usize = tail[0].strip_prefix("line ").unwrap().parse().unwrap();
        assert_eq!(n + tail.len(), 20_000);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
