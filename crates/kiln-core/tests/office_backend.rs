//! Office files to PDF through Microsoft Office (spec 4.2), on a PC that
//! has it. Each test skips with a reason when the app it needs isn't
//! installed, and leaves Office's programs as it found them: every
//! instance a test or Kiln starts is gone afterwards, and none it didn't
//! start is touched. Run with `--test-threads=1`; they also take turns.
//!
//! Office to PDF is switched off (`office::OFFICE_TO_PDF`, a decision
//! D14), so each test is ignored with that reason. Turn the switch on and
//! remove the ignores together.
#![cfg(windows)]

#[path = "../../../tests/support/mod.rs"]
mod support;

use std::fs;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use kiln_core::office::{self, Apartment, App, Arg, Object, Value, TIMEOUT};
use kiln_core::{Engines, Kiln, Options};
use support::{files_in, fixture};

/// One Office test at a time, whatever the test threads.
fn turn() -> MutexGuard<'static, ()> {
    static TURN: Mutex<()> = Mutex::new(());
    TURN.lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether `app` is installed and not already running for someone else;
/// a test that needs it says why it is skipped.
fn office(app: App, test: &str) -> bool {
    if !office::find_installed().has(app) {
        eprintln!("{test}: skipped, Microsoft {} isn't installed", app.name());
        return false;
    }
    true
}

/// The programs of every Office app that run now.
fn snapshot() -> Vec<(App, Vec<u32>)> {
    App::ALL
        .into_iter()
        .map(|a| {
            let mut pids = office::running(a);
            pids.sort_unstable();
            (a, pids)
        })
        .collect()
}

