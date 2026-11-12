//! PDF, in process: text out of a PDF, a PDF's pages as images, images
//! into a PDF, and the PDF tools (merge, split and make smaller).
//!
//! Text comes from `pdf-extract`. Pages are drawn by the PDF renderer
//! built into Windows (Windows.Data.Pdf), so no engine has to be
//! installed. Everything that writes a PDF uses `lopdf`.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use image::{DynamicImage, GenericImageView};
use lopdf::content::Content;
use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};

use crate::converter::{CancelToken, Context, Converter, Progress, Request};
use crate::error::{Error, Result};
use crate::raster;

pub struct PdfConverter;

/// Images placed in a PDF are taken to be this many pixels per inch.
const IMAGE_DPI: f32 = 96.0;

impl Converter for PdfConverter {
    fn id(&self) -> &'static str {
        "pdf"
    }

    fn convert(&self, req: &Request<'_>, ctx: &mut Context<'_>) -> Result<()> {
        match (req.from.ext.as_str(), req.to.ext.as_str()) {
            ("pdf", "txt") => {
                (ctx.progress)(Progress::Indeterminate);
                let text = extract_text(req.input)?;
                fs::write(req.output, text).map_err(|e| Error::io(req.output, e))
            }
            ("pdf", to) => render_pages(req, to, ctx),
            (from, "pdf") => {
                (ctx.progress)(Progress::Indeterminate);
                let mut doc = Document::with_version("1.7");
                let fallback = raster::Fallback {
                    options: req.options,
                    work: req.output.parent().unwrap_or(Path::new(".")),
                };
                let page = image_page(&mut doc, req.input, from, ctx.cancel, Some(fallback))?;
                finish(&mut doc, vec![page]);
                save(&mut doc, req.output)
            }
            _ => Err(Error::Registry(format!(
                "Kiln can't convert {} to {}.",
                req.from.code(),
                req.to.code()
            ))),
        }
    }
}

fn failed(summary: impl Into<String>) -> Error {
    Error::EngineFailed {
        engine: "Kiln",
        summary: summary.into(),
        details: String::new(),
    }
}

fn read(input: &Path) -> Result<Vec<u8>> {
    let bytes = fs::read(input).map_err(|e| Error::io(input, e))?;
    if !bytes.starts_with(b"%PDF")
        && !bytes[..bytes.len().min(1024)]
            .windows(4)
            .any(|w| w == b"%PDF")
    {
        return Err(Error::WrongContent {
            path: input.to_path_buf(),
            code: "PDF".to_owned(),
        });
    }
    Ok(bytes)
}

fn load(input: &Path) -> Result<Document> {
    let bytes = read(input)?;
    let doc = Document::load_mem(&bytes).map_err(|e| failed(load_reason(&e.to_string())))?;
    // lopdf opens a PDF that has only an owner password with the empty user
    // password and then drops its /Encrypt entry, so `is_encrypted` is false
    // for it. The tools would write it out with its restrictions gone, so it
    // is turned away like one that needs a password.
    if doc.is_encrypted() || doc.was_encrypted() {
        return Err(failed("the PDF is protected with a password."));
    }
    if doc.get_pages().is_empty() {
        return Err(failed("the PDF has no pages."));
    }
    Ok(doc)
}

fn load_reason(e: &str) -> String {
    if e.to_ascii_lowercase().contains("decrypt") || e.to_ascii_lowercase().contains("password") {
        "the PDF is protected with a password.".to_owned()
    } else {
        format!("the PDF couldn't be read ({e}).")
    }
}

/// The PDF's text, page by page. Scanned pages have none.
fn extract_text(input: &Path) -> Result<String> {
    let bytes = read(input)?;
    // pdf-extract panics on some malformed files; that is a failed
    // conversion, not a crash.
    let pages = std::panic::catch_unwind(|| -> Result<Vec<String>> {
        // pdf-extract draws the forms a page draws with no limit, so a PDF
        // whose forms draw themselves is turned away before it starts.
        let doc = Document::load_mem(&bytes).map_err(|e| failed(load_reason(&e.to_string())))?;
        if doc.is_encrypted() {
            return Err(failed("the PDF is protected with a password."));
        }
        forms_are_safe(&doc)?;
        drop(doc);
        pdf_extract::extract_text_from_mem_by_pages(&bytes)
            .map_err(|e| failed(load_reason(&e.to_string())))
    })
    .map_err(|_| failed("the PDF's text couldn't be read."))??;
    let text = tidy_text(&pages);
    if text.trim().is_empty() {
        return Err(failed(
            "the PDF has no text to take out. Its pages may be scanned images.",
        ));
    }
    Ok(text)
}

/// Whether any of the first `pages` pages of `doc` draws text, so a scan
/// can be told from a document. Only those pages are read; reading stops
/// at the first page that can't be, as pdf-extract's own page loop does.
/// A PDF whose forms draw themselves has no text Kiln can read, so it has
/// none here.
pub(crate) fn text_in_first(doc: &Document, pages: u32) -> Result<bool> {
    let all = doc.get_pages();
    let first = (1..=pages).filter_map(|n| all.get(&n).copied());
    if check_forms(doc, first).is_err() {
        return Ok(false);
    }
    // pdf-extract panics on some malformed files, as above.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        for n in 1..=pages {
            let mut text = String::new();
            let read = {
                let mut out = pdf_extract::PlainTextOutput::new(&mut text);
                pdf_extract::output_doc_page(doc, &mut out, n)
            };
            if read.is_err() {
                break;
            }
            if !text.trim().is_empty() {
                return true;
            }
        }
        false
    }))
    .map_err(|_| failed("the PDF's text couldn't be read."))
}

/// Forms drawn inside forms may go this deep, counting the one a page
/// draws. Real documents stop at a handful.
pub(crate) const MAX_FORM_DEPTH: usize = 32;

/// How far up the page tree a page's inherited entries are looked for.
const MAX_PARENTS: usize = 64;

fn self_reference() -> Error {
    Error::Unsafe("This PDF's pages refer to themselves, so Kiln can't read its text.".to_owned())
}

/// Whether the forms the pages of `doc` draw end, so pdf-extract, which
/// draws a form by calling itself for it with no limit, can read them.
/// `Err` when a form draws itself, directly or through others, or forms
/// are nested more than [`MAX_FORM_DEPTH`] deep, or a page is its own
/// ancestor in the page tree, which pdf-extract also follows without end.
pub(crate) fn forms_are_safe(doc: &Document) -> Result<()> {
    check_forms(doc, doc.get_pages().into_values())
}

/// [`forms_are_safe`] for just `pages`.
fn check_forms(doc: &Document, pages: impl IntoIterator<Item = ObjectId>) -> Result<()> {
    let mut walk = FormWalk {
        doc,
        drawn: HashMap::new(),
        walked: HashMap::new(),
    };
    for page in pages {
        walk.page(page)?;
    }
    Ok(())
}

/// One step of the walk, which keeps its own stack so a hostile file can
/// make it long but never deep.
enum Step<'a> {
    /// Draw `form` at `depth`; it takes `caller`'s resources if it has none.
    Draw {
        form: ObjectId,
        caller: &'a Dictionary,
        depth: usize,
    },
    /// Everything `form` draws has been walked.
    Leave(ObjectId),
}

struct FormWalk<'a> {
    doc: &'a Document,
    /// The names each form's content draws, read once.
    drawn: HashMap<ObjectId, Vec<Vec<u8>>>,
    /// Forms already walked without trouble, with the resources they were
    /// drawn with, and the deepest level they were walked at. Reaching one
    /// again no deeper than that needs no second walk, so forms shared by
    /// many others are walked once, not once for every way to reach them.
    walked: HashMap<(ObjectId, *const Dictionary), usize>,
}

impl<'a> FormWalk<'a> {
    fn page(&mut self, page: ObjectId) -> Result<()> {
        let doc = self.doc;
        let Some(resources) = page_resources(doc, page)? else {
            return Ok(());
        };
        let content = doc.get_page_content(page).unwrap_or_default();
        let mut stack = Vec::new();
        push_forms(doc, &drawn_names(&content), resources, 1, &mut stack);
        let mut on_path = HashSet::new();
        while let Some(step) = stack.pop() {
            let (form, caller, depth) = match step {
                Step::Leave(form) => {
                    on_path.remove(&form);
                    continue;
                }
                Step::Draw {
                    form,
                    caller,
                    depth,
                } => (form, caller, depth),
            };
            if depth > MAX_FORM_DEPTH || on_path.contains(&form) {
                return Err(self_reference());
            }
            let Some(stream) = doc.get_object(form).ok().and_then(|o| o.as_stream().ok()) else {
                continue;
            };
            // As pdf-extract does: a form without resources of its own is
            // drawn with those of whatever draws it.
            let resources = dict_entry(doc, &stream.dict, b"Resources").unwrap_or(caller);
            let key = (form, std::ptr::from_ref(resources));
            if self
                .walked
                .get(&key)
                .is_some_and(|&deepest| depth <= deepest)
            {
                continue;
            }
            self.walked.insert(key, depth);
            on_path.insert(form);
            stack.push(Step::Leave(form));
            let names = self
                .drawn
                .entry(form)
                .or_insert_with(|| drawn_names(&stream_content(stream)));
            push_forms(doc, names, resources, depth + 1, &mut stack);
        }
        Ok(())
    }
}

