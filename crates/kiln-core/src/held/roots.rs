//! Where results wait: a holding root per drive, per user.
//!
//! A result waits on its source's volume, so saving it next to the
//! original is a rename. On the system drive (and the drive
//! `%LOCALAPPDATA%` is on) that is `%LOCALAPPDATA%\Kiln\held`, as before.
//! On any other local fixed or removable drive it is the user's own folder
//! in a hidden `.kiln-held` folder at the volume's root:
//! `<volume>\.kiln-held\<user SID>`, made with a DACL that lets in only
//! that user, SYSTEM and Administrators. A network path, another kind of
//! drive, or a root Kiln can't make or write to falls back to
//! `%LOCALAPPDATA%\Kiln\held`.
//!
//! A run keeps one phase 1 run folder ([`Holding`]) per root it uses, each
//! opened when first needed ([`HeldRuns`]). Every drive root Kiln makes is
//! noted in `%LOCALAPPDATA%\Kiln\held-roots.txt` before it is made, so the
//! next start looks there even for a drive that was unplugged since
//! ([`recovery_roots`]), and only a root noted as Kiln's is ever used again
//! or removed.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::{stale_runs, sweep_stale, Holding, StaleRun};
use crate::error::{Error, Result};
use crate::output;

/// The name of the holding folder at a drive's root. Each user's root is
/// a folder in it named by the user's SID.
pub const HELD_DIR_NAME: &str = ".kiln-held";

/// The name of the record of drive roots, in `%LOCALAPPDATA%\Kiln`.
pub const ROOTS_NAME: &str = "held-roots.txt";

/// Taken while the record is changed, so two Kilns noting roots at once
/// both keep theirs.
const ROOTS_LOCK_NAME: &str = "held-roots.lock";

/// The most of the record that is read: far more than any PC's drives.
const ROOTS_LIMIT: u64 = 1024 * 1024;

/// A record line's mark: Kiln made the user's folder.
const MADE: &str = "kiln";
/// A record line's mark: Kiln made the user's folder and `.kiln-held`.
const MADE_BOTH: &str = "kiln+held";

/// What kind of drive a volume is, as `GetDriveTypeW` tells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveKind {
    Fixed,
    Removable,
    Network,
    /// A CD or DVD, a RAM disk, or a kind Windows can't tell.
    Other,
}

/// What [`prepare_held_dir`] made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Prepared {
    /// The user's folder, `<volume>\.kiln-held\<SID>`.
    pub made: bool,
    /// `.kiln-held` itself.
    pub made_parent: bool,
}

/// What the choice of a holding root needs to know of the PC's volumes.
/// [`SystemVolumes`] asks Windows; tests use fakes.
pub trait Volumes {
    /// The root of the volume `path` is on: `E:\`, a mounted folder such
    /// as `C:\mnt\disk\`, or `\\server\share\`. `None` when it can't be
    /// told.
    fn volume_root(&self, path: &Path) -> Option<PathBuf>;
    /// What kind of drive the volume at `root` is.
    fn kind(&self, root: &Path) -> DriveKind;
    /// The root of the volume Windows is on (`%SystemDrive%`).
    fn system_root(&self) -> Option<PathBuf>;
    /// The roots of the local fixed and removable drives mounted now.
    fn local_drive_roots(&self) -> Vec<PathBuf>;
    /// The SID of the user Kiln runs as, in its string form.
    fn user_sid(&self) -> Option<String>;
    /// Make `root`, a user's folder in a volume's `.kiln-held`, ready for
    /// run folders: see [`prepare_held_dir`]. An error means Kiln can't
    /// hold results there, and nothing it made is left.
    fn prepare(&self, root: &Path) -> io::Result<Prepared>;
    /// Which holding folder `source`'s results go to, from its path alone:
    /// no disk or network access. [`HoldKey::of`].
    fn key_of(&self, source: &Path) -> HoldKey {
        HoldKey::of(source)
    }
    /// The root of the drive with this letter: `X:\`.
    fn drive_root(&self, letter: char) -> PathBuf {
        PathBuf::from(format!("{letter}:\\"))
    }
}

/// The PC's own volumes.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemVolumes;

#[cfg(windows)]
impl Volumes for SystemVolumes {
    fn volume_root(&self, path: &Path) -> Option<PathBuf> {
        super::win::volume_root(path)
    }

    fn kind(&self, root: &Path) -> DriveKind {
        super::win::drive_kind(root)
    }

    fn system_root(&self) -> Option<PathBuf> {
        let drive = std::env::var("SystemDrive")
            .ok()
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| "C:".to_owned());
        super::win::volume_root(&PathBuf::from(format!("{drive}\\")))
    }

    fn local_drive_roots(&self) -> Vec<PathBuf> {
        super::win::logical_drives()
            .into_iter()
            .filter(|r| matches!(self.kind(r), DriveKind::Fixed | DriveKind::Removable))
            .collect()
    }

    fn user_sid(&self) -> Option<String> {
        super::win::user_sid()
    }

    fn prepare(&self, root: &Path) -> io::Result<Prepared> {
        prepare_held_dir(root)
    }
}

/// Elsewhere every result is held in the local folder.
#[cfg(not(windows))]
impl Volumes for SystemVolumes {
    fn volume_root(&self, _path: &Path) -> Option<PathBuf> {
        None
    }

    fn kind(&self, _root: &Path) -> DriveKind {
        DriveKind::Other
    }

    fn system_root(&self) -> Option<PathBuf> {
        None
    }

    fn local_drive_roots(&self) -> Vec<PathBuf> {
        Vec::new()
    }

    fn user_sid(&self) -> Option<String> {
        None
    }

    fn prepare(&self, root: &Path) -> io::Result<Prepared> {
        prepare_held_dir(root)
    }
}

/// A SID's string form: `S-1-` then digits and dashes. Safe as a folder
/// name.
pub(super) fn is_sid(s: &str) -> bool {
    s.len() > 4 && s.starts_with("S-1-") && s[4..].chars().all(|c| c.is_ascii_digit() || c == '-')
}

/// `<volume>\.kiln-held\<sid>`.
fn user_root(volume: &Path, sid: &str) -> PathBuf {
    volume.join(HELD_DIR_NAME).join(sid)
}

/// A user's root in shape: a SID-named folder in a `.kiln-held` folder.
/// Its SID, if so.
fn sid_of(root: &Path) -> Option<&str> {
    let sid = root.file_name()?.to_str().filter(|s| is_sid(s))?;
    let parent = root.parent()?.file_name()?;
    parent.eq_ignore_ascii_case(HELD_DIR_NAME).then_some(sid)
}

/// An error unless `path` is a plain folder: not a file, a link or a
/// junction, which aren't Kiln's.
fn plain_dir(path: &Path) -> io::Result<()> {
    // Doesn't follow links.
    if fs::symlink_metadata(path)?.is_dir() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "isn't a plain folder",
        ))
    }
}

/// Make the user's folder in a volume's holding folder,
/// `<volume>\.kiln-held\<SID>`, ready for run folders.
///
/// - `.kiln-held` is made if it isn't there (never `create_dir_all`: a
///   volume root that isn't there fails), checked to be a plain folder
///   (not a link or a junction), and marked hidden.
/// - The SID folder is made with a protected DACL: the user, SYSTEM and
///   Administrators, nothing inherited. On a drive with no permissions
///   (FAT, exFAT) it is a plain folder, and its name keeps other users'
///   Kilns out of it. One that is there already is checked to be a plain
///   folder.
/// - It is checked to take new files.
///
/// Says what it made. If anything fails, what it made is removed again,
/// each with a non-recursive remove.
pub fn prepare_held_dir(root: &Path) -> io::Result<Prepared> {
    let refused = || io::Error::new(io::ErrorKind::InvalidInput, "isn't a user's holding folder");
    let sid = sid_of(root).ok_or_else(refused)?;
    let parent = root.parent().ok_or_else(refused)?;
    #[cfg(windows)]
    let _quiet = super::win::QuietDrives::new();
    let mut made = Prepared::default();
    let ready = (|| {
        made.made_parent = match fs::create_dir(parent) {
            Ok(()) => true,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => false,
            Err(e) => return Err(e),
        };
        plain_dir(parent)?;
        #[cfg(windows)]
        super::win::set_hidden(parent)?;
        made.made = match create_private_dir(root, sid) {
            Ok(()) => true,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => false,
            Err(e) => return Err(e),
        };
        plain_dir(root)?;
        output::check_writable(root).map_err(|e| io::Error::other(e.to_string()))
    })();
    match ready {
        Ok(()) => Ok(made),
        Err(e) => {
            undo(root, made);
            Err(e)
        }
    }
}

/// Remove what [`prepare_held_dir`] made at `root`, if it is empty.
fn undo(root: &Path, made: Prepared) {
    if made.made {
        let _ = fs::remove_dir(root);
    }
    if made.made_parent {
        if let Some(parent) = root.parent() {
            let _ = fs::remove_dir(parent);
        }
    }
}

#[cfg(windows)]
fn create_private_dir(dir: &Path, sid: &str) -> io::Result<()> {
    super::win::create_private_dir(dir, sid)
}

#[cfg(not(windows))]
fn create_private_dir(dir: &Path, _sid: &str) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new().mode(0o700).create(dir)
}

/// Where Kiln keeps its own holding folder and its record of drive roots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Places {
    local_held: PathBuf,
    record: PathBuf,
}

impl Places {
    /// The places in `kiln_local`, Kiln's folder in `%LOCALAPPDATA%`:
    /// `held` and [`ROOTS_NAME`] in it.
    pub fn new(kiln_local: &Path) -> Places {
        Places {
            local_held: kiln_local.join("held"),
            record: kiln_local.join(ROOTS_NAME),
        }
    }

