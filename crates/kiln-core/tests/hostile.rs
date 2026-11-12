//! Files built to wear Kiln's readers out: a PDF nested far deeper than any
//! real one, a PDF whose forms draw themselves, and SVGs nested too deep, drawn
//! too large or given effects too heavy to draw, and a GIF whose header claims
//! a canvas far larger than its bytes. Each fails its own job and
//! the process lives. Files that only look like them still convert.

#[path = "../../../tests/support/mod.rs"]
mod support;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use kiln_core::{inspect, spawn_parser, CancelToken, Error, Facts, Kiln, Limit, Options, PdfFacts};
use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};
use support::{files_in, fixture, stage};

const SELF_REFERENCE: &str = "This PDF's pages refer to themselves, so Kiln can't read its text.";
const SVG_TOO_DEEP: &str = "This SVG is nested too deeply to draw.";
const SVG_TOO_HEAVY: &str = "This SVG's effects are too heavy to draw.";

thread_local! {
    /// The largest single allocation this thread has asked for.
    static LARGEST: Cell<usize> = const { Cell::new(0) };
}

/// The system allocator, noting the largest request each thread makes. A
/// request that large is made whether or not the memory is ever used, so
/// this tells a reader that reserved a huge canvas from one that did not,
/// on a machine with memory enough to let the first go through.
struct Watching;

fn note(size: usize) {
    LARGEST.with(|largest| largest.set(largest.get().max(size)));
}

// SAFETY: every call goes straight to the system allocator, which upholds
// the contract; the only addition is a count in a thread-local cell, which
// never allocates.
unsafe impl GlobalAlloc for Watching {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note(new_size);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Watching = Watching;

/// Runs `f` on a parser thread and waits for it, as the app runs jobs.
fn on_parser<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    spawn_parser("kiln-test", f)
        .expect("start a parser thread")
        .join()
        .expect("the parser thread finished without panicking")
}

/// `inspect` reads one PDF at a time, so tests that call it take turns.
static PDF: Mutex<()> = Mutex::new(());

fn one_pdf_at_a_time() -> MutexGuard<'static, ()> {
    PDF.lock().unwrap_or_else(|e| e.into_inner())
}

fn convert_to_txt(pdf: &Path) -> kiln_core::Result<PathBuf> {
    let pdf = pdf.to_path_buf();
    on_parser(move || Kiln::new().convert(&pdf, "txt", &Options::default()))
}

fn facts_of(pdf: &Path) -> Facts {
    inspect(&Kiln::new(), pdf, &Options::default(), &CancelToken::new()).expect("a PDF has facts")
}

/// What `inspect` finds in a PDF of one page whose text Kiln can't read.
const UNREADABLE: PdfFacts = PdfFacts {
    pages: 1,
    has_text: Some(false),
};

fn square() -> Object {
    vec![0.into(), 0.into(), 200.into(), 200.into()].into()
}

fn helvetica(doc: &mut Document) -> ObjectId {
    doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
    })
}

/// A form XObject of `subtype`, put at `id`. A form with `resources` `None`
/// has none of its own.
fn put_form(
    doc: &mut Document,
    id: ObjectId,
    subtype: &str,
    resources: Option<Object>,
    content: &str,
) {
    let mut dict = dictionary! {
        "Type" => "XObject",
        "Subtype" => subtype,
        "BBox" => square(),
    };
    if let Some(resources) = resources {
        dict.set("Resources", resources);
    }
    doc.objects.insert(
        id,
        Object::Stream(Stream::new(dict, content.as_bytes().to_vec())),
    );
}

/// Finishes `doc` as one page drawn with `resources` and `content`, and
/// saves it at `path`.
fn one_page(mut doc: Document, path: &Path, resources: Object, content: &str) {
    let contents = doc.add_object(Stream::new(dictionary! {}, content.as_bytes().to_vec()));
    let pages = doc.new_object_id();
    let page = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages,
        "MediaBox" => square(),
        "Contents" => contents,
        "Resources" => resources,
    });
    doc.objects.insert(
        pages,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Count" => 1,
            "Kids" => vec![Object::Reference(page)],
        }),
    );
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
    doc.trailer.set("Root", catalog);
    doc.save(path).expect("write the test PDF");
}

