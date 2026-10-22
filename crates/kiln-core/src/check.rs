//! A quick look at a file's first bytes when it is added, so Kiln can say
//! "Empty file" or "Isn't really a DOCX" instead of offering conversions
//! that cannot work. Only formats with a signature every valid file
//! carries are judged; for anything else, and for files that are valid
//! but unusual, the answer is "fine".

use std::fs::File;
use std::io::Read;
use std::path::Path;

/// What looking at a file found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FileCheck {
    #[default]
    Ok,
    /// Zero bytes.
    Empty,
    /// The content isn't the type the extension says.
    Mismatch,
    /// Windows wouldn't let Kiln read it.
    Unreadable,
    /// A type Kiln has no conversions for: the extension isn't one it knows.
    Unknown,
}

/// How much of the start of a file is looked at.
const HEAD: usize = 1024;

/// Whether `head`, the start of a file, fits a file of format `ext` (the
/// canonical lower-case extension). `None` when there is no rule for the
/// format: text formats, media containers that may begin many ways, and
/// anything not listed.
pub fn signature_fits(ext: &str, head: &[u8]) -> Option<bool> {
    let starts = |magic: &[u8]| head.starts_with(magic);
    Some(match ext {
        "png" => starts(b"\x89PNG\r\n\x1a\n"),
        "jpg" | "jpeg" => starts(&[0xFF, 0xD8]),
        "gif" => starts(b"GIF8"),
        "bmp" => starts(b"BM"),
        "tif" | "tiff" => {
            starts(b"II*\0") || starts(b"MM\0*") || starts(b"II+\0") || starts(b"MM\0+")
        }
        "webp" => head.len() >= 12 && starts(b"RIFF") && &head[8..12] == b"WEBP",
        "ico" => starts(&[0, 0, 1, 0]),
        "wav" => starts(b"RIFF") || starts(b"RF64"),
        "pdf" => head.windows(4).any(|w| w == b"%PDF"),
        // Office and e-book packages are zip files.
        "docx" | "xlsx" | "pptx" | "odt" | "ods" | "odp" | "epub" => starts(b"PK"),
        "rtf" => starts(b"{\\rtf"),
        _ => return None,
    })
}

/// Read the start of `path`, up to [`HEAD`] bytes.
fn read_head(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut head = Vec::with_capacity(HEAD);
    File::open(path)?.take(HEAD as u64).read_to_end(&mut head)?;
    Ok(head)
}

/// Look at `path`, a file of `size` bytes and format `ext` (canonical,
/// lower case; `None` when Kiln doesn't know the format).
pub fn check_file(path: &Path, ext: Option<&str>, size: u64) -> FileCheck {
    if size == 0 {
        return FileCheck::Empty;
    }
    let Some(ext) = ext else {
        return FileCheck::Unknown;
    };
    match read_head(path) {
        Ok(head) => match signature_fits(ext, &head) {
            Some(false) => FileCheck::Mismatch,
            _ => FileCheck::Ok,
        },
        // Locked by another program, or no permission. Other trouble
        // (gone since it was listed) is left for the conversion to say.
        Err(e) if matches!(e.kind(), std::io::ErrorKind::PermissionDenied) => FileCheck::Unreadable,
        Err(e) if e.raw_os_error() == Some(32) => FileCheck::Unreadable,
        Err(_) => FileCheck::Ok,
    }
}

/// Whether the content of `path` is wrong for format `ext`, judged as
/// `check_file` does but for a conversion about to start. An empty file is
/// left to the engines to report.
pub fn content_is_wrong(path: &Path, ext: &str) -> bool {
    match read_head(path) {
        Ok(head) if !head.is_empty() => signature_fits(ext, &head) == Some(false),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_files_fit_and_random_bytes_do_not() {
        assert_eq!(signature_fits("png", b"\x89PNG\r\n\x1a\nrest"), Some(true));
        assert_eq!(signature_fits("png", b"not a png at all"), Some(false));
        assert_eq!(signature_fits("jpg", &[0xFF, 0xD8, 0xFF, 0xE0]), Some(true));
        assert_eq!(signature_fits("docx", b"PK\x03\x04..."), Some(true));
        assert_eq!(signature_fits("docx", &[7u8; 2000]), Some(false));
        assert_eq!(
            signature_fits("webp", b"RIFF\x10\0\0\0WEBPVP8 "),
            Some(true)
        );
        assert_eq!(
            signature_fits("webp", b"RIFF\x10\0\0\0WAVEfmt "),
            Some(false)
        );
    }

    #[test]
    fn unusual_but_valid_files_are_not_rejected() {
        // A PDF whose header sits after some leading bytes.
        let mut pdf = vec![b' '; 200];
        pdf.extend_from_slice(b"%PDF-1.7\n");
        assert_eq!(signature_fits("pdf", &pdf), Some(true));
        // An empty zip package, a big-endian TIFF, a BigTIFF.
        assert_eq!(signature_fits("epub", b"PK\x05\x06\0\0"), Some(true));
        assert_eq!(signature_fits("tiff", b"MM\0*\0\0"), Some(true));
        assert_eq!(signature_fits("tif", b"II+\0\x08\0"), Some(true));
        assert_eq!(
            signature_fits("wav", b"RF64\xff\xff\xff\xffWAVE"),
            Some(true)
        );
        // Formats with no rule are never judged: plain text, media
        // containers, legacy Office files that may be RTF or XML.
        for ext in [
            "txt", "md", "csv", "mp4", "mov", "mp3", "flac", "doc", "xls", "svg", "heic",
        ] {
            assert_eq!(signature_fits(ext, b"anything"), None, "{ext}");
        }
    }

    #[test]
    fn a_file_is_checked_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let w = |name: &str, bytes: &[u8]| {
            let p = dir.path().join(name);
            std::fs::write(&p, bytes).unwrap();
            p
        };
        let good = w("a.png", b"\x89PNG\r\n\x1a\n0000");
        let fake = w("b.docx", &[9u8; 2000]);
        let empty = w("c.png", b"");
        let text = w("d.txt", b"hello");
        assert_eq!(check_file(&good, Some("png"), 12), FileCheck::Ok);
        assert_eq!(check_file(&fake, Some("docx"), 2000), FileCheck::Mismatch);
        assert_eq!(check_file(&empty, Some("png"), 0), FileCheck::Empty);
        assert_eq!(check_file(&text, Some("txt"), 5), FileCheck::Ok);
        assert_eq!(check_file(&text, None, 5), FileCheck::Unknown);
        // A file that has gone since it was listed is the conversion's to say.
        assert_eq!(
            check_file(&dir.path().join("gone.png"), Some("png"), 10),
            FileCheck::Ok
        );
        assert!(content_is_wrong(&fake, "docx"));
        assert!(!content_is_wrong(&good, "png"));
        assert!(!content_is_wrong(&empty, "png"));
    }
}