/// The entry `key` of `dict` when it is a dictionary, following a reference.
fn dict_entry<'a>(doc: &'a Document, dict: &'a Dictionary, key: &[u8]) -> Option<&'a Dictionary> {
    let (_, object) = doc.dereference(dict.get(key).ok()?).ok()?;
    object.as_dict().ok()
}

/// The resources `page` is drawn with: its own, or the nearest ancestor's.
/// `Err` when the page tree above it loops or is more than `MAX_PARENTS`
/// deep. The whole way up is checked, as pdf-extract also looks for the
/// page's media box there.
fn page_resources(doc: &Document, page: ObjectId) -> Result<Option<&Dictionary>> {
    let mut found = None;
    let mut seen = Vec::new();
    let mut id = page;
    loop {
        if seen.contains(&id) || seen.len() >= MAX_PARENTS {
            return Err(self_reference());
        }
        seen.push(id);
        let Ok(node) = doc.get_dictionary(id) else {
            break;
        };
        if found.is_none() {
            found = dict_entry(doc, node, b"Resources");
        }
        match node.get(b"Parent").and_then(Object::as_reference) {
            Ok(parent) => id = parent,
            Err(_) => break,
        }
    }
    Ok(found)
}

/// Pushes a step for each of `names` that `resources` names a stream for,
/// all at `depth`.
fn push_forms<'a>(
    doc: &'a Document,
    names: &[Vec<u8>],
    resources: &'a Dictionary,
    depth: usize,
    stack: &mut Vec<Step<'a>>,
) {
    let Some(xobjects) = dict_entry(doc, resources, b"XObject") else {
        return;
    };
    for name in names {
        let Ok(object) = xobjects.get(name) else {
            continue;
        };
        if let Ok((Some(form), Object::Stream(_))) = doc.dereference(object) {
            stack.push(Step::Draw {
                form,
                caller: resources,
                depth,
            });
        }
    }
}

/// A stream's content with its filters undone, as pdf-extract reads it.
fn stream_content(stream: &Stream) -> Vec<u8> {
    if stream.filters().is_ok() {
        stream
            .decompressed_content()
            .unwrap_or_else(|_| stream.content.clone())
    } else {
        stream.content.clone()
    }
}

/// The names `content` draws with the `Do` operator, each once.
fn drawn_names(content: &[u8]) -> Vec<Vec<u8>> {
    // Most content draws nothing, and is not worth parsing to find out.
    if !content.windows(2).any(|w| w == b"Do") {
        return Vec::new();
    }
    let Ok(content) = Content::decode(content) else {
        return Vec::new();
    };
    let names: BTreeSet<Vec<u8>> = content
        .operations
        .iter()
        .filter(|op| op.operator == "Do")
        .filter_map(|op| op.operands.first()?.as_name().ok())
        .map(<[u8]>::to_vec)
        .collect();
    names.into_iter().collect()
}

/// Pages joined by a blank line, with trailing spaces gone and runs of
/// blank lines cut to one.
fn tidy_text(pages: &[String]) -> String {
    let mut out = String::new();
    for page in pages {
        let mut blank = true;
        for line in page.lines() {
            let line = line.trim_end();
            if line.is_empty() {
                if !blank {
                    out.push('\n');
                }
                blank = true;
            } else {
                out.push_str(line);
                out.push('\n');
                blank = false;
            }
        }
        if !blank {
            out.push('\n');
        }
    }
    let trimmed = out.trim_end().len();
    out.truncate(trimmed);
    out.push('\n');
    out
}

/// Draw every page to a numbered image of type `to`.
fn render_pages(req: &Request<'_>, to: &str, ctx: &mut Context<'_>) -> Result<()> {
    read(req.input)?;
    let quality = req.options.jpeg_quality;
    let mut n = 0;
    let dpi = req.options.quality.page_dpi.clamp(36, 600) as f32;
    winpdf::render(req.input, dpi, ctx, |png, page, count, ctx| {
        let img = image::load_from_memory_with_format(png, image::ImageFormat::Png)
            .map_err(|e| failed(format!("a page couldn't be drawn ({e}).")))?;
        n = page + 1;
        raster::encode(&img, &Request::numbered(req.output, n), to, quality, None)?;
        (ctx.progress)(Progress::Fraction(n as f32 / count as f32));
        Ok(())
    })?;
    if n == 0 {
        return Err(failed("the PDF has no pages."));
    }
    Ok(())
}

#[cfg(windows)]
mod winpdf {
    use std::path::Path;

    use windows::core::HSTRING;
    use windows::Data::Pdf::{PdfDocument, PdfPageRenderOptions};
    use windows::Storage::StorageFile;
    use windows::Storage::Streams::{DataReader, InMemoryRandomAccessStream};
    use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

    use super::failed;
    use crate::converter::Context;
    use crate::error::{Error, Result};

    fn winrt(what: &str, e: windows::core::Error) -> Error {
        let msg = e.message();
        let lower = msg.to_ascii_lowercase();
        if lower.contains("password") {
            failed("the PDF is protected with a password.")
        } else if msg.is_empty() {
            failed(format!("Windows couldn't {what} (0x{:08X}).", e.code().0))
        } else {
            failed(format!(
                "Windows couldn't {what}: {}",
                msg.trim_end_matches('.')
            ))
        }
    }