/// A resource dictionary naming `forms` as `F1`, `F2` and so on.
fn xobjects(forms: &[ObjectId]) -> Dictionary {
    let mut named = Dictionary::new();
    for (i, form) in forms.iter().enumerate() {
        named.set(format!("F{}", i + 1), *form);
    }
    dictionary! { "XObject" => named }
}

/// How the last form of a chain ends.
enum Ending {
    /// It writes "Deep text" and draws nothing more.
    Text,
    /// It draws itself.
    Itself,
}

/// Writes a one-page PDF at `path` whose page draws a first form, which draws
/// a second, and so on for `depth` forms, then the `ending`.
fn chain_pdf(path: &Path, depth: usize, ending: Ending) {
    let mut doc = Document::with_version("1.7");
    let forms: Vec<ObjectId> = (0..depth).map(|_| doc.new_object_id()).collect();
    let font = helvetica(&mut doc);
    for (i, id) in forms.iter().enumerate() {
        let (resources, content) = match (forms.get(i + 1), &ending) {
            (Some(next), _) => (xobjects(&[*next]), "/F1 Do"),
            (None, Ending::Itself) => (xobjects(&[*id]), "/F1 Do"),
            (None, Ending::Text) => (
                dictionary! { "Font" => dictionary! { "T1" => font } },
                "BT /T1 12 Tf 10 100 Td (Deep text) Tj ET",
            ),
        };
        put_form(&mut doc, *id, "Form", Some(resources.into()), content);
    }
    one_page(doc, path, xobjects(&forms[..1]).into(), "/F1 Do");
}

#[test]
fn deep_pdfs_fail_one_job() {
    let _turn = one_pdf_at_a_time();
    for name in ["deep-2000.pdf", "deep-30000.pdf"] {
        let (dir, staged) = stage(&fixture(&format!("hostile/{name}")));

        // A checkout that turned the line ends into CRLF would make these
        // fail at the trailer, before the nesting is read, and this test
        // would prove nothing.
        let bytes = fs::read(&staged).unwrap();
        assert!(
            !bytes.windows(2).any(|w| w == b"\r\n"),
            "{name} has CRLF line ends; .gitattributes should keep tests/fixtures/hostile/*.pdf as written"
        );
        // The file reads as a PDF; what is refused is the page nested 2,000
        // or 30,000 deep, which lopdf leaves out, so the document has none.
        let pages =
            on_parser(move || lopdf::Document::load_mem(&bytes).map(|doc| doc.get_pages().len()));
        assert_eq!(pages.map_err(|e| e.to_string()), Ok(0), "{name}");

        let result = convert_to_txt(&staged);
        let err = result.expect_err(&format!("{name} converted"));
        assert!(
            !err.to_string().contains("couldn't be read"),
            "{name} failed before its nesting was read: {err}"
        );
        assert_eq!(files_in(dir.path()), [name], "{name} left something behind");
        // Reading its facts gives some answer, and the process is still here
        // to see it.
        let _ = inspect(
            &Kiln::new(),
            &staged,
            &Options::default(),
            &CancelToken::new(),
        );
    }
}

#[test]
fn a_form_that_draws_itself_is_refused() {
    let _turn = one_pdf_at_a_time();
    let dir = tempfile::tempdir().unwrap();
    let pdf = dir.path().join("loop.pdf");
    chain_pdf(&pdf, 1, Ending::Itself);

    let err = convert_to_txt(&pdf).unwrap_err();
    assert!(matches!(err, Error::Unsafe(_)), "{err:?}");
    assert_eq!(err.to_string(), SELF_REFERENCE);
    assert_eq!(files_in(dir.path()), ["loop.pdf"], "no output was left");

    // Its facts are still read: the page count is there, and the first page
    // gives no text.
    assert_eq!(facts_of(&pdf).pdf, Some(UNREADABLE));
}

