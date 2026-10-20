//! PDF, in process: text out of a PDF, a PDF's pages as images, images
//! into a PDF, and the PDF tools (merge, split and compress).
//!
//! Text comes from `pdf-extract`. Pages are drawn by the PDF renderer
//! built into Windows (Windows.Data.Pdf), so no engine has to be
//! installed. Everything that writes a PDF uses `lopdf`.

use std::fs;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use image::{DynamicImage, GenericImageView};
use lopdf::{dictionary, Document, Object, ObjectId, Stream};

use crate::converter::{Context, Converter, Progress, Request};
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
                let page = image_page(&mut doc, req.input, from)?;
                finish(&mut doc, vec![page]);
                save(&mut doc, req.output)
            }
            _ => Err(Error::Registry(format!(
                "The PDF backend can't convert {} to {}.",
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
    if doc.is_encrypted() {
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
    let pages = std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem_by_pages(&bytes))
        .map_err(|_| failed("the PDF's text couldn't be read."))?
        .map_err(|e| failed(load_reason(&e.to_string())))?;
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
pub(crate) fn text_in_first(doc: &Document, pages: u32) -> Result<bool> {
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
        raster::encode(&img, &Request::numbered(req.output, n), to, quality)?;
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
    let mut i = 2;
    while i + 9 < data.len() {
        if data[i] != 0xFF {
            return None;
        }
        let marker = data[i + 1];
        let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
        if matches!(marker, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
            return Some(data[i + 9]);
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
/// JPGs are placed as they are, without re-encoding.
fn image_page(doc: &mut Document, input: &Path, ext: &str) -> Result<ObjectId> {
    let img = raster::decode(input, ext)?;
    let (w, h) = img.dimensions();
    let original = if ext == "jpg" {
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

/// Re-save smaller: photos re-encoded as JPG at quality 70 when that
/// saves space, unused objects dropped, streams and the object table
/// compressed.
pub(crate) fn compress(input: &Path, output: &Path, ctx: &mut Context<'_>) -> Result<()> {
    let mut doc = load(input)?;
    let ids: Vec<ObjectId> = doc.objects.keys().copied().collect();
    for (i, id) in ids.iter().enumerate() {
        if ctx.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if let Some(Object::Stream(stream)) = doc.objects.get_mut(id) {
            recompress_photo(stream);
        }
        if i % 64 == 0 {
            (ctx.progress)(Progress::Fraction(i as f32 / ids.len() as f32 * 0.9));
        }
    }
    save(&mut doc, output)
}

/// Re-encode one JPG image stream at quality 70 if it is large enough to
/// matter and the result is clearly smaller.
fn recompress_photo(stream: &mut Stream) {
    let d = &stream.dict;
    let is = |key: &[u8], name: &[u8]| d.get(key).and_then(Object::as_name).ok() == Some(name);
    if !is(b"Subtype", b"Image") || !is(b"Filter", b"DCTDecode") {
        return;
    }
    if d.has(b"Decode") || d.get(b"BitsPerComponent").and_then(Object::as_i64).ok() != Some(8) {
        return;
    }
    let simple_space = is(b"ColorSpace", b"DeviceRGB") || is(b"ColorSpace", b"DeviceGray");
    if !simple_space || !matches!(jpeg_components(&stream.content), Some(1 | 3)) {
        return;
    }
    let Ok(img) = image::load_from_memory_with_format(&stream.content, image::ImageFormat::Jpeg)
    else {
        return;
    };
    if (img.width() as u64) * (img.height() as u64) < 256 * 256 {
        return;
    }
    let mut out = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 70);
    let result = if img.color().has_color() {
        DynamicImage::ImageRgb8(img.to_rgb8()).write_with_encoder(encoder)
    } else {
        DynamicImage::ImageLuma8(img.to_luma8()).write_with_encoder(encoder)
    };
    if result.is_ok() && out.len() < stream.content.len() * 9 / 10 {
        stream.set_content(out);
    }
}

/// A PDF with one page per image, in order. Used by Merge for images.
pub(crate) fn images_to_pdf(
    inputs: &[(&Path, &str)],
    output: &Path,
    ctx: &mut Context<'_>,
) -> Result<()> {
    let mut doc = Document::with_version("1.7");
    let mut pages = Vec::new();
    for (i, (input, ext)) in inputs.iter().enumerate() {
        if ctx.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        pages.push(image_page(&mut doc, input, ext)?);
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
}
