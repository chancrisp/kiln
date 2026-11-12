//! What a HEIC or AVIF says about its main picture that Windows' decoders
//! don't pass on: its colour profile (a `colr` property of type `prof` or
//! `rICC`) and whether it has an alpha plane (an `auxC` property naming
//! alpha). Read from the file's `meta` box, with every length checked; a
//! file that doesn't parse has neither.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// The largest `meta` box read, in bytes. Real ones are a few kilobytes,
/// or a few hundred with a large colour profile.
const MAX_META: u64 = 16 << 20;

/// The URNs an `auxC` property uses for an alpha plane: AVIF's (MIAF) and
/// HEVC's.
const ALPHA: [&[u8]; 2] = [
    b"urn:mpeg:mpegB:cicp:systems:auxiliary:alpha",
    b"urn:mpeg:hevc:2015:auxid:1",
];

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Extras {
    /// The main picture's ICC profile, as stored.
    pub icc: Option<Vec<u8>>,
    /// True when the file holds an alpha plane.
    pub alpha: bool,
}

/// The extras of the HEIC or AVIF at `path`.
pub(crate) fn extras(path: &Path) -> Extras {
    File::open(path)
        .ok()
        .and_then(|mut f| meta_box(&mut f))
        .map(|meta| extras_of(&meta))
        .unwrap_or_default()
}

/// The body of the top-level `meta` box, after its version and flags.
fn meta_box(file: &mut (impl Read + Seek)) -> Option<Vec<u8>> {
    let end = file.seek(SeekFrom::End(0)).ok()?;
    let mut at = 0u64;
    while at + 8 <= end {
        file.seek(SeekFrom::Start(at)).ok()?;
        let mut head = [0u8; 16];
        file.read_exact(&mut head[..8]).ok()?;
        let mut size = u64::from(u32::from_be_bytes(head[..4].try_into().ok()?));
        let mut header = 8u64;
        if size == 1 {
            file.read_exact(&mut head[8..16]).ok()?;
            size = u64::from_be_bytes(head[8..16].try_into().ok()?);
            header = 16;
        } else if size == 0 {
            size = end - at;
        }
        if size < header || at.checked_add(size)? > end {
            return None;
        }
        if &head[4..8] == b"meta" {
            let body = size - header;
            if !(4..=MAX_META).contains(&body) {
                return None;
            }
            let mut meta = vec![0u8; body as usize];
            file.read_exact(&mut meta).ok()?;
            return Some(meta.split_off(4));
        }
        at += size;
    }
    None
}

/// The child boxes in `data`, as (type, body).
fn boxes(mut data: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    while data.len() >= 8 {
        let size = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let kind = [data[4], data[5], data[6], data[7]];
        let (size, header) = match size {
            0 => (data.len(), 8),
            1 => {
                let Some(big) = data.get(8..16) else { break };
                let Ok(big) = usize::try_from(u64::from_be_bytes(big.try_into().unwrap())) else {
                    break;
                };
                (big, 16)
            }
            n => (n, 8),
        };
        let Some(body) = data.get(header..size) else {
            break;
        };
        out.push((kind, body));
        data = &data[size..];
    }
    out
}

fn child<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    boxes(data)
        .into_iter()
        .find(|(k, _)| k == kind)
        .map(|(_, b)| b)
}

fn extras_of(meta: &[u8]) -> Extras {
    let Some(iprp) = child(meta, b"iprp") else {
        return Extras::default();
    };
    let properties: Vec<([u8; 4], &[u8])> = child(iprp, b"ipco").map(boxes).unwrap_or_default();
    let alpha = properties.iter().any(|(kind, body)| {
        kind == b"auxC"
            && body
                .get(4..)
                .and_then(|urn| urn.split(|&b| b == 0).next())
                .is_some_and(|urn| ALPHA.contains(&urn))
    });
    let primary = child(meta, b"pitm").and_then(primary_item);
    let linked = match (primary, child(iprp, b"ipma")) {
        (Some(item), Some(ipma)) => associations(ipma, item),
        _ => Vec::new(),
    };
    let icc = linked
        .iter()
        .filter_map(|&i| properties.get(i.checked_sub(1)?))
        .find_map(|(kind, body)| {
            let colour = body.get(..4)?;
            (kind == b"colr" && (colour == b"prof" || colour == b"rICC"))
                .then(|| body[4..].to_vec())
                .filter(|p| !p.is_empty())
        });
    Extras { icc, alpha }
}

/// The primary item's ID, from a `pitm` body.
fn primary_item(pitm: &[u8]) -> Option<u32> {
    let version = *pitm.first()?;
    let id = pitm.get(4..)?;
    Some(if version == 0 {
        u32::from(u16::from_be_bytes(id.get(..2)?.try_into().ok()?))
    } else {
        u32::from_be_bytes(id.get(..4)?.try_into().ok()?)
    })
}

