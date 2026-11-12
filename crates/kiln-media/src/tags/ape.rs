//! APEv2 tags, as WavPack files carry them after the audio.
//!
//! Written with a header and a footer (version 2000); read from the footer, so
//! a tag without a header works too. Only the items behind the [`Tags`] fields
//! are handled: Title, Artist, Album, Album Artist, Genre, Comment, Track,
//! Disc, Year and "Cover Art (Front)" (a binary item: file name, NUL, image).
//! An item that can't be read is skipped, never fatal.

use crate::types::{Picture, Tags};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};

const PREAMBLE: &[u8; 8] = b"APETAGEX";
const VERSION: u32 = 2000;
const FOOTER_LEN: usize = 32;
const ID3V1_LEN: usize = 128;
/// The tag has a header in front of its items.
const FLAG_HAS_HEADER: u32 = 1 << 31;
/// This block is the header, not the footer.
const FLAG_IS_HEADER: u32 = 1 << 29;
/// Item type 1 (binary), in bits 1 and 2 of an item's flags.
const ITEM_BINARY: u32 = 1 << 1;
const ITEM_TYPE_MASK: u32 = 0b110;
/// A tag bigger than this is skipped rather than read.
pub const MAX_TAG: usize = 64 * 1024 * 1024;

const COVER_KEY: &str = "Cover Art (Front)";

/// Encodes `tags` as an APEv2 tag. Returns an empty `Vec` when there is
/// nothing to write, so callers can skip the tag altogether.
pub fn write(tags: &Tags) -> Vec<u8> {
    let mut items = Vec::new();
    let mut count = 0u32;
    let mut text = |key: &str, v: Option<String>| {
        if let Some(v) = v.map(|s| s.replace('\0', "")).filter(|s| !s.is_empty()) {
            put_item(&mut items, key, 0, v.as_bytes());
            count += 1;
        }
    };
    text("Title", tags.title.clone());
    text("Artist", tags.artist.clone());
    text("Album", tags.album.clone());
    text("Album Artist", tags.album_artist.clone());
    text("Genre", tags.genre.clone());
    text("Comment", tags.comment.clone());
    text("Track", tags.track.map(part_of_set));
    text("Disc", tags.disc.map(part_of_set));
    text("Year", tags.year.clone());
    if let Some(p) = tags.picture.as_ref().filter(|p| p.data.len() < MAX_TAG) {
        let mut value = format!("cover.{}", extension_of(&p.mime)).into_bytes();
        value.push(0);
        value.extend_from_slice(&p.data);
        put_item(&mut items, COVER_KEY, ITEM_BINARY, &value);
        count += 1;
    }
    if count == 0 {
        return Vec::new();
    }
    let size = (items.len() + FOOTER_LEN) as u32;
    let mut out = Vec::with_capacity(items.len() + 2 * FOOTER_LEN);
    put_block(&mut out, size, count, FLAG_HAS_HEADER | FLAG_IS_HEADER);
    out.extend_from_slice(&items);
    put_block(&mut out, size, count, FLAG_HAS_HEADER);
    out
}

fn put_block(out: &mut Vec<u8>, size: u32, count: u32, flags: u32) {
    out.extend_from_slice(PREAMBLE);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&[0u8; 8]);
}

fn put_item(out: &mut Vec<u8>, key: &str, flags: u32, value: &[u8]) {
    out.extend_from_slice(&(value.len() as u32).to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(key.as_bytes());
    out.push(0);
    out.extend_from_slice(value);
}

fn part_of_set((n, total): (u32, Option<u32>)) -> String {
    match total {
        Some(t) => format!("{n}/{t}"),
        None => n.to_string(),
    }
}

fn extension_of(mime: &str) -> &'static str {
    match mime.to_ascii_lowercase().as_str() {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        "image/webp" => "webp",
        _ => "bin",
    }
}

