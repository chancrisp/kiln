//! Running external engines. Arguments are always passed one by one to
//! `std::process::Command`; nothing is ever assembled into a shell string.

use std::ffi::{OsStr, OsString};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::converter::CancelToken;
use crate::error::{Error, Result};

/// Keep this much of the start and of the end of stderr. The start has
/// the error; the end has the last word of analysis passes.
const STDERR_KEEP: usize = 32 * 1024;
const POLL: Duration = Duration::from_millis(25);

/// Look for `program` (adding `.exe` on Windows) in each folder of a
/// PATH-style list.
pub fn find_on_path(path_var: &OsStr, program: &str) -> Option<PathBuf> {
    let name = if cfg!(windows) {
        format!("{program}.exe")
    } else {
        program.to_owned()
    };
    std::env::split_paths(path_var)
        .map(|dir| dir.join(&name))
        .find(|p| p.is_file())
}

/// PATH as a program started now would get it: the user's and then the
/// system's PATH as the registry holds them, `%VAR%` expanded, then any
/// folder only Kiln's own PATH has. A helper app installed while Kiln
/// runs is found without a restart. With neither registry value, Kiln's
/// own PATH.
///
/// It reads two registry values, so call it off the window's thread.
pub fn current_path() -> OsString {
    let process = std::env::var_os("PATH").unwrap_or_default();
    // A test that takes a helper off PATH asks for Kiln's own PATH alone
    // (`KILN_TEST_PROCESS_PATH`); it can only narrow what is found.
    // Read only by a debug or test-hooks build; a shipped one reads it as unset.
    #[cfg(any(debug_assertions, feature = "test-hooks"))]
    if std::env::var_os("KILN_TEST_PROCESS_PATH").is_some() {
        return process;
    }
    // A `%NAME%` Kiln's own environment lacks (a variable added since
    // Kiln started) is read where Windows keeps it: the user's, then the
    // machine's.
    let own = |name: &str| std::env::var(name).ok();
    let sources: [Lookup; 3] = [&own, &registry::user_var, &registry::system_var];
    let fresh = join_paths(
        registry::user_path().as_deref(),
        registry::system_path().as_deref(),
        first_of(&sources),
    );
    with_process(fresh, &process)
}

/// The user's PATH value and then the system's, as the registry holds
/// them, each folder's `%VAR%` expanded by `env`. Empty entries are
/// dropped. `None` when neither has a folder.
pub fn join_paths(
    user: Option<&str>,
    system: Option<&str>,
    env: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    let dirs: Vec<String> = [user, system]
        .into_iter()
        .flatten()
        .flat_map(|list| list.split(';'))
        .filter(|d| !d.trim().is_empty())
        .map(|d| expand(d, &env))
        .collect();
    (!dirs.is_empty()).then(|| dirs.join(";"))
}

/// Where a variable's value may be found: `None` where it isn't.
type Lookup<'a> = &'a dyn Fn(&str) -> Option<String>;

