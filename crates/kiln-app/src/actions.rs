//! The Alt wheel's file actions: copy path, move, open in Windows
//! Terminal. The parts that touch the clipboard and dialogs live in `win`;
//! this is the logic, testable anywhere.

use std::fs::OpenOptions;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileAction {
    CopyPath,
    Move,
    OpenTerminal,
}

impl FileAction {
    pub const ALL: [FileAction; 3] = [
        FileAction::CopyPath,
        FileAction::Move,
        FileAction::OpenTerminal,
    ];

    pub fn label(self) -> &'static str {
        match self {
            FileAction::CopyPath => "Copy path",
            FileAction::Move => "Move",
            FileAction::OpenTerminal => "Open in Terminal",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            FileAction::CopyPath => "copy",
            FileAction::Move => "move",
            FileAction::OpenTerminal => "terminal",
        }
    }
}

/// What goes on the clipboard: one path per line.
pub fn clipboard_text(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join("\r\n")
}

/// "Copied the path." or "Copied 3 paths."
pub fn copied_summary(n: usize) -> String {
    if n == 1 {
        "Copied the path.".to_owned()
    } else {
        format!("Copied {n} paths.")
    }
}

/// "Moved 2 files to Pictures." with any failures after it.
pub fn moved_summary(moved: usize, failed: usize, folder: &Path) -> String {
    let name = folder_name(folder);
    let mut s = match moved {
        0 => format!("Couldn't move the files to {name}."),
        1 => format!("Moved 1 file to {name}."),
        n => format!("Moved {n} files to {name}."),
    };
    if moved > 0 && failed > 0 {
        s.push_str(&format!(" {failed} didn't move."));
    }
    s
}

/// The last part of a folder path, or the whole path for a drive root.
pub fn folder_name(folder: &Path) -> String {
    folder
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| folder.display().to_string())
}