    /// Calls `page(png, index, count, ctx)` for each page, drawn on white
    /// at `dpi`.
    pub fn render(
        input: &Path,
        dpi: f32,
        ctx: &mut Context<'_>,
        mut page: impl FnMut(&[u8], usize, usize, &mut Context<'_>) -> Result<()>,
    ) -> Result<()> {
        // Worker threads aren't set up for WinRT; joining the
        // multithreaded apartment is harmless if they already are.
        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
        }
        let open = || -> windows::core::Result<PdfDocument> {
            let file =
                StorageFile::GetFileFromPathAsync(&HSTRING::from(input.as_os_str()))?.join()?;
            PdfDocument::LoadFromFileAsync(&file)?.join()
        };
        let doc = open().map_err(|e| winrt("open the PDF", e))?;
        let count = doc.PageCount().map_err(|e| winrt("read the PDF", e))? as usize;
        for i in 0..count {
            if ctx.cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let draw = || -> windows::core::Result<Vec<u8>> {
                let p = doc.GetPage(i as u32)?;
                let size = p.Size()?;
                let opts = PdfPageRenderOptions::new()?;
                // Sizes are in DIPs, 96 to the inch.
                let width = (size.Width * dpi / 96.0).round().clamp(1.0, 10_000.0);
                opts.SetDestinationWidth(width as u32)?;
                opts.SetBackgroundColor(windows::UI::Color {
                    A: 255,
                    R: 255,
                    G: 255,
                    B: 255,
                })?;
                let stream = InMemoryRandomAccessStream::new()?;
                p.RenderWithOptionsToStreamAsync(&stream, &opts)?.join()?;
                let len = stream.Size()? as u32;
                let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
                reader.LoadAsync(len)?.join()?;
                let mut buf = vec![0u8; len as usize];
                reader.ReadBytes(&mut buf)?;
                Ok(buf)
            };
            let png = draw().map_err(|e| winrt(&format!("draw page {}", i + 1), e))?;
            page(&png, i, count, ctx)?;
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod winpdf {
    use std::path::Path;

    use super::failed;
    use crate::converter::Context;
    use crate::error::Result;

    pub fn render(
        _input: &Path,
        _dpi: f32,
        _ctx: &mut Context<'_>,
        _page: impl FnMut(&[u8], usize, usize, &mut Context<'_>) -> Result<()>,
    ) -> Result<()> {
        Err(failed(
            "drawing PDF pages needs the PDF renderer in Windows.",
        ))
    }
}

/// The colour components of a baseline or progressive JPEG, from its
/// start-of-frame marker.
fn jpeg_components(data: &[u8]) -> Option<u8> {
    jpeg_frame(data).map(|(_, _, n)| n)
}

/// A JPEG's width, height and colour components, from its start-of-frame
/// marker.
fn jpeg_frame(data: &[u8]) -> Option<(u32, u32, u8)> {
    let mut i = 2;
    while i + 9 < data.len() {
        if data[i] != 0xFF {
            return None;
        }
        let marker = data[i + 1];
        let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
        if matches!(marker, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
            let height = u16::from_be_bytes([data[i + 5], data[i + 6]]);
            let width = u16::from_be_bytes([data[i + 7], data[i + 8]]);
            return Some((u32::from(width), u32::from(height), data[i + 9]));
        }
        i += 2 + len;
    }
    None
}

fn deflate(data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    enc.write_all(data).expect("writing to memory");
    enc.finish().expect("writing to memory")
}

/// Add one page showing the image at `input`, sized at 96 pixels per inch.
/// JPGs that need no turning are placed as they are, without re-encoding;
/// one with an EXIF orientation is placed turned upright, as pixels. An
/// SVG's drawing stops if `cancel` is set. FFmpeg reads a HEIC or AVIF
/// that Windows can't, as [`raster::decode_with`] says, when `fallback`
/// is given.
fn image_page(
    doc: &mut Document,
    input: &Path,
    ext: &str,
    cancel: &CancelToken,
    fallback: Option<raster::Fallback<'_>>,
) -> Result<ObjectId> {
    let raster::Decoded {
        image: img,
        orientation,
        ..
    } = raster::decode_with(input, ext, cancel, fallback)?;
    let (w, h) = img.dimensions();
    let original = if ext == "jpg" && orientation == image::metadata::Orientation::NoTransforms {
        fs::read(input).ok()
    } else {
        None
    };
    let image = match original
        .as_deref()
        .and_then(|d| Some((d, jpeg_components(d)?)))
    {
        Some((data, n @ (1 | 3))) => Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => w as i64,
                "Height" => h as i64,
                "ColorSpace" => if n == 1 { "DeviceGray" } else { "DeviceRGB" },
                "BitsPerComponent" => 8,
                "Filter" => "DCTDecode",
            },
            data.to_vec(),
        ),
        _ => flate_image(doc, &img),
    };
    let image_id = doc.add_object(image);

    let (pw, ph) = (w as f32 * 72.0 / IMAGE_DPI, h as f32 * 72.0 / IMAGE_DPI);
    let content = format!("q {pw:.3} 0 0 {ph:.3} 0 0 cm /Im0 Do Q");
    let content_id = doc.add_object(Stream::new(dictionary! {}, content.into_bytes()));
    Ok(doc.add_object(dictionary! {
        "Type" => "Page",
        "MediaBox" => vec![0.into(), 0.into(), Object::Real(pw), Object::Real(ph)],
        "Contents" => content_id,
        "Resources" => dictionary! {
            "XObject" => dictionary! { "Im0" => image_id },
        },
    }))
}

/// A lossless image: 8-bit gray or RGB, with transparency as a soft mask.
fn flate_image(doc: &mut Document, img: &DynamicImage) -> Stream {
    let (w, h) = img.dimensions();
    let gray = !img.color().has_color();
    let (pixels, space) = if gray {
        (img.to_luma8().into_raw(), "DeviceGray")
    } else {
        (img.to_rgb8().into_raw(), "DeviceRGB")
    };
    let mut dict = dictionary! {
        "Type" => "XObject",
        "Subtype" => "Image",
        "Width" => w as i64,
        "Height" => h as i64,
        "ColorSpace" => space,
        "BitsPerComponent" => 8,
        "Filter" => "FlateDecode",
    };
    if img.color().has_alpha() {
        let alpha: Vec<u8> = img.to_rgba8().pixels().map(|p| p.0[3]).collect();
        if alpha.iter().any(|&a| a != 255) {
            let mask = doc.add_object(Stream::new(
                dictionary! {
                    "Type" => "XObject",
                    "Subtype" => "Image",
                    "Width" => w as i64,
                    "Height" => h as i64,
                    "ColorSpace" => "DeviceGray",
                    "BitsPerComponent" => 8,
                    "Filter" => "FlateDecode",
                },
                deflate(&alpha),
            ));
            dict.set("SMask", mask);
        }
    }
    Stream::new(dict, deflate(&pixels))
}

/// Give `doc` a catalog and one page tree holding `pages`, in order.
fn finish(doc: &mut Document, pages: Vec<ObjectId>) {
    let pages_id = doc.new_object_id();
    for id in &pages {
        if let Ok(Object::Dictionary(page)) = doc.get_object_mut(*id) {
            page.set("Parent", pages_id);
        }
    }
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Count" => pages.len() as i64,
            "Kids" => pages.into_iter().map(Object::Reference).collect::<Vec<_>>(),
        }),
    );
    let catalog = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog);
    doc.trailer.remove(b"Encrypt");
    doc.trailer.remove(b"Info");
}

fn save(doc: &mut Document, output: &Path) -> Result<()> {
    doc.prune_objects();
    doc.renumber_objects();
    doc.compress();
    let file = fs::File::create(output).map_err(|e| Error::io(output, e))?;
    let mut out = BufWriter::new(file);
    doc.save_modern(&mut out)
        .map_err(|e| failed(format!("the PDF couldn't be written ({e}).")))?;
    use std::io::Write;
    out.flush().map_err(|e| Error::io(output, e))
}

/// Attributes a page may take from its parents in the page tree.
const INHERITED: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

/// Copy inherited attributes onto each page, so a page keeps them when it
/// moves to a new page tree.
fn settle_pages(doc: &mut Document) -> Vec<ObjectId> {
    let pages: Vec<ObjectId> = doc.get_pages().into_values().collect();
    for &id in &pages {
        let mut found: Vec<(Vec<u8>, Object)> = Vec::new();
        let Ok(page) = doc.get_dictionary(id) else {
            continue;
        };
        let mut parent = page.get(b"Parent").and_then(Object::as_reference).ok();
        let mut depth = 0;
        while let (Some(pid), true) = (parent, depth < 64) {
            let Ok(node) = doc.get_dictionary(pid) else {
                break;
            };
            for key in INHERITED {
                let have = page.has(key) || found.iter().any(|(k, _)| k == key);
                if !have {
                    if let Ok(v) = node.get(key) {
                        found.push((key.to_vec(), v.clone()));
                    }
                }
            }
            parent = node.get(b"Parent").and_then(Object::as_reference).ok();
            depth += 1;
        }
        if let Ok(Object::Dictionary(page)) = doc.get_object_mut(id) {
            for (k, v) in found {
                page.set(k, v);
            }
        }
    }
    pages
}

/// One PDF with every page of `inputs`, in order. Bookmarks and form
/// fields of the inputs are not kept.
pub(crate) fn merge(inputs: &[&Path], output: &Path, ctx: &mut Context<'_>) -> Result<()> {
    let mut merged = Document::with_version("1.7");
    let mut all_pages = Vec::new();
    for (i, input) in inputs.iter().enumerate() {
        if ctx.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let mut doc = load(input)?;
        let version = doc.version.clone();
        if version > merged.version {
            merged.version = version;
        }
        doc.renumber_objects_with(merged.max_id + 1);
        let pages = settle_pages(&mut doc);
        merged.max_id = merged.max_id.max(doc.max_id);
        merged.objects.extend(doc.objects);
        all_pages.extend(pages);
        (ctx.progress)(Progress::Fraction(
            (i + 1) as f32 / inputs.len() as f32 * 0.9,
        ));
    }
    finish(&mut merged, all_pages);
    save(&mut merged, output)
}

/// One PDF per page, written as numbered files next to `output`.
pub(crate) fn split(input: &Path, output: &Path, ctx: &mut Context<'_>) -> Result<Vec<PathBuf>> {
    let mut base = load(input)?;
    let pages = settle_pages(&mut base);
    if pages.len() < 2 {
        return Err(Error::NothingToDo(
            "The PDF has only one page, so there's nothing to split.".to_owned(),
        ));
    }
    let mut written = Vec::new();
    for (i, page) in pages.iter().enumerate() {
        if ctx.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let mut doc = base.clone();
        finish(&mut doc, vec![*page]);
        // Without these, every other page would stay reachable.
        for id in &pages {
            if id != page {
                if let Ok(Object::Dictionary(other)) = doc.get_object_mut(*id) {
                    other.remove(b"Parent");
                }
            }
        }
        let path = Request::numbered(output, i + 1);
        save(&mut doc, &path)?;
        written.push(path);
        (ctx.progress)(Progress::Fraction((i + 1) as f32 / pages.len() as f32));
    }
    Ok(written)
}