/// A variable's value from the first of `sources` that has it.
fn first_of<'a>(sources: &'a [Lookup<'a>]) -> impl Fn(&str) -> Option<String> + 'a {
    move |name| sources.iter().find_map(|s| s(name))
}

/// `text` with each `%NAME%` that `env` knows put in its place, and
/// every other `%` left as it is, as Windows expands a PATH.
fn expand(text: &str, env: &impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('%') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = &after[..end];
        match (!name.is_empty()).then(|| env(name)).flatten() {
            Some(value) => {
                out.push_str(&value);
                rest = &after[end + 1..];
            }
            // Not a variable: the second % may open one.
            None => {
                out.push('%');
                out.push_str(name);
                rest = &after[end..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// `fresh` (the registry's PATH), then each folder of `process` it
/// doesn't already have; `process` alone when the registry had none.
fn with_process(fresh: Option<String>, process: &OsStr) -> OsString {
    let Some(fresh) = fresh else {
        return process.to_owned();
    };
    let key = |d: &Path| {
        d.to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_lowercase()
    };
    let known: std::collections::HashSet<String> =
        fresh.split(';').map(|d| key(Path::new(d))).collect();
    let mut out = OsString::from(fresh);
    for dir in std::env::split_paths(process) {
        if dir.as_os_str().is_empty() || known.contains(&key(&dir)) {
            continue;
        }
        out.push(";");
        out.push(dir.as_os_str());
    }
    out
}

/// The two PATH values in the registry.
#[cfg(windows)]
mod registry {
    use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        RegGetValueW, HKEY, HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_NOEXPAND,
        RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ,
    };

    const USER: &str = "Environment";
    const SYSTEM: &str = r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment";

    pub(super) fn user_path() -> Option<String> {
        read(HKEY_CURRENT_USER, USER, "Path", false)
    }

    pub(super) fn system_path() -> Option<String> {
        read(HKEY_LOCAL_MACHINE, SYSTEM, "Path", false)
    }

    /// The user's variable `name`, its own `%VAR%` expanded.
    pub(super) fn user_var(name: &str) -> Option<String> {
        read(HKEY_CURRENT_USER, USER, name, true)
    }

    /// The machine's variable `name`, its own `%VAR%` expanded.
    pub(super) fn system_var(name: &str) -> Option<String> {
        read(HKEY_LOCAL_MACHINE, SYSTEM, name, true)
    }

    /// The default value of `key` under `HKEY_CLASSES_ROOT`.
    pub(super) fn class_value(key: &str) -> Option<String> {
        read(HKEY_CLASSES_ROOT, key, "", true)
    }

    /// The value `name` under `root\key`, expanded by Windows when
    /// `expand`; `None` when it isn't there or can't be read.
    fn read(root: HKEY, key: &str, name: &str, expand: bool) -> Option<String> {
        let key: Vec<u16> = key.encode_utf16().chain(Some(0)).collect();
        let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        // Without RRF_NOEXPAND, Windows expands a REG_EXPAND_SZ and
        // returns it as REG_SZ.
        let flags = if expand {
            RRF_RT_REG_SZ
        } else {
            RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND
        };
        // The value may grow between asking its size and reading it.
        for _ in 0..3 {
            let mut bytes = 0u32;
            // SAFETY: both names are NUL-ended UTF-16 that outlive the
            // call; a null data pointer asks only for the size, written
            // to `bytes`.
            let r = unsafe {
                RegGetValueW(
                    root,
                    key.as_ptr(),
                    name.as_ptr(),
                    flags,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut bytes,
                )
            };
            if r != ERROR_SUCCESS {
                return None;
            }
            let mut buf = vec![0u16; (bytes as usize).div_ceil(2) + 1];
            let mut size = (buf.len() * 2) as u32;
            // SAFETY: `buf` holds `size` bytes, and the call writes at
            // most that many, saying how many in `size`.
            let r = unsafe {
                RegGetValueW(
                    root,
                    key.as_ptr(),
                    name.as_ptr(),
                    flags,
                    std::ptr::null_mut(),
                    buf.as_mut_ptr().cast(),
                    &mut size,
                )
            };
            if r == ERROR_MORE_DATA {
                continue;
            }
            if r != ERROR_SUCCESS {
                return None;
            }
            buf.truncate(size as usize / 2);
            // An expanded value may be shorter than the size given, which
            // is the stored one's: it ends at its first NUL.
            if let Some(end) = buf.iter().position(|&c| c == 0) {
                buf.truncate(end);
            }
            return Some(String::from_utf16_lossy(&buf));
        }
        None
    }
}

#[cfg(not(windows))]
mod registry {
    pub(super) fn user_path() -> Option<String> {
        None
    }

    pub(super) fn system_path() -> Option<String> {
        None
    }

    pub(super) fn user_var(_: &str) -> Option<String> {
        None
    }

    pub(super) fn system_var(_: &str) -> Option<String> {
        None
    }

    pub(super) fn class_value(_: &str) -> Option<String> {
        None
    }
}

/// The default value of `key` under `HKEY_CLASSES_ROOT`, such as
/// `Word.Application\CLSID`; `None` when it isn't there.
pub(crate) fn class_value(key: &str) -> Option<String> {
    registry::class_value(key)
}

/// The executable for an engine: the configured path, or its bare name so
/// the OS searches PATH.
pub(crate) fn program(configured: Option<&Path>, name: &str) -> PathBuf {
    configured
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(name))
}

/// A sibling tool next to a configured engine ("ffprobe" beside "ffmpeg"),
/// or the bare name if the engine comes from PATH.
pub(crate) fn sibling(configured: Option<&Path>, name: &str) -> PathBuf {
    let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    match configured.and_then(Path::parent) {
        Some(dir) if !dir.as_os_str().is_empty() => dir.join(file),
        _ => PathBuf::from(name),
    }
}

/// Run `cmd` to completion, feeding each stdout line to `on_line`, and
/// return its stderr.
///
/// Kills the process and returns `Error::Cancelled` if `cancel` is set.
/// A missing executable becomes `Error::EngineMissing`; a non-zero exit
/// becomes `Error::EngineFailed` with the first line of stderr.
pub(crate) fn run(
    engine: &'static str,
    mut cmd: Command,
    cancel: &CancelToken,
    mut on_line: impl FnMut(&str),
) -> Result<String> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_console(&mut cmd);
    // An engine named without a folder is looked for on PATH as it is
    // now, so one installed while Kiln runs starts (DIST-07).
    if !cmd.get_envs().any(|(k, _)| k.eq_ignore_ascii_case("PATH")) {
        cmd.env("PATH", current_path());
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::EngineMissing {
                engine: helper_named(engine),
            })
        }
        Err(e) => return Err(Error::io(cmd.get_program(), e)),
    };
    // Dropped on every return below, which ends anything the engine
    // started and left running.
    let tree = ProcessTree::adopt(&child);

    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");

    let (tx, rx) = mpsc::channel::<String>();
    let out_reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(l) => {
                    if tx.send(l).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    let err_reader = thread::spawn(move || {
        // Keep the start and the end, and keep draining so a chatty
        // engine never blocks on a full pipe.
        let mut stderr = stderr;
        let mut head = Vec::new();
        let mut tail = std::collections::VecDeque::new();
        let mut dropped = false;
        let mut chunk = [0u8; 8192];
        while let Ok(n) = stderr.read(&mut chunk) {
            if n == 0 {
                break;
            }
            let room = STDERR_KEEP.saturating_sub(head.len());
            head.extend_from_slice(&chunk[..n.min(room)]);
            for &b in &chunk[n.min(room)..n] {
                if tail.len() == STDERR_KEEP {
                    tail.pop_front();
                    dropped = true;
                }
                tail.push_back(b);
            }
        }
        let mut text = String::from_utf8_lossy(&head).into_owned();
        if dropped {
            text.push_str("\n[...]\n");
        }
        text.push_str(&String::from_utf8_lossy(tail.make_contiguous()));
        text
    });

    let status = loop {
        if cancel.is_cancelled() {
            drop(tree);
            let _ = child.kill();
            let _ = child.wait();
            let _ = out_reader.join();
            let _ = err_reader.join();
            return Err(Error::Cancelled);
        }
        match rx.recv_timeout(POLL) {
            Ok(line) => {
                on_line(&line);
                continue;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => thread::sleep(POLL),
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => return Err(Error::io(cmd.get_program(), e)),
        }
    };

    // A helper the engine started may still hold the output pipes open.
    drop(tree);
    let _ = out_reader.join();
    for line in rx.try_iter() {
        on_line(&line);
    }
    let stderr = err_reader.join().unwrap_or_default();

    if status.success() {
        return Ok(stderr);
    }
    let summary = stderr
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| strip_tags(l).to_owned())
        .unwrap_or_else(|| match status.code() {
            Some(code) => format!("it stopped with exit code {code}."),
            None => "it was stopped by the system.".to_owned(),
        });
    Err(Error::EngineFailed {
        engine,
        summary,
        details: stderr,
    })
}

/// The helper app a person installs for `engine`: FFprobe comes with
/// FFmpeg, so a missing FFprobe names FFmpeg.
fn helper_named(engine: &'static str) -> &'static str {
    match engine {
        "FFprobe" => "FFmpeg",
        other => other,
    }
}

/// Drop the "[in#0 @ 0x55d0]" style prefixes engines put on log lines:
/// they mean nothing to the person reading the error.
fn strip_tags(line: &str) -> &str {
    let mut rest = line;
    while let Some(after) = rest.strip_prefix('[') {
        match after.find("] ") {
            Some(end) => rest = after[end + 2..].trim_start(),
            None => break,
        }
    }
    if rest.is_empty() {
        line
    } else {
        rest
    }
}

/// An engine and every process it starts. Dropping it ends them all, so a
/// cancelled job never leaves a helper behind, such as the soffice.bin
/// that LibreOffice's launcher starts. A Windows job object with
/// kill-on-close does the work; if one can't be made, only the engine
/// itself is stopped.
#[cfg(windows)]
struct ProcessTree(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl ProcessTree {
    fn adopt(child: &std::process::Child) -> Option<Self> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };

        // SAFETY: plain Win32 calls on a handle this function owns, and on
        // the child's process handle, which outlives the call.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return None;
            }
            let tree = Self(job);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let set = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if set == 0 || AssignProcessToJobObject(job, child.as_raw_handle()) == 0 {
                // Dropping `tree` closes the handle.
                return None;
            }
            Some(tree)
        }
    }
}

