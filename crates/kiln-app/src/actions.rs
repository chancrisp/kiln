//! Copy path, from the right-click menu. The parts that touch the clipboard
//! live in `win`; this is the logic, testable anywhere.

use std::path::{Path, PathBuf};

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

/// The last part of a folder path, or the whole path for a drive root.
pub fn folder_name(folder: &Path) -> String {
    folder
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| folder.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_gets_one_path_per_line() {
        let paths = [PathBuf::from("a.png"), PathBuf::from("b c.png")];
        assert_eq!(clipboard_text(&paths), "a.png\r\nb c.png");
        assert_eq!(clipboard_text(&paths[..1]), "a.png");
        assert_eq!(copied_summary(1), "Copied the path.");
        assert_eq!(copied_summary(3), "Copied 3 paths.");
    }
}