/// Make smaller's steps after the tidy, in order: the long side photos are
/// brought down to (`None`: as they are), each at these JPG qualities.
const SIDES: [Option<u32>; 4] = [None, Some(2000), Some(1400), Some(1000)];
const QUALITIES: [u8; 3] = [70, 55, 40];
/// The JPG quality photos are saved at when no size is asked, as Compress
/// had.
const PLAIN_QUALITY: u8 = 70;
/// Photos of fewer pixels than this (256 x 256) are left as they are.
const PHOTO_PIXELS: u64 = 256 * 256;
/// Photos of more pixels than this are left as they are: Kiln won't hold
/// one that large in memory, whatever the file claims.
const MAX_PHOTO_PIXELS: u64 = 50_000_000;

/// One step of Make smaller: the tidy, or the tidy with the photos saved
/// again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pass {
    Tidy,
    Photos { side: Option<u32>, quality: u8 },
}

/// How a photo's pixels are stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Coding {
    Jpeg,
    /// Flate, with each row led by a PNG filter byte when `png_rows`.
    Flate {
        png_rows: bool,
    },
}

/// A photo in the PDF that Make smaller may save again.
struct Photo {
    id: ObjectId,
    width: u32,
    height: u32,
    gray: bool,
    coding: Coding,
    /// Its size must stay as it is: its soft mask has a `/Matte`, and such
    /// a mask must be the size of its image.
    fixed_size: bool,
}

impl Photo {
    fn long_side(&self) -> u32 {
        self.width.max(self.height)
    }

    /// The long side a step may bring down: 0 when its size is fixed.
    fn shrinkable_side(&self) -> u32 {
        if self.fixed_size {
            0
        } else {
            self.long_side()
        }
    }
}

/// The photos [`trial`] has saved again at a quality, by photo and
/// quality, for photos a step's side doesn't bring down: `None` when that
/// wasn't smaller.
type Unscaled = HashMap<(usize, u8), Option<Stream>>;

