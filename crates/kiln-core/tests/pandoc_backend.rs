//! The Pandoc backend, with the document fixtures.

#[path = "../../../tests/support/mod.rs"]
mod support;

use std::fs;
use std::io::ErrorKind;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;

use kiln_core::{Engines, Error, Kiln, Options};
use support::{assert_output, files_in, fixture, sample_for, stage};

fn options(temp: &tempfile::TempDir) -> Options {
    Options {
        temp_root: Some(temp.path().join("work")),
        ..Options::default()
    }
}

#[test]
fn markdown_to_docx() {
    let (dir, input) = stage(&sample_for("md"));
    let out = Kiln::new().convert(&input, "docx", &options(&dir)).unwrap();
    assert_eq!(out, dir.path().join("sample.docx"));
    assert_output(&out, "docx");
    assert!(files_in(&dir.path().join("work")).is_empty());
}

#[test]
fn docx_round_trips_to_markdown() {
    let (dir, input) = stage(&sample_for("docx"));
    let out = Kiln::new().convert(&input, "md", &options(&dir)).unwrap();
    assert_output(&out, "md");
    let text = fs::read_to_string(&out).unwrap();
    assert!(text.contains("1222"), "table lost: {text}");
}

#[test]
fn epub_to_html_is_standalone() {
    let (dir, input) = stage(&sample_for("epub"));
    let out = Kiln::new().convert(&input, "html", &options(&dir)).unwrap();
    assert_output(&out, "html");
}

#[test]
fn html_to_rtf_and_plain_text() {
    for target in ["rtf", "txt"] {
        let (dir, input) = stage(&sample_for("html"));
        let out = Kiln::new().convert(&input, target, &options(&dir)).unwrap();
        assert_output(&out, target);
    }
}

#[test]
fn sandbox_blocks_includes_and_remote_resources() {
    // A document that tries to pull in a local file and a remote image.
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret.txt");
    fs::write(&secret, "TOP-SECRET-VALUE").unwrap();
    let input = dir.path().join("sneaky.md");
    fs::write(
        &input,
        format!(
            "# Kiln fixture\n\n![remote](http://192.0.2.1/x.png)\n\n![local]({})\n\n- glaze\n",
            secret.display()
        ),
    )
    .unwrap();
    // Either Pandoc refuses or it writes a file without the secret. It must
    // never embed the local file.
    match Kiln::new().convert(&input, "docx", &options(&dir)) {
        Ok(out) => {
            let bytes = fs::read(&out).unwrap();
            assert!(!bytes.windows(16).any(|w| w == b"TOP-SECRET-VALUE"));
            assert!(!bytes.windows(11).any(|w| w == b"word/media/"));
        }
        Err(Error::EngineFailed { engine, .. }) => assert_eq!(engine, "Pandoc"),
        Err(other) => panic!("{other:?}"),
    }
}

#[test]
fn missing_pandoc_says_where_to_get_it() {
    let (dir, input) = stage(&sample_for("md"));
    let opts = Options {
        engines: Engines {
            pandoc: Some(dir.path().join("nowhere").join("pandoc")),
            ..Engines::default()
        },
        ..options(&dir)
    };
    let err = Kiln::new().convert(&input, "html", &opts).unwrap_err();
    assert_eq!(
        err.to_string(),
        "Pandoc isn't installed. Get it from Settings > Helper apps."
    );
    assert_eq!(files_in(dir.path()), ["sample.md", "work"]);
}

#[test]
fn unreadable_document_fails_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("broken.docx");
    fs::write(&input, b"not a zip at all").unwrap();
    let err = Kiln::new()
        .convert(&input, "md", &options(&dir))
        .unwrap_err();
    // Kiln sees it isn't a DOCX before Pandoc is asked.
    assert!(
        matches!(&err, Error::WrongContent { code, .. } if code == "DOCX"),
        "{err:?}"
    );
    // Nothing is written anywhere, not even a working folder.
    assert_eq!(files_in(dir.path()), ["broken.docx"]);
}

