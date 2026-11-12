//! Where a photo was taken and the camera details: taken out of a JPG, PNG,
//! WEBP or GIF by rewriting the file around its picture, which is copied
//! byte for byte; and, when a photo is saved again, carried over to the new
//! file with the turn its pixels already have.
//!
//! What stays is what shows the picture as it was: the colour profile, the
//! orientation (as a minimal Exif block holding only that), and, in a GIF,
//! its loop count. Everything else that a file can say about itself goes:
//! Exif (the GPS block, make, model, serial numbers, dates), XMP, IPTC,
//! comments, text chunks and anything Kiln doesn't know.

use std::path::Path;

use image::metadata::Orientation;
use image::{ImageDecoder, ImageReader};

use crate::error::{Error, Result};

/// The file `bytes`, a picture of type `ext` (`jpg`, `png`, `webp` or
/// `gif`), with its location data and camera details taken out. The picture
/// itself is copied byte for byte. `input` names the file in an error.
pub(crate) fn strip(input: &Path, bytes: &[u8], ext: &str) -> Result<Vec<u8>> {
    let stripped = match ext {
        "jpg" => strip_jpeg(bytes),
        "png" => strip_png(bytes),
        "webp" => strip_webp(bytes),
        "gif" => strip_gif(bytes),
        other => Err(format!(
            "{} isn't rewritten in place.",
            other.to_uppercase()
        )),
    };
    stripped.map_err(|reason| Error::Decode {
        path: input.to_path_buf(),
        reason,
    })
}

/// Whether [`strip`] rewrites `ext` in place. Other types are decoded and
/// written again, which keeps nothing but the colour profile.
pub(crate) fn strips_in_place(ext: &str) -> bool {
    matches!(ext, "jpg" | "png" | "webp" | "gif")
}

/// What a photo said about itself, to be written into the photo saved from
/// it. Its orientation is already set to upright, as the pixels it goes
/// with have been turned.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Kept {
    /// Exif, as a TIFF block (no `Exif\0\0` in front).
    pub exif: Option<Vec<u8>>,
    /// The XMP packet.
    pub xmp: Option<Vec<u8>>,
    /// IPTC, as a JPEG's APP13 holds it. Only a JPEG keeps it.
    pub iptc: Option<Vec<u8>>,
}

impl Kept {
    /// What the photo at `input` says about itself. A file whose metadata
    /// can't be read has none to keep.
    pub(crate) fn read(input: &Path) -> Kept {
        let Ok(reader) = ImageReader::open(input).and_then(ImageReader::with_guessed_format) else {
            return Kept::default();
        };
        let Ok(mut decoder) = reader.into_decoder() else {
            return Kept::default();
        };
        let exif = decoder.exif_metadata().ok().flatten().map(|mut exif| {
            if exif.starts_with(EXIF_HEADER) {
                exif.drain(..EXIF_HEADER.len());
            }
            let _ = Orientation::remove_from_exif_chunk(&mut exif);
            without_thumbnail(&mut exif);
            exif
        });
        let xmp = decoder.xmp_metadata().ok().flatten().map(upright_xmp);
        let iptc = decoder.iptc_metadata().ok().flatten();
        Kept { exif, xmp, iptc }
    }

    /// `bytes`, a picture of type `ext` that Kiln just encoded, with this
    /// metadata written into it.
    pub(crate) fn embed(&self, bytes: Vec<u8>, ext: &str) -> Result<Vec<u8>> {
        if *self == Kept::default() {
            return Ok(bytes);
        }
        let embedded = match ext {
            "jpg" => self.embed_jpeg(&bytes),
            "png" => self.embed_png(&bytes),
            "webp" => self.embed_webp(&bytes),
            _ => return Ok(bytes),
        };
        embedded.map_err(|reason| Error::Encode {
            code: ext.to_uppercase(),
            reason,
        })
    }

    fn embed_jpeg(&self, jpeg: &[u8]) -> std::result::Result<Vec<u8>, String> {
        let mut segments = Vec::new();
        if let Some(exif) = &self.exif {
            segments.push(segment(0xE1, &[EXIF_HEADER, exif].concat()));
        }
        if let Some(xmp) = &self.xmp {
            segments.push(segment(0xE1, &[XMP_HEADER, xmp].concat()));
        }
        if let Some(iptc) = &self.iptc {
            segments.push(segment(0xED, &[IPTC_HEADER, iptc].concat()));
        }
        // A segment over 64 KB can't be written; it is left out.
        let segments: Vec<u8> = segments.into_iter().flatten().flatten().collect();
        insert_after_jfif(jpeg, &segments)
    }

