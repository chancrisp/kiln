//! Held results: a private per-run folder for finished files, and the moves
//! that export them.
//!
//! The app points jobs at a [`Holding`] folder instead of the source's
//! folder. Results stay there until the user exports them (or discards
//! them), so a conversion never lands somewhere the user didn't choose.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{Error, Result};
use crate::output;

/// Name of the file each run keeps open to say "this folder is in use".
const LOCK_NAME: &str = "lock";

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

    /// Remove this run's folder and everything in it. The lock goes last,
    /// so if a result is open elsewhere and can't be removed, the folder
    /// keeps its lock and the next launch's sweep can retry it.
    pub fn discard(self) -> Result<()> {
        let Holding { dir, _lock } = self;
        let entries = fs::read_dir(&dir).map_err(|e| Error::io(&dir, e))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_name() == LOCK_NAME {
                continue;
            }
            let removed = match entry.file_type() {
                Ok(t) if t.is_dir() => fs::remove_dir_all(&path),
                _ => fs::remove_file(&path),
            };
            removed.map_err(|e| Error::io(&path, e))?;
        }
        // Windows can't delete an open file.
        drop(_lock);
        let lock = dir.join(LOCK_NAME);
        fs::remove_file(&lock).map_err(|e| Error::io(&lock, e))?;
        fs::remove_dir(&dir).map_err(|e| Error::io(dir, e))
    }
}

/// Create the lock file and keep it open with share mode 0, so no other
/// process can open, delete or replace it while we hold it.
fn lock_file(path: &Path) -> io::Result<File> {
    let mut opts = OpenOptions::new();
    opts.read(true).write(true).create_new(true);
    #[cfg(windows)]
    std::os::windows::fs::OpenOptionsExt::share_mode(&mut opts, 0);
    opts.open(path)
}

/// Open an existing lock the same way. Succeeds only if nobody holds it.
fn probe_lock(path: &Path) -> io::Result<File> {
    let mut opts = OpenOptions::new();
    opts.read(true);
    #[cfg(windows)]
    std::os::windows::fs::OpenOptionsExt::share_mode(&mut opts, 0);
    opts.open(path)
}

/// Remove run folders under `root` that no live run holds, such as what a
/// crashed run left behind. Returns the folders removed.
///
/// Conservative on purpose. It looks only at direct child folders of
/// `root` that contain a plain file named `lock`, skips any whose lock is
/// open, never follows links, and leaves a folder alone if it holds
/// anything but plain files.
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

/// Remove one stale run folder. False if it's live, isn't ours, or
/// couldn't be fully removed.
fn sweep_one(dir: &Path) -> bool {
    let lock = dir.join(LOCK_NAME);
    match fs::symlink_metadata(&lock) {
        Ok(m) if m.is_file() => {}
        _ => return false,
    }
    // Plain files only. Anything else means this isn't a folder we made.
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    let mut files = Vec::new();
    for entry in entries.flatten() {
        match entry.file_type() {
            Ok(t) if t.is_file() => files.push(entry.path()),
            _ => return false,
        }
    }
    // If a live run holds the lock this fails, and we touch nothing. Keep
    // the handle until the files are gone, so a failure part-way leaves the
    // lock for the next launch to retry.
    let Ok(probe) = probe_lock(&lock) else {
        return false;
    };
    for file in files
        .iter()
        .filter(|f| f.file_name() != Some(LOCK_NAME.as_ref()))
    {
        if fs::remove_file(file).is_err() {
            return false;
        }
    }
    drop(probe);
    fs::remove_file(&lock).is_ok() && fs::remove_dir(dir).is_ok()
}

/// Move the held file `held` into `dir` under a free name (`name.ext`, then
/// `name (1).ext`, and so on). Never overwrites. Returns the new path.
pub fn export_into(held: &Path, dir: &Path) -> Result<PathBuf> {
    let (stem, ext) = split_name(held);
    let (placeholder, path) = output::create_unique(dir, &stem, &ext)?;
    // Close the placeholder first: Windows can't replace an open file.
    drop(placeholder);
    match move_file(held, &path) {
        Ok(()) => Ok(path),
        Err(e) => {
            let _ = fs::remove_file(&path);
            Err(Error::io(path, e))
        }
    }
}

/// Move the held file `held` to exactly `dest`, replacing what's there. Use
/// this only after a Save dialog confirmed the name. Returns `dest`.
pub fn export_as(held: &Path, dest: &Path) -> Result<PathBuf> {
    move_file(held, dest).map_err(|e| Error::io(dest, e))?;
    Ok(dest.to_path_buf())
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

/// Rename `from` to `to`, replacing `to`. Across volumes, copy instead. On
/// any error `from` is left in place and no partial copy remains.
fn move_file(from: &Path, to: &Path) -> io::Result<()> {
    if !test_hooks::force_copy() {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) if crosses_devices(&e) => {}
            Err(e) => return Err(e),
        }
    }
    copy_then_remove(from, to)
}

