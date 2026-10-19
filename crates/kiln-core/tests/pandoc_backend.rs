//! The Pandoc backend, with the document fixtures.

#[path = "../../../tests/support/mod.rs"]
mod support;

use std::fs;

use kiln_core::{Engines, Error, Kiln, Options};
use support::{assert_output, files_in, sample_for, stage};

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
        "Pandoc isn't installed. Get it from Settings > Engines."
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
    assert!(
        matches!(
            err,
            Error::EngineFailed {
                engine: "Pandoc",
                ..
            }
        ),
        "{err:?}"
    );
    assert_eq!(files_in(dir.path()), ["broken.docx", "work"]);
    assert!(files_in(&dir.path().join("work")).is_empty());
}
