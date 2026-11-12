//! ID3v2 tags: written as v2.4 with UTF-8 text, read as v2.3 or v2.4.
//!
//! Only the frames behind the [`Tags`] fields are handled: TIT2, TPE1, TALB,
//! TPE2, TCON, COMM, TRCK, TPOS, TDRC (TYER in v2.3) and APIC. A frame that
//! can't be read is skipped, never fatal.

use crate::types::{Picture, Tags};
use std::borrow::Cow;

/// The largest tag body a syncsafe size can describe.
const MAX_BODY: usize = 0x0FFF_FFFF;

/// Encodes `tags` as an ID3v2.4 tag. Returns an empty `Vec` when there is
/// nothing to write, so callers can skip the tag altogether.
pub fn write_v24(tags: &Tags) -> Vec<u8> {
    let mut frames = Vec::new();
    let text = |frames: &mut Vec<u8>, id: &[u8; 4], v: Option<&str>| {
        if let Some(s) = v.filter(|s| !s.is_empty()) {
            let mut body = vec![3u8];
            body.extend_from_slice(s.as_bytes());
            put_frame(frames, id, &body);
        }
    };
    text(&mut frames, b"TIT2", tags.title.as_deref());
    text(&mut frames, b"TPE1", tags.artist.as_deref());
    text(&mut frames, b"TALB", tags.album.as_deref());
    text(&mut frames, b"TPE2", tags.album_artist.as_deref());
    text(&mut frames, b"TCON", tags.genre.as_deref());
    text(&mut frames, b"TRCK", tags.track.map(part_of_set).as_deref());
    text(&mut frames, b"TPOS", tags.disc.map(part_of_set).as_deref());
    text(&mut frames, b"TDRC", tags.year.as_deref());
    if let Some(c) = tags.comment.as_deref().filter(|s| !s.is_empty()) {
        // encoding, language, empty description, text
        let mut body = vec![3u8, b'e', b'n', b'g', 0];
        body.extend_from_slice(c.as_bytes());
        put_frame(&mut frames, b"COMM", &body);
    }
    if let Some(p) = &tags.picture {
        // encoding, MIME, front cover, empty description, data
        let mut body = vec![3u8];
        body.extend_from_slice(p.mime.as_bytes());
        body.extend_from_slice(&[0, 3, 0]);
        body.extend_from_slice(&p.data);
        // A cover too big for the tag is dropped rather than breaking the tag.
        if frames.len() + body.len() + 10 <= MAX_BODY {
            put_frame(&mut frames, b"APIC", &body);
        }
    }
    if frames.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(10 + frames.len());
    out.extend_from_slice(b"ID3");
    out.extend_from_slice(&[4, 0, 0]);
    out.extend_from_slice(&syncsafe(frames.len() as u32));
    out.extend_from_slice(&frames);
    out
}

fn part_of_set((n, total): (u32, Option<u32>)) -> String {
    match total {
        Some(t) => format!("{n}/{t}"),
        None => n.to_string(),
    }
}

fn put_frame(out: &mut Vec<u8>, id: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(id);
    out.extend_from_slice(&syncsafe(body.len() as u32));
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(body);
}

fn syncsafe(v: u32) -> [u8; 4] {
    [
        ((v >> 21) & 0x7F) as u8,
        ((v >> 14) & 0x7F) as u8,
        ((v >> 7) & 0x7F) as u8,
        (v & 0x7F) as u8,
    ]
}

fn unsyncsafe(b: &[u8]) -> Option<usize> {
    if b.iter().any(|x| x & 0x80 != 0) {
        return None;
    }
    Some(((b[0] as usize) << 21) | ((b[1] as usize) << 14) | ((b[2] as usize) << 7) | b[3] as usize)
}