#[test]
fn forms_that_draw_each_other_are_refused() {
    let _turn = one_pdf_at_a_time();
    let dir = tempfile::tempdir().unwrap();
    let pdf = dir.path().join("pair.pdf");
    // The page draws the first form, which draws the second, which draws
    // itself.
    chain_pdf(&pdf, 2, Ending::Itself);
    let err = convert_to_txt(&pdf).unwrap_err();
    assert_eq!(err.to_string(), SELF_REFERENCE);

    // The second draws the first back.
    let mut doc = Document::with_version("1.7");
    let (a, b) = (doc.new_object_id(), doc.new_object_id());
    put_form(&mut doc, a, "Form", Some(xobjects(&[b]).into()), "/F1 Do");
    put_form(&mut doc, b, "Form", Some(xobjects(&[a]).into()), "/F1 Do");
    let pdf = dir.path().join("ring.pdf");
    one_page(doc, &pdf, xobjects(&[a]).into(), "/F1 Do");
    let err = convert_to_txt(&pdf).unwrap_err();
    assert_eq!(err.to_string(), SELF_REFERENCE);
}

#[test]
fn forms_nested_33_deep_are_refused_and_32_are_read() {
    let _turn = one_pdf_at_a_time();
    let dir = tempfile::tempdir().unwrap();

    let deep = dir.path().join("deep-33.pdf");
    chain_pdf(&deep, 33, Ending::Text);
    let err = convert_to_txt(&deep).unwrap_err();
    assert!(matches!(err, Error::Unsafe(_)), "{err:?}");
    assert_eq!(err.to_string(), SELF_REFERENCE);
    assert!(
        !dir.path().join("deep-33.txt").exists(),
        "a refused file leaves no text"
    );

    let fine = dir.path().join("deep-32.pdf");
    chain_pdf(&fine, 32, Ending::Text);
    let out = convert_to_txt(&fine).unwrap();
    let text = fs::read_to_string(&out).unwrap();
    assert!(text.contains("Deep text"), "{text:?}");
}

/// A form with no resources of its own is drawn with the page's, so it can
/// draw itself by naming itself there. Whatever the form says it is, the
/// reader draws it all the same.
#[test]
fn a_form_without_resources_that_draws_itself_is_refused() {
    let _turn = one_pdf_at_a_time();
    let dir = tempfile::tempdir().unwrap();
    for subtype in ["Form", "Image", "Unknown"] {
        let mut doc = Document::with_version("1.7");
        let form = doc.new_object_id();
        put_form(&mut doc, form, subtype, None, "/F1 Do");
        let pdf = dir.path().join(format!("{subtype}.pdf"));
        one_page(doc, &pdf, xobjects(&[form]).into(), "/F1 Do");

        let err = convert_to_txt(&pdf).unwrap_err();
        assert!(matches!(err, Error::Unsafe(_)), "{subtype}: {err:?}");
        assert_eq!(err.to_string(), SELF_REFERENCE, "{subtype}");
    }
}

/// A page that is its own ancestor in the page tree, with no resources or
/// media box of its own to find before the loop.
#[test]
fn a_page_that_is_its_own_parent_is_refused() {
    let _turn = one_pdf_at_a_time();
    let dir = tempfile::tempdir().unwrap();
    let pdf = dir.path().join("parent.pdf");
    let mut doc = Document::with_version("1.7");
    let pages = doc.new_object_id();
    let page = doc.new_object_id();
    let contents = doc.add_object(Stream::new(dictionary! {}, b"q Q".to_vec()));
    doc.objects.insert(
        page,
        Object::Dictionary(dictionary! {
            "Type" => "Page",
            "Parent" => page,
            "Contents" => contents,
        }),
    );
    doc.objects.insert(
        pages,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Count" => 1,
            "Kids" => vec![Object::Reference(page)],
        }),
    );
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
    doc.trailer.set("Root", catalog);
    doc.save(&pdf).unwrap();

    let err = convert_to_txt(&pdf).unwrap_err();
    assert!(matches!(err, Error::Unsafe(_)), "{err:?}");
    assert_eq!(err.to_string(), SELF_REFERENCE);
    assert_eq!(facts_of(&pdf).pdf, Some(UNREADABLE));
}