fn crosses_devices(e: &io::Error) -> bool {
    e.raw_os_error() == Some(CROSS_DEVICE)
}

/// Copy to a temp name beside `to`, flush it, rename it into place, then
/// remove the source. The source goes last, so any failure keeps it.
fn copy_then_remove(from: &Path, to: &Path) -> io::Result<()> {
    let parent = to.parent().unwrap_or_else(|| Path::new("."));
    let (mut tmp_file, tmp) = output::create_unique(parent, ".kiln-export", "tmp")
        .map_err(|e| io::Error::other(e.to_string()))?;

    let copied = (|| {
        let mut src = File::open(from)?;
        io::copy(&mut src, &mut tmp_file)?;
        tmp_file.sync_all()?;
        test_hooks::maybe_fail_copy()?;
        // Close before renaming: Windows can't move an open file.
        drop(tmp_file);
        fs::rename(&tmp, to)
    })();
    match copied {
        Ok(()) => {
            // The copy is complete and in place. A leftover source is
            // harmless to the user, so don't fail the export over it.
            let _ = fs::remove_file(from);
            Ok(())
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Switches that let tests exercise the cross-volume path on one volume.
/// Per thread, so tests running side by side don't affect each other.
#[cfg(test)]
mod test_hooks {
    use std::cell::Cell;
    use std::io;

    thread_local! {
        pub static FORCE_COPY: Cell<bool> = const { Cell::new(false) };
        pub static FAIL_COPY: Cell<bool> = const { Cell::new(false) };
    }

    pub fn force_copy() -> bool {
        FORCE_COPY.with(Cell::get)
    }

    pub fn maybe_fail_copy() -> io::Result<()> {
        if FAIL_COPY.with(Cell::get) {
            Err(io::Error::other("simulated copy failure"))
        } else {
            Ok(())
        }
    }
}

#[cfg(not(test))]
mod test_hooks {
    use std::io;

    pub fn force_copy() -> bool {
        false
    }

    pub fn maybe_fail_copy() -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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

    #[test]
    fn discard_removes_only_its_own_folder() {
        let root = tempfile::tempdir().unwrap();
        let a = Holding::open(root.path()).unwrap();
        let b = Holding::open(root.path()).unwrap();
        write(&a.dir().join("x.jpg"), "x");
        let (a_dir, b_dir) = (a.dir().to_path_buf(), b.dir().to_path_buf());
        a.discard().unwrap();
        assert!(!a_dir.exists());
        assert!(b_dir.join("lock").is_file());
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

        let stale = root.path().join("999-1");
        fs::create_dir(&stale).unwrap();
        write(&stale.join("lock"), "");
        write(&stale.join("old.jpg"), "old");

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

        assert_eq!(sweep_stale(&root), vec![stale]);
        assert!(outside.join("lock").is_file());
        assert!(outside.join("mine.txt").is_file());
    }

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
        if !made {
            return;
        }
        assert!(sweep_stale(&root).is_empty());
        assert!(outside.join("lock").is_file());
        assert!(outside.join("mine.txt").is_file());
    }

    #[test]
    fn sweep_of_a_missing_root_is_empty() {
        let base = tempfile::tempdir().unwrap();
        assert!(sweep_stale(&base.path().join("nope")).is_empty());
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
    fn export_as_replaces() {
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        write(&src.path().join("a.jpg"), "new");
        let dest = out.path().join("chosen.jpg");
        write(&dest, "old");

        let got = export_as(&src.path().join("a.jpg"), &dest).unwrap();
        assert_eq!(got, dest);
        assert_eq!(fs::read_to_string(&dest).unwrap(), "new");
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

        let dest = out.path().join("b.jpg");
        write(&src.path().join("b.jpg"), "bee");
        write(&dest, "old");
        export_as(&src.path().join("b.jpg"), &dest).unwrap();
        assert_eq!(fs::read_to_string(&dest).unwrap(), "bee");
        assert!(!src.path().join("b.jpg").exists());
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

        let dest = out.path().join("keep.jpg");
        write(&dest, "old");
        assert!(export_as(&src.path().join("a.jpg"), &dest).is_err());
        assert_eq!(
            fs::read_to_string(src.path().join("a.jpg")).unwrap(),
            "data"
        );
        // The file being replaced is untouched too.
        assert_eq!(fs::read_to_string(&dest).unwrap(), "old");
        assert_eq!(names(out.path()), ["keep.jpg"]);
    }
}
