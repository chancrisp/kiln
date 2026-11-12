//! Decoding HEVC on a Windows without the HEVC Video Extensions, as debug
//! builds can pretend with `KILN_HIDE_HEVC`.
//!
//! One test only, in a test binary of its own: the variable changes what
//! every reader in the process sees, so nothing may run beside it.

#![cfg(windows)]

mod common;

use kiln_media::video::{ReadMode, Reader};
use kiln_media::MediaError;
use std::fs::File;
use std::path::Path;

#[test]
fn hevc_without_its_extension_says_what_is_missing() {
    if common::no_mf() {
        return;
    }
    if !cfg!(debug_assertions) {
        eprintln!("skipped: KILN_HIDE_HEVC works in debug builds only");
        return;
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/video/hevc.mp4");
    std::env::set_var("KILN_HIDE_HEVC", "1");
    let hidden = Reader::open(File::open(&path).unwrap(), ReadMode::Decoded { d3d: None });
    // Copying needs no decoder.
    let copied = Reader::open(File::open(&path).unwrap(), ReadMode::Compressed);
    std::env::remove_var("KILN_HIDE_HEVC");
    match hidden {
        Err(e @ MediaError::NeedsExtension { .. }) => {
            assert!(matches!(
                e,
                MediaError::NeedsExtension {
                    codec: "HEVC",
                    product: "9N4WGH0Z6VHQ"
                }
            ));
            assert_eq!(
                e.to_string(),
                "Needs Microsoft's HEVC Video Extensions to read HEVC video."
            );
        }
        Err(e) => panic!("{e}"),
        Ok(_) => panic!("opened to decode with the extension hidden"),
    }
    assert!(copied.is_ok());
}