/// PDFs that list a form where it could loop, and never draw it that way,
/// are ordinary: some makers give every form the page's own resources, and
/// some give none.
#[test]
fn forms_that_only_share_the_pages_resources_are_read() {
    let _turn = one_pdf_at_a_time();
    let dir = tempfile::tempdir().unwrap();
    let mut doc = Document::with_version("1.7");
    let font = helvetica(&mut doc);
    let (bare, sharing) = (doc.new_object_id(), doc.new_object_id());
    // The resources every form and the page use: they name both forms and
    // the font. The shared dictionary is one object, as makers write it.
    let shared = doc.new_object_id();
    let mut resources = xobjects(&[bare, sharing]);
    resources.set("Font", dictionary! { "T1" => font });
    doc.objects.insert(shared, Object::Dictionary(resources));
    put_form(
        &mut doc,
        bare,
        "Form",
        None,
        "BT /T1 12 Tf 10 100 Td (Bare form) Tj ET",
    );
    put_form(
        &mut doc,
        sharing,
        "Form",
        Some(Object::Reference(shared)),
        "BT /T1 12 Tf 10 50 Td (Sharing form) Tj ET",
    );
    let pdf = dir.path().join("shared.pdf");
    one_page(doc, &pdf, Object::Reference(shared), "/F1 Do /F2 Do");

    let out = convert_to_txt(&pdf).unwrap();
    let text = fs::read_to_string(&out).unwrap();
    assert!(
        text.contains("Bare form") && text.contains("Sharing form"),
        "{text:?}"
    );
    assert_eq!(facts_of(&pdf).pdf.unwrap().has_text, Some(true));
}

fn convert_to_png(svg: &Path, size: Limit) -> kiln_core::Result<PathBuf> {
    let svg = svg.to_path_buf();
    on_parser(move || {
        let mut options = Options::default();
        options.quality.size = size;
        Kiln::new().convert(&svg, "png", &options)
    })
}

#[test]
fn deep_svgs_fail_one_job() {
    for name in ["svg-deep-500.svg", "svg-deep-1000.svg", "svg-deep-3000.svg"] {
        let (dir, staged) = stage(&fixture(&format!("hostile/{name}")));
        let err = convert_to_png(&staged, Limit::Original).expect_err(&format!("{name} converted"));
        assert!(matches!(err, Error::Unsafe(_)), "{name}: {err:?}");
        assert_eq!(err.to_string(), SVG_TOO_DEEP, "{name}");
        assert_eq!(files_in(dir.path()), [name], "{name} left something behind");

        // Its facts are still read: it has none, and the process is still
        // here to see that.
        let facts = inspect(
            &Kiln::new(),
            &staged,
            &Options::default(),
            &CancelToken::new(),
        );
        assert!(facts.is_some_and(|f| f.picture.is_none()), "{name}");
    }
}

#[test]
fn a_huge_svg_is_drawn_at_50_megapixels() {
    let (dir, staged) = stage(&fixture("hostile/svg-huge.svg"));
    let started = Instant::now();
    let out = convert_to_png(&staged, Limit::Original).expect("the huge SVG converted");
    let took = started.elapsed();
    assert!(took < Duration::from_secs(30), "took {took:?}");

    let (w, h) = image::ImageReader::open(&out)
        .unwrap()
        .into_dimensions()
        .unwrap();
    assert!(w as u64 * h as u64 <= 50_000_000, "{w} x {h}");
    assert!(w.min(h) > 7000, "{w} x {h} is far under the cap");
    assert_eq!(files_in(dir.path()).len(), 2, "the SVG and its PNG");
}

#[test]
fn heavy_svg_effects_fail_one_job() {
    let (dir, staged) = stage(&fixture("hostile/svg-filter.svg"));
    let started = Instant::now();
    let err = convert_to_png(&staged, Limit::Original).unwrap_err();
    let took = started.elapsed();
    assert!(took < Duration::from_secs(10), "took {took:?}");
    assert!(matches!(err, Error::Unsafe(_)), "{err:?}");
    assert_eq!(err.to_string(), SVG_TOO_HEAVY);
    assert_eq!(files_in(dir.path()), ["svg-filter.svg"]);
}

#[test]
fn an_ordinary_filter_still_draws() {
    let dir = tempfile::tempdir().unwrap();
    let svg = dir.path().join("blur.svg");
    fs::write(
        &svg,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="2000" height="2000"><filter id="f"><feGaussianBlur stdDeviation="5"/></filter><rect width="2000" height="2000" fill="blue" filter="url(#f)"/></svg>"##,
    )
    .unwrap();
    let out = convert_to_png(&svg, Limit::Original).expect("a blurred rect converts");
    let (w, h) = image::ImageReader::open(&out)
        .unwrap()
        .into_dimensions()
        .unwrap();
    assert_eq!((w, h), (2000, 2000));
}

