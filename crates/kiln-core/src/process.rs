//! Running external engines. Arguments are always passed one by one to
//! `std::process::Command`; nothing is ever assembled into a shell string.

use std::ffi::OsStr;
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

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::EngineMissing { engine })
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

#[cfg(windows)]
fn hide_console(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console(_cmd: &mut Command) {}

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
    fn a_missing_program_is_reported_as_missing() {
        let cmd = Command::new("kiln-test-no-such-engine");
        let err = run("Thing", cmd, &CancelToken::new(), |_| {}).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Thing isn't installed. Get it from Settings > Engines."
        );
    }
}
