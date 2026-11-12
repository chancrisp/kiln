//! Held results: a private per-run folder for finished files, and the moves
//! that export them.
//!
//! The app points jobs at a [`Holding`] folder instead of the source's
//! folder. Results stay there until they are delivered, so a conversion
//! never lands somewhere the user didn't choose.
//!
//! A run that ends without a word (a crash, a kill, a shutdown) leaves its
//! folder behind. The next run finds it with [`stale_runs`] and offers its
//! results, with the folder each one's original was in ([`SOURCES_NAME`]).
//!
//! Which root a result waits in (one per drive), and a run's folders on
//! each, are in [`roots`]. A move out across volumes is a copy, with
//! progress and cancel ([`export_into_with`]).

mod deliver;
pub mod roots;
#[cfg(windows)]
mod win;

pub use deliver::{
    deliver_into_with, deliver_left, deliver_with, kiln_results_in, log_deliveries_to, Delivery,
    Report, KILN_RESULTS,
};

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::converter::CancelToken;
use crate::error::{Error, Result};
use crate::output;

/// Name of the file each run keeps open to say "this folder is in use".
pub(crate) const LOCK_NAME: &str = "lock";

/// Name of the file each run keeps of where its results came from: one
/// line per result, its file name, a tab and the folder its original was
/// in, in UTF-8. A line with no folder says the result of that name has
/// left. It is not a result.
pub const SOURCES_NAME: &str = "sources.tsv";

/// Name of the file each run keeps of the temp copies its moves to other
/// drives make (`photo.jpg.<pid>-<millis>.kiln-export.tmp`), so a copy that a crash, a
/// kill or the end of the session cut short is removed at the next start:
/// one line per change, `+` or `-`, a tab and the temp file's path, in
/// UTF-8. No extension, so no result is ever named so. It is not a result.
pub const TEMPS_NAME: &str = "temps";

/// Name of the note an older build wrote in a run folder when a Discard
/// of everything it held might not finish before it ended. This build
/// writes none and ignores one: the results of such a folder are
/// delivered like any other run's, and the note goes with the folder once
/// the folder is empty. No extension, so no result is ever named so. It is
/// not a result.
pub const DISCARD_NAME: &str = "discard";

/// The most of a `sources.tsv` that is read: far more than a run's lines.
const SOURCES_LIMIT: u64 = 16 * 1024 * 1024;

/// The OS error for a rename between volumes: `ERROR_NOT_SAME_DEVICE` on
/// Windows, `EXDEV` elsewhere. (`ErrorKind::CrossesDevices` needs a newer
/// Rust than this crate's minimum.)
#[cfg(windows)]
const CROSS_DEVICE: i32 = 17;
#[cfg(not(windows))]
const CROSS_DEVICE: i32 = 18;

/// One run's held-results folder. Its `lock` file stays open, with no
/// sharing, for as long as this value lives. Other processes can't delete
/// it, and a crashed run's folder (whose lock is closed) can be swept.
#[derive(Debug)]
pub struct Holding {
    dir: PathBuf,
    _lock: File,
}

impl Holding {
    /// Create `root\<pid>-<unix millis>` (and `root` if needed) and lock it.
    pub fn open(root: &Path) -> Result<Holding> {
        fs::create_dir_all(root).map_err(|e| Error::io(root, e))?;
        Holding::open_in(root)
    }

    /// Create `root\<pid>-<unix millis>` and lock it, in a `root` that is
    /// there already: one that went is never made again here, so a
    /// per-user root keeps the permissions it was made with.
    pub(crate) fn open_in(root: &Path) -> Result<Holding> {
        let pid = std::process::id();
        let mut millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        loop {
            let dir = root.join(format!("{pid}-{millis}"));
            match fs::create_dir(&dir) {
                Ok(()) => {
                    return match lock_file(&dir.join(LOCK_NAME)) {
                        Ok(lock) => Ok(Holding { dir, _lock: lock }),
                        Err(e) => {
                            let _ = fs::remove_dir(&dir);
                            Err(Error::io(dir.join(LOCK_NAME), e))
                        }
                    };
                }
                // Two opens in the same millisecond: take the next one.
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => millis += 1,
                Err(e) => return Err(Error::io(dir, e)),
            }
        }
    }

    /// The folder results are written to.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Note that `result`, a file in this folder, was made from a file in
    /// `origin_dir`, so a later run can save it there if this one ends
    /// without saying what to do with it. Appends a line to
    /// [`SOURCES_NAME`]. A name or folder a line couldn't hold whole (not
    /// Unicode, with a tab or a line break, or a folder that isn't a whole
    /// path) is refused, and the result then has no origin.
    pub fn record(&self, result: &Path, origin_dir: &Path) -> Result<()> {
        record_source(&self.dir, result, origin_dir)
    }

    /// Note that `result` has left this folder: see [`forget_source`].
    pub fn forget(&self, result: &Path) -> Result<()> {
        forget_source(&self.dir, result)
    }

    /// The end of the run: remove this run's folder, with its own files
    /// and its lock last, only if no result is left in it. A folder with a
    /// result in it, or anything Kiln didn't make, is kept whole, its lock
    /// let go, for the next start to deliver. True if it was removed.
    pub fn close_if_empty(self) -> bool {
        let Holding { dir, _lock } = self;
        let files = match run_files(&dir) {
            Some(files) if files.iter().all(|f| is_bookkeeping(f)) => files,
            _ => return false,
        };
        if !remove_bookkeeping(&files) {
            return false;
        }
        // Windows can't delete an open file.
        drop(_lock);
        fs::remove_file(dir.join(LOCK_NAME)).is_ok() && fs::remove_dir(&dir).is_ok()
    }
}

/// Create the lock file and keep it open with share mode 0, so no other
/// process can open, delete or replace it while we hold it.
pub(crate) fn lock_file(path: &Path) -> io::Result<File> {
    let mut opts = OpenOptions::new();
    opts.read(true).write(true).create_new(true);
    #[cfg(windows)]
    std::os::windows::fs::OpenOptionsExt::share_mode(&mut opts, 0);
    opts.open(path)
}

/// Open an existing lock the same way. Succeeds only if nobody holds it.
pub(crate) fn probe_lock(path: &Path) -> io::Result<File> {
    let mut opts = OpenOptions::new();
    opts.read(true);
    #[cfg(windows)]
    std::os::windows::fs::OpenOptionsExt::share_mode(&mut opts, 0);
    opts.open(path)
}

/// Note in the run folder `dir`, which this run holds open as a
/// [`Holding`], that `result`, a file in it, was made from a file in
/// `origin_dir`: what [`Holding::record`] does, for a caller that knows
/// the folder by its path (the window, while a worker opens other
/// folders). Appends a line to [`SOURCES_NAME`]; a result or folder a
/// line couldn't hold whole is refused.
pub fn record_source(dir: &Path, result: &Path, origin_dir: &Path) -> Result<()> {
    let line = source_line(dir, result, origin_dir).ok_or_else(|| {
        let why = io::Error::new(io::ErrorKind::InvalidInput, "can't be noted as a source");
        Error::io(result, why)
    })?;
    let sources = dir.join(SOURCES_NAME);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&sources)
        .map_err(|e| Error::io(&sources, e))?;
    // One write: a line is there whole, or cut short at the end, which
    // reading leaves out.
    file.write_all(line.as_bytes())
        .map_err(|e| Error::io(&sources, e))
}

/// Note in the run folder `dir` that `result`, a file that was in it, has
/// left it (saved, exported or discarded): a line with its name and no
/// folder. So a later result of the same name that Kiln ended before
/// noting isn't offered as made from this one's folder. Nothing is
/// written where the run has noted nothing yet.
pub fn forget_source(dir: &Path, result: &Path) -> Result<()> {
    let refused = || {
        let why = io::Error::new(io::ErrorKind::InvalidInput, "isn't a result of this run");
        Error::io(result, why)
    };
    if result.parent() != Some(dir) {
        return Err(refused());
    }
    let name = result
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(refused)?;
    if name.is_empty() || name.contains(['\t', '\n', '\r']) {
        return Err(refused());
    }
    let sources = dir.join(SOURCES_NAME);
    // Never made here: without a list there is no line to clear.
    let mut file = match OpenOptions::new().append(true).open(&sources) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(Error::io(&sources, e)),
    };
    file.write_all(format!("{name}\t\n").as_bytes())
        .map_err(|e| Error::io(&sources, e))
}