    fn embed_png(&self, png: &[u8]) -> std::result::Result<Vec<u8>, String> {
        let mut chunks = Vec::new();
        if let Some(exif) = &self.exif {
            chunks.extend(chunk(b"eXIf", exif));
        }
        if let Some(xmp) = &self.xmp {
            // iTXt: keyword, no compression, no language, no translation.
            let body = [XMP_KEYWORD, b"\0\0\0\0\0", xmp].concat();
            chunks.extend(chunk(b"iTXt", &body));
        }
        let idat = png_chunks(png)?
            .into_iter()
            .find(|c| &c.kind == b"IDAT")
            .ok_or("it has no picture data.")?;
        Ok([&png[..idat.start], &chunks, &png[idat.start..]].concat())
    }

    fn embed_webp(&self, webp: &[u8]) -> std::result::Result<Vec<u8>, String> {
        crate::webp::with_chunks(
            webp,
            &[
                (c"EXIF", self.exif.as_deref()),
                (c"XMP ", self.xmp.as_deref()),
            ],
        )
        .map_err(|e| e.to_string())
    }
}

const EXIF_HEADER: &[u8] = b"Exif\0\0";
const XMP_HEADER: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
const IPTC_HEADER: &[u8] = b"Photoshop 3.0\0";
const XMP_KEYWORD: &[u8] = b"XML:com.adobe.xmp";
const ICC_HEADER: &[u8] = b"ICC_PROFILE\0";

/// A minimal Exif block, little-endian, holding only `orientation`.
fn orientation_exif(orientation: u16) -> Vec<u8> {
    let mut tiff = b"II*\0".to_vec();
    tiff.extend(8u32.to_le_bytes()); // the first directory follows
    tiff.extend(1u16.to_le_bytes()); // one entry
    tiff.extend(0x0112u16.to_le_bytes()); // Orientation
    tiff.extend(3u16.to_le_bytes()); // SHORT
    tiff.extend(1u32.to_le_bytes()); // one value
    tiff.extend(orientation.to_le_bytes());
    tiff.extend(0u16.to_le_bytes()); // the value's field is four bytes
    tiff.extend(0u32.to_le_bytes()); // no next directory
    tiff
}

/// The orientation an Exif block (with or without `Exif\0\0` in front) asks
/// for, when it asks for a turn or a flip.
/// `exif`, a TIFF block, with its first directory's link to the next one
/// cleared, so the thumbnail directory after it goes: that thumbnail is
/// the original's, unturned and uncropped. Exif that doesn't parse is left
/// as it is.
fn without_thumbnail(exif: &mut [u8]) {
    let big = match exif.get(..4) {
        Some(b"MM\0*") => true,
        Some(b"II*\0") => false,
        _ => return,
    };
    // A number of `n` bytes at `at`, in the block's byte order.
    let read = |at: usize, n: usize| -> Option<usize> {
        let bytes = exif.get(at..at.checked_add(n)?)?;
        let mut ordered = bytes.to_vec();
        if !big {
            ordered.reverse();
        }
        Some(ordered.iter().fold(0, |v, &b| (v << 8) | usize::from(b)))
    };
    let Some(ifd0) = read(4, 4) else { return };
    let Some(entries) = read(ifd0, 2) else { return };
    let next = ifd0 + 2 + 12 * entries;
    if let Some(link) = exif.get_mut(next..next.saturating_add(4)) {
        link.fill(0);
    }
}

fn turn_in(exif: &[u8]) -> Option<u16> {
    let tiff = exif.strip_prefix(EXIF_HEADER).unwrap_or(exif);
    Orientation::from_exif_chunk(tiff)
        .map(|o| u16::from(o.to_exif()))
        .filter(|&o| o != 1)
}

