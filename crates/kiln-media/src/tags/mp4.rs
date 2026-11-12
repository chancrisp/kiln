//! iTunes-style MP4 tags: the `udta/meta/ilst` boxes of an M4A.

use crate::types::Tags;

/// An MP4 box: a 32-bit size, the four-character name, then `body`.
pub(crate) fn mp4_box(name: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(&((8 + body.len()) as u32).to_be_bytes());
    out.extend_from_slice(name);
    out.extend_from_slice(body);
    out
}

/// A full box: a box whose body starts with a version byte and 24 bits of flags.
pub(crate) fn full_box(name: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(4 + body.len());
    b.push(version);
    b.extend_from_slice(&flags.to_be_bytes()[1..]);
    b.extend_from_slice(body);
    mp4_box(name, &b)
}

/// The `data` box's well-known types.
const TEXT: u32 = 1;
const BINARY: u32 = 0;
const JPEG: u32 = 13;
const PNG: u32 = 14;

/// One `ilst` item: the item box holding a `data` box of `kind`.
fn item(name: &[u8; 4], kind: u32, payload: &[u8]) -> Vec<u8> {
    let mut data = Vec::with_capacity(8 + payload.len());
    data.extend_from_slice(&kind.to_be_bytes()); // version 0, then the type
    data.extend_from_slice(&0u32.to_be_bytes()); // locale
    data.extend_from_slice(payload);
    mp4_box(name, &mp4_box(b"data", &data))
}

/// A number and total as `trkn` and `disk` keep them: eight bytes, the number
/// in bytes 2-3 and the total in bytes 4-5.
fn part(n: u32, total: Option<u32>) -> [u8; 8] {
    let n = n.min(u16::MAX as u32) as u16;
    let t = total.unwrap_or(0).min(u16::MAX as u32) as u16;
    let mut b = [0u8; 8];
    b[2..4].copy_from_slice(&n.to_be_bytes());
    b[4..6].copy_from_slice(&t.to_be_bytes());
    b
}

/// The image type for a cover, from its MIME type or, failing that, its first
/// bytes. `None` for anything but JPEG and PNG.
fn cover_kind(mime: &str, data: &[u8]) -> Option<u32> {
    match mime.trim().to_ascii_lowercase().as_str() {
        "image/jpeg" | "image/jpg" => Some(JPEG),
        "image/png" => Some(PNG),
        _ if data.starts_with(&[0xFF, 0xD8, 0xFF]) => Some(JPEG),
        _ if data.starts_with(&[0x89, b'P', b'N', b'G']) => Some(PNG),
        _ => None,
    }
}

/// A free-form `----` item under the `com.apple.iTunes` namespace.
fn freeform(name: &str, value: &str) -> Vec<u8> {
    let mut body = full_box(b"mean", 0, 0, b"com.apple.iTunes");
    body.extend(full_box(b"name", 0, 0, name.as_bytes()));
    let mut data = TEXT.to_be_bytes().to_vec();
    data.extend_from_slice(&0u32.to_be_bytes());
    data.extend_from_slice(value.as_bytes());
    body.extend(mp4_box(b"data", &data));
    mp4_box(b"----", &body)
}