#[cfg(windows)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateJobObjectW and is closed once.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(not(windows))]
struct ProcessTree;

#[cfg(not(windows))]
impl ProcessTree {
    fn adopt(_child: &std::process::Child) -> Option<Self> {
        Some(Self)
    }
}

/// No console window, and the below-normal priority class: every engine
/// Kiln starts runs below normal, so the rest of the PC stays smooth.
#[cfg(windows)]
fn hide_console(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::{BELOW_NORMAL_PRIORITY_CLASS, CREATE_NO_WINDOW};
    cmd.creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS);
}

#[cfg(not(windows))]
fn hide_console(_cmd: &mut Command) {}

/// Lower the calling thread's priority to below normal.
#[cfg(windows)]
pub(crate) fn lower_thread_priority() {
    use windows_sys::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
    };
    // SAFETY: GetCurrentThread returns a pseudo-handle that needs no
    // closing, and the call only changes this thread's priority.
    unsafe {
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
}

#[cfg(not(windows))]
pub(crate) fn lower_thread_priority() {}

#[cfg(test)]
mod tests {
    #[test]
    fn engine_tags_are_dropped_from_the_summary() {
        use super::strip_tags;
        assert_eq!(
            strip_tags("[in#0 @ 0000021f] Error opening input: Invalid data"),
            "Error opening input: Invalid data"
        );
        assert_eq!(strip_tags("[a] [b @ 0x1] Plain"), "Plain");
        assert_eq!(strip_tags("No tags here"), "No tags here");
        assert_eq!(strip_tags("[unclosed"), "[unclosed");
    }