/// The line [`Holding::record`] appends, or `None` if it couldn't be read
/// back as written.
fn source_line(dir: &Path, result: &Path, origin: &Path) -> Option<String> {
    if result.parent() != Some(dir) || !origin.is_absolute() {
        return None;
    }
    let name = result.file_name()?.to_str()?;
    let origin = origin.to_str()?;
    let whole = |s: &str| !s.is_empty() && !s.contains(['\t', '\n', '\r']);
    let ours = name == LOCK_NAME || name == SOURCES_NAME || name == TEMPS_NAME;
    (whole(name) && whole(origin) && !ours).then(|| format!("{name}\t{origin}\n"))
}

/// Each result's origin in `sources`, by file name: the last whole line
/// naming it counts, and a line with no folder clears it. A line cut
/// short, not Unicode, without a tab, with a second tab, or whose folder
/// isn't a whole path, says nothing.
fn read_sources(sources: &Path) -> HashMap<String, PathBuf> {
    let mut bytes = vec![];
    let read = File::open(sources).and_then(|f| f.take(SOURCES_LIMIT).read_to_end(&mut bytes));
    let mut origins = HashMap::new();
    if read.is_err() {
        return origins;
    }
    let mut lines: Vec<&[u8]> = bytes.split(|&b| b == b'\n').collect();
    // After the last line break: nothing, or a line cut short.
    lines.pop();
    for line in lines {
        let Ok(line) = std::str::from_utf8(line) else {
            continue;
        };
        let Some((name, origin)) = line.split_once('\t') else {
            continue;
        };
        if name.is_empty() || line.contains('\r') || origin.contains('\t') {
            continue;
        }
        if origin.is_empty() {
            origins.remove(name);
            continue;
        }
        let origin = PathBuf::from(origin);
        if origin.is_absolute() {
            origins.insert(name.to_owned(), origin);
        }
    }
    origins
}

/// A held result a run left behind, and the folder its original was in,
/// when the run noted it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recovered {
    pub path: PathBuf,
    pub origin: Option<PathBuf>,
}

/// A run folder that no live run holds and that still has results, as a
/// crashed or killed run leaves it. While this value lives it holds the
/// folder's lock with no sharing, so no other Kiln lists, sweeps or
/// touches it.
#[derive(Debug)]
pub struct StaleRun {
    pub dir: PathBuf,
    /// Its results, by name, each with its origin.
    pub files: Vec<Recovered>,
    _lock: File,
}

impl StaleRun {
    /// Remove the folder, with its lock, its [`SOURCES_NAME`] and any
    /// empty placeholder, only once no result is left in it. True if it
    /// was removed. Anything else still in it keeps it, whole, for the next
    /// start.
    pub fn close(self) -> bool {
        let StaleRun { dir, _lock, .. } = self;
        let files = match run_files(&dir) {
            Some(files) if files.iter().all(|f| is_bookkeeping(f)) => files,
            _ => return false,
        };
        if !remove_bookkeeping(&files) {
            return false;
        }
        // Windows can't delete an open file.
        drop(_lock);
        fs::remove_file(dir.join(LOCK_NAME)).is_ok() && fs::remove_dir(&dir).is_ok()
    }
}

/// Run folders under `root` that no live run holds and that still have
/// results, each locked while it is offered. A result is listed only if
/// its file is there; its origin comes from the last line of the run's
/// [`SOURCES_NAME`] naming it.
///
/// As careful as [`sweep_stale`]: direct child folders of `root` with a
/// plain file named `lock`, none whose lock is open, no links followed,
/// and only folders that hold nothing but plain files.
pub fn stale_runs(root: &Path) -> Vec<StaleRun> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut runs: Vec<StaleRun> = entries
        .flatten()
        // `DirEntry::file_type` doesn't follow links.
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| stale_run(&e.path()))
        .collect();
    runs.sort_by(|a, b| a.dir.cmp(&b.dir));
    runs
}

/// One stale run folder with results, locked, or `None`.
fn stale_run(dir: &Path) -> Option<StaleRun> {
    let lock = dir.join(LOCK_NAME);
    match fs::symlink_metadata(&lock) {
        Ok(m) if m.is_file() => {}
        _ => return None,
    }
    // A live run holds its lock, so this fails and nothing of it is read.
    // From here no other Kiln can take it.
    let lock = probe_lock(&lock).ok()?;
    clear_noted_temps(dir);
    let origins = read_sources(&dir.join(SOURCES_NAME));
    let mut files: Vec<Recovered> = run_files(dir)?
        .into_iter()
        .filter(|f| !is_bookkeeping(f))
        .map(|path| {
            let name = path.file_name().and_then(|n| n.to_str());
            let origin = name.and_then(|n| origins.get(n)).cloned();
            Recovered { path, origin }
        })
        .collect();
    if files.is_empty() {
        return None;
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Some(StaleRun {
        dir: dir.to_path_buf(),
        files,
        _lock: lock,
    })
}

/// The plain files in a run folder, or `None` if it holds anything else
/// (then it isn't a folder Kiln made) or can't be read.
fn run_files(dir: &Path) -> Option<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).ok()?.flatten() {
        match entry.file_type() {
            Ok(t) if t.is_file() => files.push(entry.path()),
            _ => return None,
        }
    }
    Some(files)
}

/// A run's own files, not results: its lock, its [`SOURCES_NAME`], its
/// [`TEMPS_NAME`], its [`DISCARD_NAME`], and any empty file. An empty file is the placeholder `job::place` makes
/// before the result takes its place, left by a run that ended between
/// the two: every finished result is checked to hold something first.
fn is_bookkeeping(file: &Path) -> bool {
    let name = file.file_name();
    name.is_some_and(|n| {
        n == LOCK_NAME || n == SOURCES_NAME || n == TEMPS_NAME || n == DISCARD_NAME
    }) || fs::symlink_metadata(file).is_ok_and(|m| m.is_file() && m.len() == 0)
}

/// Remove a run's own `files` but its lock, which goes last. False if one
/// is there still.
fn remove_bookkeeping(files: &[PathBuf]) -> bool {
    files
        .iter()
        .filter(|f| f.file_name() != Some(LOCK_NAME.as_ref()))
        .all(|f| match fs::remove_file(f) {
            Ok(()) => true,
            Err(e) => e.kind() == io::ErrorKind::NotFound,
        })
}

/// Remove run folders under `root` that no live run holds and that hold no
/// results (only their lock, [`SOURCES_NAME`] and empty placeholders), such
/// as what a crashed run that had saved everything left behind. Returns the folders removed. A folder with results is left for
/// [`stale_runs`] to offer.
///
/// Conservative on purpose. It looks only at direct child folders of
/// `root` that contain a plain file named `lock`, skips any whose lock is
/// open, never follows links, and leaves a folder alone if it holds
/// anything but plain files, or any file but its lock, its
/// [`SOURCES_NAME`] and empty placeholders.
pub fn sweep_stale(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut removed = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        // `DirEntry::file_type` doesn't follow links.
        match entry.file_type() {
            Ok(t) if t.is_dir() => {}
            _ => continue,
        }
        if sweep_one(&dir) {
            removed.push(dir);
        }
    }
    removed
}

/// Remove one stale run folder that holds no results. False if it's live,
/// isn't ours, has results, or couldn't be fully removed.
fn sweep_one(dir: &Path) -> bool {
    let lock = dir.join(LOCK_NAME);
    match fs::symlink_metadata(&lock) {
        Ok(m) if m.is_file() => {}
        _ => return false,
    }
    // If a live run holds the lock, or another Kiln is offering this
    // folder's results, this fails and we touch nothing. Taken before
    // looking inside, so nothing can land in the folder after the look.
    // Kept until the list of sources is gone, so a failure part-way leaves
    // the lock for the next launch to retry.
    let Ok(probe) = probe_lock(&lock) else {
        return false;
    };
    clear_noted_temps(dir);
    // Plain files only. Anything else means this isn't a folder we made.
    // And no results: those are offered, never swept.
    let files = match run_files(dir) {
        Some(files) if files.iter().all(|f| is_bookkeeping(f)) => files,
        _ => return false,
    };
    if !remove_bookkeeping(&files) {
        return false;
    }
    drop(probe);
    fs::remove_file(&lock).is_ok() && fs::remove_dir(dir).is_ok()
}

