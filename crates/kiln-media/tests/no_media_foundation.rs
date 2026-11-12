//! A Windows without Media Foundation (the N editions), as debug builds can
//! pretend with `KILN_HIDE_MF`: every Media Foundation route says so, and
//! everything else still converts.
//!
//! One test only: the variable must be set before anything in this process
//! asks what Media Foundation has.
#![cfg(debug_assertions)]

use kiln_media::pcm::wav;
use kiln_media::{
    can_run, convert, probe, Block, Channels, MediaError, Preset, Setting, SinkSpec, Tags, Target,
    Tune,
};
use std::fs::File;
use std::path::{Path, PathBuf};
use windows::core::w;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;

fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join(name)
}

fn to(input: &Path, output: &Path, t: Target) -> kiln_media::Result<()> {
    convert(
        input,
        output,
        t,
        Preset::High,
        &Tune::default(),
        &mut |_| {},
        &|| false,
    )
}

/// Half a second of a quiet tone in 4.0 (FL FR BL BR), 16-bit at 48 kHz.
fn write_quad(path: &Path) {
    let quad = Channels {
        count: 4,
        mask: 0x33,
    };
    let tags = Tags::default();
    let setting = Setting::Pcm {
        bits: 16,
        float: false,
    };
    let mut sink = wav::sink(SinkSpec {
        rate: 48_000,
        channels: quad,
        tags: &tags,
        setting: &setting,
        source_bits: None,
        out: File::create(path).unwrap(),
    })
    .unwrap();
    let samples = (0..24_000 * 4)
        .map(|i| ((i / 4) as f32 * 0.05).sin() * 0.25)
        .collect();
    sink.write(&Block {
        channels: 4,
        rate: 48_000,
        samples,
    })
    .unwrap();
    sink.finish().unwrap();
}

#[test]
fn every_media_foundation_route_needs_it() {
    std::env::set_var("KILN_HIDE_MF", "1");
    // The programs delay-load Media Foundation (DIST-04): with it hidden,
    // Kiln must not reach its DLLs at all, so it says it isn't installed.
    assert!(!kiln_media::mf::mf_installed());
    let needs = |r: kiln_media::Result<()>| matches!(r, Err(MediaError::NeedsMediaFoundation));

    for t in [Target::Aac, Target::M4a, Target::Wma, Target::Ac3] {
        assert!(needs(can_run(t)), "{t:?}");
    }
    for t in [Target::Wav, Target::Flac, Target::Opus, Target::Ogg] {
        assert!(can_run(t).is_ok(), "{t:?}");
    }

    let dir = tempfile::tempdir().unwrap();
    for t in [Target::Aac, Target::M4a, Target::Wma, Target::Ac3] {
        let out = dir.path().join(format!("out.{}", t.ext()));
        assert!(needs(to(&data("tone.wav"), &out, t)), "{t:?}");
        assert!(!out.exists(), "{t:?} left a file");
    }
    for name in ["tone.wma", "tone.ac3"] {
        assert!(
            matches!(probe(&data(name)), Err(MediaError::NeedsMediaFoundation)),
            "{name}"
        );
        let out = dir.path().join(format!("{name}.wav"));
        assert!(needs(to(&data(name), &out, Target::Wav)), "{name}");
        assert!(!out.exists());
    }

    // A WMA under an extension Kiln doesn't know: Symphonia calls it
    // damaged, but the telling answer is that it needs Media Foundation.
    let renamed = dir.path().join("tone.bin");
    std::fs::copy(data("tone.wma"), &renamed).unwrap();
    assert!(
        matches!(probe(&renamed), Err(MediaError::NeedsMediaFoundation)),
        "{:?}",
        probe(&renamed)
    );

    // Surround AC3 is Kiln's own encoder, but AC3 as a target needs Media
    // Foundation (R28): a quad source is refused too, with no file left.
    let quad = dir.path().join("quad.wav");
    write_quad(&quad);
    let out = dir.path().join("quad.ac3");
    assert!(needs(to(&quad, &out, Target::Ac3)));
    assert!(!out.exists(), "quad AC3 left a file");

    // The rest is untouched.
    let out = dir.path().join("fine.flac");
    to(&data("tone_aac.m4a"), &out, Target::Flac).unwrap();
    assert!(probe(&out).is_ok());

    // This test is linked like Kiln's programs, with Media Foundation
    // delay-loaded, so none of the above reached its DLLs: on an N edition
    // a call into a missing one would have raised an exception.
    for dll in [w!("mfplat.dll"), w!("mfreadwrite.dll"), w!("mf.dll")] {
        // SAFETY: only asks whether the module is loaded.
        let loaded = unsafe { GetModuleHandleW(dll) }.is_ok();
        // SAFETY: the name is a valid wide string literal.
        assert!(!loaded, "{} was loaded", unsafe { dll.display() });
    }
}