/// The port the remote picture in the tests points at.
const BEACON_PORT: u16 = 47392;

/// The names of the entries of a zip file, read from its central directory.
fn zip_names(path: &Path) -> Vec<String> {
    let bytes = fs::read(path).unwrap();
    let end = bytes
        .windows(4)
        .rposition(|w| w == b"PK\x05\x06")
        .unwrap_or_else(|| panic!("{} is not a zip file", path.display()));
    let le16 = |at: usize| usize::from(u16::from_le_bytes([bytes[at], bytes[at + 1]]));
    let mut at = u32::from_le_bytes(bytes[end + 16..end + 20].try_into().unwrap()) as usize;
    let mut names = Vec::new();
    for _ in 0..le16(end + 10) {
        assert_eq!(&bytes[at..at + 4], b"PK\x01\x02", "a bad central directory");
        let (name, extra, comment) = (le16(at + 28), le16(at + 30), le16(at + 32));
        names.push(String::from_utf8_lossy(&bytes[at + 46..at + 46 + name]).into_owned());
        at += 46 + name + extra + comment;
    }
    names
}

/// The pictures a DOCX, ODT or EPUB holds.
fn pictures_in(path: &Path) -> Vec<String> {
    zip_names(path)
        .into_iter()
        .filter(|n| !n.ends_with('/'))
        .filter(|n| {
            ["word/media/", "Pictures/", "EPUB/media/"]
                .iter()
                .any(|p| n.starts_with(p))
        })
        .collect()
}

/// A temp folder holding `outside.png`, and in it a folder `name` with
/// `note.md` (the text given), `pic.png` and `sub/pic two.png`. Returns the
/// temp folder and the note.
fn picture_folder(name: &str, markdown: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join(name);
    fs::create_dir_all(folder.join("sub")).unwrap();
    fs::copy(fixture("rgb_gradient.png"), folder.join("pic.png")).unwrap();
    fs::copy(fixture("photo.png"), folder.join("sub").join("pic two.png")).unwrap();
    fs::copy(fixture("gray8.png"), dir.path().join("outside.png")).unwrap();
    let note = folder.join("note.md");
    fs::write(&note, markdown).unwrap();
    (dir, note)
}

const TWO_LOCAL: &str = "![a](pic.png)\n\n![b](sub/pic%20two.png)\n";

#[test]
fn local_pictures_are_kept() {
    for target in ["docx", "odt", "epub"] {
        let (dir, note) = picture_folder("pictures", TWO_LOCAL);
        let out = Kiln::new().convert(&note, target, &options(&dir)).unwrap();
        assert_output(&out, target);
        assert_eq!(
            pictures_in(&out).len(),
            2,
            "{target}: {:?}",
            zip_names(&out)
        );
    }
}

#[test]
fn the_other_targets_that_embed_pictures_keep_them_too() {
    // What each one writes for a picture: PPTX a file, RTF a `\pict` group,
    // FB2 a `<binary>` and IPYNB an attachment.
    for (target, marker) in [
        ("pptx", "ppt/media/"),
        ("rtf", r"{\pict"),
        ("fb2", "<binary "),
        ("ipynb", "\"image/png\""),
    ] {
        let (dir, note) = picture_folder("pictures", TWO_LOCAL);
        let out = Kiln::new().convert(&note, target, &options(&dir)).unwrap();
        let found = if target == "pptx" {
            zip_names(&out)
                .iter()
                .filter(|n| n.starts_with(marker))
                .count()
        } else {
            String::from_utf8_lossy(&fs::read(&out).unwrap())
                .matches(marker)
                .count()
        };
        assert_eq!(found, 2, "{target} holds {found} pictures");
    }
}