/// The property indexes (from 1) an `ipma` body links to `item`.
fn associations(ipma: &[u8], item: u32) -> Vec<usize> {
    let read = || -> Option<Vec<usize>> {
        let version = *ipma.first()?;
        let wide = ipma.get(3)? & 1 == 1;
        let count = u32::from_be_bytes(ipma.get(4..8)?.try_into().ok()?);
        let mut at = 8usize;
        for _ in 0..count {
            let id = if version < 1 {
                let id = u16::from_be_bytes(ipma.get(at..at + 2)?.try_into().ok()?);
                at += 2;
                u32::from(id)
            } else {
                let id = u32::from_be_bytes(ipma.get(at..at + 4)?.try_into().ok()?);
                at += 4;
                id
            };
            let n = usize::from(*ipma.get(at)?);
            at += 1;
            let mut found = Vec::with_capacity(n);
            for _ in 0..n {
                let index = if wide {
                    let v = u16::from_be_bytes(ipma.get(at..at + 2)?.try_into().ok()?);
                    at += 2;
                    usize::from(v & 0x7FFF)
                } else {
                    let v = *ipma.get(at)?;
                    at += 1;
                    usize::from(v & 0x7F)
                };
                found.push(index);
            }
            if id == item {
                return Some(found);
            }
        }
        Some(Vec::new())
    };
    read().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boxed(tag: &[u8], body: &[u8]) -> Vec<u8> {
        let mut b = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        b.extend_from_slice(tag);
        b.extend_from_slice(body);
        b
    }

    fn full(tag: &[u8], version: u8, flags: u8, body: &[u8]) -> Vec<u8> {
        boxed(tag, &[&[version, 0, 0, flags][..], body].concat())
    }

    /// A file with item `primary` linked to `links` in `properties`.
    fn file(primary: u16, properties: &[Vec<u8>], links: &[(u16, &[u8])]) -> Vec<u8> {
        let pitm = full(b"pitm", 0, 0, &primary.to_be_bytes());
        let ipco = boxed(b"ipco", &properties.concat());
        let mut ipma = (links.len() as u32).to_be_bytes().to_vec();
        for (item, indexes) in links {
            ipma.extend_from_slice(&item.to_be_bytes());
            ipma.push(indexes.len() as u8);
            ipma.extend_from_slice(indexes);
        }
        let ipma = full(b"ipma", 0, 0, &ipma);
        let iprp = boxed(b"iprp", &[ipco, ipma].concat());
        let meta = full(b"meta", 0, 0, &[pitm, iprp].concat());
        [boxed(b"ftyp", b"avif\0\0\0\0"), meta, boxed(b"mdat", b"xx")].concat()
    }

    fn read(bytes: &[u8]) -> Extras {
        let meta = meta_box(&mut std::io::Cursor::new(bytes)).unwrap();
        extras_of(&meta)
    }

    #[test]
    fn the_main_picture_s_profile_is_found() {
        let profile = boxed(b"colr", b"profICCDATA");
        let nclx = boxed(b"colr", b"nclx\0\x01\0\x0d\0\x06\x80");
        let got = read(&file(1, &[nclx, profile], &[(1, &[1, 0x82])]));
        assert_eq!(got.icc.as_deref(), Some(b"ICCDATA".as_slice()));
        assert!(!got.alpha);
    }

    #[test]
    fn a_profile_on_another_item_is_not_the_main_one_s() {
        let profile = boxed(b"colr", b"profICCDATA");
        let got = read(&file(1, &[profile], &[(1, &[]), (2, &[1])]));
        assert_eq!(got.icc, None);
    }

    #[test]
    fn an_alpha_plane_is_seen() {
        let alpha = full(
            b"auxC",
            0,
            0,
            b"urn:mpeg:mpegB:cicp:systems:auxiliary:alpha\0",
        );
        assert!(read(&file(1, &[alpha], &[(2, &[1])])).alpha);
        let depth = full(b"auxC", 0, 0, b"urn:mpeg:hevc:2015:auxid:2\0");
        assert!(!read(&file(1, &[depth], &[(2, &[1])])).alpha);
    }

    #[test]
    fn damaged_boxes_give_nothing_and_never_panic() {
        let good = file(1, &[boxed(b"colr", b"profICC")], &[(1, &[1])]);
        for cut in 0..good.len() {
            let mut bytes = good[..cut].to_vec();
            let _ = meta_box(&mut std::io::Cursor::new(&bytes)).map(|m| extras_of(&m));
            // A length that runs past the end.
            if cut > 4 {
                bytes[cut - 1] = 0xFF;
                let _ = meta_box(&mut std::io::Cursor::new(&bytes)).map(|m| extras_of(&m));
            }
        }
        let mut huge = good.clone();
        huge[..4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(meta_box(&mut std::io::Cursor::new(&huge)), None);
    }

    #[test]
    fn wide_ipma_indexes_are_read() {
        let profile = boxed(b"colr", b"rICCDATA");
        let pitm = full(b"pitm", 0, 0, &1u16.to_be_bytes());
        let ipco = boxed(b"ipco", &profile);
        // Flags bit 1: two-byte indexes, the top bit marking essential.
        let ipma = full(b"ipma", 0, 1, &[0, 0, 0, 1, 0, 1, 1, 0x80, 0x01]);
        let iprp = boxed(b"iprp", &[ipco, ipma].concat());
        let meta = full(b"meta", 0, 0, &[pitm, iprp].concat());
        assert_eq!(read(&meta).icc.as_deref(), Some(b"DATA".as_slice()));
    }
}