/// An XMP packet whose `tiff:Orientation`, if any, says upright. The digit
/// is replaced in place, so the packet keeps its length.
fn upright_xmp(mut xmp: Vec<u8>) -> Vec<u8> {
    const KEY: &[u8] = b"tiff:Orientation";
    let mut from = 0;
    while let Some(at) = find(&xmp[from..], KEY).map(|i| i + from) {
        let value = at + KEY.len();
        // `="6"` as an attribute, `>6<` as an element.
        let digit = (value..(value + 3).min(xmp.len())).find(|&i| xmp[i].is_ascii_digit());
        if let Some(i) = digit {
            xmp[i] = b'1';
        }
        from = value;
    }
    xmp
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

// --- JPEG -----------------------------------------------------------------

/// A JPEG marker segment with `payload`: `FF marker`, its length, the
/// payload. `None` if the payload is too long for one.
fn segment(marker: u8, payload: &[u8]) -> Option<Vec<u8>> {
    let len = u16::try_from(payload.len() + 2).ok()?;
    let mut s = vec![0xFF, marker];
    s.extend(len.to_be_bytes());
    s.extend_from_slice(payload);
    Some(s)
}

/// `jpeg` with `segments` written after its start, and after its JFIF
/// segment when it begins with one, as JFIF asks to be first.
fn insert_after_jfif(jpeg: &[u8], segments: &[u8]) -> std::result::Result<Vec<u8>, String> {
    if !jpeg.starts_with(&[0xFF, 0xD8]) {
        return Err("it isn't a JPEG.".to_owned());
    }
    let mut at = 2;
    if jpeg.get(2..4) == Some(&[0xFF, 0xE0]) {
        let len = jpeg
            .get(4..6)
            .map(|b| u16::from_be_bytes([b[0], b[1]]) as usize)
            .ok_or("it ends early.")?;
        at = 4 + len;
        if at > jpeg.len() {
            return Err("it ends early.".to_owned());
        }
    }
    Ok([&jpeg[..at], segments, &jpeg[at..]].concat())
}

/// Whether a JPEG segment is kept: everything that draws the picture, the
/// JFIF header, the colour profile and Adobe's colour transform. Exif, XMP,
/// IPTC, other APPn blocks, multi-picture indexes and comments go.
fn jpeg_keeps(marker: u8, payload: &[u8]) -> bool {
    match marker {
        0xE0 | 0xEE => true,
        0xE2 => payload.starts_with(ICC_HEADER),
        0xE1 | 0xE3..=0xED | 0xEF | 0xFE => false,
        _ => true,
    }
}

fn strip_jpeg(src: &[u8]) -> std::result::Result<Vec<u8>, String> {
    const EARLY: &str = "it ends early.";
    if !src.starts_with(&[0xFF, 0xD8]) {
        return Err("it isn't a JPEG.".to_owned());
    }
    let mut out = vec![0xFF, 0xD8];
    let mut turn = None;
    let mut pos = 2;
    loop {
        if src.get(pos) != Some(&0xFF) {
            return Err("a marker is missing.".to_owned());
        }
        // Fill bytes before a marker.
        while src.get(pos + 1) == Some(&0xFF) {
            pos += 1;
        }
        let marker = *src.get(pos + 1).ok_or(EARLY)?;
        if marker == 0xD9 {
            // The end. Anything after it, such as the extra pictures of a
            // multi-picture file with their own Exif, is left behind.
            out.extend([0xFF, 0xD9]);
            break;
        }
        if matches!(marker, 0x01 | 0xD0..=0xD7) {
            out.extend([0xFF, marker]);
            pos += 2;
            continue;
        }
        let len = src
            .get(pos + 2..pos + 4)
            .map(|b| u16::from_be_bytes([b[0], b[1]]) as usize)
            .ok_or(EARLY)?;
        if len < 2 {
            return Err("a segment's length is wrong.".to_owned());
        }
        let end = pos + 2 + len;
        let whole = src.get(pos..end).ok_or(EARLY)?;
        let payload = &whole[4..];
        if marker == 0xE1 && payload.starts_with(EXIF_HEADER) {
            turn = turn.or(turn_in(payload));
        }
        if jpeg_keeps(marker, payload) {
            out.extend_from_slice(whole);
        }
        pos = end;
        if marker == 0xDA {
            // The coded picture runs to the next marker that isn't a
            // stuffed byte or a restart.
            let start = pos;
            while pos + 1 < src.len() {
                if src[pos] == 0xFF && !matches!(src[pos + 1], 0x00 | 0xD0..=0xD7 | 0xFF) {
                    break;
                }
                pos += 1;
            }
            if pos + 1 >= src.len() {
                return Err(EARLY.to_owned());
            }
            out.extend_from_slice(&src[start..pos]);
        }
    }
    match turn.and_then(|o| segment(0xE1, &[EXIF_HEADER, &orientation_exif(o)].concat())) {
        Some(exif) => insert_after_jfif(&out, &exif),
        None => Ok(out),
    }
}

// --- PNG ------------------------------------------------------------------

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// A chunk of a PNG: its type, and where the whole of it (length, type,
/// data, CRC) lies in the file.
struct PngChunk {
    kind: [u8; 4],
    start: usize,
    end: usize,
}

fn png_chunks(src: &[u8]) -> std::result::Result<Vec<PngChunk>, String> {
    if !src.starts_with(PNG_SIGNATURE) {
        return Err("it isn't a PNG.".to_owned());
    }
    let mut chunks = Vec::new();
    let mut pos = PNG_SIGNATURE.len();
    while pos < src.len() {
        let head = src.get(pos..pos + 8).ok_or("it ends early.")?;
        let len = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
        let kind = [head[4], head[5], head[6], head[7]];
        let end = pos
            .checked_add(12)
            .and_then(|p| p.checked_add(len))
            .filter(|&e| e <= src.len())
            .ok_or("it ends early.")?;
        chunks.push(PngChunk {
            kind,
            start: pos,
            end,
        });
        pos = end;
        if &kind == b"IEND" {
            break;
        }
    }
    Ok(chunks)
}

/// One PNG chunk of type `kind` holding `data`, with its CRC.
fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut crc = flate2::Crc::new();
    crc.update(kind);
    crc.update(data);
    let mut c = (data.len() as u32).to_be_bytes().to_vec();
    c.extend_from_slice(kind);
    c.extend_from_slice(data);
    c.extend(crc.sum().to_be_bytes());
    c
}