#[test]
fn a_folder_with_non_ascii_letters_works() {
    let (dir, note) = picture_folder("Ana María", TWO_LOCAL);
    let out = Kiln::new().convert(&note, "docx", &options(&dir)).unwrap();
    assert_eq!(pictures_in(&out).len(), 2, "{:?}", zip_names(&out));
}

#[test]
fn html_keeps_its_local_pictures_too() {
    let (dir, note) = picture_folder("pictures", "");
    let page = note.with_file_name("page.html");
    fs::write(
        &page,
        "<html><body><p><img src=\"pic.png\" alt=\"a\"></p>\
         <p><img src=\"sub/pic%20two.png\" alt=\"b\"></p>\
         <p><img src=\"../outside.png\" alt=\"c\"></p></body></html>",
    )
    .unwrap();
    let out = Kiln::new().convert(&page, "docx", &options(&dir)).unwrap();
    assert_eq!(pictures_in(&out).len(), 2, "{:?}", zip_names(&out));
}

#[test]
fn pictures_outside_the_folder_or_remote_are_dropped() {
    let listener = TcpListener::bind(("127.0.0.1", BEACON_PORT)).unwrap_or_else(|e| {
        panic!("port {BEACON_PORT} on 127.0.0.1 is taken ({e}); this test needs it free")
    });
    listener.set_nonblocking(true).unwrap();
    // A picture that is there, named by its path with a drive letter, in a
    // folder of its own away from the note's.
    let elsewhere = tempfile::tempdir().unwrap();
    let drive = elsewhere.path().join("drive-picture.jpg");
    fs::copy(fixture("sample.jpg"), &drive).unwrap();
    let drive = drive
        .to_string_lossy()
        .replace('\\', "/")
        .replace(' ', "%20");
    assert_eq!(
        drive.as_bytes()[1],
        b':',
        "the temp folder is not on a drive"
    );
    let (dir, note) = picture_folder(
        "pictures",
        &format!(
            "Before.\n\n![outside picture](../outside.png)\n\n\
             ![drive picture]({drive})\n\n\
             ![remote picture](http://127.0.0.1:{BEACON_PORT}/x.png)\n\nAfter.\n"
        ),
    );
    let docx = Kiln::new().convert(&note, "docx", &options(&dir)).unwrap();
    assert_eq!(pictures_in(&docx), Vec::<String>::new());
    // Left with a picture it can't read, the RTF writer prints where it was
    // meant to come from, so each address says whether the picture was let
    // through. A picture it could read is `\pict` data.
    let rtf = Kiln::new().convert(&note, "rtf", &options(&dir)).unwrap();
    let rtf = fs::read_to_string(&rtf).unwrap();
    assert!(!rtf.contains(r"{\pict"), "a picture is in: {rtf}");
    for address in ["outside.png", "drive-picture.jpg", "127.0.0.1"] {
        assert!(!rtf.contains(address), "{address} is in: {rtf}");
    }
    for alt in ["outside picture", "drive picture", "remote picture"] {
        assert!(rtf.contains(alt), "{alt} is gone: {rtf}");
    }
    match listener.accept() {
        Err(e) if e.kind() == ErrorKind::WouldBlock => {}
        Ok(_) => panic!("something connected to {BEACON_PORT}"),
        Err(e) => panic!("{e}"),
    }
}

#[test]
fn text_targets_keep_their_picture_references() {
    // RST and HTML only name a picture, and fetch nothing, so each reference
    // stays as the author wrote it.
    let remote = format!("http://127.0.0.1:{BEACON_PORT}/x.png");
    let (dir, note) = picture_folder(
        "pictures",
        &format!(
            "![local picture](pic.png)\n\n![outside picture](../outside.png)\n\n\
             ![remote picture]({remote})\n"
        ),
    );
    for (target, before) in [("rst", ".. figure:: "), ("html", "<img src=\"")] {
        let out = Kiln::new().convert(&note, target, &options(&dir)).unwrap();
        let text = fs::read_to_string(&out).unwrap();
        // Each is a picture there, not text that happens to hold the address.
        let lost: Vec<&str> = ["pic.png", "../outside.png", remote.as_str()]
            .into_iter()
            .filter(|address| !text.contains(&format!("{before}{address}")))
            .collect();
        assert!(lost.is_empty(), "{target} lost {lost:?}: {text}");
    }
}