/// Move the held file `held` into `dir` under a free name (`name.ext`, then
/// `name (1).ext`, and so on). Never overwrites. Returns the new path.
pub fn export_into(held: &Path, dir: &Path) -> Result<PathBuf> {
    export_into_with(held, dir, &mut |_, _| {}, &CancelToken::new())
}

/// [`export_into`], telling `progress` the bytes copied so far and the
/// file's size while a move across volumes copies, and stopping when
/// `cancel` is set. A move on one volume is a rename: `progress` isn't
/// told, and only a `cancel` set before it starts stops it.
///
/// No name is claimed before the file is whole. On one volume the held
/// file is renamed straight to the first free name. Across volumes it is
/// copied to Kiln's own temp file in `dir`
/// (`photo.jpg.<pid>-<millis>.kiln-export.tmp`), flushed, and only then renamed to the
/// first free name. Every rename is one that never replaces: a file that
/// appears at a name meanwhile, even part-way through the copy, is left
/// alone and the next name is taken.
///
/// Cancelled ([`Error::Cancelled`]), or failed, the held file is where it
/// was, whole, and only the temp file is removed. Once the copy is
/// complete and compared, every byte, with the held file, it is renamed
/// into place and the held file removed, whatever `cancel` says; a copy
/// that doesn't match is removed and the held file stays.
pub fn export_into_with(
    held: &Path,
    dir: &Path,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &CancelToken,
) -> Result<PathBuf> {
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let (stem, ext) = split_name(held);
    let names = FreeNames {
        dir,
        stem: output::sanitize_stem(&stem),
        ext,
    };
    if !test_hooks::force_copy() {
        match names.claim(held) {
            Ok(Some(path)) => {
                deliver::take_folders_permissions(&path);
                return Ok(path);
            }
            Ok(None) => return Err(names.none_free()),
            Err(e) if crosses_devices(&e) => {}
            Err(e) => return Err(Error::io(names.first(), e)),
        }
    }
    copy_then_claim(held, &names, progress, cancel).map_err(|e| e.into_error(&names.first()))
}

fn split_name(path: &Path) -> (String, String) {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    (stem, ext)
}

/// The names [`output::create_unique`] would try in `dir`, in its order.
struct FreeNames<'a> {
    dir: &'a Path,
    stem: String,
    ext: String,
}

impl FreeNames<'_> {
    fn nth(&self, n: u32) -> PathBuf {
        self.dir
            .join(output::numbered_name(&self.stem, &self.ext, n))
    }

    fn first(&self) -> PathBuf {
        self.nth(0)
    }

    /// Rename `from` to the first of these names nothing is at, never
    /// replacing. `None` if every one is taken; an error (another volume
    /// among them) leaves `from` where it is.
    fn claim(&self, from: &Path) -> io::Result<Option<PathBuf>> {
        for n in 0..output::MAX_NAME_ATTEMPTS {
            let to = self.nth(n);
            match rename_new(from, &to) {
                Ok(()) => return Ok(Some(to)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(None)
    }

    fn none_free(&self) -> Error {
        Error::NoFreeName {
            base: self.first(),
            attempts: output::MAX_NAME_ATTEMPTS,
        }
    }
}

/// Rename `from` to `to` only if nothing is at `to`.
#[cfg(windows)]
fn rename_new(from: &Path, to: &Path) -> io::Result<()> {
    win::rename_new(from, to)
}

/// Rename `from` to `to` only if nothing is at `to`: a hard link, which
/// fails on a name that is taken, then the old name goes.
#[cfg(not(windows))]
fn rename_new(from: &Path, to: &Path) -> io::Result<()> {
    fs::hard_link(from, to)?;
    fs::remove_file(from)
}

/// Why a move didn't happen.
enum MoveEnd {
    Cancelled,
    Failed(io::Error),
    Error(Error),
}

impl MoveEnd {
    fn into_error(self, path: &Path) -> Error {
        match self {
            MoveEnd::Cancelled => Error::Cancelled,
            MoveEnd::Failed(e) => Error::io(path, e),
            MoveEnd::Error(e) => e,
        }
    }
}

impl From<io::Error> for MoveEnd {
    fn from(e: io::Error) -> MoveEnd {
        MoveEnd::Failed(e)
    }
}

fn crosses_devices(e: &io::Error) -> bool {
    e.raw_os_error() == Some(CROSS_DEVICE)
}

/// The temp file's stem for a copy to `to`: named after the target, so a
/// copy cut short reads as what it was, and after this run
/// (`run_mark`), `photo.jpg.1234-1730000000000.kiln-export.tmp`. A
/// stale run's temps note names only paths that run made, so the look at
/// start never takes one for a temp copy a running Kiln is making.
/// A long name is cut short before the mark, never after, so the stem
/// stays within `output::sanitize_stem`'s limit with its suffix whole and
/// the next start still knows the file for Kiln's.
fn temp_stem(to: &Path) -> String {
    let suffix = format!(".{}.kiln-export", run_mark());
    let name = output::sanitize_stem(&to.file_name().unwrap_or_default().to_string_lossy());
    let mut cut = name.len().min(output::MAX_STEM_BYTES - suffix.len());
    while !name.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}{suffix}", &name[..cut])
}

/// This process's mark in its temp copies' names: its pid and the time it
/// first made one, `<pid>-<unix millis>`, so no other run, even one given
/// the same pid later, makes the same name.
fn run_mark() -> &'static str {
    static MARK: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    MARK.get_or_init(|| {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        format!("{}-{millis}", std::process::id())
    })
}

/// Copy `from` to a temp file Kiln makes in `dir`, named after `target`,
/// and flush it. Returns the temp file, whole. On any error, or a cancel,
/// the temp file is removed and `from` is untouched.
fn copy_to_temp(
    from: &Path,
    dir: &Path,
    target: &Path,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &CancelToken,
) -> std::result::Result<PathBuf, MoveEnd> {
    let (tmp_file, tmp) =
        output::create_unique(dir, &temp_stem(target), "tmp").map_err(MoveEnd::Error)?;
    // Closed: the copy replaces this empty file Kiln just made.
    drop(tmp_file);
    // Noted in the held file's run folder before anything is copied, so a
    // copy cut short by a crash is found and removed at the next start. A
    // note that can't be written (the held file's drive is full, say, and
    // moving files off it is how the person frees room) doesn't stop the
    // copy: at worst a crash then leaves a temp file named as Kiln's.
    let noted = run_of(from).filter(|run| note_temp(run, &tmp, true).is_ok());
    let copied = (|| {
        copy_file(from, &tmp, progress, cancel)?;
        OpenOptions::new().write(true).open(&tmp)?.sync_all()?;
        test_hooks::maybe_fail_copy()?;
        Ok(())
    })();
    match copied {
        Ok(()) => Ok(tmp),
        Err(e) => {
            if fs::remove_file(&tmp).is_ok() {
                if let Some(run) = noted {
                    let _ = note_temp(&run, &tmp, false);
                }
            }
            Err(e)
        }
    }
}

/// The run folder `held` is directly in: a folder with a `lock` file.
fn run_of(held: &Path) -> Option<PathBuf> {
    let dir = held.parent()?;
    fs::symlink_metadata(dir.join(LOCK_NAME))
        .is_ok_and(|m| m.is_file())
        .then(|| dir.to_path_buf())
}

/// Note in the run folder `run` ([`TEMPS_NAME`]) that the temp copy `tmp`
/// is there (`on`) or has gone. One write per line.
fn note_temp(run: &Path, tmp: &Path, on: bool) -> io::Result<()> {
    let path = tmp
        .to_str()
        .filter(|p| tmp.is_absolute() && !p.contains(['\t', '\n', '\r']))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "can't be noted"))?;
    let mark = if on { '+' } else { '-' };
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(run.join(TEMPS_NAME))?
        .write_all(format!("{mark}\t{path}\n").as_bytes())
}

/// Once the move that made it has ended, whichever way, the temp copy has
/// gone or been renamed into place: noted so.
struct TempNoted {
    run: Option<PathBuf>,
    tmp: PathBuf,
}

impl TempNoted {
    fn of(from: &Path, tmp: &Path) -> TempNoted {
        TempNoted {
            run: run_of(from),
            tmp: tmp.to_path_buf(),
        }
    }
}

impl Drop for TempNoted {
    fn drop(&mut self) {
        if let Some(run) = &self.run {
            if fs::symlink_metadata(&self.tmp).is_err() {
                let _ = note_temp(run, &self.tmp, false);
            }
        }
    }
}