/// Waits until Office's programs are `want` again, and says so.
fn settle(want: &[(App, Vec<u32>)]) {
    let started = Instant::now();
    loop {
        let now = snapshot();
        if now == want {
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "Office's programs are {now:?}, not {want:?} as before"
        );
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Kiln's options with its working folders in `dir`.
fn options(dir: &Path) -> Options {
    Options {
        temp_root: Some(dir.join("work")),
        ..Options::default()
    }
}

/// A fixture copied to `dir` as `name`.
fn staged(dir: &Path, fixture_name: &str, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::copy(fixture(fixture_name), &path).unwrap();
    path
}

fn pages(pdf: &Path) -> usize {
    lopdf::Document::load(pdf)
        .unwrap_or_else(|e| panic!("{}: {e}", pdf.display()))
        .get_pages()
        .len()
}

/// The PDF's Producer: who made it.
fn producer(pdf: &Path) -> String {
    let doc = lopdf::Document::load(pdf).unwrap();
    let info = doc
        .trailer
        .get(b"Info")
        .and_then(|o| o.as_reference())
        .and_then(|id| doc.get_dictionary(id));
    info.ok()
        .and_then(|d| d.get(b"Producer").ok())
        .and_then(|p| p.as_str().ok())
        .map(|b| match b.strip_prefix(b"\xFE\xFF") {
            // UTF-16, as Office writes it.
            Some(wide) => String::from_utf16_lossy(
                &wide
                    .chunks_exact(2)
                    .map(|c| u16::from_be_bytes([c[0], c[1]]))
                    .collect::<Vec<_>>(),
            ),
            None => String::from_utf8_lossy(b).into_owned(),
        })
        .unwrap_or_default()
}

/// A file's bytes and modified time.
fn stamp(path: &Path) -> (Vec<u8>, SystemTime) {
    (
        fs::read(path).unwrap(),
        fs::metadata(path).unwrap().modified().unwrap(),
    )
}

fn object(v: Value) -> Object {
    match v {
        Value::Object(o) => o,
        v => panic!("not an object: {v:?}"),
    }
}

fn int(v: Value) -> i32 {
    match v {
        Value::Int(n) => n,
        v => panic!("not a number: {v:?}"),
    }
}

/// An instance of `app` of the test's own, quit when it drops.
struct Own(Object, App);

impl Own {
    fn start(app: App) -> Own {
        let o = Object::create(app.prog_id()).expect("Office starts");
        Own(o, app)
    }
}

impl Drop for Own {
    fn drop(&mut self) {
        let args: &[Arg<'_>] = if self.1 == App::Word {
            &[Arg::Int(0)]
        } else {
            &[]
        };
        let _ = self.0.call("Quit", args);
    }
}

/// Makes `path` in a Word of the test's own: one line of text, saved as
/// `format` (Word's wdSaveFormat) with `password`, never listed in Word's
/// recent files. With `macro_code`, the document's own code; `None` when
/// Word won't let a program add code.
fn word_document(path: &Path, format: i32, password: &str, macro_code: Option<&str>) -> Option<()> {
    let word = Own::start(App::Word);
    word.0.put("DisplayAlerts", Arg::Int(0)).unwrap();
    let docs = object(word.0.get("Documents").unwrap());
    let doc = object(docs.call("Add", &[]).unwrap());
    let range = object(doc.get("Content").unwrap());
    range
        .put("Text", Arg::Str("Kiln made this to test Office."))
        .unwrap();
    if let Some(code) = macro_code {
        let Ok(Value::Object(project)) = doc.get("VBProject") else {
            let _ = doc.call("Close", &[Arg::Int(0)]);
            return None;
        };
        let parts = object(project.get("VBComponents").unwrap());
        let this = object(parts.call("Item", &[Arg::Str("ThisDocument")]).unwrap());
        let module = object(this.get("CodeModule").unwrap());
        module.call("AddFromString", &[Arg::Str(code)]).unwrap();
    }
    let name = path.to_string_lossy();
    // SaveAs2: FileName, FileFormat, LockComments, Password,
    // AddToRecentFiles.
    doc.call(
        "SaveAs2",
        &[
            Arg::Str(&name),
            Arg::Int(format),
            Arg::Bool(false),
            Arg::Str(password),
            Arg::Bool(false),
        ],
    )
    .unwrap();
    doc.call("Close", &[Arg::Int(0)]).unwrap();
    Some(())
}

/// Word's own list of recent files, as a fresh Word of the test's sees it.
fn recent_files() -> Vec<String> {
    let word = Own::start(App::Word);
    let recent = object(word.0.get("RecentFiles").unwrap());
    let count = int(recent.get("Count").unwrap());
    (1..=count)
        .map(|i| {
            let item = object(recent.call("Item", &[Arg::Int(i)]).unwrap());
            match item.get("Name").unwrap() {
                Value::Str(s) => s,
                v => format!("{v:?}"),
            }
        })
        .collect()
}

#[test]
#[ignore = "Office to PDF is switched off (office::OFFICE_TO_PDF) until review 9/10/14's C1 and I3 land"]
fn office_files_become_pdfs_with_their_pages() {
    let _turn = turn();
    to_pdfs(
        "office_files_become_pdfs_with_their_pages",
        &[
            ("sample.docx", App::Word, 1),
            ("sample.pptx", App::PowerPoint, 2),
        ],
    );
}

/// Excel lays out its pages for the default printer, and stops at a
/// Printer Setup window when it can't reach it (the maintainer's PC, with a
/// network printer, does every time). Kiln then says Excel didn't finish
/// and leaves that Excel open, so this runs only when asked:
/// `cargo test -p kiln-core --test office_backend -- --ignored`.
#[test]
#[ignore = "Office to PDF is switched off (office::OFFICE_TO_PDF) until review 9/10/14's C1 and I3 land; and Excel stops at Printer Setup when the default printer can't be reached"]
fn a_workbook_becomes_a_pdf_of_every_sheet() {
    let _turn = turn();
    to_pdfs(
        "a_workbook_becomes_a_pdf_of_every_sheet",
        &[("two-sheets.xlsx", App::Excel, 2)],
    );
}

/// Each of `files` (fixture, app, pages) to PDF through Office, checked.
fn to_pdfs(test: &str, files: &[(&str, App, usize)]) {
    let _com = Apartment::new();
    for &(name, app, want) in files {
        let test = format!("{test} ({name})");
        if !office(app, &test) {
            continue;
        }
        if app == App::PowerPoint && !office::running(app).is_empty() {
            eprintln!("{test}: skipped, PowerPoint is open on this PC");
            continue;
        }
        let before = snapshot();
        let dir = tempfile::tempdir().unwrap();
        let input = staged(dir.path(), name, name);
        let was = stamp(&input);
        let out = Kiln::new()
            .convert(&input, "pdf", &options(dir.path()))
            .unwrap();
        assert_eq!(out.extension().unwrap(), "pdf");
        assert_eq!(pages(&out), want, "{name}");
        let made_by = producer(&out);
        assert!(
            made_by.contains("Microsoft"),
            "{name} was made by {made_by:?}"
        );
        // The person's file: same bytes, same time, and nothing left in
        // the job's folder.
        assert_eq!(stamp(&input), was, "{name}");
        assert!(files_in(&dir.path().join("work")).is_empty(), "{name}");
        settle(&before);
    }
}

#[test]
#[ignore = "Office to PDF is switched off (office::OFFICE_TO_PDF) until review 9/10/14's C1 and I3 land"]
fn kilns_word_goes_and_a_word_already_open_stays() {
    let _turn = turn();
    if !office(App::Word, "kilns_word_goes_and_a_word_already_open_stays") {
        return;
    }
    let _com = Apartment::new();
    let before = snapshot();
    let dir = tempfile::tempdir().unwrap();
    let input = staged(dir.path(), "sample.docx", "letter.docx");
    {
        let theirs = Own::start(App::Word);
        let with_theirs = snapshot();
        let out = Kiln::new()
            .convert(&input, "pdf", &options(dir.path()))
            .unwrap();
        assert!(producer(&out).contains("Microsoft"));
        // Kiln's Word is gone; the one that was open still answers.
        settle(&with_theirs);
        let docs = object(theirs.0.get("Documents").unwrap());
        assert_eq!(int(docs.get("Count").unwrap()), 0);
    }
    settle(&before);
}

#[test]
#[ignore = "Office to PDF is switched off (office::OFFICE_TO_PDF) until review 9/10/14's C1 and I3 land"]
fn the_persons_file_is_never_in_words_recent_files() {
    let _turn = turn();
    if !office(App::Word, "the_persons_file_is_never_in_words_recent_files") {
        return;
    }
    let _com = Apartment::new();
    let before = snapshot();
    let dir = tempfile::tempdir().unwrap();
    let name = format!("kiln-recent-{}.docx", std::process::id());
    let input = staged(dir.path(), "sample.docx", &name);
    let listed = recent_files();
    Kiln::new()
        .convert(&input, "pdf", &options(dir.path()))
        .unwrap();
    let now = recent_files();
    assert_eq!(now, listed, "Word's recent files changed");
    assert!(!now.iter().any(|n| n.contains("kiln-recent")), "{now:?}");
    // Nor in the list Office keeps for the Start menu and its own start
    // page.
    let mru = std::process::Command::new("reg")
        .args([
            "query",
            r"HKCU\Software\Microsoft\Office",
            "/s",
            "/d",
            "/f",
            &name,
        ])
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&mru.stdout);
    assert!(!said.contains(&name), "{said}");
    settle(&before);
}

#[test]
#[ignore = "Office to PDF is switched off (office::OFFICE_TO_PDF) until review 9/10/14's C1 and I3 land"]
fn a_document_with_a_password_fails_plainly_in_time() {
    let _turn = turn();
    if !office(
        App::Word,
        "a_document_with_a_password_fails_plainly_in_time",
    ) {
        return;
    }
    let _com = Apartment::new();
    let before = snapshot();
    let dir = tempfile::tempdir().unwrap();
    // wdFormatDocumentDefault (DOCX) and wdFormatDocument97 (DOC).
    let docx = dir.path().join("locked.docx");
    let doc = dir.path().join("locked.doc");
    word_document(&docx, 16, "kiln-test", None).unwrap();
    word_document(&doc, 0, "kiln-test", None).unwrap();
    settle(&before);
    for input in [docx, doc] {
        let was = stamp(&input);
        let started = Instant::now();
        let err = Kiln::new()
            .convert(&input, "pdf", &options(dir.path()))
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "This file has a password. Open it in Word and save it without one, then try again.",
            "{}",
            input.display()
        );
        assert!(started.elapsed() < TIMEOUT, "{:?}", started.elapsed());
        assert_eq!(stamp(&input), was);
        settle(&before);
    }
}