#[test]
fn encoded_ways_out_of_the_folder_are_dropped() {
    // Pandoc reads %XX in a path it opens, so each of these is `..` or a
    // backslash once it is read, once or twice. pic.png is there to show the
    // test would see a picture that was let in.
    let (dir, note) = picture_folder(
        "pictures",
        "![a](pic.png)\n\n![b](%2e%2e/outside.png)\n\n![c](%252e%252e/outside.png)\n\n\
         ![d](sub%2F..%2F..%2Foutside.png)\n\n![e](sub%5C..%5C..%5Coutside.png)\n\n\
         ![f](%2Fwindows/win.png)\n\n![g](pic.png%00.png)\n",
    );
    let out = Kiln::new().convert(&note, "docx", &options(&dir)).unwrap();
    assert_eq!(pictures_in(&out).len(), 1, "{:?}", zip_names(&out));
}

#[test]
fn only_pictures_of_a_sane_size_are_kept() {
    // A text file, an SVG and a PNG over 64 MB are dropped; a data: address
    // is the document's own and stays, with the picture next to it.
    let (dir, note) = picture_folder(
        "pictures",
        "![a](pic.png)\n\n![b](notes.txt)\n\n![c](drawing.svg)\n\n![d](big.png)\n\n\
         ![e](data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==)\n",
    );
    let folder = note.parent().unwrap();
    fs::write(folder.join("notes.txt"), "not a picture").unwrap();
    fs::write(
        folder.join("drawing.svg"),
        "<svg xmlns='http://www.w3.org/2000/svg'/>",
    )
    .unwrap();
    let big = fs::File::create(folder.join("big.png")).unwrap();
    big.set_len(64 * 1024 * 1024 + 1).unwrap();
    let out = Kiln::new().convert(&note, "docx", &options(&dir)).unwrap();
    assert_eq!(pictures_in(&out).len(), 2, "{:?}", zip_names(&out));
}

#[test]
fn a_percent_code_in_the_folder_name_is_not_read() {
    // Pandoc would read %41 in the folder's name as A, and look in a folder
    // that isn't there.
    let (dir, note) = picture_folder("Ana%41", TWO_LOCAL);
    let out = Kiln::new().convert(&note, "docx", &options(&dir)).unwrap();
    assert_eq!(pictures_in(&out).len(), 2, "{:?}", zip_names(&out));
}

#[test]
fn docx_to_odt_keeps_its_pictures() {
    // No fixture holds a picture, so Pandoc, run as it is, makes a DOCX that does.
    let (dir, note) = picture_folder("pictures", TWO_LOCAL);
    let docx = note.with_file_name("with-pictures.docx");
    // The same Pandoc as the other tests: the one on PATH.
    let made = Command::new("pandoc")
        .current_dir(note.parent().unwrap())
        .args(["note.md", "--output"])
        .arg(&docx)
        .output()
        .unwrap_or_else(|e| {
            panic!(
                "this test runs Pandoc to make a DOCX with pictures and couldn't start it \
                 ({e}); Pandoc has to be on PATH, as for the other tests"
            )
        });
    assert!(
        made.status.success(),
        "Pandoc couldn't make the DOCX: {}",
        String::from_utf8_lossy(&made.stderr)
    );
    assert_eq!(pictures_in(&docx).len(), 2, "{:?}", zip_names(&docx));
    let out = Kiln::new().convert(&docx, "odt", &options(&dir)).unwrap();
    assert_eq!(pictures_in(&out).len(), 2, "{:?}", zip_names(&out));
}
