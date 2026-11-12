//! Output naming: sanitized file names and non-overwriting paths.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard};

use crate::error::{is_disk_full, Error, Result};

/// How many numbered names ("name (1).jpg" ... ) to try before giving up.
pub const MAX_NAME_ATTEMPTS: u32 = 10_000;

/// Longest stem we keep, in bytes. Leaves room for " (9999)" and an
/// extension inside the 255-byte component limit of NTFS and ext4.
pub(crate) const MAX_STEM_BYTES: usize = 200;

/// Device names Windows reserves regardless of extension ("CON.txt" is
/// still CON). Superscript digits are reserved too.
const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "COM¹", "COM²", "COM³", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8",
    "LPT9", "LPT¹", "LPT²", "LPT³",
];

/// Make a file stem safe to use on Windows (and therefore everywhere).
///
/// Replaces characters Windows forbids and control characters with `_`,
/// strips trailing dots and spaces, avoids reserved device names, and caps
/// the length. Never returns an empty string.
pub fn sanitize_stem(stem: &str) -> String {
    let mut out: String = stem
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();

    let trimmed_len = out.trim_end_matches(['.', ' ']).len();
    out.truncate(trimmed_len);

    if out.len() > MAX_STEM_BYTES {
        let mut cut = MAX_STEM_BYTES;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
        let trimmed_len = out.trim_end_matches(['.', ' ']).len();
        out.truncate(trimmed_len);
    }

    if out.is_empty() {
        return "untitled".to_owned();
    }

    if is_reserved(&out) {
        // Break the device name itself: "con.tar" becomes "con_.tar".
        let at = out.find('.').unwrap_or(out.len());
        out.insert(at, '_');
    }
    out
}

/// True if Windows would treat `stem` as a device name. Windows compares
/// the part before the first dot, ignoring case and trailing spaces.
fn is_reserved(stem: &str) -> bool {
    let base = stem.split('.').next().unwrap_or(stem).trim_end_matches(' ');
    RESERVED.iter().any(|r| r.eq_ignore_ascii_case(base))
}

/// Hold back the names `paths` from every new file this process makes
/// ([`create_unique`]) until [`let_go`] gives them back: held results a
/// Discard is still to remove, which a new result under the same name
/// would go with. Each call is matched by one `let_go`.
pub fn set_aside(paths: &[PathBuf]) {
    let mut aside = names_aside();
    for p in paths {
        *aside.entry(p.clone()).or_insert(0) += 1;
    }
}

/// Give back names [`set_aside`] held back.
pub fn let_go(paths: &[PathBuf]) {
    let mut aside = names_aside();
    for p in paths {
        if let Some(n) = aside.get_mut(p) {
            *n -= 1;
            if *n == 0 {
                aside.remove(p);
            }
        }
    }
}

/// The names held back, each with how many [`set_aside`] calls hold it.
fn names_aside() -> MutexGuard<'static, HashMap<PathBuf, usize>> {
    static ASIDE: LazyLock<Mutex<HashMap<PathBuf, usize>>> = LazyLock::new(Mutex::default);
    // A panic while holding it left the names as they were.
    ASIDE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Create a new, empty file in `dir` named `stem.ext`, or `stem (1).ext`,
/// `stem (2).ext` and so on if the name is taken.
///
/// The file is opened with `create_new`, so an existing file is never
/// truncated, even if another process creates it between our checks.
pub fn create_unique(dir: &Path, stem: &str, ext: &str) -> Result<(File, PathBuf)> {
    let stem = sanitize_stem(stem);
    for n in 0..MAX_NAME_ATTEMPTS {
        let path = dir.join(numbered_name(&stem, ext, n));
        if names_aside().contains_key(&path) {
            continue;
        }
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((file, path)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(Error::io(path, e)),
        }
    }
    Err(Error::NoFreeName {
        base: dir.join(format!("{stem}.{ext}")),
        attempts: MAX_NAME_ATTEMPTS,
    })
}

/// The `n`th name [`create_unique`] tries for a sanitized `stem`:
/// `stem.ext`, then `stem (1).ext`, `stem (2).ext` and so on.
pub(crate) fn numbered_name(stem: &str, ext: &str, n: u32) -> String {
    if n == 0 {
        format!("{stem}.{ext}")
    } else {
        format!("{stem} ({n}).{ext}")
    }
}