/// The PNG chunks kept: the picture, its colours and how to show it, and
/// animation. Exif, text (where XMP and comments live), times and chunks
/// Kiln doesn't know go.
const PNG_KEEPS: [&[u8; 4]; 19] = [
    b"IHDR", b"PLTE", b"IDAT", b"IEND", b"tRNS", b"cHRM", b"gAMA", b"iCCP", b"sBIT", b"sRGB",
    b"cICP", b"mDCV", b"cLLI", b"bKGD", b"hIST", b"pHYs", b"sPLT", b"acTL", b"fcTL",
];

fn strip_png(src: &[u8]) -> std::result::Result<Vec<u8>, String> {
    let chunks = png_chunks(src)?;
    let turn = chunks
        .iter()
        .find(|c| &c.kind == b"eXIf")
        .and_then(|c| turn_in(&src[c.start + 8..c.end - 4]));
    let mut out = PNG_SIGNATURE.to_vec();
    let mut turned = false;
    for c in &chunks {
        if &c.kind == b"IDAT" && !turned {
            // The orientation goes before the picture, as eXIf must.
            if let Some(o) = turn {
                out.extend(chunk(b"eXIf", &orientation_exif(o)));
            }
            turned = true;
        }
        if PNG_KEEPS.contains(&&c.kind) || &c.kind == b"fdAT" {
            out.extend_from_slice(&src[c.start..c.end]);
        }
    }
    Ok(out)
}

// --- WEBP -----------------------------------------------------------------

/// VP8X flags for the Exif and XMP chunks.
const WEBP_EXIF: u8 = 0x08;
const WEBP_XMP: u8 = 0x04;

/// The WEBP chunks kept: the picture, its transparency, its colour profile
/// and animation.
const WEBP_KEEPS: [&[u8; 4]; 7] = [
    b"VP8X", b"ICCP", b"ANIM", b"ANMF", b"ALPH", b"VP8 ", b"VP8L",
];

fn strip_webp(src: &[u8]) -> std::result::Result<Vec<u8>, String> {
    if src.len() < 12 || &src[..4] != b"RIFF" || &src[8..12] != b"WEBP" {
        return Err("it isn't a WEBP.".to_owned());
    }
    let mut body = b"WEBP".to_vec();
    let mut turn = None;
    let mut vp8x = None;
    let mut pos = 12;
    while pos + 8 <= src.len() {
        let kind: [u8; 4] = src[pos..pos + 4].try_into().expect("four bytes");
        let len =
            u32::from_le_bytes(src[pos + 4..pos + 8].try_into().expect("four bytes")) as usize;
        let data_end = pos
            .checked_add(8 + len)
            .filter(|&e| e <= src.len())
            .ok_or("it ends early.")?;
        // Chunks are padded to an even length.
        let end = (data_end + (len & 1)).min(src.len());
        if &kind == b"EXIF" {
            turn = turn.or(turn_in(&src[pos + 8..data_end]));
        }
        if WEBP_KEEPS.contains(&&kind) {
            if &kind == b"VP8X" {
                // Flags, reserved bytes and the canvas size: 10 bytes.
                if len < 10 {
                    return Err("a block is damaged.".to_owned());
                }
                vp8x = Some(body.len());
            }
            body.extend_from_slice(&src[pos..data_end]);
            if len & 1 == 1 {
                body.push(0);
            }
        }
        pos = end;
    }
    if let Some(at) = vp8x {
        // The flags byte follows the chunk's type and length.
        body[at + 8] &= !(WEBP_EXIF | WEBP_XMP);
        if let Some(o) = turn {
            body[at + 8] |= WEBP_EXIF;
            let exif = orientation_exif(o);
            body.extend_from_slice(b"EXIF");
            body.extend((exif.len() as u32).to_le_bytes());
            body.extend(exif);
        }
    }
    let mut out = b"RIFF".to_vec();
    out.extend((body.len() as u32).to_le_bytes());
    out.extend(body);
    Ok(out)
}

