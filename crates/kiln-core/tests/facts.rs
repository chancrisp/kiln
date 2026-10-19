//! What `inspect` reads from images: size, transparency, animation, photo
//! or graphic, and likely result sizes.

#[path = "../../../tests/support/mod.rs"]
mod support;

use std::fs;
use std::path::Path;

use image::{Rgb, RgbImage};
use kiln_core::{inspect, CancelToken, Facts, Kiln, Kind, Look, Options};
use support::fixture;

fn f(name: &str) -> Facts {
    inspect(
        &Kiln::new(),
        &fixture(name),
        &Options::default(),
        &CancelToken::new(),
    )
    .unwrap()
}

fn inspect_at(path: &Path) -> Facts {
    inspect(&Kiln::new(), path, &Options::default(), &CancelToken::new()).unwrap()
}

/// A GIF of `frames` 8 x 8 frames, each a different flat colour.
fn inspect_temp_gif(frames: u8) -> Facts {
    use image::codecs::gif::GifEncoder;
    use image::{Frame, Rgba, RgbaImage};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("anim.gif");
    let file = fs::File::create(&path).unwrap();
    let mut encoder = GifEncoder::new(file);
    encoder
        .encode_frames((0..frames).map(|i| {
            Frame::new(RgbaImage::from_pixel(
                8,
                8,
                Rgba([i * 100, 50, 200 - i * 50, 255]),
            ))
        }))
        .unwrap();
    drop(encoder);
    inspect_at(&path)
}

/// A `w` x `h` smooth gradient with noise of up to `amp` either way in
/// each channel, from a fixed-seed LCG.
fn smooth_plus_noise(w: u32, h: u32, amp: u32) -> RgbImage {
    let mut seed: u32 = 0x5EED_0001;
    let mut noise = move || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        ((seed >> 16) % (2 * amp + 1)) as i32 - amp as i32
    };
    RgbImage::from_fn(w, h, |x, y| {
        let base = [
            x * 200 / w + 20,
            y * 200 / h + 20,
            (x + y) * 150 / (w + h) + 60,
        ];
        Rgb(base.map(|b| (b as i32 + noise()).clamp(0, 255) as u8))
    })
}

/// The estimate `inspect` gives for `to`, and the size of a real
/// conversion to it, for a 1024 x 768 smooth picture with noise of up to
/// `amp` either way.
fn estimate_and_real(to: &str, amp: u32) -> (u64, u64) {
    let dir = tempfile::tempdir().unwrap();
    // BMP is lossless and converts to both JPG and PNG.
    let path = dir.path().join("picture.bmp");
    smooth_plus_noise(1024, 768, amp).save(&path).unwrap();
    let est = inspect_at(&path).estimates[to];
    let out = dir.path().join("out");
    fs::create_dir(&out).unwrap();
    let opts = Options {
        out_dir: Some(out),
        ..Options::default()
    };
    let real = Kiln::new().convert(&path, to, &opts).unwrap();
    (est, fs::metadata(real).unwrap().len())
}

#[test]
fn a_noisy_picture_is_a_photo() {
    let p = f("photo.png").picture.unwrap();
    assert_eq!(
        (p.width, p.height, p.look, p.alpha, p.animated),
        (64, 48, Look::Photo, false, false)
    );
}

#[test]
fn transparency_is_seen() {
    let p = f("rgba_half_transparent.png").picture.unwrap();
    assert!(p.alpha);
    assert_eq!(p.look, Look::Graphic);
}

#[test]
fn apng_frames_are_counted() {
    assert!(f("sample.apng").picture.unwrap().animated);
}

#[test]
fn a_still_gif_is_not_animated() {
    assert!(!f("sample.gif").picture.unwrap().animated);
}

#[test]
fn a_two_frame_gif_is_animated() {
    // Written to a temp folder with image::codecs::gif::GifEncoder, two 8 × 8 frames.
    assert!(inspect_temp_gif(2).picture.unwrap().animated);
}

#[test]
fn svg_is_a_graphic() {
    let p = f("sample.svg").picture.unwrap();
    assert_eq!(p.look, Look::Graphic);
    assert_eq!((p.width, p.height), (64, 48));
}

