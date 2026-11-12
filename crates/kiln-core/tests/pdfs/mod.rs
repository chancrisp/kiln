//! PDFs made in the tests for Make smaller: pages of scanned photos with a
//! hidden text layer, pages of text only, and a page holding a Flate photo,
//! a photo with a soft mask, an embedded font and vector drawing. Included
//! with `mod pdfs;`.
#![allow(dead_code)]

use std::io::Write;
use std::path::Path;

use image::codecs::jpeg::JpegEncoder;
use image::{ExtendedColorType, ImageEncoder, Rgb, RgbImage};
use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};

/// A picture that compresses like a photo: smooth shading with grain on
/// top, different for each `seed`.
pub fn photo(w: u32, h: u32, seed: u32) -> RgbImage {
    let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    RgbImage::from_fn(w, h, |x, y| {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        let grain = (state % 41) as i32 - 20;
        let shade = |base: u32| -> u8 {
            let v = base as i32 + grain;
            v.clamp(0, 255) as u8
        };
        Rgb([
            shade(x * 200 / w + 30),
            shade(y * 200 / h + 20),
            shade((x + y) * 120 / (w + h) + 60),
        ])
    })
}

pub fn jpeg(img: &RgbImage, quality: u8) -> Vec<u8> {
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, quality)
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            ExtendedColorType::Rgb8,
        )
        .unwrap();
    bytes
}

pub fn deflate(data: &[u8]) -> Vec<u8> {
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(data).unwrap();
    enc.finish().unwrap()
}

fn helvetica(doc: &mut Document) -> ObjectId {
    doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
    })
}

/// A Flate stream: compressed already, so a re-save that compresses the
/// rest leaves its bytes as they are.
fn flate_stream(dict: Dictionary, data: &[u8]) -> Stream {
    let mut dict = dict;
    dict.set("Filter", "FlateDecode");
    Stream::new(dict, deflate(data))
}

/// Gives `doc` a page tree of `pages` and a catalog, and saves it at `path`
/// as a plain PDF 1.4 with a classic cross-reference table.
fn finish(mut doc: Document, tree: ObjectId, pages: Vec<ObjectId>, path: &Path) {
    let count = pages.len() as i64;
    doc.objects.insert(
        tree,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Count" => count,
            "Kids" => pages.into_iter().map(Object::Reference).collect::<Vec<_>>(),
        }),
    );
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => tree });
    doc.trailer.set("Root", catalog);
    doc.save(path).unwrap();
}

/// A scanned document: `pages` pages, each one `w` x `h` photo stored as a
/// JPG at quality 95, with the page's words written over it in invisible
/// text ("Scanned page n"), as a scanner's text recognition does.
pub fn scan_pdf(path: &Path, pages: u32, w: u32, h: u32) {
    let mut doc = Document::with_version("1.4");
    let font = helvetica(&mut doc);
    let tree = doc.new_object_id();
    let mut kids = Vec::new();
    for n in 1..=pages {
        let image = doc.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => w as i64,
                "Height" => h as i64,
                "ColorSpace" => "DeviceRGB",
                "BitsPerComponent" => 8,
                "Filter" => "DCTDecode",
            },
            jpeg(&photo(w, h, n), 95),
        ));
        let (pw, ph) = (w as f32 * 72.0 / 200.0, h as f32 * 72.0 / 200.0);
        let content = format!(
            "q {pw:.2} 0 0 {ph:.2} 0 0 cm /Im0 Do Q BT 3 Tr /T1 12 Tf 20 20 Td (Scanned page {n}) Tj ET"
        );
        let contents = doc.add_object(flate_stream(dictionary! {}, content.as_bytes()));
        kids.push(doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => tree,
            "MediaBox" => vec![0.into(), 0.into(), Object::Real(pw), Object::Real(ph)],
            "Contents" => contents,
            "Resources" => dictionary! {
                "XObject" => dictionary! { "Im0" => image },
                "Font" => dictionary! { "T1" => font },
            },
        }));
    }
    finish(doc, tree, kids, path);
}

/// A document of `pages` pages of text only, its content streams left
/// uncompressed, as some makers write them.
pub fn text_pdf(path: &Path, pages: u32) {
    let mut doc = Document::with_version("1.4");
    let font = helvetica(&mut doc);
    let tree = doc.new_object_id();
    let mut kids = Vec::new();
    for n in 1..=pages {
        let mut content = String::from("BT /T1 10 Tf 40 760 Td 12 TL\n");
        for line in 0..60 {
            content.push_str(&format!(
                "(Page {n}, line {line}: words that only a smaller font could shorten.) '\n"
            ));
        }
        content.push_str("ET\n");
        let contents = doc.add_object(Stream::new(dictionary! {}, content.into_bytes()));
        kids.push(doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => tree,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Contents" => contents,
            "Resources" => dictionary! { "Font" => dictionary! { "T1" => font } },
        }));
    }
    finish(doc, tree, kids, path);
}

