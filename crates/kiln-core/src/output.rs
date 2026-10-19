//! Output naming: sanitized file names and non-overwriting paths.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// How many numbered names ("name (1).jpg" ... ) to try before giving up.
pub const MAX_NAME_ATTEMPTS: u32 = 10_000;

/// Longest stem we keep, in bytes. Leaves room for " (9999)" and an
/// extension inside the 255-byte component limit of NTFS and ext4.
const MAX_STEM_BYTES: usize = 200;

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

/// Create a new, empty file in `dir` named `stem.ext`, or `stem (1).ext`,
/// `stem (2).ext` and so on if the name is taken.
///
/// The file is opened with `create_new`, so an existing file is never
/// truncated, even if another process creates it between our checks.
pub fn create_unique(dir: &Path, stem: &str, ext: &str) -> Result<(File, PathBuf)> {
    let stem = sanitize_stem(stem);
    for n in 0..MAX_NAME_ATTEMPTS {
        let name = if n == 0 {
            format!("{stem}.{ext}")
        } else {
            format!("{stem} ({n}).{ext}")
        };
        let path = dir.join(name);
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
    fn create_unique_sanitizes_the_stem() {
        let dir = tempfile::tempdir().unwrap();
        let (_, path) = create_unique(dir.path(), "NUL", "jpg").unwrap();
        assert_eq!(path.file_name().unwrap(), "NUL_.jpg");
    }
}
