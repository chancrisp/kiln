//! Delivering held results: every result a run holds goes to the folder
//! it was meant for, or else to the fallback folder (`Kiln results` in
//! Documents), and the held copy goes only once the delivered one is
//! checked.
//!
//! One path for every case: a result of this run as it is ready, a
//! result this run couldn't deliver, and what a crashed, killed or older
//! run left behind ([`deliver_left`], at every start).
//!
//! Never over a file: each delivery takes the next free name
//! (`photo.jpg`, `photo (1).jpg`, ...) with a rename that never replaces.
//! A folder that isn't there is never made again: the result goes to the
//! fallback instead. A result that can't go to either stays held, whole,
//! and the next start tries again.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use super::roots::{close_run, in_holding_place, Places};
use super::{
    copy_to_temp, crosses_devices, run_of, split_name, test_hooks, FreeNames, MoveEnd, StaleRun,
    TempNoted, SOURCES_LIMIT, SOURCES_NAME,
};
use crate::converter::CancelToken;
use crate::error::{Error, Result};
use crate::output;

/// The fallback folder's name, made in Documents when it is needed.
pub const KILN_RESULTS: &str = "Kiln results";

/// The fallback folder in the Documents folder `documents`.
pub fn kiln_results_in(documents: &Path) -> PathBuf {
    documents.join(KILN_RESULTS)
}

/// Where one held result went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery {
    /// Into the folder it was meant for, at this path.
    Beside(PathBuf),
    /// Into the fallback folder, at this path.
    Fallback(PathBuf),
    /// Nowhere: it is still held, whole.
    Kept,
}

impl Delivery {
    /// Where it went, if it went.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Delivery::Beside(p) | Delivery::Fallback(p) => Some(p),
            Delivery::Kept => None,
        }
    }
}

/// What one look at what earlier runs left did ([`deliver_left`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// Delivered to the folder each was meant for.
    pub beside: usize,
    /// Delivered to the fallback folder.
    pub fallback: usize,
    /// Couldn't be delivered: still held, for the next start.
    pub kept: usize,
    /// Every folder a result went to, each once, in the order first used.
    pub folders: Vec<PathBuf>,
}

impl Report {
    /// How many were delivered.
    pub fn delivered(&self) -> usize {
        self.beside + self.fallback
    }

    /// Nothing was found to deliver.
    pub fn is_empty(&self) -> bool {
        self.delivered() + self.kept == 0
    }

    fn went(&mut self, d: &Delivery) {
        match d {
            Delivery::Beside(_) => self.beside += 1,
            Delivery::Fallback(_) => self.fallback += 1,
            Delivery::Kept => self.kept += 1,
        }
        if let Some(dir) = d.path().and_then(Path::parent) {
            if !self.folders.iter().any(|f| f == dir) {
                self.folders.push(dir.to_path_buf());
            }
        }
    }
}

/// Deliver the held file `held` into `dest` (the folder it was meant
/// for), or, if that folder can't take files ([`lasting`]), into
/// `fallback`. After a passing failure (the file open in another program,
/// tried again for a moment first) it is kept held. `dest` `None` (no
/// folder is known) goes straight to the fallback. A cancel keeps it held
/// and tries nothing more. `fallback` is made if it is missing and its
/// parent is there; its parent is never made.
pub fn deliver_with(
    held: &Path,
    dest: Option<&Path>,
    fallback: Option<&Path>,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &CancelToken,
) -> Delivery {
    if let Some(dir) = dest {
        match deliver_into_retrying(held, dir, progress, cancel) {
            Ok(to) => return Delivery::Beside(to),
            Err(Error::Cancelled) => return Delivery::Kept,
            // Only a folder that can't take files sends the result to the
            // fallback: after a passing failure it stays held, and is
            // tried again, rather than go where no one looks for it.
            Err(e) if !lasting(&e) => return Delivery::Kept,
            Err(_) => {}
        }
    }
    let Some(fallback) = fallback else {
        return Delivery::Kept;
    };
    if ready_fallback(fallback).is_err() {
        return Delivery::Kept;
    }
    match deliver_into_with(held, fallback, progress, cancel) {
        Ok(to) => Delivery::Fallback(to),
        Err(_) => Delivery::Kept,
    }
}

/// Make the fallback folder if it is missing, only in a parent that is
/// there. A file of its name is never moved or removed: it is an error.
fn ready_fallback(dir: &Path) -> io::Result<()> {
    if fs::metadata(dir).is_ok_and(|m| m.is_dir()) {
        return Ok(());
    }
    let parent_there = dir
        .parent()
        .is_some_and(|p| fs::metadata(p).is_ok_and(|m| m.is_dir()));
    if !parent_there {
        return Err(io::Error::from(io::ErrorKind::NotFound));
    }
    match fs::create_dir(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists && dir.is_dir() => Ok(()),
        Err(e) => Err(e),
    }
}