/// An SVG that holds `inner`, another SVG, as a data: image.
fn svg_holding(inner: &str) -> String {
    let inner = inner.replace('<', "&lt;").replace('>', "&gt;");
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><image width="20" height="20" href="data:image/svg+xml;utf8,{inner}"/></svg>"#
    )
}

#[test]
fn a_deep_svg_inside_an_image_fails_one_job() {
    let dir = tempfile::tempdir().unwrap();
    let inner = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='20' height='20'>{}<rect width='20' height='20' fill='red'/>{}</svg>",
        "<g>".repeat(300),
        "</g>".repeat(300)
    );
    let svg = dir.path().join("hidden.svg");
    fs::write(&svg, svg_holding(&inner)).unwrap();
    let err = convert_to_png(&svg, Limit::Original).unwrap_err();
    assert!(matches!(err, Error::Unsafe(_)), "{err:?}");
    assert_eq!(err.to_string(), SVG_TOO_DEEP);
    assert_eq!(files_in(dir.path()), ["hidden.svg"]);
}

#[test]
fn an_ordinary_svg_inside_an_image_still_draws() {
    let dir = tempfile::tempdir().unwrap();
    let inner = "<svg xmlns='http://www.w3.org/2000/svg' width='20' height='20'><g><g><rect width='20' height='20' fill='red'/></g></g></svg>";
    let svg = dir.path().join("holder.svg");
    fs::write(&svg, svg_holding(inner)).unwrap();
    let out = convert_to_png(&svg, Limit::Original).expect("an SVG in an image converts");
    let img = image::open(out).unwrap().to_rgba8();
    assert_eq!(img.get_pixel(10, 10).0, [255, 0, 0, 255]);
}

/// Writes `xml` to `name` in a fresh folder and converts it to PNG.
fn convert_text(name: &str, xml: &str) -> (tempfile::TempDir, kiln_core::Result<PathBuf>) {
    let dir = tempfile::tempdir().unwrap();
    let svg = dir.path().join(name);
    fs::write(&svg, xml).unwrap();
    let result = convert_to_png(&svg, Limit::Original);
    (dir, result)
}