/// Where an APEv2 tag sits in a file: the offset it starts at, and its
/// bytes (header included, when there is one). `None` when the file ends
/// without one. An ID3v1 tag after it is stepped over.
pub fn locate(file: &mut File) -> io::Result<Option<(u64, Vec<u8>)>> {
    let len = file.seek(SeekFrom::End(0))?;
    let mut end = len;
    if len >= ID3V1_LEN as u64 {
        let mut head = [0u8; 3];
        file.seek(SeekFrom::Start(len - ID3V1_LEN as u64))?;
        file.read_exact(&mut head)?;
        if &head == b"TAG" {
            end = len - ID3V1_LEN as u64;
        }
    }
    if end < FOOTER_LEN as u64 {
        return Ok(None);
    }
    let mut footer = [0u8; FOOTER_LEN];
    file.seek(SeekFrom::Start(end - FOOTER_LEN as u64))?;
    file.read_exact(&mut footer)?;
    let Some((size, _, flags)) = parse_block(&footer) else {
        return Ok(None);
    };
    let header = if flags & FLAG_HAS_HEADER != 0 {
        FOOTER_LEN as u64
    } else {
        0
    };
    let total = size as u64 + header;
    if size < FOOTER_LEN as u32 || total > end || total > MAX_TAG as u64 {
        return Ok(None);
    }
    let start = end - total;
    let mut bytes = vec![0u8; total as usize];
    file.seek(SeekFrom::Start(start))?;
    file.read_exact(&mut bytes)?;
    Ok(Some((start, bytes)))
}

/// (size, item count, flags) of a header or footer block.
fn parse_block(b: &[u8]) -> Option<(u32, u32, u32)> {
    if b.len() < FOOTER_LEN || &b[..8] != PREAMBLE {
        return None;
    }
    let u = |at: usize| u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
    let version = u(8);
    if version != 1000 && version != 2000 {
        return None;
    }
    Some((u(12), u(16), u(20)))
}

/// Reads the tag in `bytes`, which must end with the tag's footer (an ID3v1
/// tag after it is allowed). Anything unreadable gives empty tags.
pub fn read(bytes: &[u8]) -> Tags {
    let mut end = bytes.len();
    if end >= ID3V1_LEN + FOOTER_LEN && &bytes[end - ID3V1_LEN..end - ID3V1_LEN + 3] == b"TAG" {
        end -= ID3V1_LEN;
    }
    if end < FOOTER_LEN {
        return Tags::default();
    }
    let Some((size, count, _)) = parse_block(&bytes[end - FOOTER_LEN..end]) else {
        return Tags::default();
    };
    let size = size as usize;
    if size < FOOTER_LEN || size > end {
        return Tags::default();
    }
    parse_items(&bytes[end - size..end - FOOTER_LEN], count)
}

fn parse_items(mut items: &[u8], count: u32) -> Tags {
    let mut tags = Tags::default();
    for _ in 0..count {
        if items.len() < 8 {
            break;
        }
        let len = u32::from_le_bytes([items[0], items[1], items[2], items[3]]) as usize;
        let flags = u32::from_le_bytes([items[4], items[5], items[6], items[7]]);
        let rest = &items[8..];
        let Some(nul) = rest.iter().position(|&b| b == 0) else {
            break;
        };
        let key = &rest[..nul];
        let after = &rest[nul + 1..];
        if len > after.len() {
            break;
        }
        let value = &after[..len];
        items = &after[len..];
        let Ok(key) = std::str::from_utf8(key) else {
            continue;
        };
        apply(&mut tags, key, flags, value);
    }
    tags
}

fn apply(tags: &mut Tags, key: &str, flags: u32, value: &[u8]) {
    let kind = flags & ITEM_TYPE_MASK;
    let key = key.to_ascii_lowercase();
    if kind == ITEM_BINARY {
        if key == COVER_KEY.to_ascii_lowercase() && tags.picture.is_none() {
            tags.picture = cover_of(value);
        }
        return;
    }
    if kind != 0 {
        return; // an external locator, not text
    }
    // Several values are separated by NULs: the first one is taken.
    let first = value.split(|&b| b == 0).next().unwrap_or(&[]);
    let Ok(text) = std::str::from_utf8(first) else {
        return;
    };
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    let slot = match key.as_str() {
        "title" => &mut tags.title,
        "artist" => &mut tags.artist,
        "album" => &mut tags.album,
        "album artist" | "albumartist" => &mut tags.album_artist,
        "genre" => &mut tags.genre,
        "comment" => &mut tags.comment,
        "year" | "date" => &mut tags.year,
        "track" => {
            tags.track = tags.track.or_else(|| parse_part(text));
            return;
        }
        "disc" | "discnumber" => {
            tags.disc = tags.disc.or_else(|| parse_part(text));
            return;
        }
        _ => return,
    };
    if slot.is_none() {
        *slot = Some(text.to_string());
    }
}