/// Move the held file `held` into `dir` under a free name, never over a
/// file, and check it before the held copy goes. Returns where it went.
///
/// On one volume it is a rename, which is whole or not at all: the one
/// file moves, so nothing is removed; it is checked to be at its new name
/// and gone from the old, then takes the permissions of its new folder.
/// Across volumes it is copied to Kiln's own temp file in `dir`, flushed,
/// renamed to the first free name, and then compared, length and every
/// byte, with the held file. Only a copy that matches lets the held file
/// go; one that doesn't is removed (only Kiln has had that name) and the
/// held file stays. Before the held file is removed, the run's
/// [`SOURCES_NAME`] notes where it went, so a held copy that can't be
/// removed is never delivered a second time.
///
/// Cancelled ([`Error::Cancelled`]) or failed, the held file is where it
/// was, whole, and nothing Kiln made is left in `dir`.
pub fn deliver_into_with(
    held: &Path,
    dir: &Path,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &CancelToken,
) -> Result<PathBuf> {
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    fs::symlink_metadata(held).map_err(|e| Error::io(held, e))?;
    let (stem, ext) = split_name(held);
    let names = FreeNames {
        dir,
        stem: output::sanitize_stem(&stem),
        ext,
    };
    if !test_hooks::force_copy() {
        match names.claim(held) {
            Ok(Some(to)) => {
                test_hooks::maybe_spoil(&to);
                // A rename on one volume moves the one file: nothing is
                // copied and nothing is removed. It is delivered once it
                // is at `to` and no longer held, whatever its length now
                // (something else may have changed it as it landed).
                let there = fs::symlink_metadata(&to).is_ok_and(|m| m.is_file());
                let gone = fs::symlink_metadata(held).is_err();
                if !(there && gone) {
                    let why = io::Error::other("the delivered file isn't there");
                    return Err(Error::io(&to, why));
                }
                take_folders_permissions(&to);
                return Ok(to);
            }
            Ok(None) => return Err(names.none_free()),
            Err(e) if crosses_devices(&e) => {}
            Err(e) => return Err(Error::io(names.first(), e)),
        }
    }
    copy_check_claim(held, &names, progress, cancel).map_err(|e| e.into_error(&names.first()))
}

/// The cross-volume half of [`deliver_into_with`].
fn copy_check_claim(
    held: &Path,
    names: &FreeNames<'_>,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &CancelToken,
) -> std::result::Result<PathBuf, MoveEnd> {
    let tmp = copy_to_temp(held, names.dir, &names.first(), progress, cancel)?;
    let _noted = TempNoted::of(held, &tmp);
    let to = match names.claim(&tmp) {
        Ok(Some(to)) => to,
        Ok(None) => {
            let _ = fs::remove_file(&tmp);
            return Err(MoveEnd::Error(names.none_free()));
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            return Err(e.into());
        }
    };
    test_hooks::maybe_spoil(&to);
    match same_bytes(held, &to) {
        Ok(true) => {}
        Ok(false) => {
            // Kiln's own copy, claimed a moment ago: it goes, the held
            // file stays.
            let _ = fs::remove_file(&to);
            return Err(MoveEnd::Failed(io::Error::other(
                "the copy doesn't match the held file",
            )));
        }
        Err(e) => {
            let _ = fs::remove_file(&to);
            return Err(e.into());
        }
    }
    if let Some(run) = run_of(held) {
        // Not noted: the held copy stays, so the next start delivers it
        // again rather than lose it.
        if note_delivered(&run, held, &to).is_err() {
            return Ok(to);
        }
    }
    // In place and checked: a held copy that can't go now is no reason to
    // fail, and the mark keeps it from coming back as a second copy.
    let _ = fs::remove_file(held);
    Ok(to)
}

/// Whether `a` and `b` hold the same bytes: the same length, then every
/// byte compared.
pub(super) fn same_bytes(a: &Path, b: &Path) -> io::Result<bool> {
    let (mut fa, mut fb) = (File::open(a)?, File::open(b)?);
    if fa.metadata()?.len() != fb.metadata()?.len() {
        return Ok(false);
    }
    let (mut ba, mut bb) = (vec![0u8; 1 << 20], vec![0u8; 1 << 20]);
    loop {
        let n = read_full(&mut fa, &mut ba)?;
        let m = read_full(&mut fb, &mut bb)?;
        if n != m || ba[..n] != bb[..m] {
            return Ok(false);
        }
        if n == 0 {
            return Ok(true);
        }
    }
}