#[test]
fn a_chain_of_masks_fails_one_job() {
    // 3 levels deep as written, 100,000 as drawn: each mask holds a rect that
    // is masked by the next.
    let links = 50_000;
    let mut xml =
        String::from(r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><defs>"#);
    for n in 0..links {
        xml += &format!(
            r##"<mask id="m{n}"><rect width="10" height="10" fill="white" mask="url(#m{})"/></mask>"##,
            n + 1
        );
    }
    xml += &format!(
        r#"<mask id="m{links}"><rect width="10" height="10" fill="white"/></mask></defs>"#
    );
    xml += r##"<rect width="10" height="10" mask="url(#m0)"/></svg>"##;
    let (dir, result) = convert_text("masks.svg", &xml);
    let err = result.unwrap_err();
    assert!(matches!(err, Error::Unsafe(_)), "{err:?}");
    assert_eq!(err.to_string(), SVG_TOO_DEEP);
    assert_eq!(files_in(dir.path()), ["masks.svg"]);
}

#[test]
fn an_entity_holding_deep_markup_fails_one_job() {
    // The comment is for the check that a file looks like an SVG, which reads
    // only its first 4,096 bytes and finds the entity there.
    let deep = "<g>".repeat(200_000) + &"</g>".repeat(200_000);
    let xml = format!(
        r#"<?xml version="1.0"?><!-- <svg> --><!DOCTYPE svg [<!ENTITY deep "{deep}">]><svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">&deep;</svg>"#
    );
    let (dir, result) = convert_text("entity.svg", &xml);
    let err = result.unwrap_err();
    assert!(matches!(err, Error::Unsafe(_)), "{err:?}");
    assert_eq!(err.to_string(), SVG_TOO_DEEP);
    assert_eq!(files_in(dir.path()), ["entity.svg"]);
}

#[test]
fn a_turbulence_of_a_hundred_thousand_octaves_fails_fast() {
    let xml = r#"<svg xmlns="http://www.w3.org/2000/svg" width="500" height="500"><filter id="f"><feTurbulence baseFrequency="0.05" numOctaves="100000"/></filter><rect width="500" height="500" filter="url(#f)"/></svg>"#;
    let started = Instant::now();
    let (dir, result) = convert_text("noise.svg", xml);
    let err = result.unwrap_err();
    assert!(matches!(err, Error::Unsafe(_)), "{err:?}");
    assert_eq!(err.to_string(), SVG_TOO_HEAVY);
    assert_eq!(files_in(dir.path()), ["noise.svg"]);
    // Its preview is a drawing too, and is refused as quickly.
    let facts = facts_of(&dir.path().join("noise.svg"));
    assert!(facts.picture.is_none());
    let took = started.elapsed();
    assert!(took < Duration::from_secs(10), "took {took:?}");
}

#[test]
fn an_ordinary_turbulence_still_draws() {
    let xml = r#"<svg xmlns="http://www.w3.org/2000/svg" width="500" height="500"><filter id="f"><feTurbulence baseFrequency="0.05" numOctaves="4"/></filter><rect width="500" height="500" filter="url(#f)"/></svg>"#;
    let (_dir, result) = convert_text("noise.svg", xml);
    let out = result.expect("a turbulence of 4 octaves converts");
    let (w, h) = image::ImageReader::open(&out)
        .unwrap()
        .into_dimensions()
        .unwrap();
    assert_eq!((w, h), (500, 500));
}

/// A radius of 10 on a 2,000-pixel square reads 441 pixels for each of 5.76
/// million: the effects limit lets it through, and it converts. The reads
/// take about a minute in a build without optimisations, so this is run on
/// request: `cargo test -p kiln-core --test hostile -- --ignored`.
#[test]
#[ignore = "about a minute in a debug build"]
fn a_wide_morphology_on_a_large_drawing_converts() {
    let xml = r#"<svg xmlns="http://www.w3.org/2000/svg" width="2000" height="2000"><filter id="f"><feMorphology radius="10"/></filter><rect width="2000" height="2000" fill="blue" filter="url(#f)"/></svg>"#;
    let (_dir, result) = convert_text("morphology.svg", xml);
    let out = result.expect("a wide morphology converts");
    let (w, h) = image::ImageReader::open(&out)
        .unwrap()
        .into_dimensions()
        .unwrap();
    assert_eq!((w, h), (2000, 2000));
}

#[test]
fn a_declaration_holding_its_own_end_does_not_hide_a_deep_file() {
    // `?>` in the quotes of an encoding is not where the declaration ends.
    let deep = "<g>".repeat(1_000_000) + &"</g>".repeat(1_000_000);
    let xml = format!(
        r#"<?xml version="1.0" encoding="?>"?><!DOCTYPE svg><svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">{deep}</svg>"#
    );
    let (dir, result) = convert_text("declaration.svg", &xml);
    let err = result.unwrap_err();
    assert!(matches!(err, Error::Unsafe(_)), "{err:?}");
    assert_eq!(err.to_string(), SVG_TOO_DEEP);
    assert_eq!(files_in(dir.path()), ["declaration.svg"]);
}

/// A GIF that is only a header: a canvas of `side` x `side` pixels, and an
/// image block as big as the canvas with no pixels behind it. It is a few
/// dozen bytes.
fn gif_claiming(side: u16) -> Vec<u8> {
    let mut gif = b"GIF89a".to_vec();
    gif.extend(side.to_le_bytes()); // canvas width
    gif.extend(side.to_le_bytes()); // canvas height
    gif.extend([0, 0, 0]); // no palette, background 0, square pixels
    gif.push(0x2C); // an image block
    gif.extend([0, 0, 0, 0]); // at the corner
    gif.extend(side.to_le_bytes());
    gif.extend(side.to_le_bytes());
    gif.push(0); // no local palette
    gif
}

#[test]
fn a_gif_claiming_a_huge_canvas_fails_one_job() {
    let dir = tempfile::tempdir().unwrap();
    let gif = dir.path().join("huge.gif");
    // 30,000 x 30,000 pixels is 3.6 GB of canvas, over the memory limit.
    fs::write(&gif, gif_claiming(30_000)).unwrap();
    let (job, probe) = (gif.clone(), gif.clone());

    let (result, largest) = on_parser(move || {
        LARGEST.with(|largest| largest.set(0));
        let result = Kiln::new().convert(&job, "webp", &Options::default());
        (result, LARGEST.with(Cell::get))
    });
    let err = result.expect_err("a GIF with no pixels converted");
    assert!(
        err.to_string().to_lowercase().contains("memory"),
        "not a memory error: {err}"
    );
    assert_eq!(files_in(dir.path()), ["huge.gif"]);
    // The canvas is 3.6 GB; nothing near a gigabyte was asked for.
    assert!(largest < 1 << 30, "a single allocation of {largest} bytes");

    // Reading its facts reserves nothing either.
    let largest = on_parser(move || {
        LARGEST.with(|largest| largest.set(0));
        let _ = inspect(
            &Kiln::new(),
            &probe,
            &Options::default(),
            &CancelToken::new(),
        );
        LARGEST.with(Cell::get)
    });
    assert!(largest < 1 << 30, "a single allocation of {largest} bytes");
}

/// Make smaller on `pdf` to `max` bytes, on a parser thread as the app runs
/// jobs, with its job folder outside the PDF's own. Returns what it did and
/// the largest single allocation it asked for.
fn make_smaller(pdf: &Path, max: u64) -> (kiln_core::Result<Vec<PathBuf>>, usize) {
    let pdf = pdf.to_path_buf();
    let work = tempfile::tempdir().unwrap();
    let opts = Options {
        temp_root: Some(work.path().join("work")),
        tool: kiln_core::ToolOptions {
            max_bytes: Some(max),
            ..Default::default()
        },
        ..Options::default()
    };
    on_parser(move || {
        LARGEST.with(|largest| largest.set(0));
        let result = Kiln::new().run_tool(kiln_core::Tool::Smaller, &[pdf], &opts);
        (result, LARGEST.with(Cell::get))
    })
}

#[test]
fn make_smaller_fails_one_job_on_deep_pdfs() {
    for name in ["deep-2000.pdf", "deep-30000.pdf"] {
        let (dir, staged) = stage(&fixture(&format!("hostile/{name}")));
        let (result, _) = make_smaller(&staged, 1_000);
        let err = result.expect_err(&format!("{name} was made smaller"));
        assert!(
            !matches!(err, Error::ToolUnavailable(_)),
            "{name}: Make smaller never ran: {err}"
        );
        assert_eq!(files_in(dir.path()), [name], "{name} left something behind");
    }
}

/// Make smaller never draws a page, so forms that draw themselves are only
/// carried: it finishes, and here says how small it could get.
#[test]
fn make_smaller_carries_forms_that_draw_themselves() {
    let dir = tempfile::tempdir().unwrap();
    let pdf = dir.path().join("loop.pdf");
    chain_pdf(&pdf, 1, Ending::Itself);
    let started = Instant::now();
    let (result, _) = make_smaller(&pdf, 100);
    assert!(
        matches!(result, Err(Error::OutOfReach { max_bytes: 100, .. })),
        "{result:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(30));
    assert_eq!(files_in(dir.path()), ["loop.pdf"]);
}

/// An image whose pixels are `data`, Flate-compressed, claiming `w` x `h`.
fn flate_image(doc: &mut Document, w: i64, h: i64, data: &[u8]) -> ObjectId {
    use std::io::Write;
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    enc.write_all(data).unwrap();
    doc.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => w,
            "Height" => h,
            "ColorSpace" => "DeviceRGB",
            "BitsPerComponent" => 8,
            "Filter" => "FlateDecode",
        },
        enc.finish().unwrap(),
    ))
}