    /// The places in `%LOCALAPPDATA%\Kiln`, if LOCALAPPDATA is set.
    pub fn from_env() -> Option<Places> {
        let local = std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty())?;
        Some(Places::new(&PathBuf::from(local).join("Kiln")))
    }

    /// `%LOCALAPPDATA%\Kiln\held`: results from the system drive, network
    /// paths and drives Kiln can't hold on wait here.
    pub fn local_held(&self) -> &Path {
        &self.local_held
    }

    /// `%LOCALAPPDATA%\Kiln\held-roots.txt`.
    pub fn record(&self) -> &Path {
        &self.record
    }
}

/// The holding root for results made from `source`: the user's folder
/// `<volume root>\.kiln-held\<SID>` for a local fixed or removable drive
/// other than the system drive (and other than the one `%LOCALAPPDATA%` is
/// on); else [`Places::local_held`]. Only a choice: nothing is made or
/// checked on disk (opening it is [`HeldRuns::folder_for`]'s), and
/// everything it knows of the PC comes through `volumes`.
pub fn held_root_for(source: &Path, places: &Places, volumes: &dyn Volumes) -> PathBuf {
    let local = || places.local_held.clone();
    if is_unc(source) {
        return local();
    }
    let Some(volume) = volumes.volume_root(source) else {
        return local();
    };
    if is_unc(&volume) {
        return local();
    }
    let on = |other: Option<PathBuf>| other.is_some_and(|o| same_path(&o, &volume));
    if on(volumes.system_root()) || on(volumes.volume_root(&places.local_held)) {
        return local();
    }
    match (volumes.kind(&volume), volumes.user_sid()) {
        (DriveKind::Fixed | DriveKind::Removable, Some(sid)) if is_sid(&sid) => {
            user_root(&volume, &sid)
        }
        _ => local(),
    }
}

/// A `\\server\share` path, written plainly or as `\\?\UNC\`. `\\?\C:\`
/// and `\\.\` device paths aren't.
fn is_unc(path: &Path) -> bool {
    let s = path.as_os_str().to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        return rest.len() >= 4 && rest[..4].eq_ignore_ascii_case(r"UNC\");
    }
    s.starts_with(r"\\") && !s.starts_with(r"\\.\")
}

/// The same path, as Windows compares them: ignoring case and a last
/// backslash.
pub(super) fn same_path(a: &Path, b: &Path) -> bool {
    key(a) == key(b)
}

fn key(p: &Path) -> String {
    let s = p.as_os_str().to_string_lossy().replace('/', "\\");
    s.trim_end_matches('\\').to_lowercase()
}

/// Whether `path` is `dir` or inside it, as Windows compares paths.
fn inside(path: &Path, dir: &Path) -> bool {
    let (p, d) = (key(path), key(dir));
    p == d
        || p.strip_prefix(&d)
            .is_some_and(|rest| rest.starts_with('\\'))
}

/// Whether `path` is inside a place Kiln holds results in, which is no
/// place to save to: [`Places::local_held`], or any folder named
/// [`HELD_DIR_NAME`].
pub fn in_holding_place(path: &Path, places: &Places) -> bool {
    inside(path, &places.local_held)
        || path
            .components()
            .any(|c| c.as_os_str().eq_ignore_ascii_case(HELD_DIR_NAME))
}

/// A root in the record, and what Kiln noted it made there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldNote {
    pub root: PathBuf,
    /// Kiln made the root (the user's folder). Only such a root is used
    /// again, or removed.
    pub made: bool,
    /// Kiln made the `.kiln-held` folder it is in, too.
    pub made_parent: bool,
}

/// A record line read back, or `None` if it says nothing: not UTF-8, empty
/// or not a whole path. A line with no mark, or a mark Kiln doesn't know,
/// lists a root Kiln didn't make.
fn parse_note(line: &[u8]) -> Option<HeldNote> {
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let line = std::str::from_utf8(line).ok()?;
    let (path, mark) = line.split_once('\t').unwrap_or((line, ""));
    let root = PathBuf::from(path);
    if path.is_empty() || !root.is_absolute() {
        return None;
    }
    Some(HeldNote {
        root,
        made: mark == MADE || mark == MADE_BOTH,
        made_parent: mark == MADE_BOTH,
    })
}

/// The roots in `record`, each once, in the order noted, with what Kiln
/// made there. Tolerant: a missing or unreadable record is empty, and a
/// line that isn't UTF-8 or isn't a whole path is left out. Nothing on
/// disk is looked at, so a root on a drive that isn't mounted is listed
/// as written.
pub fn read_held_notes(record: &Path) -> Vec<HeldNote> {
    let mut bytes = vec![];
    if File::open(record)
        .and_then(|f| f.take(ROOTS_LIMIT).read_to_end(&mut bytes))
        .is_err()
    {
        return Vec::new();
    }
    let mut notes: Vec<HeldNote> = Vec::new();
    for note in bytes.split(|&b| b == b'\n').filter_map(parse_note) {
        if !notes.iter().any(|n| same_path(&n.root, &note.root)) {
            notes.push(note);
        }
    }
    notes
}

/// The roots in `record`: [`read_held_notes`] without the marks.
pub fn read_held_roots(record: &Path) -> Vec<PathBuf> {
    read_held_notes(record)
        .into_iter()
        .map(|n| n.root)
        .collect()
}

/// Note in `record` that Kiln makes `root` (and its `.kiln-held` too, if
/// `made_parent`). Done before anything is made there. A line already
/// naming `root` is replaced; true if the record changed. See
/// [`edit_record`] for how it is written. A root a line couldn't hold
/// (not Unicode, with a tab or a line break, or not a whole path) is
/// refused.
pub fn note_held_root(record: &Path, root: &Path, made_parent: bool) -> Result<bool> {
    let refused = || {
        let why = io::Error::new(io::ErrorKind::InvalidInput, "can't be noted as a root");
        Error::io(root, why)
    };
    let path = root.to_str().ok_or_else(refused)?;
    if path.is_empty() || path.contains(['\t', '\n', '\r']) || !root.is_absolute() {
        return Err(refused());
    }
    let mark = if made_parent { MADE_BOTH } else { MADE };
    edit_record(record, root, Some(format!("{path}\t{mark}")))
}

/// Take `root` out of `record`: Kiln removed it, or made nothing there
/// after all. True if the record changed.
pub fn forget_held_root(record: &Path, root: &Path) -> Result<bool> {
    edit_record(record, root, None)
}

/// Put `line` in `record` in place of the line naming `root` (at the end
/// if none does), or with `None` take that line out. Under the record's
/// lock: re-read, then rewritten whole through a temp file and a rename,
/// so it is never seen half written. Every other line is kept byte for
/// byte, those Kiln can't read included; a record that can't be read
/// (other than not being there) is left alone and the edit fails.
fn edit_record(record: &Path, root: &Path, line: Option<String>) -> Result<bool> {
    let dir = record.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    let _lock = lock_record(&dir.join(ROOTS_LOCK_NAME))?;
    let bytes = match fs::read(record) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => vec![],
        // Never rewritten from less than it holds.
        Err(e) => return Err(Error::io(record, e)),
    };
    let mut lines: Vec<&[u8]> = bytes.split(|&b| b == b'\n').collect();
    if lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    let names_root = |l: &[u8]| parse_note(l).is_some_and(|n| same_path(&n.root, root));
    let mut out: Vec<&[u8]> = Vec::with_capacity(lines.len() + 1);
    let mut placed = false;
    for l in &lines {
        if names_root(l) {
            if let (false, Some(new)) = (placed, &line) {
                // The same marks, however the path is written: kept as it is.
                let same = parse_note(l)
                    .zip(parse_note(new.as_bytes()))
                    .is_some_and(|(a, b)| (a.made, a.made_parent) == (b.made, b.made_parent));
                out.push(if same { l } else { new.as_bytes() });
            }
            placed = true;
        } else {
            out.push(l);
        }
    }
    if let (false, Some(new)) = (placed, &line) {
        out.push(new.as_bytes());
    }
    if out == lines {
        return Ok(false);
    }
    let mut text = out.join(&b'\n');
    if !text.is_empty() {
        text.push(b'\n');
    }
    replace_whole(record, &text).map_err(|e| Error::io(record, e))?;
    Ok(true)
}