/// Whether `a` and `b` lead to the one file: the same path, a link to it,
/// or another name for it. Both are opened, so links are followed.
fn same_file(a: &Path, b: &Path) -> io::Result<bool> {
    let (fa, fb) = (File::open(a)?, File::open(b)?);
    #[cfg(windows)]
    {
        Ok(super::win::file_id(&fa)? == super::win::file_id(&fb)?)
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::MetadataExt;
        let (ma, mb) = (fa.metadata()?, fb.metadata()?);
        Ok((ma.dev(), ma.ino()) == (mb.dev(), mb.ino()))
    }
}

/// Read until `buf` is full or the file ends: how many bytes were read.
fn read_full(f: &mut File, buf: &mut [u8]) -> io::Result<usize> {
    let mut got = 0;
    while got < buf.len() {
        match f.read(&mut buf[got..])? {
            0 => break,
            n => got += n,
        }
    }
    Ok(got)
}

/// Note in the run folder `run`'s [`SOURCES_NAME`] that the held file
/// `held` was delivered to `to`: `name`, two tabs and the path. Older
/// readers skip a line with a second tab. Flushed to the disk before this
/// returns.
fn note_delivered(run: &Path, held: &Path, to: &Path) -> io::Result<()> {
    let bad = || io::Error::new(io::ErrorKind::InvalidInput, "can't be noted");
    let name = held.file_name().and_then(|n| n.to_str()).ok_or_else(bad)?;
    let path = to.to_str().ok_or_else(bad)?;
    let whole = |s: &str| !s.is_empty() && !s.contains(['\t', '\n', '\r']);
    if !whole(name) || !whole(path) || !to.is_absolute() {
        return Err(bad());
    }
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(run.join(SOURCES_NAME))?;
    f.write_all(format!("{name}\t\t{path}\n").as_bytes())?;
    f.sync_all()
}

/// Where each result in `sources` was delivered, by file name, as
/// [`note_delivered`] noted it. A later line giving the name a folder (a
/// new result of that name) clears it.
pub(super) fn read_delivered(sources: &Path) -> HashMap<String, PathBuf> {
    let mut bytes = vec![];
    let read = File::open(sources).and_then(|f| f.take(SOURCES_LIMIT).read_to_end(&mut bytes));
    let mut marks = HashMap::new();
    if read.is_err() {
        return marks;
    }
    let mut lines: Vec<&[u8]> = bytes.split(|&b| b == b'\n').collect();
    // After the last line break: nothing, or a line cut short.
    lines.pop();
    for line in lines {
        let Ok(line) = std::str::from_utf8(line) else {
            continue;
        };
        let Some((name, rest)) = line.split_once('\t') else {
            continue;
        };
        if name.is_empty() || line.contains('\r') {
            continue;
        }
        match rest.strip_prefix('\t') {
            Some(path) if !path.contains('\t') && Path::new(path).is_absolute() => {
                marks.insert(name.to_owned(), PathBuf::from(path));
            }
            Some(_) => {}
            None if !rest.is_empty() => {
                marks.remove(name);
            }
            None => {}
        }
    }
    marks
}

/// Deliver every result in `runs`, what crashed, killed or older runs
/// left (`stale_runs_in`), each to the folder its run noted for it, or
/// else to `fallback`; then let go of each run, removing it, and its
/// drive's root if Kiln made it and it is empty, once only its own files
/// are left. A result whose run noted it as delivered, and whose
/// delivered copy is there holding the same bytes, every one compared, is
/// only removed: it was delivered already. A delivered copy noted inside a
/// holding place, or one that is the held file itself, counts for nothing.
/// Any other result is delivered. A folder noted that is inside a holding
/// place is no place for it: it goes to the fallback.
///
/// A cancel keeps what is left held, whole, for the next start. A result
/// that can't be delivered stays held too, and its run is let go, so the
/// next start tries again.
pub fn deliver_left(
    runs: Vec<StaleRun>,
    places: &Places,
    fallback: Option<&Path>,
    cancel: &CancelToken,
) -> Report {
    let mut report = Report::default();
    for run in runs {
        let marks = read_delivered(&run.dir.join(SOURCES_NAME));
        for f in &run.files {
            if cancel.is_cancelled() {
                report.kept += 1;
                continue;
            }
            let name = f.path.file_name().and_then(|n| n.to_str());
            if let Some(to) = name.and_then(|n| marks.get(n)) {
                // Only a held file whose delivered copy holds the same
                // bytes was delivered: a later result that took the name
                // (its own line not yet written when the run ended) is
                // another file, even at the same length, and is delivered.
                // A mark naming a file in a holding place, or the held
                // file itself under any name, names no delivered copy:
                // the held file is delivered.
                let elsewhere =
                    !in_holding_place(to, places) && same_file(&f.path, to).is_ok_and(|same| !same);
                if elsewhere && same_bytes(&f.path, to).unwrap_or(false) {
                    let _ = fs::remove_file(&f.path);
                    continue;
                }
            }
            let dest = f.origin.as_deref().filter(|d| !in_holding_place(d, places));
            let d = deliver_with(&f.path, dest, fallback, &mut |_, _| {}, cancel);
            report.went(&d);
        }
        close_run(run, places);
    }
    report
}