/// The `ilst` box for `tags`, plus an `iTunSMPB` item when `smpb` is given.
/// Empty values are left out.
pub fn ilst(tags: &Tags, smpb: Option<&str>) -> Vec<u8> {
    let mut body = Vec::new();
    let mut text = |name: &[u8; 4], v: &Option<String>| {
        if let Some(v) = v.as_deref().map(|s| s.replace('\0', "")) {
            if !v.is_empty() {
                body.extend(item(name, TEXT, v.as_bytes()));
            }
        }
    };
    text(b"\xA9nam", &tags.title);
    text(b"\xA9ART", &tags.artist);
    text(b"\xA9alb", &tags.album);
    text(b"aART", &tags.album_artist);
    text(b"\xA9gen", &tags.genre);
    text(b"\xA9cmt", &tags.comment);
    text(b"\xA9day", &tags.year);
    if let Some((n, total)) = tags.track {
        body.extend(item(b"trkn", BINARY, &part(n, total)));
    }
    if let Some((n, total)) = tags.disc {
        body.extend(item(b"disk", BINARY, &part(n, total)));
    }
    if let Some(p) = &tags.picture {
        if let Some(kind) = cover_kind(&p.mime, &p.data) {
            body.extend(item(b"covr", kind, &p.data));
        }
    }
    if let Some(s) = smpb {
        body.extend(freeform("iTunSMPB", s));
    }
    mp4_box(b"ilst", &body)
}

/// The `udta` box: `meta` with its `mdir` handler and the `ilst`.
pub fn udta(tags: &Tags, smpb: Option<&str>) -> Vec<u8> {
    let mut hdlr = vec![0u8; 4]; // pre-defined
    hdlr.extend_from_slice(b"mdir");
    hdlr.extend_from_slice(b"appl");
    hdlr.extend_from_slice(&[0u8; 8]);
    hdlr.push(0); // empty name
    let mut meta = full_box(b"hdlr", 0, 0, &hdlr);
    meta.extend(ilst(tags, smpb));
    mp4_box(b"udta", &full_box(b"meta", 0, 0, &meta))
}

/// The iTunSMPB text: encoder delay, padding and the real length, in hex.
pub fn smpb(priming: u32, padding: u32, frames: u64) -> String {
    format!(
        " 00000000 {priming:08X} {padding:08X} {frames:016X} 00000000 00000000 00000000 \
         00000000 00000000 00000000 00000000 00000000"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Picture;

    #[test]
    fn items_are_data_boxes_with_their_types() {
        let tags = Tags {
            title: Some("T".into()),
            track: Some((3, Some(12))),
            disc: Some((1, None)),
            picture: Some(Picture {
                mime: "image/jpeg".into(),
                data: vec![0xFF, 0xD8, 0xFF, 0xE0],
            }),
            ..Tags::default()
        };
        let b = ilst(&tags, None);
        assert_eq!(&b[4..8], b"ilst");
        let title = item(b"\xA9nam", 1, b"T");
        assert_eq!(
            title,
            [
                &[0, 0, 0, 25][..],
                b"\xA9nam",
                &[0, 0, 0, 17],
                b"data",
                &[0, 0, 0, 1, 0, 0, 0, 0],
                b"T"
            ]
            .concat()
        );
        assert!(b.windows(title.len()).any(|w| w == title));
        let trkn = item(b"trkn", 0, &[0, 0, 0, 3, 0, 12, 0, 0]);
        assert!(b.windows(trkn.len()).any(|w| w == trkn));
        let disk = item(b"disk", 0, &[0, 0, 0, 1, 0, 0, 0, 0]);
        assert!(b.windows(disk.len()).any(|w| w == disk));
        let covr = item(b"covr", 13, &[0xFF, 0xD8, 0xFF, 0xE0]);
        assert!(b.windows(covr.len()).any(|w| w == covr));
    }

    #[test]
    fn covers_are_typed_png_or_jpeg_and_others_are_left_out() {
        assert_eq!(cover_kind("image/png", &[]), Some(PNG));
        assert_eq!(cover_kind("IMAGE/JPEG", &[]), Some(JPEG));
        assert_eq!(cover_kind("", &[0x89, b'P', b'N', b'G']), Some(PNG));
        assert_eq!(cover_kind("image/gif", b"GIF89a"), None);
    }

    #[test]
    fn smpb_reads_like_itunes() {
        let s = smpb(1024, 448, 1_323_000);
        assert!(s.starts_with(" 00000000 00000400 000001C0 0000000000142FF8 "));
        assert_eq!(s.split(' ').count(), 13);
    }
}