/// "3" or "3/12".
fn parse_part(s: &str) -> Option<(u32, Option<u32>)> {
    let mut it = s.splitn(2, '/');
    let n = it.next()?.trim().parse().ok()?;
    let total = it.next().and_then(|t| t.trim().parse().ok());
    Some((n, total))
}

/// The picture in a "Cover Art" value: a file name, a NUL, then the image.
fn cover_of(value: &[u8]) -> Option<Picture> {
    let nul = value.iter().position(|&b| b == 0)?;
    let name = std::str::from_utf8(&value[..nul]).unwrap_or("");
    let data = &value[nul + 1..];
    if data.is_empty() {
        return None;
    }
    let mime = sniff(data).or_else(|| from_name(name))?;
    Some(Picture {
        mime: mime.to_string(),
        data: data.to_vec(),
    })
}

fn sniff(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("image/png")
    } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if data.starts_with(b"GIF8") {
        Some("image/gif")
    } else if data.starts_with(b"BM") {
        Some("image/bmp")
    } else if data.len() > 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

fn from_name(name: &str) -> Option<&'static str> {
    match name.rsplit('.').next()?.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "bmp" => Some("image/bmp"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full() -> Tags {
        Tags {
            title: Some("F\u{fc}\u{df}e \u{2013} \u{6771}\u{4eac}".into()),
            artist: Some("A".into()),
            album: Some("Al".into()),
            album_artist: Some("AA".into()),
            genre: Some("G".into()),
            comment: Some("C".into()),
            track: Some((3, Some(12))),
            disc: Some((1, Some(2))),
            year: Some("2001".into()),
            picture: Some(Picture {
                mime: "image/png".into(),
                data: vec![0x89, b'P', b'N', b'G', 1, 2, 3],
            }),
        }
    }

    #[test]
    fn round_trips_every_field() {
        let t = full();
        assert_eq!(read(&write(&t)), t);
    }

    #[test]
    fn nothing_to_write_gives_no_bytes() {
        assert!(write(&Tags::default()).is_empty());
        assert_eq!(read(&[]), Tags::default());
    }

    #[test]
    fn a_tag_without_a_header_and_with_id3v1_after_it_reads() {
        let t = full();
        let bytes = write(&t);
        // Drop the header, add an ID3v1 tag after the footer.
        let mut headless = bytes[FOOTER_LEN..].to_vec();
        headless.extend_from_slice(b"TAG");
        headless.extend_from_slice(&[0u8; ID3V1_LEN - 3]);
        assert_eq!(read(&headless), t);
    }

    #[test]
    fn damaged_tags_never_panic() {
        let bytes = write(&full());
        for cut in 0..bytes.len() {
            let _ = read(&bytes[..cut]);
            let _ = read(&bytes[cut..]);
        }
        let mut bad = bytes.clone();
        bad[FOOTER_LEN] = 0xFF; // the first item's length runs past the tag
        let _ = read(&bad);
    }

    #[test]
    fn item_keys_ignore_case_and_extra_values_are_dropped() {
        let mut items = Vec::new();
        put_item(&mut items, "TITLE", 0, b"One\0Two");
        put_item(&mut items, "track", 0, b"7");
        let size = (items.len() + FOOTER_LEN) as u32;
        let mut tag = Vec::new();
        tag.extend_from_slice(&items);
        put_block(&mut tag, size, 2, 0);
        let t = read(&tag);
        assert_eq!(t.title.as_deref(), Some("One"));
        assert_eq!(t.track, Some((7, None)));
    }
}
