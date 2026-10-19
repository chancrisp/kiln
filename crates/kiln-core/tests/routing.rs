//! Choosing a backend, and the errors before any backend runs.

#[path = "../../../tests/support/mod.rs"]
mod support;

use std::path::Path;

use kiln_core::{Error, Kiln, Options, Registry};
use support::{sample_for, stage};

#[test]
fn targets_come_from_the_registry() {
    let kiln = Kiln::new();
    let codes = |name: &str| -> Vec<String> {
        kiln.targets_for(Path::new(name))
            .iter()
            .map(|f| f.code())
            .collect()
    };
    assert_eq!(codes("a.png"), ["BMP", "GIF", "JPG", "TIFF", "WEBP"]);
    assert_eq!(codes("a.jpeg"), ["BMP", "GIF", "PNG", "TIFF", "WEBP"]);
    assert!(codes("a.MP4").contains(&"MP3".to_owned()));
    assert!(!codes("a.mp3").contains(&"MP4".to_owned()));
    assert!(codes("a.docx").contains(&"EPUB".to_owned()));
    assert!(!codes("a.docx").contains(&"DOCX".to_owned()));
}

#[test]
fn unsupported_pair_lists_what_is_possible() {
    let (_dir, input) = stage(&sample_for("png"));
    let err = Kiln::new()
        .convert(&input, "mp4", &Options::default())
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Can't convert PNG to MP4. Supported formats: BMP, GIF, JPG, TIFF, WEBP."
    );
}

#[test]
fn unknown_target_and_input() {
    let (dir, input) = stage(&sample_for("png"));
    let err = Kiln::new()
        .convert(&input, "zzz", &Options::default())
        .unwrap_err();
    assert_eq!(err.to_string(), "ZZZ isn't a format Kiln knows.");

    let odd = dir.path().join("notes.xyz");
    std::fs::write(&odd, "x").unwrap();
    let err = Kiln::new()
        .convert(&odd, "jpg", &Options::default())
        .unwrap_err();
    assert_eq!(err.to_string(), "Kiln can't convert XYZ files.");
}

#[test]
fn a_registry_naming_an_unknown_backend_is_rejected() {
    let reg = Registry::from_toml(
        r#"
        [formats.a]
        kind = "image"
        [formats.b]
        kind = "image"
        [[route]]
        backend = "magic"
        from = ["a"]
        to = ["b"]
        "#,
    )
    .unwrap();
    let err = Kiln::with_registry(reg).err().unwrap();
    assert!(matches!(err, Error::Registry(_)));
    assert!(err.to_string().contains("magic"));
}