/// The parts of [`mixed_pdf`], told apart by their sizes and keys.
pub const PHOTO: (i64, i64) = (640, 480);
pub const MASKED: (i64, i64) = (400, 300);

/// One page with a Flate RGB photo (640 x 480), a Flate RGB photo with a
/// soft mask (400 x 300, and its mask the same), an embedded font file
/// (`/Length1`), vector drawing and text.
pub fn mixed_pdf(path: &Path) {
    let mut doc = Document::with_version("1.4");
    let tree = doc.new_object_id();
    let image = |w: i64, h: i64, space: &str| {
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => w,
            "Height" => h,
            "ColorSpace" => space,
            "BitsPerComponent" => 8,
        }
    };
    let photo_px = photo(PHOTO.0 as u32, PHOTO.1 as u32, 7);
    let plain = doc.add_object(flate_stream(
        image(PHOTO.0, PHOTO.1, "DeviceRGB"),
        photo_px.as_raw(),
    ));
    let mask_px: Vec<u8> = (0..MASKED.0 * MASKED.1)
        .map(|i| ((i % MASKED.0) * 255 / MASKED.0) as u8)
        .collect();
    let mask = doc.add_object(flate_stream(
        image(MASKED.0, MASKED.1, "DeviceGray"),
        &mask_px,
    ));
    let mut masked_dict = image(MASKED.0, MASKED.1, "DeviceRGB");
    masked_dict.set("SMask", mask);
    let masked_px = photo(MASKED.0 as u32, MASKED.1 as u32, 11);
    let masked = doc.add_object(flate_stream(masked_dict, masked_px.as_raw()));

    // Not a real font program: its bytes are only carried, never read.
    let font_bytes: Vec<u8> = (0..20_000u32).map(|i| (i * 31 % 251) as u8).collect();
    let font_file = doc.add_object(flate_stream(
        dictionary! { "Length1" => font_bytes.len() as i64 },
        &font_bytes,
    ));
    let descriptor = doc.add_object(dictionary! {
        "Type" => "FontDescriptor",
        "FontName" => "KilnTest",
        "Flags" => 32,
        "FontBBox" => vec![0.into(), 0.into(), 1000.into(), 1000.into()],
        "ItalicAngle" => 0,
        "Ascent" => 800,
        "Descent" => -200,
        "CapHeight" => 700,
        "StemV" => 80,
        "FontFile2" => font_file,
    });
    let font = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "TrueType",
        "BaseFont" => "KilnTest",
        "FirstChar" => 32,
        "LastChar" => 32,
        "Widths" => vec![Object::Integer(250)],
        "FontDescriptor" => descriptor,
    });
    let content = "0.2 0.4 0.6 rg 10 10 m 300 10 l 150 200 l h f \
                   q 320 0 0 240 20 300 cm /Im0 Do Q \
                   q 200 0 0 150 360 300 cm /Im1 Do Q \
                   BT /T1 12 Tf 20 560 Td (Drawn words) Tj ET";
    let contents = doc.add_object(flate_stream(dictionary! {}, content.as_bytes()));
    let page = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => tree,
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 612.into()],
        "Contents" => contents,
        "Resources" => dictionary! {
            "XObject" => dictionary! { "Im0" => plain, "Im1" => masked },
            "Font" => dictionary! { "T1" => font },
        },
    });
    finish(doc, tree, vec![page], path);
}

/// The stream in `doc` that `pick` chooses, with its dictionary.
pub fn find_stream(doc: &Document, pick: impl Fn(&Stream) -> bool) -> &Stream {
    let found: Vec<&Stream> = doc
        .objects
        .values()
        .filter_map(|o| o.as_stream().ok())
        .filter(|s| pick(s))
        .collect();
    assert_eq!(found.len(), 1, "one stream should match");
    found[0]
}

/// Whether `s` is an image `w` x `h` in `space`.
pub fn is_image(s: &Stream, (w, h): (i64, i64), space: &[u8]) -> bool {
    let d = &s.dict;
    d.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"Image".as_slice())
        && d.get(b"Width").and_then(Object::as_i64).ok() == Some(w)
        && d.get(b"Height").and_then(Object::as_i64).ok() == Some(h)
        && d.get(b"ColorSpace").and_then(Object::as_name).ok() == Some(space)
}

pub fn filter_of(s: &Stream) -> Vec<u8> {
    s.dict
        .get(b"Filter")
        .and_then(Object::as_name)
        .map(<[u8]>::to_vec)
        .unwrap_or_default()
}

/// The page's content, decoded.
pub fn page_content(doc: &Document) -> Vec<u8> {
    let (_, page) = doc.get_pages().into_iter().next().unwrap();
    doc.get_page_content(page).unwrap()
}