/// A PDF whose photos claim more than their bytes: one that inflates to
/// 64 MB where its 400 x 300 pixels need 360 KB, one claiming 30,000 x
/// 30,000 pixels in a few bytes, and a JPG whose frame claims 60,000 x
/// 60,000. Make smaller leaves them as they are, asking for no more memory
/// than the pictures it can read need.
#[test]
fn make_smaller_reads_no_more_of_a_photo_than_it_claims() {
    let dir = tempfile::tempdir().unwrap();
    let pdf = dir.path().join("bombs.pdf");
    let mut doc = Document::with_version("1.7");
    let bomb = flate_image(&mut doc, 400, 300, &vec![0u8; 64 << 20]);
    let huge = flate_image(&mut doc, 30_000, 30_000, &[0u8; 300]);
    // A real 8 x 8 JPG, its frame header changed to say 60,000 x 60,000.
    let mut jpg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 90)
        .encode(&[128u8; 8 * 8 * 3], 8, 8, image::ExtendedColorType::Rgb8)
        .unwrap();
    let sof = jpg
        .windows(2)
        .position(|w| w == [0xFF, 0xC0])
        .expect("a baseline frame");
    jpg[sof + 5..sof + 9].copy_from_slice(&[0xEA, 0x60, 0xEA, 0x60]);
    let tall = doc.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => 60_000,
            "Height" => 60_000,
            "ColorSpace" => "DeviceRGB",
            "BitsPerComponent" => 8,
            "Filter" => "DCTDecode",
        },
        jpg,
    ));
    let mut named = Dictionary::new();
    for (i, id) in [bomb, huge, tall].into_iter().enumerate() {
        named.set(format!("Im{i}"), id);
    }
    one_page(
        doc,
        &pdf,
        dictionary! { "XObject" => named }.into(),
        "q 100 0 0 100 0 0 cm /Im0 Do /Im1 Do /Im2 Do Q",
    );

    // Out of reach, so the smallest step is tried, which reads every
    // photo. With no photo it can read, the tidy is all it has, and here
    // that is no smaller than the file.
    let (result, largest) = make_smaller(&pdf, 1_000);
    let err = result.expect_err("photos it can't read were made smaller");
    assert!(
        matches!(
            &err,
            Error::OutOfReach {
                max_bytes: 1_000,
                ..
            } | Error::NothingToDo(_)
        ),
        "{err:?}"
    );
    assert_eq!(files_in(dir.path()), ["bombs.pdf"]);
    assert!(largest < 32 << 20, "a single allocation of {largest} bytes");
}