/// Move `src` into `dest_dir`, keeping its name, or "name (1).ext" if that
/// is taken. Never replaces an existing file. Returns the new path.
///
/// The destination name is claimed first with `create_new`, then the file
/// is renamed onto the claim. Across drives, where a rename can't work,
/// the file is copied and the original removed.
pub fn move_to(src: &Path, dest_dir: &Path) -> Result<PathBuf, String> {
    let name = src
        .file_name()
        .ok_or_else(|| format!("{} isn't a file.", src.display()))?;
    if !src.is_file() {
        return Err(format!("{} isn't there any more.", name.to_string_lossy()));
    }
    if !dest_dir.is_dir() {
        return Err(format!("{} isn't a folder.", dest_dir.display()));
    }
    if src.parent().is_some_and(|p| same_dir(p, dest_dir)) {
        return Err(format!(
            "{} is already in that folder.",
            name.to_string_lossy()
        ));
    }

    let dest = claim(dest_dir, Path::new(name)).map_err(|e| e.to_string())?;
    if std::fs::rename(src, &dest).is_ok() {
        return Ok(dest);
    }
    // Probably another drive. Copy over our empty claim, then remove the
    // original.
    if let Err(e) = std::fs::copy(src, &dest) {
        let _ = std::fs::remove_file(&dest);
        return Err(format!("{} couldn't be moved: {e}", name.to_string_lossy()));
    }
    if let Err(e) = std::fs::remove_file(src) {
        return Err(format!(
            "{} was copied, but the original couldn't be removed: {e}",
            name.to_string_lossy()
        ));
    }
    Ok(dest)
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Create an empty placeholder for `name` in `dir`, numbering it if the
/// name is taken.
fn claim(dir: &Path, name: &Path) -> io::Result<PathBuf> {
    let stem = name
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = name.extension().map(|e| e.to_string_lossy().into_owned());
    for n in 0..1000 {
        let candidate = match (n, &ext) {
            (0, _) => name.to_string_lossy().into_owned(),
            (_, Some(ext)) => format!("{stem} ({n}).{ext}"),
            (_, None) => format!("{stem} ({n})"),
        };
        let path = dir.join(candidate);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => return Ok(path),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other("there's no free name in that folder"))
}

/// The folder Terminal opens in: the first file's folder.
pub fn terminal_folder(paths: &[PathBuf]) -> Option<PathBuf> {
    let first = paths.first()?;
    if first.is_dir() {
        return Some(first.clone());
    }
    first.parent().map(Path::to_path_buf)
}

/// Windows Terminal, opened in `folder`.
pub fn terminal_command(folder: &Path) -> Command {
    let mut cmd = Command::new("wt.exe");
    cmd.arg("-d").arg(folder);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_and_icons() {
        let labels: Vec<_> = FileAction::ALL.iter().map(|a| a.label()).collect();
        assert_eq!(labels, ["Copy path", "Move", "Open in Terminal"]);
        for a in FileAction::ALL {
            assert!(crate::icons::ALL.iter().any(|i| i.name == a.icon()));
        }
    }

    #[test]
    fn clipboard_gets_one_path_per_line() {
        let paths = [PathBuf::from("a.png"), PathBuf::from("b c.png")];
        assert_eq!(clipboard_text(&paths), "a.png\r\nb c.png");
        assert_eq!(clipboard_text(&paths[..1]), "a.png");
        assert_eq!(copied_summary(1), "Copied the path.");
        assert_eq!(copied_summary(3), "Copied 3 paths.");
    }

    #[test]
    fn summaries_name_the_folder() {
        let f = Path::new("/home/me/Pictures");
        assert_eq!(moved_summary(1, 0, f), "Moved 1 file to Pictures.");
        assert_eq!(
            moved_summary(2, 1, f),
            "Moved 2 files to Pictures. 1 didn't move."
        );
        assert_eq!(
            moved_summary(0, 2, f),
            "Couldn't move the files to Pictures."
        );
    }

    #[test]
    fn moving_keeps_the_name() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let src = a.join("photo.png");
        std::fs::write(&src, b"pixels").unwrap();

        let dest = move_to(&src, &b).unwrap();
        assert_eq!(dest, b.join("photo.png"));
        assert_eq!(std::fs::read(&dest).unwrap(), b"pixels");
        assert!(!src.exists());
    }

    #[test]
    fn moving_never_replaces_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(b.join("photo.png"), b"theirs").unwrap();
        std::fs::write(b.join("photo (1).png"), b"theirs too").unwrap();
        std::fs::write(b.join("README"), b"theirs").unwrap();
        std::fs::write(a.join("photo.png"), b"mine").unwrap();
        std::fs::write(a.join("README"), b"mine").unwrap();

        let dest = move_to(&a.join("photo.png"), &b).unwrap();
        assert_eq!(dest, b.join("photo (2).png"));
        assert_eq!(std::fs::read(b.join("photo.png")).unwrap(), b"theirs");
        assert_eq!(std::fs::read(&dest).unwrap(), b"mine");

        let dest = move_to(&a.join("README"), &b).unwrap();
        assert_eq!(dest, b.join("README (1)"));
        assert_eq!(std::fs::read(b.join("README")).unwrap(), b"theirs");
    }

    #[test]
    fn moving_explains_what_went_wrong() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("x.txt");
        std::fs::write(&src, b"x").unwrap();
        let e = move_to(&src, dir.path()).unwrap_err();
        assert_eq!(e, "x.txt is already in that folder.");
        let e = move_to(&dir.path().join("gone.txt"), dir.path()).unwrap_err();
        assert_eq!(e, "gone.txt isn't there any more.");
        let e = move_to(&src, &dir.path().join("nowhere")).unwrap_err();
        assert!(e.ends_with("isn't a folder."));
        assert!(src.exists());
    }

    #[test]
    fn terminal_opens_in_the_first_files_folder() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.png");
        std::fs::write(&f, b"").unwrap();
        assert_eq!(
            terminal_folder(std::slice::from_ref(&f)),
            Some(dir.path().to_path_buf())
        );
        assert_eq!(
            terminal_folder(&[dir.path().to_path_buf()]),
            Some(dir.path().to_path_buf())
        );
        assert_eq!(terminal_folder(&[]), None);

        let cmd = terminal_command(dir.path());
        assert_eq!(cmd.get_program(), "wt.exe");
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args, [std::ffi::OsStr::new("-d"), dir.path().as_os_str()]);
    }
}