/// Whether `e`, from a delivery into a folder, says the folder can't take
/// files: it is gone, Kiln may not write in it, the disk is read-only,
/// full or not there, or the share can't be reached. Only then does a
/// result go to the fallback, and its row say Kiln can't save there. A
/// passing failure (the file open in another program, a share that timed
/// out once, a copy that didn't match, every free name taken) is not
/// lasting: the result stays held for the next try.
fn lasting(e: &Error) -> bool {
    let Error::Io { source, .. } = e else {
        return false;
    };
    #[cfg(windows)]
    if let Some(code) = source.raw_os_error() {
        // FILE_NOT_FOUND, PATH_NOT_FOUND, ACCESS_DENIED, INVALID_DRIVE,
        // WRITE_PROTECT, NOT_READY, HANDLE_DISK_FULL, NOT_SUPPORTED,
        // BAD_NETPATH, NETWORK_ACCESS_DENIED, BAD_NET_NAME, CANNOT_MAKE,
        // DISK_FULL, INVALID_NAME, BAD_PATHNAME, FILENAME_EXCED_RANGE,
        // DIRECTORY, NO_NET_OR_BAD_PATH, NO_NETWORK, NETWORK_UNREACHABLE,
        // DISK_QUOTA_EXCEEDED, NOT_CONNECTED.
        const LASTING: [i32; 22] = [
            2, 3, 5, 15, 19, 21, 39, 50, 53, 65, 67, 82, 112, 123, 161, 206, 267, 1203, 1222, 1231,
            1295, 2250,
        ];
        return LASTING.contains(&code);
    }
    use io::ErrorKind as K;
    matches!(
        source.kind(),
        K::NotFound
            | K::PermissionDenied
            | K::ReadOnlyFilesystem
            | K::StorageFull
            | K::QuotaExceeded
            | K::NotADirectory
            | K::InvalidFilename
            | K::Unsupported
    )
}

/// Whether `e` is another program having the file open: a sharing or a
/// lock violation, which is often over in a moment (a virus scanner, a
/// preview).
fn busy(e: &Error) -> bool {
    matches!(e, Error::Io { source, .. } if matches!(source.raw_os_error(), Some(32 | 33)))
}

/// How long to wait before each new try at a delivery that found the file
/// busy ([`busy`]): about a second and a half in all.
const BUSY_WAITS_MS: [u64; 4] = [100, 200, 400, 800];

/// [`deliver_into_with`], tried again a few times over a second and a half
/// while the file is busy. A cancel ends the wait at once.
fn deliver_into_retrying(
    held: &Path,
    dir: &Path,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &CancelToken,
) -> Result<PathBuf> {
    let mut tried = deliver_into_with(held, dir, progress, cancel);
    for wait in BUSY_WAITS_MS {
        match &tried {
            Err(e) if busy(e) => {}
            _ => break,
        }
        let until = Instant::now() + Duration::from_millis(wait);
        while Instant::now() < until {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        tried = deliver_into_with(held, dir, progress, cancel);
    }
    tried
}

/// Where a delivery's lines go: the app's log, once it says
/// ([`log_deliveries_to`]).
static LOG: OnceLock<fn(&str)> = OnceLock::new();

/// Send what deliveries have to note (a delivered file that kept the
/// holding folder's permissions) to `log`. The first call counts.
pub fn log_deliveries_to(log: fn(&str)) {
    let _ = LOG.set(log);
}

fn log(msg: &str) {
    if let Some(log) = LOG.get() {
        log(msg);
    }
}

/// Let `to`, just renamed out of a holding folder, take the permissions of
/// the folder it is in now, as a file made there would have. Failing that
/// it is still delivered, and the log says so; nothing is undone.
pub(super) fn take_folders_permissions(to: &Path) {
    #[cfg(windows)]
    if let Err(e) = super::win::inherit_dacl(to) {
        log(&format!(
            "{} kept the holding folder's permissions: {e}",
            to.display()
        ));
    }
    #[cfg(not(windows))]
    let _ = to;
}

#[cfg(test)]
mod tests;