#[test]
#[ignore = "Office to PDF is switched off (office::OFFICE_TO_PDF) until review 9/10/14's C1 and I3 land"]
fn a_macro_in_a_document_never_runs() {
    let _turn = turn();
    let test = "a_macro_in_a_document_never_runs";
    if !office(App::Word, test) {
        return;
    }
    let _com = Apartment::new();
    let before = snapshot();
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("the-macro-ran.txt");
    let code = format!(
        "Sub AutoOpen()\r\n    Open \"{}\" For Output As #1\r\n    Close #1\r\nEnd Sub\r\n",
        marker.display()
    );
    let docm = dir.path().join("macro.docm");
    // wdFormatXMLDocumentMacroEnabled.
    if word_document(&docm, 13, "", Some(&code)).is_none() {
        settle(&before);
        eprintln!(
            "{test}: skipped, Word doesn't let programs add code here (Trust access to the VBA project object model is off), so the document can't be made"
        );
        return;
    }
    settle(&before);
    let work = dir.path().join("work");
    fs::create_dir(&work).unwrap();
    let out = work.join("output.pdf");
    office::to_pdf(App::Word, &docm, &out, &Default::default(), TIMEOUT).unwrap();
    assert!(out.is_file());
    assert!(!marker.exists(), "the AutoOpen macro ran");
    settle(&before);
}