/// How many probe names `check_writable` tries before it gives up.
const MAX_PROBE_ATTEMPTS: u32 = 100;

/// Find out before any work is done whether `dir` takes new files: make a
/// file there and remove it. A folder that doesn't (read-only, denied to this
/// user, on a locked card) gives [`Error::CantSaveIn`], so the job fails at
/// once instead of after the conversion. A full disk is told apart, since
/// that has a row line of its own ([`Error::no_room`]).
pub fn check_writable(dir: &Path) -> Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let pid = std::process::id();
    for _ in 0..MAX_PROBE_ATTEMPTS {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let probe = dir.join(format!(".kiln-write-check-{pid}-{n}.tmp"));
        match OpenOptions::new().write(true).create_new(true).open(&probe) {
            Ok(file) => {
                // Closed first: Windows can't remove an open file. A probe
                // that can't be removed is empty and harmless, and the
                // folder did take it.
                drop(file);
                let _ = fs::remove_file(&probe);
                return Ok(());
            }
            // Left by a run that ended in between, under an id this process
            // has since taken: another name.
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(refusal(dir, e)),
        }
    }
    Err(Error::CantSaveIn(dir.to_path_buf()))
}

/// What a probe file that couldn't be made in `dir` says: the folder takes
/// no files, or the drive has no room, which is told as that.
fn refusal(dir: &Path, e: io::Error) -> Error {
    if is_disk_full(&e) {
        Error::io(dir, e)
    } else {
        Error::CantSaveIn(dir.to_path_buf())
    }
}