/// The temp copies run folder `run` noted and never noted gone.
fn noted_temps(run: &Path) -> Vec<PathBuf> {
    let mut bytes = vec![];
    let read = File::open(run.join(TEMPS_NAME))
        .and_then(|f| f.take(SOURCES_LIMIT).read_to_end(&mut bytes));
    if read.is_err() {
        return vec![];
    }
    let mut on: Vec<PathBuf> = vec![];
    let mut lines: Vec<&[u8]> = bytes.split(|&b| b == b'\n').collect();
    // After the last line break: nothing, or a line cut short.
    lines.pop();
    for line in lines {
        let Ok(line) = std::str::from_utf8(line) else {
            continue;
        };
        let Some((mark, path)) = line.split_once('\t') else {
            continue;
        };
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            continue;
        }
        on.retain(|p| p != &path);
        if mark == "+" {
            on.push(path);
        }
    }
    on
}

/// Whether `path` is named as Kiln names a temp copy:
/// `<name>.kiln-export.tmp` or `<name>.kiln-export (n).tmp`, where
/// `<name>` ends in the run's mark (`temp_stem`) or, from earlier builds,
/// doesn't.
fn is_export_temp(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let Some(stem) = name.strip_suffix(".tmp") else {
        return false;
    };
    let stem = match stem.strip_suffix(')').and_then(|s| s.rsplit_once(" (")) {
        Some((head, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => head,
        _ => stem,
    };
    stem.len() > ".kiln-export".len() && stem.ends_with(".kiln-export")
}

/// Remove the temp copies the run folder `dir`, locked by the caller,
/// noted and never noted gone: copies a crash, a kill or the end of the
/// session cut short. Only those exact files, only if they are named as
/// Kiln names them and are plain files (not links), each with a plain
/// remove. Anything else is left be.
fn clear_noted_temps(dir: &Path) {
    for tmp in noted_temps(dir) {
        let plain = fs::symlink_metadata(&tmp).is_ok_and(|m| m.is_file());
        let gone = if plain && is_export_temp(&tmp) {
            fs::remove_file(&tmp).is_ok()
        } else {
            !plain && fs::symlink_metadata(&tmp).is_err()
        };
        if gone {
            let _ = note_temp(dir, &tmp, false);
        }
    }
}

/// Copy `from` to a temp file in `names.dir`, then rename it to the first
/// of `names` nothing is at, never replacing, then remove `from`. The
/// source goes last, so any failure, or a cancel, keeps it; the temp file,
/// which only Kiln made, goes instead.
fn copy_then_claim(
    from: &Path,
    names: &FreeNames<'_>,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &CancelToken,
) -> std::result::Result<PathBuf, MoveEnd> {
    let tmp = copy_to_temp(from, names.dir, &names.first(), progress, cancel)?;
    let _noted = TempNoted::of(from, &tmp);
    // The source goes only once the copy is the same file, every byte.
    copy_matches(from, &tmp)?;
    match names.claim(&tmp) {
        Ok(Some(path)) => {
            // The copy is complete and in place, so a leftover source
            // is no reason to fail.
            let _ = fs::remove_file(from);
            Ok(path)
        }
        Ok(None) => {
            let _ = fs::remove_file(&tmp);
            Err(MoveEnd::Error(names.none_free()))
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e.into())
        }
    }
}

/// Check Kiln's temp copy `tmp` holds every byte of `from`. If it
/// doesn't, or either can't be read, the temp copy goes (only Kiln has had
/// it) and `from` stays.
fn copy_matches(from: &Path, tmp: &Path) -> std::result::Result<(), MoveEnd> {
    test_hooks::maybe_spoil(tmp);
    let fail = |e: io::Error| {
        let _ = fs::remove_file(tmp);
        MoveEnd::Failed(e)
    };
    match deliver::same_bytes(from, tmp) {
        Ok(true) => Ok(()),
        Ok(false) => Err(fail(io::Error::other(
            "the copy doesn't match the held file",
        ))),
        Err(e) => Err(fail(e)),
    }
}

/// Copy `from` over the temp file `to` with `CopyFileExW`, its progress
/// routine telling `progress` and answering `cancel`.
#[cfg(windows)]
fn copy_file(
    from: &Path,
    to: &Path,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &CancelToken,
) -> std::result::Result<(), MoveEnd> {
    let mut paced = |done: u64, total: u64| {
        test_hooks::pace(done, total);
        progress(done, total);
    };
    win::copy_file(from, to, &mut paced, cancel).map_err(|e| match e {
        win::CopyEnd::Cancelled => MoveEnd::Cancelled,
        win::CopyEnd::Failed(e) => MoveEnd::Failed(e),
    })
}

/// Copy `from` over the temp file `to` a piece at a time, telling
/// `progress` and answering `cancel` between pieces.
#[cfg(not(windows))]
fn copy_file(
    from: &Path,
    to: &Path,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &CancelToken,
) -> std::result::Result<(), MoveEnd> {
    let mut src = File::open(from)?;
    let total = src.metadata()?.len();
    let mut dst = OpenOptions::new().write(true).truncate(true).open(to)?;
    let mut buf = vec![0u8; 1024 * 1024];
    let mut done = 0u64;
    loop {
        if cancel.is_cancelled() {
            return Err(MoveEnd::Cancelled);
        }
        let n = src.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        dst.write_all(&buf[..n])?;
        done += n as u64;
        progress(done, total);
    }
}

/// Switches that let tests exercise the cross-volume path on one volume.
/// Per thread, so tests running side by side don't affect each other.
/// For a test Kiln only, in a debug or test-hooks build: every move of a
/// held file from now on is a copy, as to another drive, with a pause
/// between its pieces, so a test can end Kiln part-way through one. `tell`
/// is told how far each copy is, bytes copied and its size, after each
/// pause, so a test can see it is part-way.
#[cfg(all(not(test), any(debug_assertions, feature = "test-hooks")))]
pub fn slow_copies_for_tests(tell: fn(u64, u64)) {
    let _ = test_hooks::TELL.set(tell);
    test_hooks::SLOW_COPY.store(true, std::sync::atomic::Ordering::Relaxed);
}

#[cfg(test)]
mod test_hooks {
    use std::cell::Cell;
    use std::io;

    pub fn pace(_done: u64, _total: u64) {}

    thread_local! {
        pub static FORCE_COPY: Cell<bool> = const { Cell::new(false) };
        pub static FAIL_COPY: Cell<bool> = const { Cell::new(false) };
        pub static FAIL_CHECK: Cell<bool> = const { Cell::new(false) };
    }

    pub fn force_copy() -> bool {
        FORCE_COPY.with(Cell::get)
    }

    /// Cut a delivered copy short before it is checked, as a disk that
    /// lost the end of it would.
    pub fn maybe_spoil(to: &std::path::Path) {
        if FAIL_CHECK.with(Cell::get) {
            if let Ok(f) = std::fs::OpenOptions::new().write(true).open(to) {
                let len = f.metadata().map(|m| m.len()).unwrap_or(0);
                let _ = f.set_len(len / 2);
            }
        }
    }

    /// A copy that fails as one to a full disk does.
    pub fn maybe_fail_copy() -> io::Result<()> {
        if FAIL_COPY.with(Cell::get) {
            Err(io::Error::from(io::ErrorKind::StorageFull))
        } else {
            Ok(())
        }
    }
}

#[cfg(not(test))]
mod test_hooks {
    use std::io;
    #[cfg(any(debug_assertions, feature = "test-hooks"))]
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Set by [`super::slow_copies_for_tests`].
    #[cfg(any(debug_assertions, feature = "test-hooks"))]
    pub static SLOW_COPY: AtomicBool = AtomicBool::new(false);

    /// Told how far a slowed copy is ([`super::slow_copies_for_tests`]).
    #[cfg(any(debug_assertions, feature = "test-hooks"))]
    pub static TELL: std::sync::OnceLock<fn(u64, u64)> = std::sync::OnceLock::new();

    #[cfg(any(debug_assertions, feature = "test-hooks"))]
    fn slow() -> bool {
        SLOW_COPY.load(Ordering::Relaxed)
    }

    #[cfg(not(any(debug_assertions, feature = "test-hooks")))]
    fn slow() -> bool {
        false
    }

    pub fn force_copy() -> bool {
        slow()
    }

    /// A pause between the pieces of a slowed copy, then how far it is
    /// told.
    pub fn pace(_done: u64, _total: u64) {
        if slow() {
            std::thread::sleep(std::time::Duration::from_millis(50));
            #[cfg(any(debug_assertions, feature = "test-hooks"))]
            if let Some(tell) = TELL.get() {
                tell(_done, _total);
            }
        }
    }

    pub fn maybe_fail_copy() -> io::Result<()> {
        Ok(())
    }

    pub fn maybe_spoil(_to: &std::path::Path) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn a_run_with_an_older_builds_discard_note_is_offered_like_any_other() {
        let root = tempfile::tempdir().unwrap();
        // An older build noted a Discard and ended before it could finish.
        let run = root.path().join("2-2");
        fs::create_dir(&run).unwrap();
        fs::write(run.join(LOCK_NAME), "").unwrap();
        let b = run.join("b.jpg");
        fs::write(&b, "b").unwrap();
        fs::write(run.join(DISCARD_NAME), "2-2").unwrap();
        let runs = stale_runs(root.path());
        assert!(b.exists(), "a held result is never removed undelivered");
        assert_eq!(runs.len(), 1, "offered, to be delivered");
        assert_eq!(runs[0].files.len(), 1, "the note is no result");
        assert_eq!(runs[0].files[0].path, b);
        drop(runs);
        // Once its results have gone, the note goes with the folder.
        fs::remove_file(&b).unwrap();
        sweep_stale(root.path());
        assert!(!run.exists());
    }

    #[test]
    fn a_run_with_a_discard_note_and_more_than_plain_files_is_left_whole() {
        let root = tempfile::tempdir().unwrap();
        let run = root.path().join("1-1");
        fs::create_dir(&run).unwrap();
        fs::write(run.join(LOCK_NAME), "").unwrap();
        fs::write(run.join(DISCARD_NAME), "1-1").unwrap();
        let a = run.join("a.jpg");
        fs::write(&a, "a").unwrap();
        let inner = run.join("folder");
        fs::create_dir(&inner).unwrap();
        fs::write(inner.join("b.jpg"), "b").unwrap();
        assert!(stale_runs(root.path()).is_empty());
        assert!(
            a.exists() && inner.join("b.jpg").exists(),
            "not a folder Kiln made"
        );
    }

    #[test]
    fn a_temp_copy_left_by_a_crash_is_removed_at_the_next_start_and_nothing_else() {
        let root = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let run = root.path().join("1-1");
        fs::create_dir(&run).unwrap();
        fs::write(run.join(LOCK_NAME), "").unwrap();
        fs::write(run.join("photo.jpg"), "held").unwrap();
        // What a copy cut short left, noted; one noted and since gone;
        // and files Kiln didn't note or doesn't name so, which stay.
        let cut = out.path().join("photo.jpg.kiln-export.tmp");
        fs::write(&cut, "half").unwrap();
        let done = out.path().join("other.jpg.kiln-export (1).tmp");
        let theirs = out.path().join("photo.jpg.kiln-export (2).tmp");
        fs::write(&theirs, "theirs").unwrap();
        let odd = out.path().join("notes.txt");
        fs::write(&odd, "noted, but not a temp copy's name").unwrap();
        note_temp(&run, &cut, true).unwrap();
        note_temp(&run, &done, true).unwrap();
        note_temp(&run, &done, false).unwrap();
        note_temp(&run, &odd, true).unwrap();

        let runs = stale_runs(root.path());
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].files.len(), 1, "the note is no result");
        assert!(!cut.exists(), "the copy cut short goes");
        assert!(theirs.exists() && odd.exists(), "nothing else does");
        assert_eq!(noted_temps(&run), std::slice::from_ref(&odd));
        // Once its result is saved, the run folder goes, note and all.
        fs::remove_file(run.join("photo.jpg")).unwrap();
        assert!(runs.into_iter().next().unwrap().close());
        assert!(!run.exists());
    }

    #[test]
    fn a_stale_runs_temps_note_never_removes_a_temp_copy_this_run_is_making() {
        // The last run renamed its temp copy into place and was killed
        // before it noted it gone: its note still names the path.
        let root = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let run = root.path().join("1-1");
        fs::create_dir(&run).unwrap();
        fs::write(run.join(LOCK_NAME), "").unwrap();
        fs::write(run.join("photo.jpg"), "held").unwrap();
        note_temp(&run, &out.path().join("photo.jpg.kiln-export.tmp"), true).unwrap();
        // While the look is slow, this run exports a new photo.jpg to the
        // same folder: its temp copy is the first free name it makes.
        let (_file, mine) =
            output::create_unique(out.path(), &temp_stem(&out.path().join("photo.jpg")), "tmp")
                .unwrap();
        fs::write(&mine, "copying").unwrap();
        // The look reaches the stale run.
        let runs = stale_runs(root.path());
        assert_eq!(runs.len(), 1);
        assert!(
            mine.exists(),
            "this run's temp copy stays: {}",
            mine.display()
        );
        assert_eq!(fs::read_to_string(&mine).unwrap(), "copying");
    }

    #[test]
    fn a_copy_goes_ahead_when_its_temp_copy_cant_be_noted() {
        test_hooks::FORCE_COPY.with(|c| c.set(true));
        let root = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let run = root.path().join("1-1");
        fs::create_dir(&run).unwrap();
        fs::write(run.join(LOCK_NAME), "").unwrap();
        // A folder where the note goes: it can't be written.
        fs::create_dir(run.join(TEMPS_NAME)).unwrap();
        let held = run.join("photo.jpg");
        fs::write(&held, "held").unwrap();
        let got = export_into(&held, out.path()).unwrap();
        assert_eq!(fs::read_to_string(&got).unwrap(), "held");
        assert!(!held.exists());
        let left: Vec<_> = fs::read_dir(out.path()).unwrap().flatten().collect();
        assert_eq!(left.len(), 1, "no temp copy is left: {left:?}");
        test_hooks::FORCE_COPY.with(|c| c.set(false));
    }

    #[test]
    fn a_long_names_temp_copy_keeps_its_kiln_suffix() {
        let names = [
            "a".repeat(400),
            "é".repeat(150),
            format!("{}.jpg", "b".repeat(300)),
        ];
        for name in names {
            let stem = temp_stem(Path::new(&name));
            assert_eq!(output::sanitize_stem(&stem), stem, "nothing more is cut");
            assert!(stem.len() <= output::MAX_STEM_BYTES);
            assert!(is_export_temp(Path::new(&format!("{stem}.tmp"))), "{stem}");
            assert!(is_export_temp(Path::new(&format!("{stem} (3).tmp"))));
        }
    }

    #[test]
    fn a_sweep_removes_a_noted_temp_copy_too() {
        let root = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let run = root.path().join("1-1");
        fs::create_dir(&run).unwrap();
        fs::write(run.join(LOCK_NAME), "").unwrap();
        let cut = out.path().join("a.png.kiln-export.tmp");
        fs::write(&cut, "half").unwrap();
        note_temp(&run, &cut, true).unwrap();
        assert_eq!(sweep_stale(root.path()), std::slice::from_ref(&run));
        assert!(!cut.exists() && !run.exists());
    }

    #[test]
    fn a_live_runs_temp_copy_is_never_touched() {
        let root = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let live = Holding::open(root.path()).unwrap();
        let tmp = out.path().join("a.png.kiln-export.tmp");
        fs::write(&tmp, "copying").unwrap();
        note_temp(live.dir(), &tmp, true).unwrap();
        assert!(stale_runs(root.path()).is_empty());
        assert!(sweep_stale(root.path()).is_empty());
        assert!(tmp.exists());
    }

    #[test]
    fn only_kilns_temp_names_count() {
        for yes in [
            "a.jpg.kiln-export.tmp",
            "a.jpg.kiln-export (3).tmp",
            "a.jpg.1234-1730000000000.kiln-export.tmp",
            "a.jpg.1234-1730000000000.kiln-export (2).tmp",
        ] {
            assert!(is_export_temp(Path::new(yes)), "{yes}");
        }
        for no in [
            ".kiln-export.tmp",
            "a.jpg",
            "a.jpg.tmp",
            "a.kiln-export (x).tmp",
        ] {
            assert!(!is_export_temp(Path::new(no)), "{no}");
        }
    }

    fn write(path: &Path, text: &str) {
        fs::write(path, text).unwrap();
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn two_opens_give_different_dirs() {
        let root = tempfile::tempdir().unwrap();
        let a = Holding::open(root.path()).unwrap();
        let b = Holding::open(root.path()).unwrap();
        assert_ne!(a.dir(), b.dir());
        assert!(a.dir().join("lock").is_file());
        assert!(a.dir().starts_with(root.path()));
    }

    #[test]
    fn open_creates_a_missing_root() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("held");
        let h = Holding::open(&root).unwrap();
        assert!(h.dir().is_dir());
    }

    #[cfg(windows)]
    #[test]
    fn the_lock_cannot_be_deleted_while_held() {
        let root = tempfile::tempdir().unwrap();
        let h = Holding::open(root.path()).unwrap();
        assert!(fs::remove_file(h.dir().join("lock")).is_err());
    }

    #[test]
    fn sweep_keeps_a_locked_dir_and_removes_an_unlocked_one() {
        let root = tempfile::tempdir().unwrap();
        let live = Holding::open(root.path()).unwrap();
        write(&live.dir().join("keep.jpg"), "keep");

        // Without results: a stale run's results are offered, not swept.
        let stale = root.path().join("999-1");
        fs::create_dir(&stale).unwrap();
        write(&stale.join("lock"), "");
        write(&stale.join(SOURCES_NAME), "old.jpg\tC:\\x\n");

        let removed = sweep_stale(root.path());
        assert_eq!(removed, vec![stale.clone()]);
        assert!(!stale.exists());
        assert!(live.dir().join("keep.jpg").is_file());
        assert!(live.dir().join("lock").is_file());
    }

    #[test]
    fn sweep_ignores_files_and_dirs_without_a_lock() {
        let root = tempfile::tempdir().unwrap();
        write(&root.path().join("stray.txt"), "s");
        let plain = root.path().join("plain");
        fs::create_dir(&plain).unwrap();
        write(&plain.join("a.jpg"), "a");
        // A file called "lock" at the top level is not a run folder.
        write(&root.path().join("lock"), "");

        assert!(sweep_stale(root.path()).is_empty());
        assert_eq!(names(root.path()), ["lock", "plain", "stray.txt"]);
        assert_eq!(names(&plain), ["a.jpg"]);
    }

    #[test]
    fn sweep_leaves_a_folder_with_subfolders_alone() {
        let root = tempfile::tempdir().unwrap();
        let odd = root.path().join("odd");
        fs::create_dir_all(odd.join("inner")).unwrap();
        write(&odd.join("lock"), "");
        write(&odd.join("inner").join("deep.txt"), "d");

        assert!(sweep_stale(root.path()).is_empty());
        assert!(odd.join("lock").is_file());
        assert!(odd.join("inner").join("deep.txt").is_file());
    }

    #[test]
    fn sweep_touches_nothing_outside_root() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("held");
        fs::create_dir(&root).unwrap();
        let outside = base.path().join("outside");
        fs::create_dir(&outside).unwrap();
        write(&outside.join("lock"), "");
        write(&outside.join("mine.txt"), "m");

        let stale = root.join("1-1");
        fs::create_dir(&stale).unwrap();
        write(&stale.join("lock"), "");

        assert!(stale_runs(&root).is_empty());
        assert_eq!(sweep_stale(&root), vec![stale]);
        assert!(outside.join("lock").is_file());
        assert!(outside.join("mine.txt").is_file());
    }

    #[cfg(windows)]
    #[test]
    fn sweep_does_not_follow_a_link_out_of_root() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("held");
        fs::create_dir(&root).unwrap();
        let outside = base.path().join("outside");
        fs::create_dir(&outside).unwrap();
        write(&outside.join("lock"), "");
        write(&outside.join("mine.txt"), "m");

        // A junction needs no privilege on Windows; use `mklink /J`.
        let link = root.join("link");
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&outside)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        // Without the junction there's nothing to test: say so, don't pass.
        assert!(made, "couldn't make a junction with `cmd /C mklink /J`");
        assert!(stale_runs(&root).is_empty());
        assert!(sweep_stale(&root).is_empty());
        assert!(outside.join("lock").is_file());
        assert!(outside.join("mine.txt").is_file());
    }

    #[test]
    fn a_cross_drive_copy_is_named_after_its_target() {
        let stem = format!("photo.jpg.{}.kiln-export", run_mark());
        assert_eq!(temp_stem(Path::new(r"D:\out\photo.jpg")), stem);
        let dir = tempfile::tempdir().unwrap();
        let (_, tmp) =
            output::create_unique(dir.path(), &temp_stem(&dir.path().join("photo.jpg")), "tmp")
                .unwrap();
        assert_eq!(
            tmp.file_name().unwrap().to_str(),
            Some(&*format!("{stem}.tmp"))
        );
        assert!(is_export_temp(&tmp));
    }

    #[test]
    fn sweep_of_a_missing_root_is_empty() {
        let base = tempfile::tempdir().unwrap();
        assert!(sweep_stale(&base.path().join("nope")).is_empty());
        assert!(stale_runs(&base.path().join("nope")).is_empty());
    }

    /// An absolute folder named `rest` under a drive, as a source's folder
    /// is.
    fn origin(rest: &str) -> PathBuf {
        #[cfg(windows)]
        let base = PathBuf::from(r"C:\x");
        #[cfg(not(windows))]
        let base = PathBuf::from("/x");
        base.join(rest)
    }

    /// A run folder under `root` as a run that ended without saving
    /// left it: its lock closed, and `files` in it.
    fn left_run(root: &Path, name: &str, files: &[&str], sources: Option<&str>) -> PathBuf {
        let dir = root.join(name);
        fs::create_dir(&dir).unwrap();
        write(&dir.join("lock"), "");
        for f in files {
            write(&dir.join(f), f);
        }
        if let Some(text) = sources {
            write(&dir.join(SOURCES_NAME), text);
        }
        dir
    }

    #[test]
    fn recorded_sources_come_back_with_their_files() {
        let root = tempfile::tempdir().unwrap();
        let h = Holding::open(root.path()).unwrap();
        let a = h.dir().join("a.jpg");
        write(&a, "a");
        // A long folder, and not ASCII: 240 characters in all.
        let mut long = origin("Ana María").to_string_lossy().into_owned();
        while long.chars().count() < 240 {
            long.push_str("\\fotos");
        }
        let long = PathBuf::from(long.chars().take(240).collect::<String>());
        assert_eq!(long.to_string_lossy().chars().count(), 240);
        // The last line naming a result is the one that counts.
        h.record(&a, &origin("before")).unwrap();
        h.record(&a, &long).unwrap();
        let dir = h.dir().to_path_buf();
        // Ended without a word: no discard.
        drop(h);

        let runs = stale_runs(root.path());
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].dir, dir);
        assert_eq!(
            runs[0].files,
            [Recovered {
                path: a,
                origin: Some(long),
            }]
        );
    }

    #[test]
    fn a_live_run_is_never_listed() {
        let root = tempfile::tempdir().unwrap();
        let live = Holding::open(root.path()).unwrap();
        let a = live.dir().join("a.jpg");
        write(&a, "a");
        live.record(&a, &origin("photos")).unwrap();
        let sources = fs::read(live.dir().join(SOURCES_NAME)).unwrap();

        assert!(stale_runs(root.path()).is_empty());
        assert!(sweep_stale(root.path()).is_empty());
        assert_eq!(fs::read_to_string(&a).unwrap(), "a");
        assert_eq!(fs::read(live.dir().join(SOURCES_NAME)).unwrap(), sources);
        assert_eq!(names(live.dir()), ["a.jpg", "lock", SOURCES_NAME]);
        // Still its own: once its result has gone, it closes its folder.
        let dir = live.dir().to_path_buf();
        fs::remove_file(&a).unwrap();
        assert!(live.close_if_empty());
        assert!(!dir.exists());
    }

    #[test]
    fn a_listed_run_is_locked_while_offered() {
        let root = tempfile::tempdir().unwrap();
        let dir = left_run(root.path(), "1-1", &["a.jpg"], None);
        let runs = stale_runs(root.path());
        assert_eq!(runs.len(), 1);

        // Another Kiln looking now finds nothing to offer.
        assert!(stale_runs(root.path()).is_empty());
        // Nor to sweep, even once its results have gone.
        fs::remove_file(dir.join("a.jpg")).unwrap();
        assert!(sweep_stale(root.path()).is_empty());
        assert_eq!(names(&dir), ["lock"]);

        // Let go, it is an empty stale run like any other.
        drop(runs);
        assert_eq!(sweep_stale(root.path()), vec![dir]);
    }

    #[test]
    fn files_already_moved_are_not_offered() {
        let root = tempfile::tempdir().unwrap();
        let photos = origin("photos");
        let sources = format!(
            "a.jpg\t{}\nb.jpg\t{}\nb.jpg\t{}\n",
            photos.display(),
            origin("old").display(),
            photos.display()
        );
        let dir = left_run(root.path(), "1-1", &["b.jpg"], Some(&sources));

        let runs = stale_runs(root.path());
        assert_eq!(runs.len(), 1);
        assert_eq!(
            runs[0].files,
            [Recovered {
                path: dir.join("b.jpg"),
                origin: Some(photos),
            }]
        );
    }

    #[test]
    fn an_old_run_without_sources_has_no_origins() {
        let root = tempfile::tempdir().unwrap();
        let dir = left_run(root.path(), "999-1", &["b.png", "a.jpg"], None);

        let runs = stale_runs(root.path());
        assert_eq!(runs.len(), 1);
        assert_eq!(
            runs[0].files,
            [
                Recovered {
                    path: dir.join("a.jpg"),
                    origin: None,
                },
                Recovered {
                    path: dir.join("b.png"),
                    origin: None,
                },
            ]
        );
    }

    #[test]
    fn a_line_that_cannot_be_trusted_gives_no_origin() {
        let root = tempfile::tempdir().unwrap();
        // A line cut short as Kiln ended, one without a tab, one whose
        // folder isn't a whole path, and one with a tab in its folder.
        let sources = format!(
            "nothing here\nb.jpg\tphotos\nc.jpg\t{}\tmore\na.jpg\t{}",
            origin("photos").display(),
            origin("photos").display()
        );
        let dir = left_run(
            root.path(),
            "1-1",
            &["a.jpg", "b.jpg", "c.jpg"],
            Some(&sources),
        );

        let runs = stale_runs(root.path());
        assert_eq!(runs.len(), 1);
        assert!(runs[0].files.iter().all(|f| f.origin.is_none()));
        assert_eq!(runs[0].files.len(), 3);
        assert_eq!(runs[0].dir, dir);
    }

    #[test]
    fn record_refuses_what_it_could_not_read_back() {
        let root = tempfile::tempdir().unwrap();
        let h = Holding::open(root.path()).unwrap();
        let a = h.dir().join("a.jpg");
        write(&a, "a");
        // A folder that isn't a whole path, and a result elsewhere.
        assert!(h.record(&a, Path::new("photos")).is_err());
        let elsewhere = root.path().join("a.jpg");
        assert!(h.record(&elsewhere, &origin("photos")).is_err());
        assert!(!h.dir().join(SOURCES_NAME).exists());
    }

    #[test]
    fn a_result_that_left_gives_its_name_no_origin() {
        let root = tempfile::tempdir().unwrap();
        let h = Holding::open(root.path()).unwrap();
        let a = h.dir().join("a.jpg");
        write(&a, "first");
        h.record(&a, &origin("photos")).unwrap();
        // Saved, exported or discarded: it leaves, and is cleared.
        fs::remove_file(&a).unwrap();
        h.forget(&a).unwrap();
        // A new a.jpg takes its place, and Kiln ends before noting it.
        write(&a, "second");
        drop(h);

        let runs = stale_runs(root.path());
        assert_eq!(runs.len(), 1);
        assert_eq!(
            runs[0].files,
            [Recovered {
                path: a,
                origin: None,
            }]
        );
    }

    #[test]
    fn a_result_noted_again_after_it_was_cleared_has_its_new_origin() {
        let root = tempfile::tempdir().unwrap();
        let h = Holding::open(root.path()).unwrap();
        let a = h.dir().join("a.jpg");
        write(&a, "first");
        h.record(&a, &origin("old")).unwrap();
        h.forget(&a).unwrap();
        h.record(&a, &origin("new")).unwrap();
        let text = fs::read_to_string(h.dir().join(SOURCES_NAME)).unwrap();
        assert_eq!(
            text,
            format!(
                "a.jpg\t{}\na.jpg\t\na.jpg\t{}\n",
                origin("old").display(),
                origin("new").display()
            )
        );
        drop(h);
        let runs = stale_runs(root.path());
        assert_eq!(runs[0].files[0].origin, Some(origin("new")));
    }

    #[test]
    fn forgetting_writes_nothing_where_nothing_was_noted() {
        let root = tempfile::tempdir().unwrap();
        let h = Holding::open(root.path()).unwrap();
        let a = h.dir().join("a.jpg");
        write(&a, "a");
        h.forget(&a).unwrap();
        assert!(!h.dir().join(SOURCES_NAME).exists());
        // Nor for a file that isn't in the folder.
        h.record(&a, &origin("photos")).unwrap();
        assert!(forget_source(h.dir(), &root.path().join("a.jpg")).is_err());
        assert!(h.forget(&root.path().join("a.jpg")).is_err());
    }

    #[test]
    fn an_empty_placeholder_is_not_a_result_and_goes_with_its_run() {
        let root = tempfile::tempdir().unwrap();
        // `place` made b.jpg's placeholder, and Kiln ended before the
        // result took its place.
        let dir = left_run(root.path(), "1-1", &["a.jpg"], None);
        write(&dir.join("b.jpg"), "");
        let only = left_run(root.path(), "1-2", &[], None);
        write(&only.join("c.jpg"), "");

        let mut runs = stale_runs(root.path());
        assert_eq!(
            runs.len(),
            1,
            "a run with only a placeholder has nothing to offer"
        );
        assert_eq!(
            runs[0].files,
            [Recovered {
                path: dir.join("a.jpg"),
                origin: None,
            }]
        );
        // Its last result saved, the run goes, placeholder and all.
        fs::remove_file(dir.join("a.jpg")).unwrap();
        assert!(runs.remove(0).close());
        assert!(!dir.exists());

        // And a run with only a placeholder is swept.
        assert_eq!(sweep_stale(root.path()), vec![only.clone()]);
        assert!(!only.exists());
    }

    #[test]
    fn a_folder_with_subfolders_is_not_offered() {
        let root = tempfile::tempdir().unwrap();
        let odd = left_run(root.path(), "odd", &["a.jpg"], None);
        fs::create_dir(odd.join("inner")).unwrap();
        assert!(stale_runs(root.path()).is_empty());
        assert_eq!(names(&odd), ["a.jpg", "inner", "lock"]);
    }

    #[test]
    fn sweep_leaves_runs_with_results_and_removes_empty_ones() {
        let root = tempfile::tempdir().unwrap();
        let photos = format!("a.jpg\t{}\n", origin("photos").display());
        let full = left_run(root.path(), "1-1", &["a.jpg"], Some(&photos));
        let old = left_run(root.path(), "1-2", &["b.jpg"], None);
        let saved = left_run(root.path(), "1-3", &[], Some(&photos));
        let bare = left_run(root.path(), "1-4", &[], None);

        let mut removed = sweep_stale(root.path());
        removed.sort();
        assert_eq!(removed, [saved.clone(), bare.clone()]);
        assert!(!saved.exists() && !bare.exists());
        assert_eq!(names(&full), ["a.jpg", "lock", SOURCES_NAME]);
        assert_eq!(names(&old), ["b.jpg", "lock"]);
    }

    #[test]
    fn close_removes_the_run_only_once_empty() {
        let root = tempfile::tempdir().unwrap();
        let photos = format!("a.jpg\t{}\n", origin("photos").display());
        let dir = left_run(root.path(), "1-1", &["a.jpg", "b.jpg"], Some(&photos));

        // Something is still in it: it stays, whole.
        let mut runs = stale_runs(root.path());
        fs::remove_file(dir.join("b.jpg")).unwrap();
        assert!(!runs.remove(0).close());
        assert_eq!(names(&dir), ["a.jpg", "lock", SOURCES_NAME]);

        // Its last result saved, it goes.
        let mut runs = stale_runs(root.path());
        assert_eq!(runs[0].files.len(), 1);
        fs::remove_file(dir.join("a.jpg")).unwrap();
        assert!(runs.remove(0).close());
        assert!(!dir.exists());
        assert!(names(root.path()).is_empty());
    }

    #[test]
    fn export_into_collides_to_numbered_names() {
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        write(&src.path().join("a.jpg"), "one");
        let first = export_into(&src.path().join("a.jpg"), out.path()).unwrap();
        write(&src.path().join("a.jpg"), "two");
        let second = export_into(&src.path().join("a.jpg"), out.path()).unwrap();

        assert_eq!(first.file_name().unwrap(), "a.jpg");
        assert_eq!(second.file_name().unwrap(), "a (1).jpg");
        assert_eq!(fs::read_to_string(&first).unwrap(), "one");
        assert_eq!(fs::read_to_string(&second).unwrap(), "two");
        assert!(!src.path().join("a.jpg").exists());
    }

    #[test]
    fn export_of_a_missing_file_fails_and_leaves_no_placeholder() {
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        assert!(export_into(&src.path().join("gone.jpg"), out.path()).is_err());
        assert!(names(out.path()).is_empty());
    }

    #[test]
    fn cross_volume_copy_path_moves_the_file() {
        test_hooks::FORCE_COPY.with(|c| c.set(true));
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        write(&src.path().join("a.jpg"), "data");
        write(&out.path().join("a.jpg"), "taken");

        let got = export_into(&src.path().join("a.jpg"), out.path()).unwrap();
        assert_eq!(got.file_name().unwrap(), "a (1).jpg");
        assert_eq!(fs::read_to_string(&got).unwrap(), "data");
        assert_eq!(
            fs::read_to_string(out.path().join("a.jpg")).unwrap(),
            "taken"
        );
        assert!(!src.path().join("a.jpg").exists());
        // No temp copy is left behind.
        assert_eq!(names(out.path()), ["a (1).jpg", "a.jpg"]);
    }

    #[test]
    fn a_failed_copy_keeps_the_source_and_removes_the_partial() {
        test_hooks::FORCE_COPY.with(|c| c.set(true));
        test_hooks::FAIL_COPY.with(|c| c.set(true));
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        write(&src.path().join("a.jpg"), "data");

        assert!(export_into(&src.path().join("a.jpg"), out.path()).is_err());
        assert_eq!(
            fs::read_to_string(src.path().join("a.jpg")).unwrap(),
            "data"
        );
        assert!(names(out.path()).is_empty());
    }

    #[test]
    fn an_export_copy_that_doesnt_match_keeps_the_held_file() {
        test_hooks::FORCE_COPY.with(|c| c.set(true));
        test_hooks::FAIL_CHECK.with(|c| c.set(true));
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let bytes = "a held result long enough to cut in half";
        write(&src.path().join("a.jpg"), bytes);
        let into = export_into(&src.path().join("a.jpg"), out.path());
        test_hooks::FAIL_CHECK.with(|c| c.set(false));
        assert!(into.is_err(), "{into:?}");
        assert_eq!(
            fs::read_to_string(src.path().join("a.jpg")).unwrap(),
            bytes,
            "the held file stays until a copy is the same file"
        );
        assert!(names(out.path()).is_empty());
    }

    /// A held file big enough that a copy reports progress part-way.
    fn big_held(dir: &Path) -> (PathBuf, Vec<u8>) {
        let bytes: Vec<u8> = (0..48u32 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
        let held = dir.join("big.bin");
        fs::write(&held, &bytes).unwrap();
        (held, bytes)
    }

    #[test]
    fn a_copy_cancelled_midway_keeps_the_held_result_and_leaves_nothing() {
        test_hooks::FORCE_COPY.with(|c| c.set(true));
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let (held, bytes) = big_held(src.path());
        let total = bytes.len() as u64;

        let cancel = CancelToken::new();
        let mut seen = vec![];
        let mut progress = |done: u64, size: u64| {
            seen.push((done, size));
            if done > 0 && done < size {
                cancel.cancel();
            }
        };
        let err = export_into_with(&held, out.path(), &mut progress, &cancel).unwrap_err();
        assert!(matches!(err, Error::Cancelled), "{err}");
        // It stopped part-way: no report reached the end.
        assert!(
            seen.iter().any(|&(d, s)| d > 0 && d < s && s == total),
            "{seen:?}"
        );
        assert!(seen.iter().all(|&(d, _)| d < total), "{seen:?}");
        // The held result is whole; no temp copy, no destination file.
        assert_eq!(fs::read(&held).unwrap(), bytes);
        assert!(names(out.path()).is_empty());
    }

    #[test]
    fn a_copy_reports_progress_to_the_end() {
        test_hooks::FORCE_COPY.with(|c| c.set(true));
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let (held, bytes) = big_held(src.path());
        write(&out.path().join("big.bin"), "taken");

        let mut last = (0, 0);
        let got = export_into_with(
            &held,
            out.path(),
            &mut |d, s| last = (d, s),
            &CancelToken::new(),
        )
        .unwrap();
        assert_eq!(got.file_name().unwrap(), "big (1).bin");
        assert_eq!(last, (bytes.len() as u64, bytes.len() as u64));
        assert_eq!(fs::read(&got).unwrap(), bytes);
        assert!(!held.exists());
        assert_eq!(names(out.path()), ["big (1).bin", "big.bin"]);
    }

    #[test]
    fn a_file_that_appears_at_the_name_mid_copy_is_never_replaced_or_deleted() {
        test_hooks::FORCE_COPY.with(|c| c.set(true));
        let src = tempfile::tempdir().unwrap();
        let (held, bytes) = big_held(src.path());

        for cancelled in [false, true] {
            let out = tempfile::tempdir().unwrap();
            let theirs = out.path().join("big.bin");
            let cancel = CancelToken::new();
            let mut placed = false;
            let mut progress = |done: u64, size: u64| {
                if !placed && done > 0 && done < size {
                    // No name is claimed while the copy runs: only Kiln's
                    // temp file is there.
                    let tmp = format!("big.bin.{}.kiln-export.tmp", run_mark());
                    assert_eq!(names(out.path()), [tmp]);
                    write(&theirs, "theirs");
                    placed = true;
                    if cancelled {
                        cancel.cancel();
                    }
                }
            };
            let got = export_into_with(&held, out.path(), &mut progress, &cancel);
            assert!(placed, "the copy never reported progress part-way");
            // Theirs, whatever happened: never replaced, never removed.
            assert_eq!(fs::read_to_string(&theirs).unwrap(), "theirs");
            if cancelled {
                assert!(matches!(got, Err(Error::Cancelled)));
                assert_eq!(fs::read(&held).unwrap(), bytes);
                assert_eq!(names(out.path()), ["big.bin"]);
            } else {
                let got = got.unwrap();
                assert_eq!(got.file_name().unwrap(), "big (1).bin");
                assert_eq!(fs::read(&got).unwrap(), bytes);
                assert_eq!(names(out.path()), ["big (1).bin", "big.bin"]);
                // Put back for the cancelled round.
                fs::rename(&got, &held).unwrap();
            }
        }
    }

    #[test]
    fn a_same_volume_move_takes_the_next_free_name() {
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        write(&src.path().join("a.jpg"), "data");
        write(&out.path().join("a.jpg"), "one");
        fs::create_dir(out.path().join("a (1).jpg")).unwrap();
        let got = export_into(&src.path().join("a.jpg"), out.path()).unwrap();
        assert_eq!(got.file_name().unwrap(), "a (2).jpg");
        assert_eq!(fs::read_to_string(&got).unwrap(), "data");
        assert_eq!(fs::read_to_string(out.path().join("a.jpg")).unwrap(), "one");
    }

    #[test]
    fn a_move_cancelled_before_it_starts_moves_nothing() {
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        write(&src.path().join("a.jpg"), "a");
        let cancel = CancelToken::new();
        cancel.cancel();
        let err = export_into_with(
            &src.path().join("a.jpg"),
            out.path(),
            &mut |_, _| {},
            &cancel,
        )
        .unwrap_err();
        assert!(matches!(err, Error::Cancelled));
        assert_eq!(fs::read_to_string(src.path().join("a.jpg")).unwrap(), "a");
        assert!(names(out.path()).is_empty());
    }

    /// A panic must not unwind through `CopyFileExW`.
    #[cfg(windows)]
    #[test]
    fn a_panicking_progress_report_stops_the_copy_safely() {
        test_hooks::FORCE_COPY.with(|c| c.set(true));
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        write(&src.path().join("a.jpg"), "data");
        let err = export_into_with(
            &src.path().join("a.jpg"),
            out.path(),
            &mut |_, _| panic!("report"),
            &CancelToken::new(),
        )
        .unwrap_err();
        assert!(!matches!(err, Error::Cancelled), "{err}");
        assert_eq!(
            fs::read_to_string(src.path().join("a.jpg")).unwrap(),
            "data"
        );
        assert!(names(out.path()).is_empty());
    }
}