/// Reads an ID3v2.3 or v2.4 tag from the start of `bytes`. Anything that
/// isn't one, or a version this doesn't know, gives empty tags.
pub fn read(bytes: &[u8]) -> Tags {
    let mut tags = Tags::default();
    if bytes.len() < 10 || &bytes[0..3] != b"ID3" {
        return tags;
    }
    let major = bytes[3];
    if major != 3 && major != 4 {
        return tags;
    }
    let flags = bytes[5];
    let Some(size) = unsyncsafe(&bytes[6..10]) else {
        return tags;
    };
    let end = (10 + size).min(bytes.len());
    let mut body: Cow<[u8]> = Cow::Borrowed(&bytes[10..end]);
    let tag_unsync = flags & 0x80 != 0;
    // v2.3 unsynchronises the whole tag at once; v2.4 does it frame by frame.
    if major == 3 && tag_unsync {
        body = Cow::Owned(unsync(&body));
    }
    let mut pos = 0usize;
    if flags & 0x40 != 0 {
        // Extended header: v2.3 size excludes its own 4 bytes, v2.4 includes them.
        if body.len() < 4 {
            return tags;
        }
        pos = if major == 3 {
            4 + u32::from_be_bytes([body[0], body[1], body[2], body[3]]) as usize
        } else {
            unsyncsafe(&body[0..4]).unwrap_or(body.len())
        };
    }

    let mut comment: Option<(bool, String)> = None;
    let mut picture: Option<(u8, Picture)> = None;
    while pos + 10 <= body.len() {
        let id = [body[pos], body[pos + 1], body[pos + 2], body[pos + 3]];
        if id[0] == 0
            || !id
                .iter()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        {
            break;
        }
        let size = if major == 3 {
            u32::from_be_bytes([body[pos + 4], body[pos + 5], body[pos + 6], body[pos + 7]])
                as usize
        } else {
            match unsyncsafe(&body[pos + 4..pos + 8]) {
                Some(s) => s,
                None => break,
            }
        };
        let flag2 = body[pos + 9];
        let start = pos + 10;
        let Some(stop) = start.checked_add(size).filter(|&s| s <= body.len()) else {
            break;
        };
        pos = stop;
        let Some(data) = frame_data(major, tag_unsync, flag2, &body[start..stop]) else {
            continue;
        };
        match &id {
            b"TIT2" => set_text(&mut tags.title, &data),
            b"TPE1" => set_text(&mut tags.artist, &data),
            b"TALB" => set_text(&mut tags.album, &data),
            b"TPE2" => set_text(&mut tags.album_artist, &data),
            b"TCON" => set_text(&mut tags.genre, &data),
            b"TRCK" => tags.track = text_of(&data).and_then(|s| parse_of_set(&s)).or(tags.track),
            b"TPOS" => tags.disc = text_of(&data).and_then(|s| parse_of_set(&s)).or(tags.disc),
            b"TDRC" => set_text(&mut tags.year, &data),
            b"TYER" if tags.year.is_none() => set_text(&mut tags.year, &data),
            b"COMM" => {
                if let Some((empty_desc, text)) = parse_comm(&data) {
                    let better = match &comment {
                        None => true,
                        Some((had_empty, _)) => empty_desc && !had_empty,
                    };
                    if better {
                        comment = Some((empty_desc, text));
                    }
                }
            }
            b"APIC" => {
                if let Some((kind, pic)) = parse_apic(&data) {
                    let better = match &picture {
                        None => true,
                        Some((k, _)) => kind == 3 && *k != 3,
                    };
                    if better {
                        picture = Some((kind, pic));
                    }
                }
            }
            _ => {}
        }
    }
    tags.comment = comment.map(|(_, t)| t).filter(|t| !t.is_empty());
    tags.picture = picture.map(|(_, p)| p);
    tags
}

/// Strips a frame's flag prefixes and undoes unsynchronisation. `None` when
/// the frame is compressed or encrypted, which Kiln doesn't read.
fn frame_data(major: u8, tag_unsync: bool, flag2: u8, raw: &[u8]) -> Option<Vec<u8>> {
    let mut data = raw;
    let mut unsynced = false;
    if major == 3 {
        // compression 0x80, encryption 0x40, grouping 0x20
        if flag2 & 0xC0 != 0 {
            return None;
        }
        if flag2 & 0x20 != 0 {
            data = data.get(1..)?;
        }
    } else {
        // compression 0x08, encryption 0x04, unsynchronised 0x02, length 0x01, grouping 0x40
        if flag2 & 0x0C != 0 {
            return None;
        }
        unsynced = flag2 & 0x02 != 0 || tag_unsync;
        if flag2 & 0x40 != 0 {
            data = data.get(1..)?;
        }
        if flag2 & 0x01 != 0 {
            data = data.get(4..)?;
        }
    }
    Some(if unsynced {
        unsync(data)
    } else {
        data.to_vec()
    })
}