/// Write `bytes` to a temp file beside `path`, flush it, and rename it
/// over `path`. The temp file goes if anything fails.
fn replace_whole(path: &Path, bytes: &[u8]) -> io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = path.with_file_name(format!("{name}.{}-{n}.tmp", std::process::id()));
    let written = (|| {
        let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// Hold `lock` open with no sharing, waiting a little for another Kiln
/// that has it.
fn lock_record(lock: &Path) -> Result<File> {
    let mut last = None;
    for _ in 0..200 {
        let mut opts = OpenOptions::new();
        opts.read(true).write(true).create(true).truncate(false);
        #[cfg(windows)]
        std::os::windows::fs::OpenOptionsExt::share_mode(&mut opts, 0);
        match opts.open(lock) {
            Ok(f) => return Ok(f),
            // ERROR_SHARING_VIOLATION: another Kiln has it.
            Err(e) if e.raw_os_error() == Some(32) => {
                last = Some(e);
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => return Err(Error::io(lock, e)),
        }
    }
    Err(Error::io(
        lock,
        last.unwrap_or_else(|| io::Error::other("busy")),
    ))
}

/// Every holding root to look in for what this user's earlier runs left:
/// the local folder; each root in the record that is this user's
/// (`...\.kiln-held\<SID>`); and `<drive>\.kiln-held\<SID>` on every local
/// fixed or removable drive mounted now. Each once, the local folder first.
/// Another user's folder is never among them. A recorded root on a drive
/// that isn't mounted is listed, finds nothing, and stays in the record
/// for next time.
pub fn recovery_roots(places: &Places, volumes: &dyn Volumes) -> Vec<PathBuf> {
    let mut roots = vec![places.local_held.clone()];
    let Some(sid) = volumes.user_sid().filter(|s| is_sid(s)) else {
        return roots;
    };
    let recorded = read_held_roots(&places.record)
        .into_iter()
        .filter(|r| sid_of(r).is_some_and(|s| s.eq_ignore_ascii_case(&sid)));
    let mounted = volumes
        .local_drive_roots()
        .into_iter()
        .map(|v| user_root(&v, &sid));
    for root in recorded.chain(mounted) {
        if !roots.iter().any(|r| same_path(r, &root)) {
            roots.push(root);
        }
    }
    roots
}

/// Whether a holding root may be looked in: the local folder always (as
/// in phase 1); a drive's user folder only if it and its `.kiln-held` are
/// plain folders, not links or junctions, and it is the user's own
/// ([`is_users_own`]).
fn may_look_in(root: &Path, places: &Places) -> bool {
    if same_path(root, &places.local_held) {
        return true;
    }
    sid_of(root).is_some()
        && plain_dir(root).is_ok()
        && root.parent().is_some_and(|p| plain_dir(p).is_ok())
        && is_users_own(root)
}

/// Whether a drive's user folder `...\.kiln-held\<SID>` is that user's own:
/// owned by the SID it is named for, with a protected DACL, as
/// [`prepare_held_dir`] makes it. Another account can make a folder of
/// that name while it is absent (`.kiln-held` lets anyone make folders),
/// but can't make the user its owner, so a planted one is never looked in
/// or used. On FAT and exFAT, which keep no owners or ACLs, there is
/// nothing to check: the name alone keeps other users' Kilns out, and a
/// folder planted there is accepted as the user's (by design: such a drive
/// gives no way to tell).
fn is_users_own(root: &Path) -> bool {
    let Some(sid) = sid_of(root) else {
        return false;
    };
    #[cfg(windows)]
    return super::win::is_users_own(root, sid);
    #[cfg(not(windows))]
    {
        let _ = sid;
        plain_dir(root).is_ok()
    }
}

/// [`stale_runs`] in each of `roots` Kiln may look in (see
/// [`recovery_roots`]; a link or junction is skipped), together: what to
/// offer in one prompt. Each run comes back locked, so no other Kiln lists
/// or touches it, and a live run is never listed. A root that isn't there
/// gives nothing.
pub fn stale_runs_in(roots: &[PathBuf], places: &Places) -> Vec<StaleRun> {
    #[cfg(windows)]
    let _quiet = super::win::QuietDrives::new();
    roots
        .iter()
        .filter(|r| may_look_in(r, places))
        .flat_map(|r| stale_runs(r))
        .collect()
}

/// [`sweep_stale`] in each of `roots` Kiln may look in; then each drive
/// root left empty that Kiln made goes too ([`remove_empty_held_root`]).
/// Returns the run folders removed.
pub fn sweep_stale_in(roots: &[PathBuf], places: &Places) -> Vec<PathBuf> {
    #[cfg(windows)]
    let _quiet = super::win::QuietDrives::new();
    let mut removed = Vec::new();
    for root in roots.iter().filter(|r| may_look_in(r, places)) {
        removed.extend(sweep_stale(root));
        remove_empty_held_root(root, places);
    }
    removed
}

/// Let go of a run from last time, as [`StaleRun::close`] does, and if its
/// folder went and left its drive root empty, remove that root too. True
/// if the run's folder was removed.
pub fn close_run(run: StaleRun, places: &Places) -> bool {
    let root = run.dir.parent().map(Path::to_path_buf);
    let closed = run.close();
    if let (true, Some(root)) = (closed, root) {
        remove_empty_held_root(&root, places);
    }
    closed
}

/// Remove a drive's user root, `...\.kiln-held\<SID>`, if Kiln made it
/// (the record says so) and it is empty; then its `.kiln-held`, if Kiln
/// made that too and it is empty now. Each is checked to be a plain
/// folder (not a link) and removed with a non-recursive remove, which fails
/// if anything is in it, another user's folder included. Never
/// [`Places::local_held`]. The root's line leaves the record once the root
/// has gone. True if the root was removed.
pub fn remove_empty_held_root(root: &Path, places: &Places) -> bool {
    if sid_of(root).is_none() || same_path(root, &places.local_held) {
        return false;
    }
    let Some(note) = read_held_notes(&places.record)
        .into_iter()
        .find(|n| same_path(&n.root, root) && n.made)
    else {
        return false;
    };
    let Some(parent) = root.parent() else {
        return false;
    };
    if plain_dir(root).is_err() || plain_dir(parent).is_err() || fs::remove_dir(root).is_err() {
        return false;
    }
    // Gone: a folder of that name made later isn't Kiln's.
    let _ = forget_held_root(&places.record, root);
    if note.made_parent && plain_dir(parent).is_ok() {
        let _ = fs::remove_dir(parent);
    }
    true
}

/// Which holding folder a source's results go to, told from its path
/// alone, with no disk or network access: a drive letter, or the local
/// folder for a network (UNC) path or anything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HoldKey {
    Local,
    /// The drive letter, upper case.
    Drive(char),
}

impl HoldKey {
    /// The key for `source`. A volume mounted in a folder (`C:\mnt\disk`)
    /// counts as its drive letter's: its results are held where that
    /// letter's are, and saving beside the original is then a copy.
    pub fn of(source: &Path) -> HoldKey {
        if is_unc(source) {
            return HoldKey::Local;
        }
        let s = source.as_os_str().to_string_lossy();
        let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
        let b = s.as_bytes();
        match (b.first(), b.get(1)) {
            (Some(c), Some(b':')) if c.is_ascii_alphabetic() => {
                HoldKey::Drive(char::from(c.to_ascii_uppercase()))
            }
            _ => HoldKey::Local,
        }
    }
}

/// One run's holding folders: a phase 1 run folder ([`Holding`]) for each
/// holding root it uses, each with its own lock and `sources.tsv`, opened
/// the first time a job needs that root.
pub struct HeldRuns {
    places: Places,
    volumes: Box<dyn Volumes + Send>,
    /// The folder each key's results go to, by index in `folders`.
    by_key: HashMap<HoldKey, usize>,
    /// Open run folders, with their roots, in the order opened.
    folders: Vec<(PathBuf, Holding)>,
}

impl std::fmt::Debug for HeldRuns {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeldRuns")
            .field("places", &self.places)
            .field("folders", &self.folders)
            .finish_non_exhaustive()
    }
}

impl HeldRuns {
    /// No folder is made until a job needs one.
    pub fn new(places: Places, volumes: Box<dyn Volumes + Send>) -> HeldRuns {
        HeldRuns {
            places,
            volumes,
            by_key: HashMap::new(),
            folders: Vec::new(),
        }
    }

    /// On this PC's own volumes.
    pub fn on_this_pc(places: Places) -> HeldRuns {
        HeldRuns::new(places, Box::new(SystemVolumes))
    }

    pub fn places(&self) -> &Places {
        &self.places
    }

    /// The run folder already open for results made from `source`, if
    /// there is one. No disk or network access: safe on the UI thread.
    pub fn cached(&self, source: &Path) -> Option<&Holding> {
        let i = *self.by_key.get(&self.volumes.key_of(source))?;
        Some(&self.folders[i].1)
    }

    /// The run folder for results made from `source` (a job's first
    /// source): [`HeldRuns::cached`], or else the first time its
    /// [`HoldKey`] is used, one opened now. **Does disk work, and may wait
    /// on a slow drive: never call it on the UI thread** (only `cached` is
    /// for that).
    ///
    /// A network path goes to the local root without looking at it. A
    /// drive's user root ([`held_root_for`]) is used if it is there and the
    /// record says Kiln made it, or if it isn't there: then it is noted in
    /// the record first, and made ([`Volumes::prepare`]). A root that is
    /// there and isn't noted as Kiln's (made by someone else) is never
    /// used. If the drive's root can't be used, the local root is, and from
    /// then on for that drive. An error only when the local root fails
    /// too.
    pub fn folder_for(&mut self, source: &Path) -> Result<&Holding> {
        let key = self.volumes.key_of(source);
        if let Some(&i) = self.by_key.get(&key) {
            return Ok(&self.folders[i].1);
        }
        let i = match key {
            HoldKey::Local => self.open_local()?,
            HoldKey::Drive(letter) => {
                let drive = self.volumes.drive_root(letter);
                let root = held_root_for(&drive, &self.places, self.volumes.as_ref());
                if same_path(&root, &self.places.local_held) {
                    self.open_local()?
                } else {
                    match self.open_drive(&root) {
                        Ok(i) => i,
                        Err(_) => self.open_local()?,
                    }
                }
            }
        };
        self.by_key.insert(key, i);
        Ok(&self.folders[i].1)
    }

    fn position(&self, root: &Path) -> Option<usize> {
        self.folders.iter().position(|(r, _)| same_path(r, root))
    }

    fn open_local(&mut self) -> Result<usize> {
        let root = self.places.local_held.clone();
        if let Some(i) = self.position(&root) {
            return Ok(i);
        }
        let h = Holding::open(&root)?;
        self.folders.push((root, h));
        Ok(self.folders.len() - 1)
    }

    fn open_drive(&mut self, root: &Path) -> Result<usize> {
        if let Some(i) = self.position(root) {
            return Ok(i);
        }
        let record = self.places.record.clone();
        let parent = root.parent().unwrap_or(root).to_path_buf();
        let not_kilns = || {
            let why = io::Error::new(io::ErrorKind::AlreadyExists, "isn't a folder Kiln made");
            Error::io(root, why)
        };
        let note = read_held_notes(&record)
            .into_iter()
            .find(|n| same_path(&n.root, root) && n.made);
        if fs::symlink_metadata(root).is_ok() {
            // There already: used only if Kiln noted it made it, and it is
            // the user's own (owner and protected DACL), which another
            // account can't fake.
            if note.is_none() || !is_users_own(root) {
                return Err(not_kilns());
            }
            self.volumes.prepare(root).map_err(|e| Error::io(root, e))?;
        } else {
            // Noted before anything is made, so whatever Kiln makes is in
            // the record even if Kiln ends right after. A line Kiln has
            // already (another Kiln of this user's, or a run that ended
            // before making it) is kept as it is.
            let parent_there = fs::symlink_metadata(&parent).is_ok();
            let added = match &note {
                Some(_) => false,
                None => note_held_root(&record, root, !parent_there)?,
            };
            let noted_parent = note.as_ref().map_or(!parent_there, |n| n.made_parent);
            // Only a line this call added is taken out, and only while
            // nothing is there: another Kiln of this user's may have made
            // the folder and need its line.
            let take_back = || {
                if added && fs::symlink_metadata(root).is_err() {
                    let _ = forget_held_root(&record, root);
                }
            };
            let made = match self.volumes.prepare(root) {
                Ok(made) => made,
                Err(e) => {
                    take_back();
                    return Err(Error::io(root, e));
                }
            };
            if !made.made {
                // Made meanwhile, by another Kiln of this user's or by
                // someone else: used only if it is the user's own. Its
                // line stays, since the folder is there, and a folder that
                // isn't the user's own is never taken whatever the record
                // says.
                if !is_users_own(root) {
                    return Err(not_kilns());
                }
            } else if made.made_parent != noted_parent {
                // `.kiln-held` came or went meanwhile: note who made it.
                if let Err(e) = note_held_root(&record, root, made.made_parent) {
                    undo(root, made);
                    take_back();
                    return Err(e);
                }
            }
        }
        // `open_in`, not `open`: a root that went meanwhile isn't made
        // again without its permissions.
        let h = Holding::open_in(root)?;
        #[cfg(windows)]
        let _ = super::win::set_hidden(&parent);
        self.folders.push((root.to_path_buf(), h));
        Ok(self.folders.len() - 1)
    }

    /// The run folders open, in the order opened.
    pub fn folders(&self) -> impl Iterator<Item = &Holding> {
        self.folders.iter().map(|(_, h)| h)
    }

    /// No folder has been opened yet.
    pub fn is_empty(&self) -> bool {
        self.folders.is_empty()
    }

    /// The run folder `file` is in, if it is directly in one of them.
    pub fn holding_of(&self, file: &Path) -> Option<&Holding> {
        let parent = file.parent()?;
        self.folders().find(|h| same_path(h.dir(), parent))
    }

    /// Note in its run folder's `sources.tsv` that `result` was made from
    /// a file in `origin_dir`: [`Holding::record`]. Refused if `result`
    /// isn't in one of this run's folders.
    pub fn record(&self, result: &Path, origin_dir: &Path) -> Result<()> {
        let h = self.holding_of(result).ok_or_else(|| not_held(result))?;
        h.record(
            &h.dir().join(result.file_name().unwrap_or_default()),
            origin_dir,
        )
    }

    /// Note that `result` has left its run folder: [`Holding::forget`].
    pub fn forget(&self, result: &Path) -> Result<()> {
        let h = self.holding_of(result).ok_or_else(|| not_held(result))?;
        h.forget(&h.dir().join(result.file_name().unwrap_or_default()))
    }

    /// Whether `path` is no place to save to: in a place Kiln holds
    /// results ([`in_holding_place`]), or in one of this run's folders.
    pub fn holds(&self, path: &Path) -> bool {
        in_holding_place(path, &self.places) || self.folders().any(|h| inside(path, h.dir()))
    }

    /// The end of the run: remove each run folder that holds no result
    /// ([`Holding::close_if_empty`]), then each drive root left empty that
    /// Kiln made. A folder with a result in it is kept whole, its lock let
    /// go, for the next start to deliver. Returns the folders kept.
    pub fn close_empty(self) -> Vec<PathBuf> {
        let places = self.places;
        let mut kept = vec![];
        for (root, h) in self.folders {
            let dir = h.dir().to_path_buf();
            if h.close_if_empty() {
                remove_empty_held_root(&root, &places);
            } else {
                kept.push(dir);
            }
        }
        kept
    }

    /// Let go of every run folder as it is, for the next start to offer:
    /// their locks close. Returns the folders kept.
    pub fn keep(self) -> Vec<PathBuf> {
        self.folders
            .into_iter()
            .map(|(_, h)| h.dir().to_path_buf())
            .collect()
    }
}

fn not_held(result: &Path) -> Error {
    let why = io::Error::new(io::ErrorKind::InvalidInput, "isn't a result of this run");
    Error::io(result, why)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::held::{probe_lock, Recovered, LOCK_NAME, SOURCES_NAME};
    use std::sync::{Arc, Mutex};

    /// This user's SID: the real one where there is one, since the folders
    /// made with it in scratch roots must let this test in.
    fn sid() -> String {
        #[cfg(windows)]
        return super::super::win::user_sid().expect("this process's SID");
        #[cfg(not(windows))]
        return "S-1-5-21-1-2-3-1000".to_owned();
    }

    /// Another user's SID.
    const OTHER: &str = "S-1-5-21-9-9-9-4242";

    /// A volume of a fake PC.
    #[derive(Clone)]
    struct Vol {
        root: PathBuf,
        kind: DriveKind,
        writable: bool,
        mounted: bool,
        /// A scratch folder standing in for a drive, with its own letter:
        /// `prepare` makes folders there for real. Otherwise (`E:\` and the
        /// like, which may be real drives on this PC) nothing is made.
        letter: Option<char>,
    }

    /// A fake PC. Paths are matched to volumes by prefix. `prepare` notes
    /// each root it is asked for, and makes it only on a scratch volume.
    #[derive(Clone)]
    struct Fake {
        vols: Vec<Vol>,
        system: PathBuf,
        prepared: Arc<Mutex<Vec<PathBuf>>>,
        /// Another Kiln of this user's makes the root just before this one
        /// does: `prepare` on a scratch volume then finds it made.
        winner_first: bool,
    }

    impl Fake {
        fn new(system: &str) -> Fake {
            Fake {
                vols: vec![Vol {
                    root: PathBuf::from(system),
                    kind: DriveKind::Fixed,
                    writable: true,
                    // Its real `.kiln-held` is never looked at.
                    mounted: false,
                    letter: None,
                }],
                system: PathBuf::from(system),
                prepared: Arc::default(),
                winner_first: false,
            }
        }

        fn with(mut self, root: &str, kind: DriveKind) -> Fake {
            self.vols.push(Vol {
                root: PathBuf::from(root),
                kind,
                writable: true,
                mounted: true,
                letter: None,
            });
            self
        }

        fn with_scratch(mut self, root: &Path, kind: DriveKind) -> Fake {
            let letter = char::from(b'K' + self.vols.len() as u8);
            self.vols.push(Vol {
                root: root.to_path_buf(),
                kind,
                writable: true,
                mounted: true,
                letter: Some(letter),
            });
            self
        }

        fn unwritable(mut self, root: &Path) -> Fake {
            for v in &mut self.vols {
                if same_path(&v.root, root) {
                    v.writable = false;
                }
            }
            self
        }

        fn vol(&self, path: &Path) -> Option<&Vol> {
            self.vols
                .iter()
                .filter(|v| inside(path, &v.root))
                .max_by_key(|v| key(&v.root).len())
        }
    }

    impl Volumes for Fake {
        fn volume_root(&self, path: &Path) -> Option<PathBuf> {
            self.vol(path).map(|v| v.root.clone())
        }

        fn kind(&self, root: &Path) -> DriveKind {
            self.vol(root).map_or(DriveKind::Other, |v| v.kind)
        }

        fn system_root(&self) -> Option<PathBuf> {
            Some(self.system.clone())
        }

        fn local_drive_roots(&self) -> Vec<PathBuf> {
            self.vols
                .iter()
                .filter(|v| v.mounted && matches!(v.kind, DriveKind::Fixed | DriveKind::Removable))
                .map(|v| v.root.clone())
                .collect()
        }

        fn user_sid(&self) -> Option<String> {
            Some(sid())
        }

        fn prepare(&self, root: &Path) -> io::Result<Prepared> {
            let v = self.vol(root).expect("a volume");
            if !v.writable {
                return Err(io::Error::from(io::ErrorKind::PermissionDenied));
            }
            self.prepared.lock().unwrap().push(root.to_path_buf());
            match v.letter {
                Some(_) if self.winner_first => {
                    prepare_held_dir(root)?;
                    prepare_held_dir(root)
                }
                Some(_) => prepare_held_dir(root),
                None => Ok(Prepared {
                    made: true,
                    made_parent: true,
                }),
            }
        }

        fn key_of(&self, source: &Path) -> HoldKey {
            match self.vol(source).and_then(|v| v.letter) {
                Some(letter) => HoldKey::Drive(letter),
                None => HoldKey::of(source),
            }
        }

        fn drive_root(&self, letter: char) -> PathBuf {
            match self.vols.iter().find(|v| v.letter == Some(letter)) {
                Some(v) => v.root.clone(),
                None => PathBuf::from(format!("{letter}:\\")),
            }
        }
    }

    fn places() -> Places {
        Places::new(Path::new(r"C:\Users\me\AppData\Local\Kiln"))
    }

    #[test]
    fn each_kind_of_drive_gets_its_root() {
        let p = places();
        let pc = Fake::new(r"C:\")
            .with(r"E:\", DriveKind::Fixed)
            .with(r"F:\", DriveKind::Removable)
            .with(r"Z:\", DriveKind::Network)
            .with(r"\\server\share\", DriveKind::Network)
            .with(r"G:\", DriveKind::Other)
            .with(r"C:\mnt\disk\", DriveKind::Fixed);
        let local = p.local_held().to_path_buf();
        let root = |s: &str| held_root_for(Path::new(s), &p, &pc);
        let mine = |v: &str| Path::new(v).join(HELD_DIR_NAME).join(sid());

        assert_eq!(root(r"C:\Users\me\photo.jpg"), local);
        assert_eq!(root(r"E:\Photos\photo.jpg"), mine(r"E:\"));
        assert_eq!(root(r"F:\DCIM\photo.jpg"), mine(r"F:\"));
        // A folder another volume is mounted on is that volume's.
        assert_eq!(root(r"C:\mnt\disk\a\photo.jpg"), mine(r"C:\mnt\disk\"));
        assert_eq!(root(r"Z:\share\photo.jpg"), local);
        assert_eq!(root(r"\\server\share\photo.jpg"), local);
        assert_eq!(root(r"\\?\UNC\server\share\photo.jpg"), local);
        assert_eq!(root(r"G:\disc\photo.jpg"), local);
        // A path on no volume Kiln can tell.
        assert_eq!(root(r"Q:\photo.jpg"), local);
        // Choosing makes nothing.
        assert!(pc.prepared.lock().unwrap().is_empty());
    }

    #[test]
    fn the_drive_localappdata_is_on_holds_locally() {
        let p = Places::new(Path::new(r"D:\Profiles\me\AppData\Local\Kiln"));
        let pc = Fake::new(r"C:\").with(r"D:\", DriveKind::Fixed);
        assert_eq!(
            held_root_for(Path::new(r"D:\Photos\a.jpg"), &p, &pc),
            p.local_held()
        );
    }

    #[test]
    fn a_hold_key_is_told_from_the_path_alone() {
        assert_eq!(HoldKey::of(Path::new(r"e:\a.jpg")), HoldKey::Drive('E'));
        assert_eq!(HoldKey::of(Path::new(r"\\?\E:\a.jpg")), HoldKey::Drive('E'));
        assert_eq!(HoldKey::of(Path::new(r"\\server\s\a.jpg")), HoldKey::Local);
        assert_eq!(
            HoldKey::of(Path::new(r"\\?\UNC\server\s\a.jpg")),
            HoldKey::Local
        );
        assert_eq!(HoldKey::of(Path::new(r"\\.\pipe\x")), HoldKey::Local);
        assert_eq!(HoldKey::of(Path::new("relative.jpg")), HoldKey::Local);
    }

    #[test]
    fn a_drive_that_cannot_be_written_holds_locally() {
        let kiln = tempfile::tempdir().unwrap();
        let places = Places::new(kiln.path());
        // E: and F: here are fakes: nothing is made on the real ones.
        let pc = Fake::new(r"C:\")
            .with(r"E:\", DriveKind::Fixed)
            .with(r"F:\", DriveKind::Removable)
            .unwritable(Path::new(r"E:\"))
            .unwritable(Path::new(r"F:\"));
        let mut runs = HeldRuns::new(places.clone(), Box::new(pc));
        for s in [r"E:\a.jpg", r"F:\b.jpg"] {
            let dir = runs.folder_for(Path::new(s)).unwrap().dir();
            assert_eq!(dir.parent().unwrap(), places.local_held());
        }
        assert_eq!(runs.folders().count(), 1);
        // Noted before trying, and taken out again when it failed.
        assert!(read_held_roots(places.record()).is_empty());
        // A network path goes to the local folder without a look at it.
        let dir = runs.folder_for(Path::new(r"\\server\share\c.jpg")).unwrap();
        assert_eq!(dir.dir().parent().unwrap(), places.local_held());
    }

    #[cfg(windows)]
    #[test]
    fn a_user_root_is_made_private_and_hidden_and_takes_files() {
        let vol = tempfile::tempdir().unwrap();
        let root = user_root(vol.path(), &sid());
        let made = prepare_held_dir(&root).unwrap();
        assert_eq!(
            made,
            Prepared {
                made: true,
                made_parent: true
            }
        );
        let parent = vol.path().join(HELD_DIR_NAME);
        assert!(super::super::win::is_hidden(&parent));
        let sddl = super::super::win::dacl_sddl(&root).unwrap();
        assert!(sddl.starts_with("D:P"), "{sddl}");
        assert_eq!(sddl.matches("(A;").count(), 3, "{sddl}");
        // Its write check leaves nothing.
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        // Again: fine, nothing made, and what's in it is left.
        fs::write(root.join("keep.txt"), "k").unwrap();
        assert_eq!(prepare_held_dir(&root).unwrap(), Prepared::default());
        assert!(root.join("keep.txt").is_file());
    }

    #[test]
    fn what_is_not_a_plain_folder_is_refused_and_left() {
        let vol = tempfile::tempdir().unwrap();
        let parent = vol.path().join(HELD_DIR_NAME);
        // `.kiln-held` is a file.
        fs::write(&parent, "mine").unwrap();
        assert!(prepare_held_dir(&user_root(vol.path(), &sid())).is_err());
        assert_eq!(fs::read_to_string(&parent).unwrap(), "mine");
        fs::remove_file(&parent).unwrap();

        // The user's folder is a file: `.kiln-held`, made for it, goes again.
        let other = tempfile::tempdir().unwrap();
        let root = user_root(other.path(), &sid());
        fs::create_dir(root.parent().unwrap()).unwrap();
        fs::write(&root, "mine").unwrap();
        assert!(prepare_held_dir(&root).is_err());
        assert_eq!(fs::read_to_string(&root).unwrap(), "mine");

        // Not a user's folder in shape, and a volume that isn't there.
        assert!(prepare_held_dir(&vol.path().join(HELD_DIR_NAME)).is_err());
        assert!(prepare_held_dir(&vol.path().join("x").join(sid())).is_err());
        let gone = vol.path().join("gone");
        assert!(prepare_held_dir(&user_root(&gone, &sid())).is_err());
        assert!(!gone.exists());
        assert!(!parent.exists());
    }

    /// Make a junction at `link` to `target` with `mklink /J`, which needs
    /// no privilege.
    #[cfg(windows)]
    fn junction(link: &Path, target: &Path) {
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        assert!(made, "couldn't make a junction with `cmd /C mklink /J`");
    }

    #[cfg(windows)]
    #[test]
    fn a_junction_is_never_made_ready() {
        let vol = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let parent = vol.path().join(HELD_DIR_NAME);
        junction(&parent, elsewhere.path());
        assert!(prepare_held_dir(&user_root(vol.path(), &sid())).is_err());
        assert_eq!(fs::read_dir(elsewhere.path()).unwrap().count(), 0);
        fs::remove_dir(&parent).unwrap();

        fs::create_dir(&parent).unwrap();
        let root = user_root(vol.path(), &sid());
        junction(&root, elsewhere.path());
        assert!(prepare_held_dir(&root).is_err());
        assert_eq!(fs::read_dir(elsewhere.path()).unwrap().count(), 0);
        fs::remove_dir(&root).unwrap();
    }

    /// A long root, not ASCII, past `MAX_PATH`.
    fn long_root(base: &Path) -> PathBuf {
        let mut p = base.join("Ana María");
        while p.as_os_str().len() < 300 {
            p.push("fotos ñ");
        }
        p.join(HELD_DIR_NAME).join(sid())
    }

    #[test]
    fn the_record_keeps_each_root_once_in_utf8_with_what_kiln_made() {
        let scratch = tempfile::tempdir().unwrap();
        let record = scratch.path().join("Kiln").join(ROOTS_NAME);
        let e = user_root(Path::new(r"E:\"), &sid());
        let long = long_root(Path::new(r"F:\"));
        assert!(long.to_string_lossy().chars().count() > 260);

        assert!(read_held_roots(&record).is_empty());
        assert!(note_held_root(&record, &e, true).unwrap());
        assert!(note_held_root(&record, &long, false).unwrap());
        // Once: again, in another case, changes nothing.
        let e_upper = PathBuf::from(e.to_string_lossy().to_uppercase() + "\\");
        assert!(!note_held_root(&record, &e_upper, true).unwrap());
        assert_eq!(read_held_roots(&record), [e.clone(), long.clone()]);
        assert_eq!(
            fs::read_to_string(&record).unwrap(),
            format!("{}\tkiln+held\n{}\tkiln\n", e.display(), long.display())
        );
        let notes = read_held_notes(&record);
        assert!(notes[0].made && notes[0].made_parent);
        assert!(notes[1].made && !notes[1].made_parent);
        // A new mark replaces the line, in its place.
        assert!(note_held_root(&record, &e, false).unwrap());
        assert_eq!(read_held_roots(&record), [e.clone(), long.clone()]);
        assert!(!read_held_notes(&record)[0].made_parent);
        // Forgotten, it goes; the rest stays.
        assert!(forget_held_root(&record, &e).unwrap());
        assert!(!forget_held_root(&record, &e).unwrap());
        assert_eq!(read_held_roots(&record), std::slice::from_ref(&long));
        // No temp file or anything else is left beside it, but the lock.
        let mut names: Vec<String> = fs::read_dir(record.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, [ROOTS_LOCK_NAME, ROOTS_NAME]);
    }

    #[test]
    fn the_record_is_read_tolerantly_and_keeps_what_it_cannot_read() {
        let scratch = tempfile::tempdir().unwrap();
        let record = scratch.path().join(ROOTS_NAME);
        let mut bytes = b"E:\\.kiln-held\\S-1-5-21-1\tkiln\r\n\nnot a path\n".to_vec();
        bytes.extend_from_slice(b"\xff\xfe broken\n");
        // A drive that isn't mounted, a repeat, and a line with no mark
        // (not Kiln's to remove), last with no line break.
        bytes.extend_from_slice(
            "Q:\\.kiln-held\\S-1-5-21-1\tkiln\nE:\\.kiln-held\\S-1-5-21-1\tkiln\nR:\\x".as_bytes(),
        );
        fs::write(&record, &bytes).unwrap();

        let want = [
            r"E:\.kiln-held\S-1-5-21-1",
            r"Q:\.kiln-held\S-1-5-21-1",
            r"R:\x",
        ]
        .map(PathBuf::from);
        assert_eq!(read_held_roots(&record), want);
        assert!(!read_held_notes(&record)[2].made);
        // A new root keeps every line as it was, the odd ones too.
        let s = PathBuf::from(r"S:\.kiln-held\S-1-5-21-1");
        assert!(note_held_root(&record, &s, false).unwrap());
        let after = fs::read(&record).unwrap();
        assert!(after.starts_with(&bytes));
        assert_eq!(
            &after[bytes.len()..],
            b"\nS:\\.kiln-held\\S-1-5-21-1\tkiln\n"
        );
    }

    #[test]
    fn a_root_a_line_cannot_hold_is_refused() {
        let scratch = tempfile::tempdir().unwrap();
        let record = scratch.path().join(ROOTS_NAME);
        assert!(note_held_root(&record, Path::new("relative"), false).is_err());
        assert!(note_held_root(&record, Path::new("E:\\a\nb"), false).is_err());
        assert!(note_held_root(&record, Path::new("E:\\a\tb"), false).is_err());
        assert!(!record.exists());
    }

    #[test]
    fn two_kilns_noting_at_once_both_keep_theirs() {
        let scratch = tempfile::tempdir().unwrap();
        let record = scratch.path().join(ROOTS_NAME);
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let record = record.clone();
                std::thread::spawn(move || {
                    let drive = format!(r"{}:\", char::from(b'D' + i));
                    let root = user_root(Path::new(&drive), "S-1-5-21-1");
                    note_held_root(&record, &root, true).unwrap();
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(read_held_roots(&record).len(), 8);
    }

    /// A fake PC whose system drive is the scratch folder `kiln` (with
    /// Kiln's folder in it) and whose other drives are the scratch folders
    /// `vols`.
    fn scratch_pc(kiln: &Path, vols: &[&Path]) -> (Places, Fake) {
        let mut pc = Fake::new(r"C:\");
        for v in vols {
            pc = pc.with_scratch(v, DriveKind::Removable);
        }
        pc = pc.with_scratch(kiln, DriveKind::Fixed);
        pc.system = kiln.to_path_buf();
        (Places::new(kiln), pc)
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
    fn one_run_opens_a_folder_per_root_when_first_needed() {
        let kiln = tempfile::tempdir().unwrap();
        let e = tempfile::tempdir().unwrap();
        let f = tempfile::tempdir().unwrap();
        let (places, pc) = scratch_pc(kiln.path(), &[e.path(), f.path()]);
        let mut runs = HeldRuns::new(places.clone(), Box::new(pc));
        let (root_e, root_f) = (user_root(e.path(), &sid()), user_root(f.path(), &sid()));

        // Nothing is made before a job needs it.
        assert!(runs.is_empty());
        assert!(runs.cached(&e.path().join("a.jpg")).is_none());
        assert!(!e.path().join(HELD_DIR_NAME).exists());
        assert!(!places.record().exists());

        let on_e = runs
            .folder_for(&e.path().join("Photos").join("a.jpg"))
            .unwrap()
            .dir()
            .to_path_buf();
        assert_eq!(on_e.parent().unwrap(), root_e);
        assert!(on_e.join(LOCK_NAME).is_file());
        // The same drive again: the same folder, cached or opened.
        assert_eq!(runs.cached(&e.path().join("b.jpg")).unwrap().dir(), on_e);
        assert_eq!(
            runs.folder_for(&e.path().join("b.jpg")).unwrap().dir(),
            on_e
        );

        let on_f = runs
            .folder_for(&f.path().join("c.jpg"))
            .unwrap()
            .dir()
            .to_path_buf();
        let local = runs
            .folder_for(&kiln.path().join("d.jpg"))
            .unwrap()
            .dir()
            .to_path_buf();
        assert_eq!(local.parent().unwrap(), places.local_held());
        assert_eq!(runs.folders().count(), 3);
        // Each drive root noted once, as Kiln's; the local folder needs no
        // note.
        let notes = read_held_notes(places.record());
        assert_eq!(notes.len(), 2);
        assert_eq!((&notes[0].root, notes[0].made), (&root_e, true));
        assert_eq!((&notes[1].root, notes[1].made), (&root_f, true));
        assert!(notes[0].made_parent);

        // Each result is noted in its own folder's list, however its path
        // is cased.
        let a = on_e.join("a.jpg");
        let c = on_f.join("c.jpg");
        fs::write(&a, "a").unwrap();
        fs::write(&c, "c").unwrap();
        let a_cased = PathBuf::from(a.to_string_lossy().to_uppercase());
        assert!(runs.holding_of(&a_cased).is_some());
        runs.record(&a_cased, &e.path().join("Photos")).unwrap();
        runs.record(&c, f.path()).unwrap();
        assert!(on_e.join(SOURCES_NAME).is_file());
        assert!(on_f.join(SOURCES_NAME).is_file());
        assert!(!local.join(SOURCES_NAME).exists());
        assert!(runs.record(&kiln.path().join("x.jpg"), f.path()).is_err());
        runs.forget(&c).unwrap();

        assert!(runs.holds(&a));
        assert!(runs.holds(&places.local_held().join("x")));
        assert!(!runs.holds(&e.path().join("Photos")));

        // Emptied and closed, each folder goes, and so does each root Kiln
        // made, with its `.kiln-held`, and its line in the record.
        fs::remove_file(&a).unwrap();
        fs::remove_file(&c).unwrap();
        assert!(runs.close_empty().is_empty());
        assert!(!e.path().join(HELD_DIR_NAME).exists());
        assert!(!f.path().join(HELD_DIR_NAME).exists());
        assert!(read_held_roots(places.record()).is_empty());
        // The local folder stays, empty, as in phase 1.
        assert!(names(places.local_held()).is_empty());
    }

    #[test]
    fn a_drive_kiln_cannot_hold_on_uses_the_local_folder() {
        let kiln = tempfile::tempdir().unwrap();
        let e = tempfile::tempdir().unwrap();
        let (places, pc) = scratch_pc(kiln.path(), &[e.path()]);
        let pc = pc.unwritable(e.path());
        let mut runs = HeldRuns::new(places.clone(), Box::new(pc));
        let dir = runs.folder_for(&e.path().join("a.jpg")).unwrap().dir();
        assert_eq!(dir.parent().unwrap(), places.local_held());
        assert!(!e.path().join(HELD_DIR_NAME).exists());
        assert!(read_held_roots(places.record()).is_empty());
    }

    /// Changed in fix round 1: the root is noted before anything is made,
    /// so a root that can't be noted now leaves nothing on the drive
    /// (it used to leave an empty `.kiln-held` Kiln never removed).
    #[test]
    fn a_root_that_cannot_be_noted_is_not_held_on() {
        let kiln = tempfile::tempdir().unwrap();
        let e = tempfile::tempdir().unwrap();
        let (places, pc) = scratch_pc(kiln.path(), &[e.path()]);
        // The record's folder is a file: no note can be made.
        fs::write(places.record(), "").unwrap();
        let blocked = Places {
            local_held: places.local_held().to_path_buf(),
            record: places.record().join(ROOTS_NAME),
        };
        let mut runs = HeldRuns::new(blocked.clone(), Box::new(pc.clone()));
        let dir = runs
            .folder_for(&e.path().join("a.jpg"))
            .unwrap()
            .dir()
            .to_path_buf();
        assert_eq!(dir.parent().unwrap(), blocked.local_held());
        // And the drive stays on the local folder for this run.
        assert_eq!(runs.folder_for(&e.path().join("b.jpg")).unwrap().dir(), dir);
        assert_eq!(runs.folders().count(), 1);
        // Nothing was made on the drive, nor even asked for.
        assert!(names(e.path()).is_empty());
        assert!(pc.prepared.lock().unwrap().is_empty());
    }

    #[test]
    fn a_root_kiln_did_not_make_is_never_used_or_removed() {
        let kiln = tempfile::tempdir().unwrap();
        let e = tempfile::tempdir().unwrap();
        let (places, pc) = scratch_pc(kiln.path(), &[e.path()]);
        // Someone else made this user's folder: it isn't adopted.
        let root = user_root(e.path(), &sid());
        fs::create_dir_all(&root).unwrap();
        let mut runs = HeldRuns::new(places.clone(), Box::new(pc.clone()));
        let dir = runs.folder_for(&e.path().join("a.jpg")).unwrap().dir();
        assert_eq!(dir.parent().unwrap(), places.local_held());
        assert!(names(&root).is_empty());
        assert!(read_held_roots(places.record()).is_empty());
        assert!(!remove_empty_held_root(&root, &places));
        assert!(root.is_dir());
        assert!(runs.close_empty().is_empty());
        assert!(root.is_dir());
    }

    #[test]
    fn a_kiln_held_kiln_did_not_make_stays_when_its_root_goes() {
        let kiln = tempfile::tempdir().unwrap();
        let e = tempfile::tempdir().unwrap();
        let (places, pc) = scratch_pc(kiln.path(), &[e.path()]);
        let parent = e.path().join(HELD_DIR_NAME);
        fs::create_dir(&parent).unwrap();
        let mut runs = HeldRuns::new(places.clone(), Box::new(pc));
        let dir = runs
            .folder_for(&e.path().join("a.jpg"))
            .unwrap()
            .dir()
            .to_path_buf();
        assert_eq!(dir.parent().unwrap(), user_root(e.path(), &sid()));
        let notes = read_held_notes(places.record());
        assert!(notes[0].made && !notes[0].made_parent);
        assert!(runs.close_empty().is_empty());
        // The user's folder went; `.kiln-held`, not Kiln's, stays.
        assert!(parent.is_dir());
        assert!(names(&parent).is_empty());
    }

    #[test]
    fn kept_folders_are_offered_next_time() {
        let kiln = tempfile::tempdir().unwrap();
        let e = tempfile::tempdir().unwrap();
        let (places, pc) = scratch_pc(kiln.path(), &[e.path()]);
        let mut runs = HeldRuns::new(places.clone(), Box::new(pc.clone()));
        let dir = runs
            .folder_for(&e.path().join("a.jpg"))
            .unwrap()
            .dir()
            .to_path_buf();
        fs::write(dir.join("a.jpg"), "a").unwrap();
        runs.record(&dir.join("a.jpg"), e.path()).unwrap();
        assert_eq!(runs.keep(), std::slice::from_ref(&dir));

        let roots = recovery_roots(&places, &pc);
        let stale = stale_runs_in(&roots, &places);
        assert_eq!(stale.len(), 1);
        assert_eq!(
            stale[0].files,
            [Recovered {
                path: dir.join("a.jpg"),
                origin: Some(e.path().to_path_buf()),
            }]
        );
        // Next time the drive's folder, Kiln's by the record, is used again.
        drop(stale);
        let mut again = HeldRuns::new(places.clone(), Box::new(pc));
        let next = again.folder_for(&e.path().join("b.jpg")).unwrap().dir();
        assert_eq!(next.parent(), dir.parent());
    }

    #[test]
    fn results_left_on_every_root_are_delivered_in_one_pass() {
        let kiln = tempfile::tempdir().unwrap();
        let e = tempfile::tempdir().unwrap();
        let docs = tempfile::tempdir().unwrap();
        let (places, pc) = scratch_pc(kiln.path(), &[e.path()]);
        let photos = e.path().join("Photos");
        fs::create_dir(&photos).unwrap();
        let mut runs = HeldRuns::new(places.clone(), Box::new(pc.clone()));
        let on_e = runs
            .folder_for(&photos.join("a.jpg"))
            .unwrap()
            .dir()
            .to_path_buf();
        let local = runs
            .folder_for(&kiln.path().join("b.jpg"))
            .unwrap()
            .dir()
            .to_path_buf();
        fs::write(on_e.join("a.jpg"), "a").unwrap();
        fs::write(local.join("b.jpg"), "b").unwrap();
        runs.record(&on_e.join("a.jpg"), &photos).unwrap();
        runs.record(&local.join("b.jpg"), kiln.path()).unwrap();
        // The session ended: both kept as they are.
        assert_eq!(runs.keep().len(), 2);

        let roots = recovery_roots(&places, &pc);
        let stale = stale_runs_in(&roots, &places);
        assert_eq!(stale.len(), 2);
        let fallback = crate::held::kiln_results_in(docs.path());
        let report = crate::held::deliver_left(
            stale,
            &places,
            Some(&fallback),
            &crate::converter::CancelToken::new(),
        );
        assert_eq!((report.beside, report.kept), (2, 0), "{report:?}");
        assert_eq!(fs::read_to_string(photos.join("a.jpg")).unwrap(), "a");
        assert_eq!(fs::read_to_string(kiln.path().join("b.jpg")).unwrap(), "b");
        // The drive root Kiln made, left empty, goes, and so does its line.
        assert!(!e.path().join(HELD_DIR_NAME).exists());
        assert!(read_held_roots(places.record()).is_empty());
        assert!(names(places.local_held()).is_empty());
    }

    #[test]
    fn a_quit_keeps_folders_with_results_and_removes_the_rest_with_their_roots() {
        let kiln = tempfile::tempdir().unwrap();
        let e = tempfile::tempdir().unwrap();
        let f = tempfile::tempdir().unwrap();
        let (places, pc) = scratch_pc(kiln.path(), &[e.path(), f.path()]);
        let mut runs = HeldRuns::new(places.clone(), Box::new(pc));
        let on_e = runs
            .folder_for(&e.path().join("a.jpg"))
            .unwrap()
            .dir()
            .to_path_buf();
        let on_f = runs
            .folder_for(&f.path().join("b.jpg"))
            .unwrap()
            .dir()
            .to_path_buf();
        fs::write(on_e.join("a.jpg"), "a").unwrap();
        assert_eq!(runs.close_empty(), std::slice::from_ref(&on_e));
        assert!(on_e.join("a.jpg").is_file());
        assert!(probe_lock(&on_e.join(LOCK_NAME)).is_ok(), "let go");
        assert!(!on_f.exists());
        assert!(!f.path().join(HELD_DIR_NAME).exists());
        assert_eq!(
            read_held_roots(places.record()),
            [user_root(e.path(), &sid())]
        );
    }

    /// This user's root on the volume `vol`, made as Kiln makes it: owned
    /// by the user, with a protected DACL.
    fn own_root(vol: &Path) -> PathBuf {
        let root = user_root(vol, &sid());
        prepare_held_dir(&root).unwrap();
        root
    }

    /// A run folder as a run that ended without saving left it.
    fn left_run(root: &Path, name: &str, file: &str, origin: &Path) -> PathBuf {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(LOCK_NAME), "").unwrap();
        fs::write(dir.join(file), file).unwrap();
        fs::write(
            dir.join(SOURCES_NAME),
            format!("{file}\t{}\n", origin.display()),
        )
        .unwrap();
        dir
    }

    #[test]
    fn recovery_finds_runs_on_two_roots_in_one_list() {
        let kiln = tempfile::tempdir().unwrap();
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let places = Places::new(kiln.path());
        let (root_a, root_b) = (own_root(a.path()), own_root(b.path()));
        // A drive that isn't mounted, between the two.
        let missing = user_root(Path::new(&format!(r"{}\", unmounted_drive())), &sid());
        for r in [&root_a, &missing, &root_b] {
            note_held_root(places.record(), r, true).unwrap();
        }
        let run_a = left_run(&root_a, "1-1", "a.jpg", a.path());
        let run_b = left_run(&root_b, "2-1", "b.jpg", b.path());
        let run_c = left_run(places.local_held(), "3-1", "c.jpg", kiln.path());
        // A live run on a: never listed, locked or touched.
        let live = Holding::open(&root_a).unwrap();
        fs::write(live.dir().join("live.jpg"), "live").unwrap();
        // Root b holds a finished run too, with nothing left to offer.
        let done = root_b.join("2-2");
        fs::create_dir(&done).unwrap();
        fs::write(done.join(LOCK_NAME), "").unwrap();

        // No drive is mounted on this fake PC: the record alone finds them.
        let pc = Fake::new(r"C:\");
        let roots = recovery_roots(&places, &pc);
        assert_eq!(
            roots,
            [
                places.local_held().to_path_buf(),
                root_a.clone(),
                missing.clone(),
                root_b.clone()
            ]
        );
        let stale = stale_runs_in(&roots, &places);
        let dirs: Vec<&Path> = stale.iter().map(|r| r.dir.as_path()).collect();
        assert_eq!(dirs, [run_c.as_path(), run_a.as_path(), run_b.as_path()]);
        assert_eq!(stale[1].files[0].origin.as_deref(), Some(a.path()));
        // Another Kiln looking now finds nothing to offer.
        assert!(stale_runs_in(&roots, &places).is_empty());

        assert_eq!(sweep_stale_in(&roots, &places), [done]);
        // The missing drive stays in the record.
        assert_eq!(
            read_held_roots(places.record()),
            [root_a.clone(), missing.clone(), root_b.clone()]
        );

        // Saved, each run goes; b's root, left empty, goes with its
        // `.kiln-held`; a's holds the live run, so it stays.
        for run in stale {
            for f in &run.files {
                fs::remove_file(&f.path).unwrap();
            }
            assert!(close_run(run, &places));
        }
        assert!(!b.path().join(HELD_DIR_NAME).exists());
        assert!(root_a.is_dir());
        assert!(live.dir().join("live.jpg").is_file());
        assert!(places.local_held().is_dir());
        assert_eq!(
            read_held_roots(places.record()),
            [root_a.clone(), missing.clone()]
        );
        // The live run emptied and closed, its root goes too.
        fs::remove_file(live.dir().join("live.jpg")).unwrap();
        assert!(live.close_if_empty());
        assert!(remove_empty_held_root(&root_a, &places));
        assert!(!a.path().join(HELD_DIR_NAME).exists());
        assert_eq!(read_held_roots(places.record()), [missing]);
    }

    #[test]
    fn another_users_runs_are_never_listed_swept_or_locked() {
        let kiln = tempfile::tempdir().unwrap();
        let e = tempfile::tempdir().unwrap();
        let (places, pc) = scratch_pc(kiln.path(), &[e.path()]);
        let theirs = user_root(e.path(), OTHER);
        let their_run = left_run(&theirs, "1-1", "a.jpg", e.path());
        let their_done = theirs.join("1-2");
        fs::create_dir(&their_done).unwrap();
        fs::write(their_done.join(LOCK_NAME), "").unwrap();
        let mine = left_run(&own_root(e.path()), "2-1", "b.jpg", e.path());
        // Even named in this user's record, theirs isn't looked in.
        note_held_root(places.record(), &theirs, true).unwrap();

        let roots = recovery_roots(&places, &pc);
        assert!(!roots.iter().any(|r| same_path(r, &theirs)));
        let stale = stale_runs_in(&roots, &places);
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].dir, mine);
        assert!(sweep_stale_in(&roots, &places).is_empty());
        assert!(their_done.join(LOCK_NAME).is_file());
        assert_eq!(names(&their_run), ["a.jpg", "lock", SOURCES_NAME]);
        // Not locked: their own Kiln can still take it.
        assert!(probe_lock(&their_run.join(LOCK_NAME)).is_ok());
        assert!(!remove_empty_held_root(&theirs, &places));
        assert!(theirs.is_dir());
    }

    #[cfg(windows)]
    #[test]
    fn a_root_that_is_a_junction_is_never_looked_in() {
        let kiln = tempfile::tempdir().unwrap();
        let e = tempfile::tempdir().unwrap();
        let f = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let (places, pc) = scratch_pc(kiln.path(), &[e.path(), f.path()]);
        // Runs behind the links, as if they were this user's.
        let behind = left_run(&elsewhere.path().join(sid()), "1-1", "a.jpg", e.path());
        let empty = elsewhere.path().join(sid()).join("1-2");
        fs::create_dir(&empty).unwrap();
        fs::write(empty.join(LOCK_NAME), "").unwrap();
        // E's `.kiln-held` is a junction; F's user folder is.
        junction(&e.path().join(HELD_DIR_NAME), elsewhere.path());
        fs::create_dir(f.path().join(HELD_DIR_NAME)).unwrap();
        junction(&user_root(f.path(), &sid()), &elsewhere.path().join(sid()));

        let roots = recovery_roots(&places, &pc);
        assert!(stale_runs_in(&roots, &places).is_empty());
        assert!(sweep_stale_in(&roots, &places).is_empty());
        assert_eq!(names(&behind), ["a.jpg", "lock", SOURCES_NAME]);
        assert!(empty.join(LOCK_NAME).is_file());
        fs::remove_dir(e.path().join(HELD_DIR_NAME)).unwrap();
        fs::remove_dir(user_root(f.path(), &sid())).unwrap();
    }

    #[test]
    fn mounted_drives_are_looked_at_without_a_record() {
        let kiln = tempfile::tempdir().unwrap();
        let e = tempfile::tempdir().unwrap();
        let (places, pc) = scratch_pc(kiln.path(), &[e.path()]);
        let root = own_root(e.path());
        let run = left_run(&root, "1-1", "a.jpg", e.path());
        let roots = recovery_roots(&places, &pc);
        assert!(roots.contains(&root));
        let stale = stale_runs_in(&roots, &places);
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].dir, run);
        // Not noted, so not Kiln's to remove once empty.
        fs::remove_file(run.join("a.jpg")).unwrap();
        assert!(close_run(stale.into_iter().next().unwrap(), &places));
        assert!(root.is_dir());
    }

    #[test]
    fn only_an_empty_root_kiln_made_is_removed() {
        let kiln = tempfile::tempdir().unwrap();
        let e = tempfile::tempdir().unwrap();
        let places = Places::new(kiln.path());
        let root = user_root(e.path(), &sid());
        let parent = e.path().join(HELD_DIR_NAME);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("mine.txt"), "m").unwrap();

        // Not noted.
        assert!(!remove_empty_held_root(&root, &places));
        // Noted, but with no mark: not Kiln's.
        fs::write(places.record(), format!("{}\n", root.display())).unwrap();
        assert!(!remove_empty_held_root(&root, &places));
        note_held_root(places.record(), &root, true).unwrap();
        // Not empty.
        assert!(!remove_empty_held_root(&root, &places));
        assert!(root.join("mine.txt").is_file());
        // Not a user's folder in shape, though noted.
        let other = e.path().join("photos");
        fs::create_dir(&other).unwrap();
        note_held_root(places.record(), &other, true).unwrap();
        assert!(!remove_empty_held_root(&other, &places));
        assert!(other.is_dir());
        // Never the local folder.
        let local = Places {
            local_held: root.clone(),
            record: places.record().to_path_buf(),
        };
        fs::remove_file(root.join("mine.txt")).unwrap();
        assert!(!remove_empty_held_root(&root, &local));
        // Empty and Kiln's: it goes; `.kiln-held` holds another user's
        // folder, so it stays.
        fs::create_dir(parent.join(OTHER)).unwrap();
        assert!(remove_empty_held_root(&root, &places));
        assert!(!root.exists());
        assert_eq!(names(&parent), [OTHER]);
        assert_eq!(read_held_roots(places.record()), [other]);
    }

    /// Another account can make `.kiln-held\<this user's SID>` while it is
    /// absent: here a folder made the plain way stands in for it (its DACL
    /// inherited, not protected). A run planted in it, naming any folder
    /// to save to, is never offered, swept or used, even with a line in
    /// the record that says Kiln made it.
    #[cfg(windows)]
    #[test]
    fn a_planted_own_sid_folder_is_neither_looked_in_nor_used() {
        let kiln = tempfile::tempdir().unwrap();
        let e = tempfile::tempdir().unwrap();
        let (places, pc) = scratch_pc(kiln.path(), &[e.path()]);
        let root = user_root(e.path(), &sid());
        let target = tempfile::tempdir().unwrap();
        let planted = left_run(&root, "1-1", "payload.exe", target.path());
        let empty = root.join("1-2");
        fs::create_dir(&empty).unwrap();
        fs::write(empty.join(LOCK_NAME), "").unwrap();
        note_held_root(places.record(), &root, true).unwrap();
        assert!(!is_users_own(&root));

        let roots = recovery_roots(&places, &pc);
        assert!(roots.iter().any(|r| same_path(r, &root)));
        assert!(stale_runs_in(&roots, &places).is_empty());
        assert!(sweep_stale_in(&roots, &places).is_empty());
        assert_eq!(names(&planted), ["lock", "payload.exe", SOURCES_NAME]);
        assert!(empty.join(LOCK_NAME).is_file());

        // Not adopted either: results go to the local folder.
        let mut runs = HeldRuns::new(places.clone(), Box::new(pc));
        let dir = runs.folder_for(&e.path().join("a.jpg")).unwrap().dir();
        assert_eq!(dir.parent().unwrap(), places.local_held());
        assert_eq!(names(&root), ["1-1", "1-2"]);
        assert!(names(target.path()).is_empty());
    }

    /// Two Kilns of this user's open the same drive at once. The loser
    /// finds the folder made by the winner (`made == false`), uses it,
    /// since it is the user's own, and leaves the winner's line alone.
    #[test]
    fn a_kiln_that_loses_the_race_keeps_the_winners_line() {
        let kiln = tempfile::tempdir().unwrap();
        let e = tempfile::tempdir().unwrap();
        let (places, mut pc) = scratch_pc(kiln.path(), &[e.path()]);
        pc.winner_first = true;
        let mut runs = HeldRuns::new(places.clone(), Box::new(pc));
        let dir = runs
            .folder_for(&e.path().join("a.jpg"))
            .unwrap()
            .dir()
            .to_path_buf();
        let root = user_root(e.path(), &sid());
        assert_eq!(dir.parent().unwrap(), root);
        let notes = read_held_notes(places.record());
        assert_eq!(notes.len(), 1);
        assert!(same_path(&notes[0].root, &root) && notes[0].made);
        // And it still goes when emptied.
        assert!(runs.close_empty().is_empty());
        assert!(!e.path().join(HELD_DIR_NAME).exists());
    }

    #[test]
    fn paths_compare_as_windows_does() {
        assert!(same_path(
            Path::new(r"E:\.kiln-held"),
            Path::new(r"e:\.KILN-HELD\")
        ));
        assert!(inside(
            Path::new(r"E:\.kiln-held\1-1\a.jpg"),
            Path::new(r"e:\.kiln-held")
        ));
        assert!(!inside(
            Path::new(r"E:\.kiln-helder"),
            Path::new(r"E:\.kiln-held")
        ));
        let p = places();
        assert!(in_holding_place(Path::new(r"E:\.Kiln-Held\1-1"), &p));
        assert!(in_holding_place(&p.local_held().join("1-1"), &p));
        assert!(!in_holding_place(Path::new(r"E:\Photos"), &p));
        assert_eq!(
            sid_of(Path::new(r"E:\.kiln-held\S-1-5-21-1")),
            Some("S-1-5-21-1")
        );
        assert_eq!(sid_of(Path::new(r"E:\other\S-1-5-21-1")), None);
        assert_eq!(sid_of(Path::new(r"E:\.kiln-held\..")), None);
    }

    /// A drive letter with nothing mounted on it.
    fn unmounted_drive() -> String {
        (b'J'..=b'Y')
            .rev()
            .map(|c| format!("{}:", char::from(c)))
            .find(|d| !Path::new(&format!("{d}\\")).exists())
            .expect("a free drive letter")
    }
}