#[cfg(test)]
thread_local! {
    /// How many files Make smaller has tried on this thread, for tests.
    static TRIALS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// `Err(Cancelled)` once `cancel` is set.
fn going(cancel: &CancelToken) -> Result<()> {
    if cancel.is_cancelled() {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

/// The PDF at `input` made smaller and written to `output`, a new file.
///
/// With `max_bytes` it is tidied (unused objects and empty pieces of page
/// content dropped, the rest compressed and packed), then, until it fits,
/// its photos are saved as JPG at quality 70, 55 and 40, then the same
/// with photos over 2000, 1400 and 1000 pixels on the long side brought
/// down to it. A JPG photo or a Flate one in RGB or grey without a soft
/// mask counts; text, vector drawing, fonts, masks and every other stream
/// are carried as they are. After the tidy the smallest step is tried:
/// when even that doesn't fit, nothing is written and the error says how
/// small it got. Otherwise it stops at the first step that fits. With no
/// size it is tidied and its JPG photos saved at quality 70 where that
/// saves a tenth. Either way the result is smaller than the original, or
/// nothing is written. Every trial is held in memory.
pub(crate) fn smaller(
    input: &Path,
    output: &Path,
    max_bytes: Option<u64>,
    ctx: &mut Context<'_>,
) -> Result<()> {
    let bytes = smaller_bytes(input, max_bytes, ctx)?;
    going(ctx.cancel)?;
    write_new(output, &bytes)
}

/// How big [`smaller`] would make the PDF at `input` for `max_bytes`,
/// worked out in memory with the same trials, writing nothing: the Make
/// smaller dialog's estimate. Its errors are `smaller`'s.
pub(crate) fn smaller_size(input: &Path, max_bytes: u64, cancel: &CancelToken) -> Result<u64> {
    going(cancel)?;
    let mut ctx = Context {
        progress: &mut |_| {},
        cancel,
    };
    let n = smaller_bytes(input, Some(max_bytes), &mut ctx)?.len() as u64;
    // Stopped during its last step: the answer is for a choice no longer
    // asked about.
    going(cancel)?;
    Ok(n)
}

/// [`smaller`]'s file, in memory.
fn smaller_bytes(input: &Path, max_bytes: Option<u64>, ctx: &mut Context<'_>) -> Result<Vec<u8>> {
    let original = fs::metadata(input).map_err(|e| Error::io(input, e))?.len();
    if let Some(max) = max_bytes.filter(|&max| max >= original) {
        return Err(Error::NothingToDo(format!(
            "This file is already smaller than {}.",
            crate::suggest::about(max)
        )));
    }
    let mut doc = load(input)?;
    (ctx.progress)(Progress::Fraction(0.0));
    going(ctx.cancel)?;
    tidy(&mut doc);
    let photos = photos_in(&doc);
    let mut unscaled = Unscaled::new();
    let as_small =
        || Error::NothingToDo("This file is already as small as Kiln can make it.".to_owned());
    let bytes = match max_bytes {
        None => {
            let step = Pass::Photos {
                side: None,
                quality: PLAIN_QUALITY,
            };
            let progress = &mut *ctx.progress;
            let mut tell = |f: f32| progress(Progress::Fraction(f * 0.9));
            let bytes = trial(
                &mut doc,
                &photos,
                step,
                true,
                &mut unscaled,
                ctx.cancel,
                &mut tell,
            )?;
            if bytes.len() as u64 >= original {
                return Err(as_small());
            }
            bytes
        }
        Some(max) => {
            let steps = steps_for(&photos);
            let total = steps.len() as f32;
            let mut tried = 0;
            let found = first_fitting(max, &steps, |step| {
                going(ctx.cancel)?;
                let before = tried as f32;
                tried += 1;
                let progress = &mut *ctx.progress;
                let mut tell = |f: f32| progress(Progress::Fraction((before + f) / total * 0.9));
                tell(0.0);
                trial(
                    &mut doc,
                    &photos,
                    step,
                    false,
                    &mut unscaled,
                    ctx.cancel,
                    &mut tell,
                )
            });
            let bytes = match found {
                Err(Error::OutOfReach { smallest, .. }) if smallest >= original => {
                    return Err(as_small())
                }
                other => other?,
            };
            // Never a file over the size asked, or one larger than the
            // original.
            let n = bytes.len() as u64;
            if n > max || n >= original {
                return Err(Error::OutOfReach {
                    max_bytes: max,
                    smallest: n,
                });
            }
            bytes
        }
    };
    Ok(bytes)
}

/// `bytes` written to `output`, which must not exist yet.
fn write_new(output: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .and_then(|mut f| f.write_all(bytes))
        .map_err(|e| Error::io(output, e))
}

/// `doc` as a file, in memory: object and cross-reference streams.
fn save_bytes(doc: &mut Document) -> Result<Vec<u8>> {
    #[cfg(test)]
    TRIALS.with(|t| t.set(t.get() + 1));
    let mut out = Vec::new();
    doc.save_modern(&mut out)
        .map_err(|e| failed(format!("the PDF couldn't be written ({e}).")))?;
    Ok(out)
}

/// Drop what nothing uses and empty pieces of page content, and compress
/// every stream that isn't, without changing what any of them holds.
fn tidy(doc: &mut Document) {
    drop_empty_contents(doc);
    doc.prune_objects();
    doc.renumber_objects();
    doc.compress();
}

/// Empty streams taken out of each page's content, where they draw
/// nothing. Any other empty stream is kept: a form a page draws, or the
/// look of a form field, is named where it is used, and a page or field
/// without it is broken.
fn drop_empty_contents(doc: &mut Document) {
    let empty = |doc: &Document, o: &Object| {
        o.as_reference()
            .ok()
            .and_then(|id| doc.objects.get(&id))
            .is_some_and(|o| matches!(o, Object::Stream(s) if s.content.is_empty()))
    };
    for page in doc.get_pages().into_values() {
        let Some(Object::Dictionary(dict)) = doc.objects.get(&page) else {
            continue;
        };
        let now = match dict.get(b"Contents") {
            Ok(one @ Object::Reference(_)) if empty(doc, one) => None,
            Ok(Object::Array(items)) if items.iter().any(|o| empty(doc, o)) => Some(
                items
                    .iter()
                    .filter(|o| !empty(doc, o))
                    .cloned()
                    .collect::<Vec<_>>(),
            ),
            _ => continue,
        };
        if let Some(Object::Dictionary(dict)) = doc.objects.get_mut(&page) {
            match now {
                Some(items) => dict.set("Contents", items),
                None => {
                    dict.remove(b"Contents");
                }
            }
        }
    }
}

/// The steps Make smaller tries for a PDF holding `photos`: the tidy, then
/// each quality at each long side that brings at least one photo down.
fn steps_for(photos: &[Photo]) -> Vec<Pass> {
    steps_for_sides(
        &photos
            .iter()
            .map(Photo::shrinkable_side)
            .collect::<Vec<_>>(),
    )
}

/// [`steps_for`] photos whose long sides are `longs`.
fn steps_for_sides(longs: &[u32]) -> Vec<Pass> {
    let mut steps = vec![Pass::Tidy];
    for side in SIDES {
        let changes = match side {
            None => !longs.is_empty(),
            Some(side) => longs.iter().any(|&long| long > side),
        };
        if changes {
            steps.extend(QUALITIES.map(|quality| Pass::Photos { side, quality }));
        }
    }
    steps
}

/// The first file of `max` bytes or fewer that `make` gives for `steps`.
/// The first step is tried, then the last, the smallest: when that doesn't
/// fit, none will, and the error says the smallest it reached. Otherwise
/// the steps between are tried in order, and the last's file is kept for
/// when none of them fits.
fn first_fitting(
    max: u64,
    steps: &[Pass],
    mut make: impl FnMut(Pass) -> Result<Vec<u8>>,
) -> Result<Vec<u8>> {
    let out_of_reach = |smallest: usize| Error::OutOfReach {
        max_bytes: max,
        smallest: smallest as u64,
    };
    let fits = |bytes: &[u8]| bytes.len() as u64 <= max;
    let Some((&first, rest)) = steps.split_first() else {
        return Err(out_of_reach(usize::MAX));
    };
    let tidied = make(first)?;
    if fits(&tidied) {
        return Ok(tidied);
    }
    let Some((&last, between)) = rest.split_last() else {
        return Err(out_of_reach(tidied.len()));
    };
    let least = make(last)?;
    if !fits(&least) {
        return Err(out_of_reach(tidied.len().min(least.len())));
    }
    drop(tidied);
    for &step in between {
        let bytes = make(step)?;
        if fits(&bytes) {
            return Ok(bytes);
        }
    }
    Ok(least)
}

/// `doc` saved, in memory, with each of `photos` as `step` has it: as the
/// file has it for the tidy, else saved again as a JPG when that is
/// smaller (a tenth smaller when `plain`, and then only JPG photos). A
/// photo that can't be read stays as it was. A photo the step's side
/// doesn't bring down is saved once for each quality and kept in
/// `unscaled`. The new photos are put in `doc` only while it is saved, and
/// the file's own put back after, so no copy of any photo is made. `tell`
/// hears the share of photos done.
fn trial(
    doc: &mut Document,
    photos: &[Photo],
    step: Pass,
    plain: bool,
    unscaled: &mut Unscaled,
    cancel: &CancelToken,
    tell: &mut dyn FnMut(f32),
) -> Result<Vec<u8>> {
    let Pass::Photos { side, quality } = step else {
        return save_bytes(doc);
    };
    // Each new photo, with where it goes and whether it is kept.
    let mut new: Vec<(usize, Stream, bool)> = Vec::new();
    for (i, photo) in photos.iter().enumerate() {
        going(cancel)?;
        let original = match doc.objects.get(&photo.id) {
            Some(Object::Stream(s)) if !(plain && photo.coding != Coding::Jpeg) => s,
            _ => continue,
        };
        let made = match side.filter(|&s| photo.shrinkable_side() > s) {
            Some(side) => {
                smaller_photo(photo, original, Some(side), quality, plain).map(|s| (s, false))
            }
            None => {
                let key = (i, quality);
                let kept = match unscaled.remove(&key) {
                    Some(kept) => kept,
                    None => smaller_photo(photo, original, None, quality, plain),
                };
                if kept.is_none() {
                    unscaled.insert(key, None);
                }
                kept.map(|s| (s, true))
            }
        };
        if let Some((stream, keep)) = made {
            new.push((i, stream, keep));
        }
        tell((i + 1) as f32 / photos.len() as f32);
    }
    let swap = |doc: &mut Document, new: &mut Vec<(usize, Stream, bool)>| {
        for (i, stream, _) in new.iter_mut() {
            if let Some(Object::Stream(s)) = doc.objects.get_mut(&photos[*i].id) {
                std::mem::swap(s, stream);
            }
        }
    };
    swap(doc, &mut new);
    let saved = save_bytes(doc);
    swap(doc, &mut new);

    for (i, stream, keep) in new {
        if keep {
            unscaled.insert((i, quality), Some(stream));
        }
    }
    saved
}

/// `photo`, whose stream is `original`, as a JPG at `quality`, its long
/// side brought down to `side`; `None` if it can't be read or isn't
/// smaller (by a tenth when `plain`).
fn smaller_photo(
    photo: &Photo,
    original: &Stream,
    side: Option<u32>,
    quality: u8,
    plain: bool,
) -> Option<Stream> {
    let img = read_photo(photo, &original.content)?;
    let img = match side {
        Some(side) => crate::photo::scaled(&img, side),
        None => img,
    };
    let bytes = raster::encode_bytes(&img, "jpg", quality, None).ok()?;
    let was = original.content.len();
    let worth = if plain {
        bytes.len() < was * 9 / 10
    } else {
        bytes.len() < was
    };
    if !worth {
        return None;
    }
    let mut dict = original.dict.clone();
    dict.set("Filter", "DCTDecode");
    dict.remove(b"DecodeParms");
    dict.set("Width", i64::from(img.width()));
    dict.set("Height", i64::from(img.height()));
    let mut stream = Stream::new(dict, bytes);
    stream.allows_compression = false;
    Some(stream)
}

/// The photos in `doc` Make smaller may save again: 8-bit RGB or grey
/// images, stored as JPG, or as Flate without a soft mask, of 256 x 256
/// pixels or more and no more than [`MAX_PHOTO_PIXELS`]. Masks, and images
/// with a colour key, a stencil or a decode array, are left out.
fn photos_in(doc: &Document) -> Vec<Photo> {
    let masks: HashSet<ObjectId> = doc
        .objects
        .values()
        .filter_map(|o| o.as_stream().ok())
        .flat_map(|s| {
            [b"SMask".as_slice(), b"Mask"]
                .into_iter()
                .filter_map(|key| s.dict.get(key).and_then(Object::as_reference).ok())
        })
        .collect();
    doc.objects
        .iter()
        .filter(|(id, _)| !masks.contains(id))
        .filter_map(|(&id, o)| photo_at(doc, id, o.as_stream().ok()?))
        .collect()
}

fn photo_at(doc: &Document, id: ObjectId, stream: &Stream) -> Option<Photo> {
    let d = &stream.dict;
    let is = |key: &[u8], name: &[u8]| d.get(key).and_then(Object::as_name).ok() == Some(name);
    if !is(b"Subtype", b"Image")
        || d.has(b"Decode")
        || d.has(b"Mask")
        || d.get(b"ImageMask")
            .and_then(Object::as_bool)
            .unwrap_or(false)
        || d.get(b"BitsPerComponent").and_then(Object::as_i64).ok() != Some(8)
    {
        return None;
    }
    let components = components(doc, d.get(b"ColorSpace").ok()?)?;
    let side = |key: &[u8]| u32::try_from(d.get(key).and_then(Object::as_i64).ok()?).ok();
    let (width, height) = (side(b"Width")?, side(b"Height")?);
    let pixels = u64::from(width) * u64::from(height);
    if !(PHOTO_PIXELS..=MAX_PHOTO_PIXELS).contains(&pixels) {
        return None;
    }
    let coding = match only_filter(doc, d)? {
        b"DCTDecode" => {
            // Decode parameters (ColorTransform) change how the JPG's
            // colours read, and the decoder never sees them.
            if !no_parms(doc, d) || jpeg_frame(&stream.content)? != (width, height, components) {
                return None;
            }
            Coding::Jpeg
        }
        b"FlateDecode" if !d.has(b"SMask") => Coding::Flate {
            png_rows: png_rows(doc, d, width, components)?,
        },
        _ => return None,
    };
    let fixed_size = d
        .get(b"SMask")
        .ok()
        .and_then(|mask| doc.dereference(mask).ok())
        .and_then(|(_, mask)| mask.as_stream().ok())
        .is_some_and(|mask| mask.dict.has(b"Matte"));
    Some(Photo {
        id,
        width,
        height,
        gray: components == 1,
        coding,
        fixed_size,
    })
}

/// Whether the stream has no decode parameters: none, null, or an empty
/// dictionary (or an array of one of those).
fn no_parms(doc: &Document, d: &Dictionary) -> bool {
    let Ok(parms) = d.get(b"DecodeParms") else {
        return true;
    };
    let none = |o: &Object| match doc.dereference(o) {
        Ok((_, Object::Null)) => true,
        Ok((_, Object::Dictionary(p))) => p.is_empty(),
        _ => false,
    };
    match doc.dereference(parms) {
        Ok((_, Object::Array(items))) => items.len() <= 1 && items.iter().all(none),
        _ => none(parms),
    }
}

/// The stream's one filter, written as a name or an array of one.
fn only_filter<'a>(doc: &'a Document, d: &'a Dictionary) -> Option<&'a [u8]> {
    let (_, filter) = doc.dereference(d.get(b"Filter").ok()?).ok()?;
    match filter {
        Object::Name(name) => Some(name),
        Object::Array(items) if items.len() == 1 => items[0].as_name().ok(),
        _ => None,
    }
}

/// The colour components of a DeviceRGB, DeviceGray or ICC-based RGB or
/// grey colour space: 3 or 1. `None` for any other.
fn components(doc: &Document, space: &Object) -> Option<u8> {
    let (_, space) = doc.dereference(space).ok()?;
    match space {
        Object::Name(name) if name == b"DeviceRGB" => Some(3),
        Object::Name(name) if name == b"DeviceGray" => Some(1),
        Object::Array(items)
            if items.len() == 2 && items[0].as_name().ok() == Some(b"ICCBased".as_slice()) =>
        {
            let (_, profile) = doc.dereference(&items[1]).ok()?;
            match profile
                .as_stream()
                .ok()?
                .dict
                .get(b"N")
                .and_then(Object::as_i64)
            {
                Ok(n @ (1 | 3)) => Some(n as u8),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Whether a Flate photo's rows each start with a PNG filter byte: `false`
/// with no predictor, `true` with a PNG predictor that matches the picture,
/// `None` with any other.
fn png_rows(doc: &Document, d: &Dictionary, width: u32, components: u8) -> Option<bool> {
    let Ok(parms) = d.get(b"DecodeParms") else {
        return Some(false);
    };
    let (_, parms) = doc.dereference(parms).ok()?;
    let parms = match parms {
        Object::Null => return Some(false),
        Object::Dictionary(p) => p,
        Object::Array(items) if items.len() == 1 => match doc.dereference(&items[0]).ok()?.1 {
            Object::Null => return Some(false),
            Object::Dictionary(p) => p,
            _ => return None,
        },
        _ => return None,
    };
    let int = |key: &[u8], default: i64| parms.get(key).and_then(Object::as_i64).unwrap_or(default);
    match int(b"Predictor", 1) {
        1 => Some(false),
        10..=15
            if int(b"Colors", 1) == i64::from(components)
                && int(b"BitsPerComponent", 8) == 8
                && int(b"Columns", 1) == i64::from(width) =>
        {
            Some(true)
        }
        _ => None,
    }
}

/// The pixels of `photo`, whose stream holds `data`, read with no more
/// memory than its picture
/// needs; `None` if they can't be read or aren't what the PDF says.
fn read_photo(photo: &Photo, data: &[u8]) -> Option<DynamicImage> {
    use image::{GrayImage, ImageDecoder, Limits, RgbImage};
    let (w, h) = (photo.width, photo.height);
    let img = match photo.coding {
        Coding::Jpeg => {
            let mut decoder =
                image::codecs::jpeg::JpegDecoder::new(std::io::Cursor::new(data)).ok()?;
            let mut limits = Limits::default();
            limits.reserve(decoder.total_bytes()).ok()?;
            decoder.set_limits(limits).ok()?;
            DynamicImage::from_decoder(decoder).ok()?
        }
        Coding::Flate { png_rows } => {
            let n = if photo.gray { 1 } else { 3 };
            let row = w as usize * n;
            let rows = inflate_exactly(data, h as usize * (row + usize::from(png_rows)))?;
            let pixels = if png_rows {
                unfilter(rows, row, n)?
            } else {
                rows
            };
            if photo.gray {
                DynamicImage::ImageLuma8(GrayImage::from_raw(w, h, pixels)?)
            } else {
                DynamicImage::ImageRgb8(RgbImage::from_raw(w, h, pixels)?)
            }
        }
    };
    let fits = img.dimensions() == (w, h)
        && match &img {
            DynamicImage::ImageLuma8(_) => photo.gray,
            DynamicImage::ImageRgb8(_) => !photo.gray,
            _ => false,
        };
    fits.then_some(img)
}

/// `data` inflated, when it holds exactly `want` bytes. Reading stops one
/// byte past `want`, so a stream that inflates to far more than its picture
/// costs no more than the picture.
fn inflate_exactly(data: &[u8], want: usize) -> Option<Vec<u8>> {
    use std::io::Read;
    // Grown as the data comes, so a large claim over little data takes
    // little.
    let mut out = Vec::with_capacity(want.min(data.len().saturating_mul(4)));
    flate2::read::ZlibDecoder::new(data)
        .take(want as u64 + 1)
        .read_to_end(&mut out)
        .ok()?;
    (out.len() == want).then_some(out)
}

/// PNG-filtered rows of `row` bytes, each led by its filter byte, undone
/// in place, for pixels of `bpp` bytes. `None` for an unknown filter.
///
/// Row `i`'s pixels are written from `i * row`, which is never past where
/// its own filtered bytes still wait to be read, and the row above has
/// been written just before it.
fn unfilter(mut data: Vec<u8>, row: usize, bpp: usize) -> Option<Vec<u8>> {
    let lines = data.len() / (row + 1);
    for i in 0..lines {
        let (from, to) = (i * (row + 1), i * row);
        let kind = data[from];
        for x in 0..row {
            let a = if x >= bpp { data[to + x - bpp] } else { 0 };
            let b = if i > 0 { data[to - row + x] } else { 0 };
            let c = if i > 0 && x >= bpp {
                data[to - row + x - bpp]
            } else {
                0
            };
            let add = match kind {
                0 => 0,
                1 => a,
                2 => b,
                3 => ((u16::from(a) + u16::from(b)) / 2) as u8,
                4 => paeth(a, b, c),
                _ => return None,
            };
            data[to + x] = data[from + 1 + x].wrapping_add(add);
        }
    }
    data.truncate(lines * row);
    Some(data)
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = i16::from(a) + i16::from(b) - i16::from(c);
    let (pa, pb, pc) = (
        (p - i16::from(a)).abs(),
        (p - i16::from(b)).abs(),
        (p - i16::from(c)).abs(),
    );
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// A PDF with one page per image, in order. Used by Merge for images,
/// with FFmpeg reading a HEIC or AVIF through `fallback`.
pub(crate) fn images_to_pdf(
    inputs: &[(&Path, &str)],
    output: &Path,
    fallback: raster::Fallback<'_>,
    ctx: &mut Context<'_>,
) -> Result<()> {
    let mut doc = Document::with_version("1.7");
    let mut pages = Vec::new();
    for (i, (input, ext)) in inputs.iter().enumerate() {
        if ctx.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        pages.push(image_page(
            &mut doc,
            input,
            ext,
            ctx.cancel,
            Some(fallback),
        )?);
        (ctx.progress)(Progress::Fraction(
            (i + 1) as f32 / inputs.len() as f32 * 0.9,
        ));
    }
    finish(&mut doc, pages);
    save(&mut doc, output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_tidied() {
        let pages = vec![
            "Title  \n\n\n\nBody line\n".to_owned(),
            "\n\nPage two\n\n".to_owned(),
        ];
        assert_eq!(tidy_text(&pages), "Title\n\nBody line\n\nPage two\n");
    }

    #[test]
    fn jpeg_components_come_from_the_frame_header() {
        // SOI, an APP0 of length 4, then SOF0 with three components.
        let data = [
            0xFF, 0xD8, 0xFF, 0xE0, 0, 4, 0, 0, 0xFF, 0xC0, 0, 17, 8, 0, 16, 0, 16, 3, 0, 0, 0,
        ];
        assert_eq!(jpeg_components(&data), Some(3));
        assert_eq!(jpeg_components(&[0xFF, 0xD8, 0x00]), None);
    }

    /// A one-page document whose page draws `/A` and `/B`, which are the two
    /// forms of the first of `levels` levels; both forms of each level draw
    /// both of the next.
    fn wide_forms(levels: usize) -> Document {
        let mut doc = Document::with_version("1.7");
        let ids: Vec<[ObjectId; 2]> = (0..levels)
            .map(|_| [doc.new_object_id(), doc.new_object_id()])
            .collect();
        let pair = |ids: &[ObjectId; 2]| {
            dictionary! { "XObject" => dictionary! { "A" => ids[0], "B" => ids[1] } }
        };
        for (i, level) in ids.iter().enumerate() {
            let (resources, content) = match ids.get(i + 1) {
                Some(next) => (pair(next), "/A Do /B Do"),
                None => (dictionary! {}, "q Q"),
            };
            for id in level {
                let dict = dictionary! { "Subtype" => "Form", "Resources" => resources.clone() };
                doc.objects.insert(
                    *id,
                    Object::Stream(Stream::new(dict, content.as_bytes().to_vec())),
                );
            }
        }
        let contents = doc.add_object(Stream::new(dictionary! {}, b"/A Do /B Do".to_vec()));
        let page = doc.add_object(dictionary! {
            "Type" => "Page",
            "MediaBox" => vec![0.into(), 0.into(), 10.into(), 10.into()],
            "Contents" => contents,
            "Resources" => pair(&ids[0]),
        });
        finish(&mut doc, vec![page]);
        doc
    }

    #[test]
    fn forms_shared_by_many_are_walked_once() {
        // 30 levels of two forms is 2^30 ways down. Walked one way at a
        // time, this would not finish; walked once each, it is instant.
        assert!(forms_are_safe(&wide_forms(MAX_FORM_DEPTH - 2)).is_ok());
        assert!(forms_are_safe(&wide_forms(MAX_FORM_DEPTH)).is_ok());
        let err = forms_are_safe(&wide_forms(MAX_FORM_DEPTH + 1)).unwrap_err();
        assert!(matches!(err, Error::Unsafe(_)), "{err:?}");
    }

    /// Writes a one-page PDF at `path` that says "Secret words", encrypted
    /// with `user` as its user password and "owner" as its owner password.
    fn locked_pdf(path: &Path, user: &str) {
        let mut doc = Document::with_version("1.7");
        let font = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
        });
        let contents = doc.add_object(Stream::new(
            dictionary! {},
            b"BT /T1 12 Tf 10 100 Td (Secret words) Tj ET".to_vec(),
        ));
        let page = doc.add_object(dictionary! {
            "Type" => "Page",
            "MediaBox" => vec![0.into(), 0.into(), 200.into(), 200.into()],
            "Contents" => contents,
            "Resources" => dictionary! { "Font" => dictionary! { "T1" => font } },
        });
        finish(&mut doc, vec![page]);
        doc.trailer.set(
            "ID",
            vec![
                Object::string_literal("kiln-id-a"),
                Object::string_literal("kiln-id-b"),
            ],
        );
        let state = lopdf::EncryptionState::try_from(lopdf::EncryptionVersion::V2 {
            document: &doc,
            owner_password: "owner",
            user_password: user,
            key_length: 128,
            permissions: lopdf::Permissions::all(),
        })
        .unwrap();
        doc.encrypt(&state).unwrap();
        doc.save(path).unwrap();
    }

    #[test]
    fn a_password_is_told_from_a_damaged_file() {
        // The words of lopdf 0.42's two password errors, as they reach
        // `load_reason` straight from lopdf and by way of pdf-extract.
        let password = "the PDF is protected with a password.";
        for e in [
            lopdf::Error::InvalidPassword,
            lopdf::Error::Decryption(lopdf::encryption::DecryptionError::IncorrectPassword),
        ] {
            assert_eq!(load_reason(&e.to_string()), password);
            let wrapped = pdf_extract::OutputError::PdfError(e);
            assert_eq!(load_reason(&wrapped.to_string()), password);
        }
        assert!(load_reason("couldn't parse input: invalid file trailer")
            .starts_with("the PDF couldn't be read"));

        let dir = tempfile::tempdir().unwrap();
        // A PDF that asks for a password is turned away...
        let locked = dir.path().join("locked.pdf");
        locked_pdf(&locked, "secret");
        let err = extract_text(&locked).unwrap_err();
        assert!(err.to_string().ends_with(password), "{err}");
        // ...and one with only an owner password opens without it.
        let open = dir.path().join("open.pdf");
        locked_pdf(&open, "");
        assert!(extract_text(&open).unwrap().contains("Secret words"));
    }

    #[test]
    fn an_svg_page_stops_when_its_job_is_cancelled() {
        // Merge makes each picture a page this way, with its job's token.
        let dir = tempfile::tempdir().unwrap();
        let svg = dir.path().join("a.svg");
        fs::write(
            &svg,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect width="10" height="10" fill="red"/><rect x="10" width="10" height="10" fill="blue"/></svg>"#,
        )
        .unwrap();
        let cancel = CancelToken::new();
        cancel.cancel();
        let mut doc = Document::with_version("1.7");
        let err = image_page(&mut doc, &svg, "svg", &cancel, None).unwrap_err();
        assert!(matches!(err, Error::Cancelled), "{err:?}");
        // Not cancelled, it is a page.
        assert!(image_page(&mut doc, &svg, "svg", &CancelToken::new(), None).is_ok());
    }

    #[test]
    fn make_smaller_tries_only_the_steps_that_change_something() {
        let photos = |q: [u8; 3], side| q.map(|quality| Pass::Photos { side, quality });
        // No photos: only the tidy.
        assert_eq!(steps_for_sides(&[]), [Pass::Tidy]);
        // Photos no side brings down: the three qualities as they are.
        let small = steps_for_sides(&[900, 1000]);
        assert_eq!(small[0], Pass::Tidy);
        assert_eq!(small[1..], photos(QUALITIES, None));
        // A 1500 px photo: as it is, then at 1400 and 1000, not at 2000.
        let mid = steps_for_sides(&[600, 1500]);
        let mut want = vec![Pass::Tidy];
        want.extend(photos(QUALITIES, None));
        want.extend(photos(QUALITIES, Some(1400)));
        want.extend(photos(QUALITIES, Some(1000)));
        assert_eq!(mid, want);
        assert_eq!(steps_for_sides(&[2400]).len(), 13);
        assert_eq!(QUALITIES, [70, 55, 40]);
    }

    #[test]
    fn make_smaller_stops_at_the_first_step_that_fits() {
        // 13 steps, each 50 bytes smaller than the one before: 1000 to 400.
        let steps = steps_for_sides(&[2400]);
        let size = |step: Pass| 1000 - 50 * steps.iter().position(|&s| s == step).unwrap();
        let mut tried = Vec::new();
        let got = first_fitting(650, &steps, |step| {
            tried.push(size(step));
            Ok(vec![0; size(step)])
        })
        .unwrap();
        // The tidy, the smallest, then in order from the first.
        assert_eq!(got.len(), 650);
        assert_eq!(tried, [1000, 400, 950, 900, 850, 800, 750, 700, 650]);
        // Only the smallest fits: it is kept, not made again.
        let mut tried = 0;
        let got = first_fitting(400, &steps, |step| {
            tried += 1;
            Ok(vec![0; size(step)])
        })
        .unwrap();
        assert_eq!((got.len(), tried), (400, steps.len()));
        // Not even the smallest fits: no other step is tried, and the
        // smallest is told.
        let mut tried = 0;
        let err = first_fitting(10, &steps, |step| {
            tried += 1;
            Ok(vec![0; size(step)])
        })
        .unwrap_err();
        assert!(
            matches!(
                err,
                Error::OutOfReach {
                    max_bytes: 10,
                    smallest: 400
                }
            ),
            "{err:?}"
        );
        assert_eq!(tried, 2);
        // Only the tidy: it is the smallest.
        let err = first_fitting(10, &[Pass::Tidy], |_| Ok(vec![0; 100])).unwrap_err();
        assert!(
            matches!(
                err,
                Error::OutOfReach {
                    max_bytes: 10,
                    smallest: 100
                }
            ),
            "{err:?}"
        );
    }

    #[test]
    fn a_photo_is_inflated_no_further_than_its_picture() {
        let picture = deflate(&[7u8; 1000]);
        assert_eq!(inflate_exactly(&picture, 1000), Some(vec![7u8; 1000]));
        // More or less than the picture holds is not a picture.
        assert_eq!(inflate_exactly(&picture, 999), None);
        assert_eq!(inflate_exactly(&picture, 1001), None);
        assert_eq!(inflate_exactly(b"not flate", 10), None);
    }

    #[test]
    fn png_rows_are_undone() {
        // Two RGB pixels a row, two rows: Sub, then Up, then Average and
        // Paeth over them.
        let row = |kind: u8, bytes: [u8; 6]| {
            let mut r = vec![kind];
            r.extend(bytes);
            r
        };
        let mut data = row(1, [10, 20, 30, 1, 2, 3]);
        data.extend(row(2, [1, 1, 1, 1, 1, 1]));
        assert_eq!(
            unfilter(data.clone(), 6, 3).unwrap(),
            [10, 20, 30, 11, 22, 33, 11, 21, 31, 12, 23, 34]
        );
        let mut data = row(0, [10, 20, 30, 40, 50, 60]);
        data.extend(row(3, [0, 0, 0, 0, 0, 0]));
        data.extend(row(4, [0, 0, 0, 0, 0, 0]));
        assert_eq!(
            unfilter(data.clone(), 6, 3).unwrap()[6..],
            [5, 10, 15, 22, 30, 37, 5, 10, 15, 22, 30, 37]
        );
        assert_eq!(unfilter(row(5, [0; 6]), 6, 3), None);
    }

    /// A grainy RGB picture, `w` x `h`, as a JPG at quality 95.
    fn grainy_jpeg(w: u32, h: u32) -> Vec<u8> {
        let mut state = 0x2545_F491u32;
        let img = image::RgbImage::from_fn(w, h, |x, y| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let g = state % 61;
            image::Rgb([
                (x * 150 / w + g) as u8,
                (y * 150 / h + g) as u8,
                ((x + y) * 90 / (w + h) + g) as u8,
            ])
        });
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 95)
            .encode_image(&img)
            .unwrap();
        bytes
    }

    /// A JPG image's dictionary, `w` x `h`, in RGB.
    fn jpeg_dict(w: u32, h: u32) -> Dictionary {
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => i64::from(w),
            "Height" => i64::from(h),
            "ColorSpace" => "DeviceRGB",
            "BitsPerComponent" => 8,
            "Filter" => "DCTDecode",
        }
    }

    /// A one-page document whose page draws each of `images` in turn.
    fn photo_doc(images: Vec<Stream>) -> Document {
        let mut doc = Document::with_version("1.7");
        let mut xobjects = Dictionary::new();
        let mut content = String::new();
        for (i, image) in images.into_iter().enumerate() {
            let id = doc.add_object(image);
            xobjects.set(format!("Im{i}"), id);
            content.push_str(&format!("q 100 0 0 100 0 0 cm /Im{i} Do Q "));
        }
        let contents = doc.add_object(Stream::new(dictionary! {}, content.into_bytes()));
        let page = doc.add_object(dictionary! {
            "Type" => "Page",
            "MediaBox" => vec![0.into(), 0.into(), 200.into(), 200.into()],
            "Contents" => contents,
            "Resources" => dictionary! { "XObject" => xobjects },
        });
        finish(&mut doc, vec![page]);
        doc
    }

    /// An empty form, 1 x 1.
    fn empty_form() -> Stream {
        Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Form",
                "BBox" => vec![0.into(), 0.into(), 1.into(), 1.into()],
            },
            Vec::new(),
        )
    }

    #[test]
    fn the_tidy_keeps_empty_streams_that_something_names() {
        // An empty form the page draws, an empty appearance a field
        // needs, and an empty piece of the page's content.
        let mut doc = Document::with_version("1.7");
        let form = doc.add_object(empty_form());
        let look = doc.add_object(empty_form());
        let field = doc.add_object(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Widget",
            "Rect" => vec![0.into(), 0.into(), 10.into(), 10.into()],
            "AP" => dictionary! { "N" => look },
        });
        let nothing = doc.add_object(Stream::new(dictionary! {}, Vec::new()));
        let drawing = doc.add_object(Stream::new(dictionary! {}, b"/Fm0 Do".to_vec()));
        let page = doc.add_object(dictionary! {
            "Type" => "Page",
            "MediaBox" => vec![0.into(), 0.into(), 10.into(), 10.into()],
            "Contents" => vec![nothing.into(), drawing.into()],
            "Resources" => dictionary! { "XObject" => dictionary! { "Fm0" => form } },
            "Annots" => vec![field.into()],
        });
        finish(&mut doc, vec![page]);

        tidy(&mut doc);
        let (_, page) = doc.get_pages().into_iter().next().unwrap();
        let page = doc.get_dictionary(page).unwrap();
        let a_stream = |o: &Object| {
            o.as_reference()
                .and_then(|id| doc.get_object(id))
                .and_then(Object::as_stream)
                .is_ok()
        };
        let xobjects = page
            .get(b"Resources")
            .and_then(Object::as_dict)
            .and_then(|r| r.get(b"XObject"))
            .and_then(Object::as_dict)
            .unwrap();
        assert!(
            xobjects.get(b"Fm0").is_ok_and(a_stream),
            "the form the page draws is gone"
        );
        let annots = page.get(b"Annots").and_then(Object::as_array).unwrap();
        let field = doc
            .get_dictionary(annots[0].as_reference().unwrap())
            .unwrap();
        let look = field.get(b"AP").and_then(Object::as_dict).unwrap();
        assert!(
            look.get(b"N").is_ok_and(a_stream),
            "the field's appearance is gone"
        );
        // The empty piece of content is let go; the drawing stays.
        let contents = page.get(b"Contents").and_then(Object::as_array).unwrap();
        assert_eq!(contents.len(), 1);
        assert!(a_stream(&contents[0]));
    }

    #[test]
    fn a_jpg_photo_with_decode_parms_is_left_as_it_is() {
        let bytes = grainy_jpeg(300, 300);
        let plain = photo_doc(vec![Stream::new(jpeg_dict(300, 300), bytes.clone())]);
        assert_eq!(photos_in(&plain).len(), 1);
        // With ColorTransform 0 the JPG holds RGB, which the decoder would
        // read as YCbCr.
        let mut dict = jpeg_dict(300, 300);
        dict.set("DecodeParms", dictionary! { "ColorTransform" => 0 });
        let rgb = photo_doc(vec![Stream::new(dict, bytes)]);
        assert!(photos_in(&rgb).is_empty());
    }

    /// The width and height an image's dictionary gives.
    fn size_in(d: &Dictionary) -> (i64, i64) {
        let n = |key: &[u8]| d.get(key).and_then(Object::as_i64).unwrap();
        (n(b"Width"), n(b"Height"))
    }

    #[test]
    fn a_jpg_photo_whose_mask_has_matte_keeps_its_size() {
        let (w, h) = (2100, 40);
        let mask = Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => i64::from(w),
                "Height" => i64::from(h),
                "ColorSpace" => "DeviceGray",
                "BitsPerComponent" => 8,
                "Matte" => vec![0.into(), 0.into(), 0.into()],
            },
            vec![128; (w * h) as usize],
        );
        let mut doc = photo_doc(vec![]);
        let mask = doc.add_object(mask);
        let mut dict = jpeg_dict(w, h);
        dict.set("SMask", mask);
        let image = doc.add_object(Stream::new(dict, grainy_jpeg(w, h)));

        let photos = photos_in(&doc);
        assert_eq!(photos.len(), 1);
        // No step brings its side down; the qualities may still apply.
        assert_eq!(steps_for(&photos).len(), 1 + QUALITIES.len());
        // Even a step with a side leaves it the size of its mask.
        let step = Pass::Photos {
            side: Some(1000),
            quality: 40,
        };
        let bytes = trial_bytes(&mut doc, &photos, step);
        let out = Document::load_mem(&bytes).unwrap();
        let image = out.get_object(image).unwrap().as_stream().unwrap();
        let mask = image.dict.get(b"SMask").unwrap().as_reference().unwrap();
        let mask = out.get_object(mask).unwrap().as_stream().unwrap();
        assert_eq!(size_in(&image.dict), size_in(&mask.dict));
        assert_eq!(size_in(&image.dict), (i64::from(w), i64::from(h)));
    }

    /// The PDF one trial of `step` makes of `doc`.
    fn trial_bytes(doc: &mut Document, photos: &[Photo], step: Pass) -> Vec<u8> {
        let mut unscaled = Unscaled::new();
        let cancel = CancelToken::new();
        trial(
            doc,
            photos,
            step,
            false,
            &mut unscaled,
            &cancel,
            &mut |_| {},
        )
        .unwrap()
    }

    #[test]
    fn a_scan_out_of_reach_is_known_after_one_trial_past_the_tidy() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("scan.pdf");
        let images = (0..3)
            .map(|_| Stream::new(jpeg_dict(2100, 300), grainy_jpeg(2100, 300)))
            .collect();
        photo_doc(images).save(&input).unwrap();
        let mut heard = Vec::new();
        let cancel = CancelToken::new();
        let mut progress = |p| {
            if let Progress::Fraction(f) = p {
                heard.push(f)
            }
        };
        let mut ctx = Context {
            progress: &mut progress,
            cancel: &cancel,
        };
        TRIALS.with(|t| t.set(0));
        let output = dir.path().join("out.pdf");
        let err = smaller(&input, &output, Some(100), &mut ctx).unwrap_err();
        assert!(
            matches!(err, Error::OutOfReach { max_bytes: 100, .. }),
            "{err:?}"
        );
        // The tidy, then the smallest step: it doesn't fit, so none would.
        assert_eq!(TRIALS.with(|t| t.get()), 2);
        // The bar moves with each photo, not only with each step.
        assert!(heard.len() > 3, "{heard:?}");
        assert!(heard.windows(2).all(|w| w[0] <= w[1]), "{heard:?}");
        assert!(!output.exists());
    }
}
