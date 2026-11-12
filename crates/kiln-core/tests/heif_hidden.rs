//! HEIC and AVIF on a Windows without Microsoft's HEIF Image Extensions
//! or AV1 Video Extension, as debug builds can pretend with
//! `KILN_HIDE_WIC`: FFmpeg reads them when it is there, and without it the
//! job names what to get.
//!
//! One test only, in a test binary of its own: the variable changes what
//! every reader in the process sees, so nothing may run beside it.

#![cfg(windows)]

mod pictures;
#[path = "../../../tests/support/mod.rs"]
mod support;

use image::GenericImageView;
use kiln_core::{inspect, CancelToken, Engines, Error, JobError, Kiln, Options, Tool};
use support::{files_in, fixture, stage};

fn no_ffmpeg(dir: &tempfile::TempDir) -> Options {
    Options {
        engines: Engines {
            ffmpeg: Some(dir.path().join("no-such-dir").join("ffmpeg.exe")),
            ..Engines::default()
        },
        ..Options::default()
    }
}

#[test]
fn without_windows_decoders_ffmpeg_reads_heic_and_avif() {
    if !cfg!(debug_assertions) {
        eprintln!("skipped: KILN_HIDE_WIC works in debug builds only");
        return;
    }
    std::env::set_var("KILN_HIDE_WIC", "1");
    let kiln = Kiln::new();

    // Neither: the job says what to get, and leaves nothing.
    for (name, message, product) in [
        (
            "sample.heic",
            "Needs Microsoft's HEIF Image and HEVC Video Extensions, or FFmpeg.",
            "9PMMSR1CGPWG",
        ),
        (
            "sample.avif",
            "Needs Microsoft's AV1 Video Extension, or FFmpeg.",
            "9MVZQVXJBQ9V",
        ),
    ] {
        let (dir, input) = stage(&fixture(name));
        let err = kiln.convert(&input, "jpg", &no_ffmpeg(&dir)).unwrap_err();
        assert!(
            matches!(&err, Error::NeedsExtension { product: p, .. } if *p == product),
            "{name}: {err:?}"
        );
        let job = JobError::from(err);
        assert_eq!(job.message, message, "{name}");
        assert_eq!(job.store_product, Some(product), "{name}");
        assert_eq!(files_in(dir.path()), [name], "{name}");
    }
    // A tool says the same.
    let (dir, input) = stage(&fixture("sample.heic"));
    let err = kiln
        .run_tool(Tool::RemoveEdges, &[input], &no_ffmpeg(&dir))
        .unwrap_err();
    assert!(matches!(err, Error::NeedsExtension { .. }), "{err:?}");

    // With FFmpeg: it decodes in the job folder, and the image backend
    // writes the result.
    let found = kiln_core::find_on_path(&std::env::var_os("PATH").unwrap_or_default(), "ffmpeg");
    let Some(ffmpeg) = found else {
        std::env::remove_var("KILN_HIDE_WIC");
        eprintln!("FFmpeg half skipped: FFmpeg isn't on PATH");
        return;
    };
    let with = Options {
        engines: Engines {
            ffmpeg: Some(ffmpeg),
            ..Engines::default()
        },
        ..Options::default()
    };
    for name in ["sample.heic", "sample.avif"] {
        let (dir, input) = stage(&fixture(name));
        let out = kiln.convert(&input, "jpg", &with).unwrap();
        let img = image::open(&out).unwrap();
        assert_eq!(img.dimensions(), (64, 48), "{name}");
        let stem = name.trim_end_matches(".heic").trim_end_matches(".avif");
        let mut want = vec![name.to_owned(), format!("{stem}.jpg")];
        want.sort();
        assert_eq!(files_in(dir.path()), want, "{name}");
        // Facts fall back to FFprobe for the size.
        let facts = inspect(&kiln, &input, &with, &CancelToken::new()).unwrap();
        let p = facts.picture.unwrap();
        assert_eq!((p.width, p.height), (64, 48), "{name}");
    }
    // FFmpeg turns a HEIC upright too.
    let dir = tempfile::tempdir().unwrap();
    let turned = dir.path().join("turned.heic");
    let sample = std::fs::read(fixture("sample.heic")).unwrap();
    std::fs::write(&turned, pictures::heic_with(&sample, Some(1), None)).unwrap();
    let out = kiln.convert(&turned, "png", &with).unwrap();
    let img = image::open(&out).unwrap().to_rgb8();
    assert_eq!(img.dimensions(), (48, 64));
    let top_left = img.get_pixel(1, 1).0;
    assert!(top_left[0] > 220 && top_left[1] < 40, "{top_left:?}");

    // A phone's HEIC, with the turn in `irot` and in an Exif item, is
    // turned once: the stored bottom left (green) at the top left.
    let phone = dir.path().join("phone.heic");
    let file = pictures::heif(&pictures::Heif {
        quarters: Some(3),
        exif_orientation: Some(6),
        ..pictures::Heif::new(&sample)
    });
    std::fs::write(&phone, file).unwrap();
    let out = kiln.convert(&phone, "png", &with).unwrap();
    let img = image::open(&out).unwrap().to_rgb8();
    assert_eq!(img.dimensions(), (48, 64));
    let (top_left, top_right) = (img.get_pixel(1, 1).0, img.get_pixel(46, 1).0);
    assert!(top_left[1] > 200 && top_left[0] < 40, "{top_left:?}");
    assert!(top_right[0] < 40 && top_right[1] < 40, "{top_right:?}");

    // A grid, as phones store their photos, is read whole: two copies of
    // the gradient side by side, for HEIC and AVIF alike.
    for name in ["sample.heic", "sample.avif"] {
        let sample = std::fs::read(fixture(name)).unwrap();
        let grid = dir.path().join(format!("grid-{name}"));
        let file = pictures::heif(&pictures::Heif {
            grid: true,
            ..pictures::Heif::new(&sample)
        });
        std::fs::write(&grid, file).unwrap();
        let out = kiln.convert(&grid, "png", &with).unwrap();
        let img = image::open(&out).unwrap();
        assert_eq!(img.dimensions(), (128, 48), "{name}");
        assert!(!img.color().has_alpha(), "{name}: {:?}", img.color());
        let img = img.to_rgb8();
        for x in [40, 104] {
            let p = img.get_pixel(x, 20).0;
            assert!(
                pictures::near(p, [160, 100, 128], 24),
                "{name} at {x}: {p:?}"
            );
        }
    }

    // A tool reads it through FFmpeg too: the gradient has no border.
    let (_dir, input) = stage(&fixture("sample.heic"));
    let err = kiln
        .run_tool(Tool::RemoveEdges, &[input], &with)
        .unwrap_err();
    assert!(matches!(err, Error::NothingToDo(_)), "{err:?}");

    // So does Merge: a HEIC and an AVIF become a PDF of two pages.
    let dir = tempfile::tempdir().unwrap();
    let mut inputs = vec![];
    for name in ["sample.heic", "sample.avif"] {
        let path = dir.path().join(name);
        std::fs::copy(fixture(name), &path).unwrap();
        inputs.push(path);
    }
    let out = kiln.run_tool(Tool::Merge, &inputs, &with).unwrap();
    assert_eq!(out.len(), 1);
    let pdf = lopdf::Document::load(&out[0]).unwrap();
    assert_eq!(pdf.get_pages().len(), 2);
    std::env::remove_var("KILN_HIDE_WIC");
}