/// A JPG photo whose frame says 4,000 x 3,000, as its dictionary does,
/// over the data of an 8 x 8 picture. It is within the size Kiln reads, so
/// it reaches the decoder, which asks for no more than that picture needs.
#[test]
fn make_smaller_decodes_a_lying_jpg_within_its_claim() {
    let dir = tempfile::tempdir().unwrap();
    let pdf = dir.path().join("liar.pdf");
    let mut doc = Document::with_version("1.7");
    let mut jpg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 90)
        .encode(&[128u8; 8 * 8 * 3], 8, 8, image::ExtendedColorType::Rgb8)
        .unwrap();
    let sof = jpg
        .windows(2)
        .position(|w| w == [0xFF, 0xC0])
        .expect("a baseline frame");
    // Height 3000, then width 4000.
    jpg[sof + 5..sof + 9].copy_from_slice(&[0x0B, 0xB8, 0x0F, 0xA0]);
    let liar = doc.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => 4_000,
            "Height" => 3_000,
            "ColorSpace" => "DeviceRGB",
            "BitsPerComponent" => 8,
            "Filter" => "DCTDecode",
        },
        jpg,
    ));
    one_page(
        doc,
        &pdf,
        dictionary! { "XObject" => dictionary! { "Im0" => liar } }.into(),
        "q 100 0 0 100 0 0 cm /Im0 Do Q",
    );

    let (result, largest) = make_smaller(&pdf, 1_000);
    let err = result.expect_err("a picture of nothing was made smaller");
    assert!(
        matches!(
            &err,
            Error::OutOfReach {
                max_bytes: 1_000,
                ..
            } | Error::NothingToDo(_)
        ),
        "{err:?}"
    );
    assert_eq!(files_in(dir.path()), ["liar.pdf"]);
    // Four bytes a pixel at most: the decoder's own buffer.
    let picture = 4_000 * 3_000 * 4;
    assert!(largest <= picture, "a single allocation of {largest} bytes");
}