    use super::*;

    #[cfg(windows)]
    #[test]
    fn every_engine_starts_below_normal() {
        let class = || {
            let mut cmd = Command::new("powershell.exe");
            cmd.args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "(Get-Process -Id $PID).PriorityClass",
            ]);
            let mut out = String::new();
            run("PowerShell", cmd, &CancelToken::new(), |l| {
                out.push_str(l.trim())
            })
            .unwrap();
            out
        };
        assert_eq!(class(), "BelowNormal");
    }

    #[test]
    fn sibling_sits_next_to_a_configured_engine() {
        let ffmpeg = Path::new("tools").join("ffmpeg");
        let probe = sibling(Some(&ffmpeg), "ffprobe");
        assert_eq!(probe.parent(), Some(Path::new("tools")));
        assert_eq!(sibling(None, "ffprobe"), PathBuf::from("ffprobe"));
        assert_eq!(
            sibling(Some(Path::new("ffmpeg")), "ffprobe"),
            PathBuf::from("ffprobe")
        );
    }

    #[test]
    fn path_joins_user_then_system_and_expands_variables() {
        let env = |name: &str| match name {
            "USERPROFILE" => Some(r"C:\Users\Ana Lúcia".to_owned()),
            "SystemRoot" => Some(r"C:\Windows".to_owned()),
            _ => None,
        };
        assert_eq!(
            join_paths(
                Some(r"%USERPROFILE%\tools;;C:\Pandoc"),
                Some(r"%SystemRoot%\system32;%NOPE%\x;C:\50%"),
                env,
            )
            .as_deref(),
            Some(r"C:\Users\Ana Lúcia\tools;C:\Pandoc;C:\Windows\system32;%NOPE%\x;C:\50%")
        );
        assert_eq!(
            join_paths(None, Some(r"C:\Sys"), env).as_deref(),
            Some(r"C:\Sys")
        );
        assert_eq!(
            join_paths(Some(r"C:\User"), None, env).as_deref(),
            Some(r"C:\User")
        );
        assert_eq!(join_paths(None, None, env), None);
        assert_eq!(join_paths(Some(""), Some(" ; "), env), None);
    }

    #[test]
    fn a_variable_kiln_lacks_is_read_from_the_registry_environment() {
        // Kiln's own environment first, then the user's Environment key,
        // then the machine's, as a program started now would see them.
        let process = |n: &str| (n == "SystemRoot").then(|| r"C:\Windows".to_owned());
        let user = |n: &str| match n {
            "TOOLS" => Some(r"D:\Tools".to_owned()),
            "SystemRoot" => Some(r"X:\Wrong".to_owned()),
            _ => None,
        };
        let machine = |n: &str| match n {
            "TOOLS" => Some(r"E:\Wrong".to_owned()),
            "SDK" => Some(r"C:\SDK".to_owned()),
            _ => None,
        };
        let sources: [Lookup; 3] = [&process, &user, &machine];
        assert_eq!(
            join_paths(
                Some(r"%TOOLS%\bin;%SystemRoot%"),
                Some(r"%SDK%\x;%NONE%\y"),
                first_of(&sources),
            )
            .as_deref(),
            Some(r"D:\Tools\bin;C:\Windows;C:\SDK\x;%NONE%\y")
        );
    }

    #[test]
    fn the_process_path_stands_in_when_the_registry_has_none() {
        let process = OsStr::new(r"C:\Old;C:\Shared");
        assert_eq!(with_process(None, process), process);
        // The fresh list first, then what only the process had, once.
        assert_eq!(
            with_process(Some(r"C:\New;c:\shared\".to_owned()), process),
            OsStr::new(r"C:\New;c:\shared\;C:\Old")
        );
        assert_eq!(
            with_process(Some(r"C:\New".to_owned()), OsStr::new("")),
            OsStr::new(r"C:\New")
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_registry_variable_comes_back_expanded() {
        // ComSpec is a REG_EXPAND_SZ naming %SystemRoot% on every Windows.
        let comspec = registry::system_var("ComSpec").unwrap_or_default();
        assert!(
            comspec.to_ascii_lowercase().ends_with(r"\system32\cmd.exe"),
            "{comspec}"
        );
        assert!(!comspec.contains('%'), "{comspec}");
        assert_eq!(registry::user_var("KILN_NO_SUCH_VARIABLE"), None);
    }

    #[cfg(windows)]
    #[test]
    fn the_current_path_reads_the_registry() {
        let path = current_path().to_string_lossy().to_ascii_lowercase();
        assert!(path.contains("system32"), "{path}");
        assert!(!path.contains("%systemroot%"), "{path}");
    }

    /// An engine named without a folder is found on the PATH its command
    /// carries, not only on the one Kiln started with, so a helper
    /// installed while Kiln runs starts.
    #[cfg(windows)]
    #[test]
    fn an_engine_is_found_on_the_path_its_command_carries() {
        let dir = tempfile::tempdir().unwrap();
        let system = std::env::var_os("SystemRoot").unwrap();
        std::fs::copy(
            Path::new(&system).join("System32").join("whoami.exe"),
            dir.path().join("kiln-path-probe.exe"),
        )
        .unwrap();
        let mut cmd = Command::new("kiln-path-probe");
        cmd.env("PATH", dir.path());
        let ran = run("Probe", cmd, &CancelToken::new(), |_| {});
        assert!(
            !matches!(ran, Err(Error::EngineMissing { .. })),
            "not found on the PATH given"
        );
    }

    #[test]
    fn ffprobe_missing_names_ffmpeg() {
        let cmd = Command::new("kiln-test-no-such-ffprobe");
        let err = run("FFprobe", cmd, &CancelToken::new(), |_| {}).unwrap_err();
        // FFmpeg comes with Kiln: there is no Get it, but another copy
        // can be chosen.
        assert_eq!(
            err.to_string(),
            "FFmpeg isn't installed. Choose a copy in Settings > Helper apps."
        );
    }

    #[test]
    fn a_missing_program_is_reported_as_missing() {
        let cmd = Command::new("kiln-test-no-such-engine");
        let err = run("Thing", cmd, &CancelToken::new(), |_| {}).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Thing isn't installed. Get it from Settings > Helper apps."
        );
    }
}