#[test]
fn heic_is_a_photo_by_its_type() {
    let p = f("sample.heic").picture.unwrap();
    assert_eq!(p.look, Look::Photo);
    assert!(p.width > 0);
}

#[test]
fn unreadable_images_keep_type_and_size() {
    for name in ["not_really.png", "truncated.png"] {
        let facts = f(name);
        assert_eq!(facts.kind, Kind::Image);
        assert_eq!(facts.bytes, std::fs::metadata(fixture(name)).unwrap().len());
        assert!(facts.picture.is_none());
        assert!(facts.estimates.is_empty());
    }
}

#[test]
fn unknown_or_missing_files_give_nothing() {
    let k = Kiln::new();
    let (o, c) = (Options::default(), CancelToken::new());
    assert!(inspect(&k, Path::new("no_such_file.png"), &o, &c).is_none());
    let dir = tempfile::tempdir().unwrap();
    let odd = dir.path().join("notes.xyz");
    fs::write(&odd, b"hello").unwrap();
    assert!(inspect(&k, &odd, &o, &c).is_none());
}

#[test]
fn image_estimates_land_within_2x() {
    // A 1024 × 768 smooth-plus-noise picture drawn into a temp folder, then inspected.
    // Each estimate is compared with a real Kiln::convert to that target.
    // A detailed picture (noise of up to 40 either way) and a smooth one
    // (up to 4, about the grain of a camera sensor).
    for amp in [40, 4] {
        for to in ["jpg", "png"] {
            let (est, real) = estimate_and_real(to, amp);
            assert!(
                est * 2 >= real && est <= real * 2,
                "±{amp} {to}: {est} vs {real}"
            );
        }
    }
}

/// Converts `path` to JPG with Kiln into `out` and returns the bytes
/// written. A JPG goes through lossless BMP first, since Kiln doesn't
/// convert a type to itself.
fn real_jpg_size(path: &Path, out: &Path) -> u64 {
    let kiln = Kiln::new();
    let opts = Options {
        out_dir: Some(out.to_path_buf()),
        ..Options::default()
    };
    let is_jpg = kiln
        .registry()
        .format_of(path)
        .is_some_and(|f| f.ext == "jpg");
    let source = if is_jpg {
        kiln.convert(path, "bmp", &opts).unwrap()
    } else {
        path.to_path_buf()
    };
    let jpg = kiln.convert(&source, "jpg", &opts).unwrap();
    fs::metadata(jpg).unwrap().len()
}

#[test]
#[ignore = "needs KILN_EXTRA_SAMPLES"]
fn real_photos_and_screenshots_are_told_apart() {
    // $KILN_EXTRA_SAMPLES/photos/* must be Photo, $KILN_EXTRA_SAMPLES/graphics/* Graphic,
    // and every jpg estimate in photos/ within 2× of a real encode.
    let Some(root) = std::env::var_os("KILN_EXTRA_SAMPLES") else {
        return;
    };
    let root = Path::new(&root);
    let kiln = Kiln::new();
    let mut checked = 0;
    let mut wrong = Vec::new();
    for (folder, want) in [("photos", Look::Photo), ("graphics", Look::Graphic)] {
        let Ok(entries) = fs::read_dir(root.join(folder)) else {
            continue;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            let is_image = kiln
                .registry()
                .format_of(&path)
                .is_some_and(|f| f.kind == Kind::Image);
            if !path.is_file() || !is_image {
                continue;
            }
            let facts = inspect_at(&path);
            let picture = facts
                .picture
                .as_ref()
                .unwrap_or_else(|| panic!("{} has no picture facts", path.display()));
            checked += 1;
            if picture.look != want {
                wrong.push(format!("{}: {:?}", path.display(), picture.look));
            }
            if want == Look::Photo {
                if let Some(&est) = facts.estimates.get("jpg") {
                    let out = tempfile::tempdir().unwrap();
                    let real = real_jpg_size(&path, out.path());
                    if !(est * 2 >= real && est <= real * 2) {
                        wrong.push(format!("{}: jpg {est} vs {real}", path.display()));
                    }
                }
            }
        }
    }
    assert!(
        checked > 0,
        "no images in {}/photos or graphics",
        root.display()
    );
    assert!(wrong.is_empty(), "{checked} checked:\n{}", wrong.join("\n"));
}
