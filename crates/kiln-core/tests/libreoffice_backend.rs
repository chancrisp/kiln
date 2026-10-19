//! The LibreOffice backend, with the office fixtures.

#[path = "../../../tests/support/mod.rs"]
mod support;

use std::fs;
use std::time::{Duration, Instant};

use kiln_core::{CancelToken, Engines, Error, Kiln, Options};
use support::{assert_output, files_in, sample_for, stage};

fn options(temp: &tempfile::TempDir) -> Options {
    Options {
        temp_root: Some(temp.path().join("work")),
        ..Options::default()
    }
}

#[test]
fn docx_to_pdf() {
    let (dir, input) = stage(&sample_for("docx"));
    let out = Kiln::new().convert(&input, "pdf", &options(&dir)).unwrap();
    assert_eq!(out, dir.path().join("sample.pdf"));
    assert_output(&out, "pdf");
    // The profile and LibreOffice's own output folder went with the job.
    assert!(files_in(&dir.path().join("work")).is_empty());
}

#[test]
fn legacy_word_becomes_docx_with_its_table() {
    let (dir, input) = stage(&sample_for("doc"));
    let out = Kiln::new().convert(&input, "docx", &options(&dir)).unwrap();
    assert_output(&out, "docx");
    // Read it back through Pandoc to see the content made it.
    let md = Kiln::new().convert(&out, "md", &options(&dir)).unwrap();
    let text = fs::read_to_string(md).unwrap();
    assert!(text.contains("1222"), "table lost: {text}");
}

#[test]
fn spreadsheets_keep_their_cells() {
    for from in ["xlsx", "xls", "ods"] {
        let (dir, input) = stage(&sample_for(from));
        let out = Kiln::new().convert(&input, "csv", &options(&dir)).unwrap();
        assert_output(&out, "csv");
        let text = fs::read_to_string(&out).unwrap();
        assert!(text.starts_with("Cone,Temperature,Glaze"), "{from}: {text}");
    }
}

#[test]
fn csv_becomes_a_workbook() {
    let (dir, input) = stage(&sample_for("csv"));
    let out = Kiln::new().convert(&input, "xlsx", &options(&dir)).unwrap();
    assert_output(&out, "xlsx");
}

#[test]
fn slides_to_pdf_keep_both_pages() {
    let (dir, input) = stage(&sample_for("pptx"));
    let out = Kiln::new().convert(&input, "pdf", &options(&dir)).unwrap();
    assert_output(&out, "pdf");
    let bytes = fs::read(&out).unwrap();
    let pages = bytes.windows(10).filter(|w| w == b"/Type/Page").count()
        - bytes.windows(11).filter(|w| w == b"/Type/Pages").count();
    assert_eq!(pages, 2);
}

#[test]
fn outputs_never_replace_existing_files() {
    let (dir, input) = stage(&sample_for("odt"));
    fs::write(dir.path().join("sample.pdf"), b"keep me").unwrap();
    let out = Kiln::new().convert(&input, "pdf", &options(&dir)).unwrap();
    assert_eq!(out, dir.path().join("sample (1).pdf"));
    assert_eq!(fs::read(dir.path().join("sample.pdf")).unwrap(), b"keep me");
}

#[test]
fn missing_libreoffice_says_where_to_get_it() {
    let (dir, input) = stage(&sample_for("xlsx"));
    let opts = Options {
        engines: Engines {
            libreoffice: Some(dir.path().join("nowhere").join("soffice.exe")),
            ..Engines::default()
        },
        ..options(&dir)
    };
    let err = Kiln::new().convert(&input, "pdf", &opts).unwrap_err();
    assert_eq!(
        err.to_string(),
        "LibreOffice isn't installed. Get it from Settings > Engines."
    );
    assert_eq!(files_in(dir.path()), ["sample.xlsx", "work"]);
    assert!(files_in(&dir.path().join("work")).is_empty());
}

#[test]
fn unreadable_document_fails_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("broken.docx");
    // A zip header and then nothing: LibreOffice can't load it.
    let bytes = fs::read(sample_for("docx")).unwrap();
    fs::write(&input, &bytes[..1500]).unwrap();
    let err = Kiln::new()
        .convert(&input, "pdf", &options(&dir))
        .unwrap_err();
    match &err {
        Error::EngineFailed {
            engine, summary, ..
        } => {
            assert_eq!(*engine, "LibreOffice");
            assert_eq!(summary, "source file could not be loaded");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        err.to_string(),
        "LibreOffice couldn't convert the file: source file could not be loaded"
    );
    assert_eq!(files_in(dir.path()), ["broken.docx", "work"]);
    assert!(files_in(&dir.path().join("work")).is_empty());
}

#[test]
fn cancel_stops_libreoffice_and_cleans_up() {
    let (dir, input) = stage(&sample_for("ppt"));
    let opts = options(&dir);
    let cancel = CancelToken::new();
    let started = Instant::now();
    let result = std::thread::scope(|s| {
        let job = s.spawn(|| Kiln::new().convert_with(&input, "pdf", &opts, &mut |_| {}, &cancel));
        // Long enough for the launcher to have started soffice.bin.
        std::thread::sleep(Duration::from_millis(400));
        cancel.cancel();
        job.join().unwrap()
    });
    assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
    assert!(started.elapsed() < Duration::from_secs(10));
    // Nothing is left: no output and no working folder, which a leftover
    // soffice.bin would have kept open.
    assert_eq!(files_in(dir.path()), ["sample.ppt", "work"]);
    assert!(files_in(&dir.path().join("work")).is_empty());
}
