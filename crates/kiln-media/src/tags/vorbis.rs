//! Vorbis comments and the FLAC PICTURE block.
//!
//! FLAC keeps both as metadata blocks; Ogg Vorbis and Opus (Task 8) keep the
//! same two things as comment lines, the picture base64-encoded under
//! `METADATA_BLOCK_PICTURE`. This module builds the pieces; the containers
//! wrap them.

use crate::types::{Picture, Tags};

/// The comment lines for `tags`, as (NAME, value) pairs in a fixed order.
/// Empty values are left out, and so is a total with no number.
pub fn comments(tags: &Tags) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut put = |name: &str, value: Option<&str>| {
        if let Some(v) = value.map(clean).filter(|v| !v.is_empty()) {
            out.push((name.to_string(), v));
        }
    };
    put("TITLE", tags.title.as_deref());
    put("ARTIST", tags.artist.as_deref());
    put("ALBUM", tags.album.as_deref());
    put("ALBUMARTIST", tags.album_artist.as_deref());
    put("GENRE", tags.genre.as_deref());
    put("COMMENT", tags.comment.as_deref());
    if let Some((n, total)) = tags.track {
        put("TRACKNUMBER", Some(&n.to_string()));
        put("TRACKTOTAL", total.map(|t| t.to_string()).as_deref());
    }
    if let Some((n, total)) = tags.disc {
        put("DISCNUMBER", Some(&n.to_string()));
        put("DISCTOTAL", total.map(|t| t.to_string()).as_deref());
    }
    put("DATE", tags.year.as_deref());
    out
}

/// A value without NUL characters, which no comment can hold.
fn clean(s: &str) -> String {
    s.replace('\0', "")
}

/// The body of a FLAC PICTURE metadata block for a front cover: the type,
/// the MIME type, an empty description, unknown dimensions, and the image.
/// All numbers are big-endian. (The block header is not included.)
pub fn picture_block(p: &Picture) -> Vec<u8> {
    let mime: String = p.mime.chars().filter(|c| (' '..='~').contains(c)).collect();
    let mut out = Vec::with_capacity(32 + mime.len() + p.data.len());
    out.extend_from_slice(&3u32.to_be_bytes()); // front cover
    out.extend_from_slice(&(mime.len() as u32).to_be_bytes());
    out.extend_from_slice(mime.as_bytes());
    out.extend_from_slice(&0u32.to_be_bytes()); // description length
    out.extend_from_slice(&[0u8; 16]); // width, height, depth, colours: unknown
    out.extend_from_slice(&(p.data.len() as u32).to_be_bytes());
    out.extend_from_slice(&p.data);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_follow_the_fixed_order_and_skip_empties() {
        let t = Tags {
            title: Some("T".into()),
            artist: Some("".into()),
            album: Some("Al".into()),
            album_artist: Some("AA".into()),
            genre: Some("G".into()),
            comment: Some("C".into()),
            track: Some((3, Some(12))),
            disc: Some((1, None)),
            year: Some("2001".into()),
            picture: None,
        };
        let got: Vec<String> = comments(&t)
            .into_iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        assert_eq!(
            got,
            [
                "TITLE=T",
                "ALBUM=Al",
                "ALBUMARTIST=AA",
                "GENRE=G",
                "COMMENT=C",
                "TRACKNUMBER=3",
                "TRACKTOTAL=12",
                "DISCNUMBER=1",
                "DATE=2001"
            ]
        );
        assert!(comments(&Tags::default()).is_empty());
    }

    #[test]
    fn picture_block_has_the_flac_layout() {
        let p = Picture {
            mime: "image/png".into(),
            data: vec![1, 2, 3],
        };
        let b = picture_block(&p);
        assert_eq!(&b[0..4], &[0, 0, 0, 3]);
        assert_eq!(&b[4..8], &[0, 0, 0, 9]);
        assert_eq!(&b[8..17], b"image/png");
        assert_eq!(&b[17..21], &[0, 0, 0, 0]);
        assert_eq!(&b[21..37], &[0u8; 16]);
        assert_eq!(&b[37..41], &[0, 0, 0, 3]);
        assert_eq!(&b[41..], &[1, 2, 3]);
    }
}