#[test]
#[ignore = "Office to PDF is switched off (office::OFFICE_TO_PDF) until review 9/10/14's C1 and I3 land"]
fn an_open_powerpoint_is_left_alone() {
    let _turn = turn();
    let test = "an_open_powerpoint_is_left_alone";
    if !office(App::PowerPoint, test) {
        return;
    }
    if !office::running(App::PowerPoint).is_empty() {
        eprintln!("{test}: skipped, PowerPoint is already open on this PC");
        return;
    }
    let _com = Apartment::new();
    let before = snapshot();
    let dir = tempfile::tempdir().unwrap();
    let input = staged(dir.path(), "sample.pptx", "talk.pptx");
    {
        let theirs = Own::start(App::PowerPoint);
        let with_theirs = snapshot();
        // Without LibreOffice, the row says what to do.
        let no_lo = Options {
            engines: Engines {
                libreoffice: Some(dir.path().join("nowhere").join("soffice.exe")),
                ..Engines::default()
            },
            ..options(dir.path())
        };
        let err = Kiln::new().convert(&input, "pdf", &no_lo).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Close PowerPoint to convert this, or install LibreOffice."
        );
        // With it, LibreOffice makes the PDF.
        if kiln_core::find_libreoffice().is_some() {
            let out = Kiln::new()
                .convert(&input, "pdf", &options(dir.path()))
                .unwrap();
            assert_eq!(pages(&out), 2);
            let made_by = producer(&out);
            assert!(!made_by.contains("Microsoft"), "{made_by}");
        } else {
            eprintln!("{test}: LibreOffice isn't installed, so its half is skipped");
        }
        // Their PowerPoint was never asked to do anything, and still runs.
        assert_eq!(snapshot(), with_theirs);
        let shows = object(theirs.0.get("Presentations").unwrap());
        assert_eq!(int(shows.get("Count").unwrap()), 0);
    }
    settle(&before);
}

/// The port the pictures in `remote-picture.docx` and `remote.rtf` point
/// at (as `libreoffice_backend.rs`).
const BEACON_PORT: u16 = 47391;

/// Counts the connections made to `BEACON_PORT`.
struct Beacon {
    hits: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Beacon {
    fn start() -> Beacon {
        let listener = TcpListener::bind(("127.0.0.1", BEACON_PORT))
            .unwrap_or_else(|e| panic!("port {BEACON_PORT} is taken ({e})"));
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
                            let _ = stream.write_all(
                                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
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

    fn finish(mut self) -> usize {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            t.join().unwrap();
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
#[ignore = "Office to PDF is switched off (office::OFFICE_TO_PDF) until review 9/10/14's C1 and I3 land"]
fn word_fetches_no_address_a_document_names() {
    let _turn = turn();
    if !office(App::Word, "word_fetches_no_address_a_document_names") {
        return;
    }
    let _com = Apartment::new();
    let before = snapshot();
    let beacon = Beacon::start();
    TcpStream::connect(("127.0.0.1", BEACON_PORT)).unwrap();
    let started = Instant::now();
    while beacon.hits.load(Ordering::SeqCst) < 1 {
        assert!(started.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(10));
    }
    for name in ["remote.rtf", "remote-picture.docx"] {
        let dir = tempfile::tempdir().unwrap();
        let input = staged(dir.path(), &format!("hostile/{name}"), name);
        let out = Kiln::new()
            .convert(&input, "pdf", &options(dir.path()))
            .unwrap();
        assert!(producer(&out).contains("Microsoft"), "{name}");
        assert_eq!(
            beacon.hits.load(Ordering::SeqCst),
            1,
            "Word asked for {name}'s picture by its address"
        );
    }
    assert_eq!(beacon.finish(), 1);
    settle(&before);
}

#[test]
#[ignore = "Office to PDF is switched off (office::OFFICE_TO_PDF) until review 9/10/14's C1 and I3 land"]
fn word_embeds_no_linked_local_picture() {
    let _turn = turn();
    if !office(App::Word, "word_embeds_no_linked_local_picture") {
        return;
    }
    let picture = Path::new(r"C:\Windows\Web\Screen\img100.jpg");
    assert!(picture.is_file(), "linked.rtf names {}", picture.display());
    let _com = Apartment::new();
    let before = snapshot();
    let dir = tempfile::tempdir().unwrap();
    let input = staged(dir.path(), "hostile/linked.rtf", "linked.rtf");
    let out = Kiln::new()
        .convert(&input, "pdf", &options(dir.path()))
        .unwrap();
    assert!(producer(&out).contains("Microsoft"));
    let bytes = fs::read(&out).unwrap();
    let images = bytes
        .windows(b"/Subtype/Image".len())
        .filter(|w| *w == b"/Subtype/Image" || *w == b"/Subtype /Imag")
        .count();
    assert_eq!(images, 0, "Word put the linked picture in the PDF");
    assert_eq!(
        fs::read(&input).unwrap(),
        fs::read(fixture("hostile/linked.rtf")).unwrap()
    );
    settle(&before);
}