/// Undoes unsynchronisation: every `FF 00` becomes `FF`.
fn unsync(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        out.push(b[i]);
        if b[i] == 0xFF && b.get(i + 1) == Some(&0) {
            i += 1;
        }
        i += 1;
    }
    out
}

fn set_text(slot: &mut Option<String>, data: &[u8]) {
    if let Some(s) = text_of(data) {
        *slot = Some(s);
    }
}

/// The text of a text frame: several values (separated by NUL) join with "/".
fn text_of(data: &[u8]) -> Option<String> {
    let (&enc, rest) = data.split_first()?;
    let s = decode(enc, rest)?;
    let parts: Vec<&str> = s
        .split('\0')
        .map(|p| p.trim_start_matches('\u{FEFF}'))
        .filter(|p| !p.is_empty())
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

fn decode(enc: u8, b: &[u8]) -> Option<String> {
    match enc {
        0 => Some(b.iter().map(|&c| c as char).collect()),
        1 => {
            if b.len() < 2 {
                return Some(String::new());
            }
            match (b[0], b[1]) {
                (0xFF, 0xFE) => Some(utf16(&b[2..], false)),
                (0xFE, 0xFF) => Some(utf16(&b[2..], true)),
                _ => Some(utf16(b, false)),
            }
        }
        2 => Some(utf16(b, true)),
        3 => Some(String::from_utf8_lossy(b).into_owned()),
        _ => None,
    }
}

fn utf16(b: &[u8], big: bool) -> String {
    let units = b.chunks_exact(2).map(|c| {
        if big {
            u16::from_be_bytes([c[0], c[1]])
        } else {
            u16::from_le_bytes([c[0], c[1]])
        }
    });
    char::decode_utf16(units)
        .map(|r| r.unwrap_or('\u{FFFD}'))
        .collect()
}

fn parse_of_set(s: &str) -> Option<(u32, Option<u32>)> {
    let mut it = s.splitn(2, '/');
    let n = it.next()?.trim().parse().ok()?;
    let total = it.next().and_then(|t| t.trim().parse().ok());
    Some((n, total))
}

/// Splits at the first terminator for the encoding (one NUL byte, or an
/// aligned pair for UTF-16): `(before, after)`.
fn split_terminated(enc: u8, b: &[u8]) -> Option<(&[u8], &[u8])> {
    if enc == 1 || enc == 2 {
        let mut i = 0;
        while i + 1 < b.len() {
            if b[i] == 0 && b[i + 1] == 0 {
                return Some((&b[..i], &b[i + 2..]));
            }
            i += 2;
        }
        None
    } else {
        let i = b.iter().position(|&c| c == 0)?;
        Some((&b[..i], &b[i + 1..]))
    }
}

/// Returns (description is empty, text).
fn parse_comm(data: &[u8]) -> Option<(bool, String)> {
    let enc = *data.first()?;
    let rest = data.get(4..)?; // encoding + 3-byte language
    let (desc, text) = split_terminated(enc, rest)?;
    let text = decode(enc, text)?;
    Some((desc.is_empty(), text.trim_end_matches('\0').to_string()))
}

/// Returns (picture type, picture).
fn parse_apic(data: &[u8]) -> Option<(u8, Picture)> {
    let enc = *data.first()?;
    let rest = &data[1..];
    let (mime, rest) = split_terminated(0, rest)?;
    let (&kind, rest) = rest.split_first()?;
    let (_desc, img) = split_terminated(enc, rest)?;
    if img.is_empty() {
        return None;
    }
    let mime = String::from_utf8_lossy(mime).into_owned();
    if mime == "-->" {
        return None; // a link, not an image
    }
    let mime = if !mime.is_empty() {
        mime
    } else if img.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png".to_string()
    } else if img.starts_with(&[0xFF, 0xD8]) {
        "image/jpeg".to_string()
    } else {
        mime
    };
    Some((
        kind,
        Picture {
            mime,
            data: img.to_vec(),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full() -> Tags {
        Tags {
            title: Some("Füße – 東京".into()),
            artist: Some("Ana".into()),
            album: Some("Album".into()),
            album_artist: Some("Various".into()),
            genre: Some("Jazz".into()),
            comment: Some("hello\nworld".into()),
            track: Some((3, Some(12))),
            disc: Some((1, None)),
            year: Some("1999".into()),
            picture: Some(Picture {
                mime: "image/png".into(),
                data: vec![0x89, b'P', b'N', b'G', 0xFF, 0x00, 0xFF, 0xFF],
            }),
        }
    }

    #[test]
    fn v24_round_trips_every_field() {
        let t = full();
        assert_eq!(read(&write_v24(&t)), t);
    }

    #[test]
    fn nothing_to_write_gives_no_bytes() {
        assert!(write_v24(&Tags::default()).is_empty());
        assert_eq!(read(&[]), Tags::default());
        assert_eq!(read(b"not a tag at all"), Tags::default());
    }

    #[test]
    fn header_is_v24_with_syncsafe_size() {
        let b = write_v24(&full());
        assert_eq!(&b[0..5], b"ID3\x04\x00");
        assert_eq!(unsyncsafe(&b[6..10]).unwrap(), b.len() - 10);
    }

    fn frame23(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut f = id.to_vec();
        f.extend_from_slice(&(body.len() as u32).to_be_bytes());
        f.extend_from_slice(&[0, 0]);
        f.extend_from_slice(body);
        f
    }

    fn tag23(frames: &[u8], flags: u8) -> Vec<u8> {
        let mut t = vec![b'I', b'D', b'3', 3, 0, flags];
        t.extend_from_slice(&syncsafe(frames.len() as u32));
        t.extend_from_slice(frames);
        t
    }

    #[test]
    fn v23_utf16_text_is_read() {
        let mut title = vec![1u8, 0xFF, 0xFE];
        for u in "Füße 東京".encode_utf16() {
            title.extend_from_slice(&u.to_le_bytes());
        }
        let mut frames = frame23(b"TIT2", &title);
        frames.extend(frame23(b"TYER", b"\x002001"));
        frames.extend(frame23(b"TRCK", b"\x007/9"));
        frames.extend(frame23(b"TPE1", b"\x00Caf\xE9"));
        let t = read(&tag23(&frames, 0));
        assert_eq!(t.title.as_deref(), Some("Füße 東京"));
        assert_eq!(t.year.as_deref(), Some("2001"));
        assert_eq!(t.track, Some((7, Some(9))));
        assert_eq!(t.artist.as_deref(), Some("Café"));
    }

    #[test]
    fn a_bad_frame_is_skipped_not_fatal() {
        let mut frames = frame23(b"TIT2", &[9, b'x']); // unknown encoding
        let mut compressed = frame23(b"TALB", b"\x00zz");
        compressed[9] = 0x80; // compression flag
        frames.extend(compressed);
        frames.extend(frame23(b"APIC", b"\x00image/png\x00")); // truncated picture
        frames.extend(frame23(b"TPE1", b"\x00Ana"));
        let t = read(&tag23(&frames, 0));
        assert_eq!(t.title, None);
        assert_eq!(t.album, None);
        assert_eq!(t.picture, None);
        assert_eq!(t.artist.as_deref(), Some("Ana"));
    }

    #[test]
    fn a_frame_running_past_the_end_stops_the_read() {
        let mut frames = frame23(b"TPE1", b"\x00Ana");
        let mut bad = frame23(b"TIT2", b"\x00x");
        bad[4..8].copy_from_slice(&1000u32.to_be_bytes());
        frames.extend(bad);
        let t = read(&tag23(&frames, 0));
        assert_eq!(t.artist.as_deref(), Some("Ana"));
        assert_eq!(t.title, None);
    }

    #[test]
    fn unsynchronised_v23_tag_is_undone() {
        // The artist is "A", 0xFF, "B" (latin-1); the writer stuffs a 00 after the FF.
        let frames = frame23(b"TPE1", b"\x00A\xFFB");
        let mut stuffed = Vec::new();
        for &b in &frames {
            stuffed.push(b);
            if b == 0xFF {
                stuffed.push(0);
            }
        }
        let t = read(&tag23(&stuffed, 0x80));
        assert_eq!(t.artist.as_deref(), Some("A\u{FF}B"));
    }

    #[test]
    fn multiple_values_join_with_a_slash() {
        let mut f = Vec::new();
        put_frame(&mut f, b"TPE1", b"\x03One\0Two");
        let mut t = b"ID3\x04\x00\x00".to_vec();
        t.extend_from_slice(&syncsafe(f.len() as u32));
        t.extend(f);
        assert_eq!(read(&t).artist.as_deref(), Some("One/Two"));
    }
}