/// The last part of a folder path, or the whole path for a drive root.
pub(crate) fn folder_name(folder: &Path) -> String {
    folder
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| folder.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_names_pass_through() {
        assert_eq!(sanitize_stem("holiday photo"), "holiday photo");
        assert_eq!(sanitize_stem("café-2024"), "café-2024");
    }

    #[test]
    fn forbidden_characters_are_replaced() {
        assert_eq!(
            sanitize_stem(r#"a<b>c:d"e/f\g|h?i*j"#),
            "a_b_c_d_e_f_g_h_i_j"
        );
        assert_eq!(sanitize_stem("tab\there\u{7}"), "tab_here_");
    }

    #[test]
    fn trailing_dots_and_spaces_are_stripped() {
        assert_eq!(sanitize_stem("name. . "), "name");
        assert_eq!(sanitize_stem("..."), "untitled");
        assert_eq!(sanitize_stem(""), "untitled");
    }

    #[test]
    fn reserved_device_names_are_escaped() {
        for name in [
            "CON", "con", "Prn", "AUX", "nul", "COM1", "lpt9", "COM¹", "con.tar", "NUL .txt",
        ] {
            let s = sanitize_stem(name);
            assert!(!is_reserved(&s), "{name:?} became {s:?}, still reserved");
        }
        assert_eq!(sanitize_stem("CON"), "CON_");
        assert_eq!(sanitize_stem("con.tar"), "con_.tar");
    }

    #[test]
    fn near_reserved_names_are_left_alone() {
        for name in ["CONSOLE", "COM10", "LPT0", "NULL", "icon"] {
            assert_eq!(sanitize_stem(name), name);
        }
    }

    #[test]
    fn long_names_are_capped_on_a_char_boundary() {
        let long = "é".repeat(300);
        let s = sanitize_stem(&long);
        assert!(s.len() <= MAX_STEM_BYTES);
        assert!(s.chars().all(|c| c == 'é'));
    }

    #[test]
    fn create_unique_never_reuses_a_name() {
        let dir = tempfile::tempdir().unwrap();
        let (_, a) = create_unique(dir.path(), "photo", "jpg").unwrap();
        let (_, b) = create_unique(dir.path(), "photo", "jpg").unwrap();
        let (_, c) = create_unique(dir.path(), "photo", "jpg").unwrap();
        assert_eq!(a.file_name().unwrap(), "photo.jpg");
        assert_eq!(b.file_name().unwrap(), "photo (1).jpg");
        assert_eq!(c.file_name().unwrap(), "photo (2).jpg");
    }

    #[test]
    fn create_unique_does_not_truncate_existing_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("photo.jpg"), b"keep me").unwrap();
        let (_, path) = create_unique(dir.path(), "photo", "jpg").unwrap();
        assert_eq!(path.file_name().unwrap(), "photo (1).jpg");
        assert_eq!(
            std::fs::read(dir.path().join("photo.jpg")).unwrap(),
            b"keep me"
        );
    }

    #[test]
    fn create_unique_skips_a_name_set_aside_until_it_is_let_go() {
        let dir = tempfile::tempdir().unwrap();
        // A result a Discard is still to remove, already gone from disk.
        let waiting = dir.path().join("photo.jpg");
        set_aside(std::slice::from_ref(&waiting));
        let (_, a) = create_unique(dir.path(), "photo", "jpg").unwrap();
        assert_eq!(a.file_name().unwrap(), "photo (1).jpg");
        // Set aside twice, it is held back until both let it go.
        set_aside(std::slice::from_ref(&waiting));
        let_go(std::slice::from_ref(&waiting));
        let (_, b) = create_unique(dir.path(), "photo", "jpg").unwrap();
        assert_eq!(b.file_name().unwrap(), "photo (2).jpg");
        let_go(std::slice::from_ref(&waiting));
        let (_, c) = create_unique(dir.path(), "photo", "jpg").unwrap();
        assert_eq!(c, waiting);
    }

    #[test]
    fn create_unique_sanitizes_the_stem() {
        let dir = tempfile::tempdir().unwrap();
        let (_, path) = create_unique(dir.path(), "NUL", "jpg").unwrap();
        assert_eq!(path.file_name().unwrap(), "NUL_.jpg");
    }

    #[cfg(windows)]
    #[test]
    fn check_writable_refuses_a_read_only_folder() {
        let dir = tempfile::tempdir().unwrap();
        // A normal folder passes and keeps nothing of the check.
        check_writable(dir.path()).unwrap();
        assert_eq!(names_in(dir.path()), Vec::<String>::new());

        let folder = dir.path().join("card");
        std::fs::create_dir(&folder).unwrap();
        // Declared after `dir`, so it is undone before `dir` is removed.
        let _locked = test_support::Locked::new(&folder);
        let err = check_writable(&folder).unwrap_err();
        assert!(
            matches!(&err, Error::CantSaveIn(d) if d == &folder),
            "{err:?}"
        );
        assert_eq!(
            err.to_string(),
            format!("Kiln can't save in {}.", folder.display())
        );
        assert_eq!(names_in(&folder), Vec::<String>::new(), "a probe was left");
    }

    #[test]
    fn a_missing_folder_cant_be_saved_in_either() {
        let dir = tempfile::tempdir().unwrap();
        let err = check_writable(&dir.path().join("gone")).unwrap_err();
        assert!(matches!(err, Error::CantSaveIn(_)), "{err:?}");
    }

    #[test]
    fn a_full_disk_is_not_a_folder_that_cant_be_saved_in() {
        let full = io::Error::from_raw_os_error(if cfg!(windows) { 112 } else { 28 });
        let err = refusal(Path::new("out"), full);
        assert_eq!(err.no_room(), Some(Path::new("out")));
        let denied = io::Error::from(io::ErrorKind::PermissionDenied);
        assert!(matches!(
            refusal(Path::new("out"), denied),
            Error::CantSaveIn(_)
        ));
    }

    #[test]
    fn a_folder_is_named_by_its_own_name() {
        assert_eq!(folder_name(Path::new("Pictures")), "Pictures");
        assert_eq!(
            folder_name(&Path::new("Users").join("Ana").join("Pictures")),
            "Pictures"
        );
        // A drive has no name of its own.
        #[cfg(windows)]
        assert_eq!(folder_name(Path::new(r"E:\")), r"E:\");
    }

    fn names_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

/// What the tests that need a folder nothing can be made in share.
#[cfg(all(test, windows))]
pub(crate) mod test_support {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    /// A folder nothing new can be made in, as on a locked camera card: its
    /// files can still be read. Everyone is denied adding files and folders
    /// to it until this is dropped.
    pub(crate) struct Locked(PathBuf);

    /// Everyone, by its well-known SID, so the deny holds whatever the
    /// account's name is.
    const EVERYONE: &str = "*S-1-1-0";

    impl Locked {
        pub(crate) fn new(folder: &Path) -> Locked {
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
}