// --- GIF ------------------------------------------------------------------

/// The application extensions a GIF keeps: its loop count, and its colour
/// profile.
const GIF_KEEPS: [&[u8]; 3] = [b"NETSCAPE2.0", b"ANIMEXTS1.0", b"ICCRGBG1012"];

/// Where the run of sub-blocks starting at `pos` ends, past its zero
/// terminator.
fn gif_blocks_end(src: &[u8], mut pos: usize) -> std::result::Result<usize, String> {
    loop {
        let n = *src.get(pos).ok_or("it ends early.")? as usize;
        pos += 1 + n;
        if n == 0 {
            return Ok(pos);
        }
    }
}

fn strip_gif(src: &[u8]) -> std::result::Result<Vec<u8>, String> {
    if !(src.starts_with(b"GIF87a") || src.starts_with(b"GIF89a")) || src.len() < 13 {
        return Err("it isn't a GIF.".to_owned());
    }
    let packed = src[10];
    let mut pos = 13;
    if packed & 0x80 != 0 {
        pos += 3 << ((packed & 7) + 1);
    }
    let mut out = src.get(..pos).ok_or("it ends early.")?.to_vec();
    loop {
        match *src.get(pos).ok_or("it ends early.")? {
            0x3B => {
                out.push(0x3B);
                return Ok(out);
            }
            0x21 => {
                let label = *src.get(pos + 1).ok_or("it ends early.")?;
                let end = gif_blocks_end(src, pos + 2)?;
                let first = src
                    .get(pos + 2)
                    .and_then(|&n| src.get(pos + 3..pos + 3 + n as usize))
                    .unwrap_or_default();
                let keep = match label {
                    // Frame timing and plain text draw the picture.
                    0xF9 | 0x01 => true,
                    0xFF => GIF_KEEPS.contains(&first),
                    _ => false,
                };
                if keep {
                    out.extend_from_slice(src.get(pos..end).ok_or("it ends early.")?);
                }
                pos = end;
            }
            0x2C => {
                let descriptor = src.get(pos..pos + 10).ok_or("it ends early.")?;
                let mut data = pos + 10;
                if descriptor[9] & 0x80 != 0 {
                    data += 3 << ((descriptor[9] & 7) + 1);
                }
                // The LZW code size, then the picture's sub-blocks.
                let end = gif_blocks_end(src, data + 1)?;
                out.extend_from_slice(src.get(pos..end).ok_or("it ends early.")?);
                pos = end;
            }
            _ => return Err("a block is damaged.".to_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rust_sources_hold_no_raw_nul_bytes() {
        // Git takes a file with one for binary, and shows no diff of it
        // (M5): a NUL in a literal is written `\0`.
        fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut files = vec![];
        for krate in std::fs::read_dir(&crates).unwrap() {
            let krate = krate.unwrap().path();
            for part in ["src", "tests", "examples", "benches"] {
                if krate.join(part).is_dir() {
                    walk(&krate.join(part), &mut files);
                }
            }
        }
        assert!(files.len() > 50, "{}", files.len());
        let with_nul: Vec<_> = files
            .iter()
            .filter(|f| std::fs::read(f).unwrap().contains(&0))
            .collect();
        assert!(with_nul.is_empty(), "{with_nul:?}");
    }

    fn jpeg(segments: &[Vec<u8>], scan: &[u8], tail: &[u8]) -> Vec<u8> {
        let mut j = vec![0xFF, 0xD8];
        for s in segments {
            j.extend_from_slice(s);
        }
        j.extend(segment(0xDA, &[1, 1, 0, 0, 63, 0]).unwrap());
        j.extend_from_slice(scan);
        j.extend([0xFF, 0xD9]);
        j.extend_from_slice(tail);
        j
    }

    fn markers(j: &[u8]) -> Vec<(u8, Vec<u8>)> {
        let mut found = vec![];
        let mut pos = 2;
        while pos + 4 <= j.len() && j[pos + 1] != 0xDA {
            let len = u16::from_be_bytes([j[pos + 2], j[pos + 3]]) as usize;
            found.push((j[pos + 1], j[pos + 4..pos + 2 + len].to_vec()));
            pos += 2 + len;
        }
        found
    }

    #[test]
    fn a_jpeg_keeps_its_picture_profile_and_turn_and_nothing_else() {
        let jfif = segment(0xE0, b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0").unwrap();
        let mut camera = orientation_exif(6);
        camera.extend_from_slice(b"Canon EOS");
        let exif = segment(0xE1, &[EXIF_HEADER, &camera].concat()).unwrap();
        let xmp = segment(0xE1, &[XMP_HEADER, b"<x:xmpmeta/>"].concat()).unwrap();
        let icc = segment(0xE2, &[ICC_HEADER, b"\x01\x01profile"].concat()).unwrap();
        let mpf = segment(0xE2, b"MPF\0index").unwrap();
        let iptc = segment(0xED, b"Photoshop 3.0\0city").unwrap();
        let adobe = segment(0xEE, b"Adobe\0\x64\0\0\0\0\x01").unwrap();
        let comment = segment(0xFE, b"taken at home").unwrap();
        let dqt = segment(0xDB, &[0; 65]).unwrap();
        // Stuffed bytes and a restart marker inside the coded picture.
        let scan = [1, 2, 0xFF, 0x00, 3, 0xFF, 0xD0, 4];
        let tail = b"\xFF\xD8second picture with Exif";
        let src = jpeg(
            &[
                jfif.clone(),
                exif,
                xmp,
                icc.clone(),
                mpf,
                iptc,
                adobe.clone(),
                comment,
                dqt.clone(),
            ],
            &scan,
            b"\xFF\xD8second picture with Exif",
        );
        let out = strip_jpeg(&src).unwrap();
        let kept = markers(&out);
        let kinds: Vec<u8> = kept.iter().map(|(m, _)| *m).collect();
        assert_eq!(kinds, [0xE0, 0xE1, 0xE2, 0xEE, 0xDB]);
        assert_eq!(kept[1].1, [EXIF_HEADER, &orientation_exif(6)].concat());
        assert_eq!(kept[2].1, icc[4..]);
        // The picture from its scan to its end, byte for byte, and nothing
        // after.
        let at = |j: &[u8]| j.windows(2).position(|w| w == [0xFF, 0xDA]).unwrap();
        assert_eq!(out[at(&out)..], src[at(&src)..src.len() - tail.len()]);
        assert!(out.ends_with(&[0xFF, 0xD9]));
    }

    #[test]
    fn an_upright_jpeg_gets_no_exif_at_all() {
        let exif = segment(0xE1, &[EXIF_HEADER, &orientation_exif(1)].concat()).unwrap();
        let src = jpeg(&[exif], &[5, 6], b"");
        let out = strip_jpeg(&src).unwrap();
        assert!(markers(&out).is_empty());
        let clean = jpeg(&[], &[5, 6], b"");
        assert_eq!(strip_jpeg(&clean).unwrap(), clean);
    }

    #[test]
    fn a_progressive_jpeg_keeps_every_scan() {
        let mut src = jpeg(&[], &[1, 2, 3], b"");
        src.truncate(src.len() - 2);
        src.extend(segment(0xC4, &[0; 17]).unwrap());
        src.extend(segment(0xFE, b"between scans").unwrap());
        src.extend(segment(0xDA, &[1, 1, 0, 0, 63, 0]).unwrap());
        src.extend([7, 8, 0xFF, 0x00, 9, 0xFF, 0xD9]);
        let out = strip_jpeg(&src).unwrap();
        assert!(find(&out, b"between scans").is_none());
        assert_eq!(out.len(), src.len() - (4 + 13));
        assert!(out.ends_with(&[7, 8, 0xFF, 0x00, 9, 0xFF, 0xD9]));
    }

    #[test]
    fn a_damaged_jpeg_is_an_error_not_a_panic() {
        for bad in [
            &b"\xFF\xD8"[..],
            b"\xFF\xD8\xFF\xE1\x00",
            b"\xFF\xD8\xFF\xE1\xFF\xFFxx",
            b"\xFF\xD8\xFF\xDA\x00\x02\x01\x02",
            b"\xFF\xD8\xFF\xE1\x00\x01",
            b"not a jpeg",
        ] {
            assert!(strip_jpeg(bad).is_err(), "{bad:?}");
        }
    }

    fn png(chunks: &[Vec<u8>]) -> Vec<u8> {
        let mut p = PNG_SIGNATURE.to_vec();
        p.extend(chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0]));
        for c in chunks {
            p.extend_from_slice(c);
        }
        p.extend(chunk(b"IDAT", b"pixels"));
        p.extend(chunk(b"IEND", b""));
        p
    }

    fn kinds(p: &[u8]) -> Vec<String> {
        png_chunks(p)
            .unwrap()
            .iter()
            .map(|c| String::from_utf8_lossy(&c.kind).into_owned())
            .collect()
    }

    #[test]
    fn a_png_keeps_its_picture_profile_and_turn_and_nothing_else() {
        let src = png(&[
            chunk(b"iCCP", b"icc\0\0data"),
            chunk(b"sRGB", &[0]),
            chunk(b"gAMA", &[0, 0, 0xB1, 0x8F]),
            chunk(b"pHYs", &[0; 9]),
            chunk(b"eXIf", &orientation_exif(8)),
            chunk(b"iTXt", b"XML:com.adobe.xmp\0\0\0\0\0<x:xmpmeta/>"),
            chunk(b"tEXt", b"Comment\0home"),
            chunk(b"zTXt", b"Raw profile type exif\0\0xx"),
            chunk(b"tIME", &[0; 7]),
            chunk(b"caBX", b"manifest"),
        ]);
        let out = strip_png(&src).unwrap();
        assert_eq!(
            kinds(&out),
            ["IHDR", "iCCP", "sRGB", "gAMA", "pHYs", "eXIf", "IDAT", "IEND"]
        );
        assert!(find(&out, &orientation_exif(8)).is_some());
        let clean = png(&[chunk(b"sRGB", &[0])]);
        assert_eq!(strip_png(&clean).unwrap(), clean);
        assert!(strip_png(&src[..src.len() - 5]).is_err());
    }

    fn webp(chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let mut body = b"WEBP".to_vec();
        for (kind, data) in chunks {
            body.extend_from_slice(*kind);
            body.extend((data.len() as u32).to_le_bytes());
            body.extend_from_slice(data);
            if data.len() % 2 == 1 {
                body.push(0);
            }
        }
        let mut w = b"RIFF".to_vec();
        w.extend((body.len() as u32).to_le_bytes());
        w.extend(body);
        w
    }

    #[test]
    fn a_webp_keeps_its_picture_profile_and_turn_and_nothing_else() {
        let flags = 0x20 | WEBP_EXIF | WEBP_XMP | 0x10;
        let vp8x = [flags, 0, 0, 0, 1, 0, 0, 1, 0, 0];
        let mut camera = orientation_exif(3);
        camera.extend_from_slice(b"Pixel 9");
        let src = webp(&[
            (b"VP8X", &vp8x),
            (b"ICCP", b"profile"),
            (b"ALPH", b"a"),
            (b"VP8 ", b"picture"),
            (b"EXIF", &camera),
            (b"XMP ", b"<x:xmpmeta/>"),
            (b"JUNK", b"x"),
        ]);
        let out = strip_webp(&src).unwrap();
        let want = webp(&[
            (
                b"VP8X",
                &[0x20 | 0x10 | WEBP_EXIF, 0, 0, 0, 1, 0, 0, 1, 0, 0],
            ),
            (b"ICCP", b"profile"),
            (b"ALPH", b"a"),
            (b"VP8 ", b"picture"),
            (b"EXIF", &orientation_exif(3)),
        ]);
        assert_eq!(out, want);
        // Upright: the Exif flag goes too.
        let src = webp(&[
            (b"VP8X", &vp8x),
            (b"VP8L", b"pic"),
            (b"EXIF", &orientation_exif(1)),
        ]);
        let want = webp(&[
            (b"VP8X", &[0x20 | 0x10, 0, 0, 0, 1, 0, 0, 1, 0, 0]),
            (b"VP8L", b"pic"),
        ]);
        assert_eq!(strip_webp(&src).unwrap(), want);
        assert!(strip_webp(b"RIFF\0\0\0\0WEBPVP8 \xFF\0\0\0").is_err());
        // A VP8X too short to hold its flags is damaged: no panic, and the
        // next chunk's type is never written over.
        assert!(strip_webp(b"RIFF\x0c\0\0\0WEBPVP8X\0\0\0\0").is_err());
        let short = webp(&[(b"VP8X", b""), (b"VP8L", b"pic")]);
        assert!(strip_webp(&short).is_err());
        let short = webp(&[(b"VP8X", &[0x08; 9]), (b"VP8L", b"pic")]);
        assert!(strip_webp(&short).is_err());
    }

    fn gif(blocks: &[&[u8]]) -> Vec<u8> {
        // 1 x 1, a global table of two colours.
        let mut g = b"GIF89a\x01\0\x01\0\x80\0\0".to_vec();
        g.extend([0, 0, 0, 255, 255, 255]);
        for b in blocks {
            g.extend_from_slice(b);
        }
        g.push(0x3B);
        g
    }

    #[test]
    fn a_gif_keeps_its_frames_and_loop_and_nothing_else() {
        let looped: &[u8] = b"\x21\xFF\x0BNETSCAPE2.0\x03\x01\0\0\0";
        let xmp: &[u8] = b"\x21\xFF\x0BXMP DataXMP\x05<xmp>\0";
        let comment: &[u8] = b"\x21\xFE\x04home\0";
        let timing: &[u8] = b"\x21\xF9\x04\0\x0A\0\0\0";
        let frame: &[u8] = b"\x2C\0\0\0\0\x01\0\x01\0\0\x02\x02\x44\x01\0";
        let src = gif(&[looped, xmp, comment, timing, frame, timing, frame]);
        let out = strip_gif(&src).unwrap();
        assert_eq!(out, gif(&[looped, timing, frame, timing, frame]));
        assert!(strip_gif(&src[..src.len() - 3]).is_err());
    }

    #[test]
    fn kept_metadata_says_upright() {
        let xmp =
            b"<rdf:Description tiff:Orientation=\"6\"/><tiff:Orientation>8</tiff:Orientation>";
        let out = upright_xmp(xmp.to_vec());
        assert_eq!(
            out,
            b"<rdf:Description tiff:Orientation=\"1\"/><tiff:Orientation>1</tiff:Orientation>"
        );
        assert_eq!(turn_in(&orientation_exif(6)), Some(6));
        assert_eq!(
            turn_in(&[EXIF_HEADER, &orientation_exif(5)].concat()),
            Some(5)
        );
        assert_eq!(turn_in(&orientation_exif(1)), None);
        assert_eq!(turn_in(b"junk"), None);
    }

    /// Exif with orientation `o` in its first directory and a thumbnail
    /// directory after it, in either byte order.
    fn exif_with_thumbnail(big: bool, o: u16) -> Vec<u8> {
        let u16b = |v: u16| {
            if big {
                v.to_be_bytes()
            } else {
                v.to_le_bytes()
            }
        };
        let u32b = |v: u32| {
            if big {
                v.to_be_bytes()
            } else {
                v.to_le_bytes()
            }
        };
        let mut t = if big {
            b"MM\0*".to_vec()
        } else {
            b"II*\0".to_vec()
        };
        t.extend(u32b(8));
        t.extend(u16b(1));
        t.extend(u16b(0x0112));
        t.extend(u16b(3));
        t.extend(u32b(1));
        t.extend(u16b(o));
        t.extend([0, 0]);
        // The thumbnail's directory, at 26: its size, as a camera writes.
        t.extend(u32b(26));
        t.extend(u16b(1));
        t.extend(u16b(0x0100));
        t.extend(u16b(4));
        t.extend(u32b(1));
        t.extend(u32b(160));
        t.extend(u32b(0));
        t
    }

    #[test]
    fn kept_exif_drops_the_old_thumbnail() {
        for big in [false, true] {
            let mut exif = exif_with_thumbnail(big, 6);
            without_thumbnail(&mut exif);
            assert_eq!(&exif[22..26], &[0, 0, 0, 0], "big-endian: {big}");
            // Damaged Exif is left as it is, and nothing panics.
            for n in 0..26 {
                let mut cut = exif_with_thumbnail(big, 6)[..n].to_vec();
                let before = cut.clone();
                without_thumbnail(&mut cut);
                assert_eq!(cut, before);
            }
        }
        // Read from a photo, the kept Exif is upright and has no thumbnail.
        use image::ImageEncoder;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("turned.jpg");
        let mut bytes = Vec::new();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new(&mut bytes);
        encoder
            .set_exif_metadata(exif_with_thumbnail(false, 6))
            .unwrap();
        encoder
            .write_image(&[90u8; 4 * 2 * 3], 4, 2, image::ExtendedColorType::Rgb8)
            .unwrap();
        std::fs::write(&path, bytes).unwrap();
        let exif = Kept::read(&path).exif.unwrap();
        assert_eq!(
            Orientation::from_exif_chunk(&exif),
            Some(Orientation::NoTransforms)
        );
        assert_eq!(&exif[22..26], &[0, 0, 0, 0]);
    }

    #[test]
    fn kept_metadata_goes_into_a_new_jpeg_png_and_webp() {
        let kept = Kept {
            exif: Some(orientation_exif(1)),
            xmp: Some(b"<x:xmpmeta/>".to_vec()),
            iptc: Some(b"8BIM".to_vec()),
        };
        let jfif = segment(0xE0, b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0").unwrap();
        let j = kept.embed(jpeg(&[jfif], &[1], b""), "jpg").unwrap();
        let found: Vec<u8> = markers(&j).iter().map(|(m, _)| *m).collect();
        assert_eq!(found, [0xE0, 0xE1, 0xE1, 0xED]);
        let p = kept.embed(png(&[]), "png").unwrap();
        assert_eq!(kinds(&p), ["IHDR", "eXIf", "iTXt", "IDAT", "IEND"]);
        // Nothing to keep leaves the file as it was.
        let plain = png(&[]);
        assert_eq!(Kept::default().embed(plain.clone(), "png").unwrap(), plain);
    }
}
