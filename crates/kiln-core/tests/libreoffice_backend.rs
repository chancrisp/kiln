//! The LibreOffice backend, with the office fixtures.

#[path = "../../../tests/support/mod.rs"]
mod support;

use std::fs;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use kiln_core::{CancelToken, Engines, Error, Kiln, Options};
use support::{assert_output, files_in, fixture, sample_for, stage};

fn options(temp: &tempfile::TempDir) -> Options {
    // Office files go to PDF through Microsoft Office first where it is
    // installed (office_backend.rs); these tests are LibreOffice's.
    std::env::set_var(kiln_core::office::NO_OFFICE, "1");
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
        "LibreOffice isn't installed. Get it from Settings > Helper apps."
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

/// Whether LibreOffice is installed; a test that needs it says it is
/// skipped.
fn libreoffice(test: &str) -> bool {
    let has = Kiln::new()
        .availability(&Options::default())
        .has("libreoffice");
    if !has {
        eprintln!("{test}: skipped, LibreOffice isn't installed");
    }
    has
}

/// The PDF at `pdf` as text, read back through Kiln.
fn pdf_text(pdf: &Path, temp: &tempfile::TempDir) -> String {
    let txt = Kiln::new().convert(pdf, "txt", &options(temp)).unwrap();
    fs::read_to_string(txt).unwrap()
}

/// The cells of a CSV file Kiln wrote: one list per line, split at commas
/// (the cells in these tests hold none).
fn cells(csv: &Path) -> Vec<Vec<String>> {
    fs::read_to_string(csv)
        .unwrap()
        .lines()
        .map(|line| line.split(',').map(str::to_owned).collect())
        .collect()
}

/// Rows written out, to compare with [`cells`].
fn rows(rows: &[&[&str]]) -> Vec<Vec<String>> {
    rows.iter()
        .map(|row| row.iter().map(|c| (*c).to_owned()).collect())
        .collect()
}

#[test]
fn every_sheet_becomes_a_csv() {
    if !libreoffice("every_sheet_becomes_a_csv") {
        return;
    }
    // Two sheets, Sales and Notes, in a book called book.xlsx.
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("book.xlsx");
    fs::copy(fixture("two-sheets.xlsx"), &input).unwrap();
    let outputs = Kiln::new()
        .convert_outputs(
            &input,
            "csv",
            &options(&dir),
            &mut |_| {},
            &CancelToken::new(),
        )
        .unwrap();
    // In the order of the sheets' names.
    assert_eq!(
        outputs,
        [
            dir.path().join("book-Notes.csv"),
            dir.path().join("book-Sales.csv")
        ]
    );
    assert_eq!(
        cells(&outputs[0]),
        rows(&[&["Note"], &["Count the plums again"]])
    );
    assert_eq!(
        cells(&outputs[1]),
        rows(&[&["Item", "Quantity"], &["Plums", "12"], &["Pears", "7"]])
    );
    assert_eq!(
        files_in(dir.path()),
        ["book-Notes.csv", "book-Sales.csv", "book.xlsx", "work"]
    );
    assert!(files_in(&dir.path().join("work")).is_empty());
}

#[test]
fn a_book_with_dots_and_accents_in_its_name_keeps_them_in_each_csv() {
    if !libreoffice("a_book_with_dots_and_accents_in_its_name_keeps_them_in_each_csv") {
        return;
    }
    // LibreOffice puts the book's whole stem in front of each sheet's name,
    // and Kiln takes exactly that off again to find the name.
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("Ana María's book v1.2.xlsx");
    fs::copy(fixture("two-sheets.xlsx"), &input).unwrap();
    let outputs = Kiln::new()
        .convert_outputs(
            &input,
            "csv",
            &options(&dir),
            &mut |_| {},
            &CancelToken::new(),
        )
        .unwrap();
    assert_eq!(
        outputs,
        [
            dir.path().join("Ana María's book v1.2-Notes.csv"),
            dir.path().join("Ana María's book v1.2-Sales.csv")
        ]
    );
    assert_eq!(
        cells(&outputs[0]),
        rows(&[&["Note"], &["Count the plums again"]])
    );
}

#[test]
fn a_one_sheet_book_is_one_csv_named_as_before() {
    if !libreoffice("a_one_sheet_book_is_one_csv_named_as_before") {
        return;
    }
    for from in ["xlsx", "xls", "ods"] {
        let (dir, input) = stage(&sample_for(from));
        let outputs = Kiln::new()
            .convert_outputs(
                &input,
                "csv",
                &options(&dir),
                &mut |_| {},
                &CancelToken::new(),
            )
            .unwrap();
        assert_eq!(outputs, [dir.path().join("sample.csv")], "{from}");
        let cells = cells(&outputs[0]);
        assert_eq!(cells.len(), 3, "{from}: {cells:?}");
        assert_eq!(cells[0], ["Cone", "Temperature", "Glaze"], "{from}");
        assert_eq!(
            files_in(dir.path()),
            [
                "sample.csv".to_owned(),
                format!("sample.{from}"),
                "work".to_owned()
            ],
            "{from}"
        );
    }
}

/// The local picture linked.rtf names. Without it, a test on that file
/// proves nothing, so a PC that lacks it fails the test.
fn local_picture() {
    let picture = Path::new(r"C:\Windows\Web\Screen\img100.jpg");
    assert!(
        picture.is_file(),
        "{} is the picture linked.rtf names, and it isn't here",
        picture.display()
    );
}

/// How many pictures a PDF holds.
fn images_in(pdf: &[u8]) -> usize {
    const SUBTYPE: &[u8] = b"/Subtype";
    pdf.windows(SUBTYPE.len())
        .enumerate()
        .filter(|(_, w)| *w == SUBTYPE)
        .filter(|(at, _)| {
            let rest = &pdf[at + SUBTYPE.len()..];
            let skip = rest.iter().take_while(|b| b.is_ascii_whitespace()).count();
            rest[skip..].starts_with(b"/Image")
        })
        .count()
}

/// The port the pictures in `remote.rtf` and `remote-picture.docx` point at.
const BEACON_PORT: u16 = 47391;

/// Listens on `BEACON_PORT` and counts the connections made to it.
struct Beacon {
    hits: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Beacon {
    fn start() -> Beacon {
        let listener = TcpListener::bind(("127.0.0.1", BEACON_PORT)).unwrap_or_else(|e| {
            panic!(
                "port {BEACON_PORT} on 127.0.0.1 is taken ({e}); this test needs it free to count the connections LibreOffice makes"
            )
        });
        listener.set_nonblocking(true).unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (hits, stop) = (hits.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            hits.fetch_add(1, Ordering::SeqCst);
                            // Answer at once, so a client that does get here
                            // goes on instead of waiting for us.
                            let _ = stream.write_all(
                                b"HTTP/1.1 404 Not Found
Content-Length: 0
Connection: close

",
                            );
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(10)),
                    }
                }
            })
        };
        Beacon {
            hits,
            stop,
            thread: Some(thread),
        }
    }

    /// Waits until `n` connections have been counted.
    fn wait_for(&self, n: usize) {
        let started = Instant::now();
        while self.hits.load(Ordering::SeqCst) < n {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "the beacon did not count a connection made to it"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Stops listening; how many connections were made.
    fn finish(mut self) -> usize {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
        self.hits.load(Ordering::SeqCst)
    }
}

impl Drop for Beacon {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

#[test]
fn a_linked_local_picture_is_not_embedded() {
    if !libreoffice("a_linked_local_picture_is_not_embedded") {
        return;
    }
    // INCLUDEPICTURE "C:\Windows\Web\Screen\img100.jpg" between two
    // lines of text. LibreOffice used to put that file's pixels in the PDF.
    local_picture();
    let (dir, input) = stage(&fixture("hostile/linked.rtf"));
    let out = Kiln::new().convert(&input, "pdf", &options(&dir)).unwrap();
    assert_output(&out, "pdf");
    assert_eq!(images_in(&fs::read(&out).unwrap()), 0);
    let text = pdf_text(&out, &dir);
    assert!(text.contains("Before the picture."), "{text}");
    assert!(text.contains("After the picture."), "{text}");
    // The copy with the link defused is the engine's; the original keeps
    // its bytes.
    assert_eq!(
        fs::read_to_string(&input).unwrap(),
        fs::read_to_string(fixture("hostile/linked.rtf")).unwrap()
    );
    assert!(files_in(&dir.path().join("work")).is_empty());
}

#[test]
fn an_rtf_with_another_extension_is_neutralised_too() {
    if !libreoffice("an_rtf_with_another_extension_is_neutralised_too") {
        return;
    }
    // LibreOffice goes by what a file holds, not by its name, so linked.rtf
    // renamed to a Word 97, WordPerfect or Excel name is read as RTF still.
    local_picture();
    for ext in ["doc", "wpd", "xls"] {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join(format!("linked.{ext}"));
        fs::copy(fixture("hostile/linked.rtf"), &input).unwrap();
        let out = Kiln::new().convert(&input, "pdf", &options(&dir)).unwrap();
        assert_output(&out, "pdf");
        assert_eq!(images_in(&fs::read(&out).unwrap()), 0, "{ext}");
        let text = pdf_text(&out, &dir);
        assert!(text.contains("Before the picture."), "{ext}: {text}");
        assert!(text.contains("After the picture."), "{ext}: {text}");
    }
}

#[test]
fn a_document_without_linked_files_converts_as_before() {
    if !libreoffice("a_document_without_linked_files_converts_as_before") {
        return;
    }
    let (dir, input) = stage(&sample_for("rtf"));
    let out = Kiln::new().convert(&input, "pdf", &options(&dir)).unwrap();
    assert_output(&out, "pdf");
    assert!(files_in(&dir.path().join("work")).is_empty());
}

#[test]
fn no_url_is_fetched() {
    if !libreoffice("no_url_is_fetched") {
        return;
    }
    // BlockUntrustedRefererLinks already keeps LibreOffice from asking for
    // these pictures, and the dead proxy backs it up. The proxy has its
    // own test (profile_has_a_dead_proxy); this one guards the outcome.
    let beacon = Beacon::start();
    // One connection of our own, so a count of none below means none.
    TcpStream::connect(("127.0.0.1", BEACON_PORT)).unwrap();
    beacon.wait_for(1);
    for name in ["remote-picture.docx", "remote.rtf"] {
        let (dir, input) = stage(&fixture(&format!("hostile/{name}")));
        let out = Kiln::new().convert(&input, "pdf", &options(&dir)).unwrap();
        assert_output(&out, "pdf");
        let text = pdf_text(&out, &dir);
        assert!(text.contains("picture."), "{name}: {text}");
    }
    assert_eq!(
        beacon.finish(),
        1,
        "only the test's own connection was expected, so LibreOffice made one"
    );
}
