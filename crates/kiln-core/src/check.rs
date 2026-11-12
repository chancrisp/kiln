//! A quick look at a file's first bytes when it is added, so Kiln can say
//! "Empty file" or "Isn't really a DOCX" instead of offering conversions
//! that cannot work. Only formats with a signature every valid file
//! carries are judged; for anything else, and for files that are valid
//! but unusual, the answer is "fine".

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
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
    /// A JPG that stops before the end of its picture, as a copy or a
    /// download that didn't finish leaves it (decision D26).
    CutShort,
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
        // Office and e-book packages are zip files. A Word, Excel or
        // PowerPoint file with a password is kept in a compound file,
        // which only Office to PDF takes, to say the password. While it is
        // switched off such a file is refused here: LibreOffice would get
        // it, with no time limit.
        "docx" | "xlsx" | "pptx" => {
            starts(b"PK")
                || (crate::office::OFFICE_TO_PDF && starts(b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1"))
        }
        "odt" | "ods" | "odp" | "epub" => starts(b"PK"),
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
            Some(true) if matches!(ext, "jpg" | "jpeg") && jpeg_cut_short(path) => {
                FileCheck::CutShort
            }
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

/// Whether the JPG at `path` stops before the end marker (FF D9) of its
/// picture, as one cut short by a copy or a download that didn't finish
/// does (D26). A file whose last two bytes are the marker is whole, as
/// nearly every JPG is. Otherwise its blocks are walked from the start to
/// the picture's own end marker: a camera may add data after it (a phone's
/// motion photo puts a whole video there), and an Exif thumbnail holds an
/// end marker of its own, so neither the last bytes nor the last marker
/// found by searching will do. `false` when the file can't be read: the
/// read that follows says why.
pub fn jpeg_cut_short(path: &Path) -> bool {
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let mut tail = [0; 2];
    let ends = file
        .seek(SeekFrom::End(-2))
        .and_then(|_| file.read_exact(&mut tail));
    if ends.is_ok() && tail == [0xFF, 0xD9] {
        return false;
    }
    file.rewind()
        .and_then(|()| cut_short_in(BufReader::with_capacity(1 << 16, file)))
        .unwrap_or(false)
}

/// Where a walk of a JPG's blocks is.
#[derive(Clone, Copy)]
enum At {
    /// Between blocks, looking for the FF of the next marker. Other bytes
    /// are passed over, as readers pass them.
    Between,
    /// After an FF between blocks: the marker's code is next.
    Code,
    /// In a scan's data, looking for an FF.
    Data,
    /// After an FF in a scan's data.
    DataCode,
    /// At the two bytes of a block's length, the first once read. `scan`
    /// for a scan's header, which its data follows.
    Length { scan: bool, high: Option<u8> },
    /// Bytes of a block still to pass over.
    Skip { left: usize, scan: bool },
}

/// [`jpeg_cut_short`] for the JPG read from `src`, from its start: whether
/// it ends before the end marker that closes its picture. A block's length
/// is believed and its bytes passed over, so an end marker inside one (an
/// Exif thumbnail's) is not taken for the picture's. In a scan's data an
/// FF is followed by 00 (an FF of the data itself), a restart marker, or
/// the marker that ends the scan. `false` for a file that doesn't start
/// as a JPG: it isn't judged.
fn cut_short_in(mut src: impl BufRead) -> std::io::Result<bool> {
    let mut start = [0; 2];
    src.read_exact(&mut start)?;
    if start != [0xFF, 0xD8] {
        return Ok(false);
    }
    let mut at = At::Between;
    loop {
        let buf = src.fill_buf()?;
        if buf.is_empty() {
            return Ok(true);
        }
        let mut i = 0;
        while i < buf.len() {
            at = match at {
                At::Between | At::Data => match buf[i..].iter().position(|&b| b == 0xFF) {
                    Some(n) => {
                        i += n + 1;
                        if matches!(at, At::Data) {
                            At::DataCode
                        } else {
                            At::Code
                        }
                    }
                    None => {
                        i = buf.len();
                        at
                    }
                },
                At::Skip { left, scan } => {
                    let n = left.min(buf.len() - i);
                    i += n;
                    match (left - n, scan) {
                        (0, true) => At::Data,
                        (0, false) => At::Between,
                        (left, scan) => At::Skip { left, scan },
                    }
                }
                At::Length { scan, high: None } => {
                    i += 1;
                    At::Length {
                        scan,
                        high: Some(buf[i - 1]),
                    }
                }
                At::Length {
                    scan,
                    high: Some(high),
                } => {
                    let len = usize::from(u16::from_be_bytes([high, buf[i]]));
                    i += 1;
                    At::Skip {
                        left: len.saturating_sub(2),
                        scan,
                    }
                }
                At::Code | At::DataCode => {
                    let code = buf[i];
                    i += 1;
                    let data = matches!(at, At::DataCode);
                    match code {
                        0xD9 => return Ok(false),
                        // Fill bytes before a marker.
                        0xFF => at,
                        // An FF of the data itself, a restart marker, and
                        // the markers that carry no length.
                        0x00 | 0x01 | 0xD0..=0xD8 if data => At::Data,
                        0x00 | 0x01 | 0xD0..=0xD8 => At::Between,
                        code => At::Length {
                            scan: code == 0xDA,
                            high: None,
                        },
                    }
                }
            };
        }
        let n = buf.len();
        src.consume(n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_password_protected_office_file_is_refused_at_once_while_office_is_off() {
        let dir = tempfile::tempdir().unwrap();
        let locked = crate::office::compound_file(&["EncryptionInfo", "EncryptedPackage"]);
        let size = locked.len() as u64;
        let want = if crate::office::OFFICE_TO_PDF {
            FileCheck::Ok
        } else {
            FileCheck::Mismatch
        };
        for ext in ["docx", "xlsx", "pptx"] {
            let path = dir.path().join(format!("locked.{ext}"));
            std::fs::write(&path, &locked).unwrap();
            assert_eq!(check_file(&path, Some(ext), size), want, "{ext}");
            assert_eq!(
                content_is_wrong(&path, ext),
                !crate::office::OFFICE_TO_PDF,
                "{ext}"
            );
        }
    }

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
        // A Word, Excel or PowerPoint file with a password is a compound
        // file, not a zip. Office to PDF says the password; while it is
        // switched off (D14) the file is refused at once, as before it,
        // and never reaches LibreOffice (M3). An ODF file stays a zip with
        // or without one.
        let compound = b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1\0\0";
        for ext in ["docx", "xlsx", "pptx"] {
            assert_eq!(
                signature_fits(ext, compound),
                Some(crate::office::OFFICE_TO_PDF),
                "{ext}"
            );
        }
        for ext in ["odt", "ods", "odp", "epub"] {
            assert_eq!(signature_fits(ext, compound), Some(false), "{ext}");
        }
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

    /// A 64 x 48 JPG built in code, with an Exif block holding a thumbnail
    /// that is a whole JPG of its own, end marker and all, as a camera
    /// writes one.
    fn camera_like_jpeg() -> Vec<u8> {
        use image::ImageEncoder;
        let encode = |img: &image::RgbImage| {
            let mut bytes = vec![];
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 90)
                .write_image(
                    img.as_raw(),
                    img.width(),
                    img.height(),
                    image::ExtendedColorType::Rgb8,
                )
                .unwrap();
            bytes
        };
        let picture = encode(&image::RgbImage::from_fn(64, 48, |x, y| {
            image::Rgb([(x * 4) as u8, (y * 5) as u8, ((x * y) % 256) as u8])
        }));
        let thumbnail = encode(&image::RgbImage::new(8, 8));
        let body = [b"Exif\0\0".as_slice(), &thumbnail].concat();
        let len = u16::try_from(body.len() + 2).unwrap();
        [
            &picture[..2],
            &[0xFF, 0xE1],
            &len.to_be_bytes(),
            &body,
            &picture[2..],
        ]
        .concat()
    }

    #[test]
    fn a_jpg_cut_short_is_told_from_a_whole_one() {
        let whole = camera_like_jpeg();
        assert!(whole.ends_with(&[0xFF, 0xD9]));
        let dir = tempfile::tempdir().unwrap();
        let w = |name: &str, bytes: &[u8]| {
            let p = dir.path().join(name);
            std::fs::write(&p, bytes).unwrap();
            (
                check_file(&p, Some("jpg"), bytes.len() as u64),
                jpeg_cut_short(&p),
            )
        };
        assert_eq!(w("whole.jpg", &whole), (FileCheck::Ok, false));
        // Cut in its picture's data, after the thumbnail's end marker: an
        // end marker is in the file, but not the picture's own.
        let cut = &whole[..whole.len() - 40];
        assert!(cut.windows(2).any(|m| m == [0xFF, 0xD9]));
        assert_eq!(w("cut.jpg", cut), (FileCheck::CutShort, true));
        assert_eq!(
            w("half.jpg", &whole[..whole.len() / 2]),
            (FileCheck::CutShort, true)
        );
        // Cut inside the Exif block, before any picture.
        assert_eq!(w("head.jpg", &whole[..30]), (FileCheck::CutShort, true));
        // Bytes after the end marker, as some cameras add: a phone's motion
        // photo puts a whole video there. Long enough to cross the reads.
        let mut video = b"\0\0\0\x18ftypmp42".to_vec();
        video.extend((0..300_000u32).map(|i| (i * 7 % 251) as u8));
        let trailed = [whole.as_slice(), &video].concat();
        assert_eq!(w("motion.jpg", &trailed), (FileCheck::Ok, false));
        assert_eq!(
            w("padded.jpg", &[&whole[..], &[0; 2]].concat()),
            (FileCheck::Ok, false)
        );
        // Only a JPG is judged so.
        let p = dir.path().join("cut.png");
        std::fs::write(&p, b"\x89PNG\r\n\x1a\n0000").unwrap();
        assert_eq!(check_file(&p, Some("png"), 12), FileCheck::Ok);
        // A file that can't be read is left to the read that follows.
        assert!(!jpeg_cut_short(&dir.path().join("gone.jpg")));
    }

    #[test]
    fn every_cut_of_a_jpg_is_found_whatever_the_reads() {
        let whole = camera_like_jpeg();
        for capacity in [1, 7, 4096] {
            let walk = |bytes: &[u8]| cut_short_in(BufReader::with_capacity(capacity, bytes));
            assert!(!walk(&whole).unwrap(), "{capacity}");
            for end in 2..whole.len() {
                assert!(
                    walk(&whole[..end]).unwrap(),
                    "cut at {end} of {}",
                    whole.len()
                );
            }
        }
    }

    #[test]
    fn a_jpgs_markers_are_walked_as_a_reader_walks_them() {
        // Start, an APP0 block, a scan whose data holds a stuffed FF and a
        // restart marker, a table between scans, a second scan whose data
        // opens with fill bytes, then the end.
        let jpg: &[u8] = &[
            0xFF, 0xD8, 0xFF, 0xE0, 0, 4, 0xFF, 0xD9, 0xFF, 0xDA, 0, 2, 0x12, 0xFF, 0x00, 0x34,
            0xFF, 0xD3, 0x56, 0xFF, 0xC4, 0, 3, 0xD9, 0xFF, 0xDA, 0, 2, 0xFF, 0xFF, 0x00, 0x78,
            0xFF, 0xD9,
        ];
        let walk = |bytes: &[u8]| cut_short_in(BufReader::with_capacity(3, bytes)).unwrap();
        assert!(!walk(jpg));
        for end in 2..jpg.len() {
            assert!(walk(&jpg[..end]), "cut at {end}");
        }
        // Stray bytes between blocks are passed over, as readers do.
        let stray = [&jpg[..6 + 2], &[0x00, 0x13], &jpg[8..]].concat();
        assert!(!walk(&stray));
        // A file that isn't a JPG is not judged.
        assert!(!walk(b"not a jpg"));
    }
}
