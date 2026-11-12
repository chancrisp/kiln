//! Choosing a backend, and the errors before any backend runs.

#[path = "../../../tests/support/mod.rs"]
mod support;

use std::path::Path;

use kiln_core::{Engines, Error, Kiln, MediaInfo, Options, Registry};
use support::{media_data, sample_for, stage};

#[test]
fn targets_come_from_the_registry() {
    let kiln = Kiln::new();
    let codes = |name: &str| -> Vec<String> {
        kiln.targets_for(Path::new(name))
            .iter()
            .map(|f| f.code())
            .collect()
    };
    assert_eq!(
        codes("a.png"),
        ["AVIF", "BMP", "GIF", "ICO", "JPG", "JXL", "PDF", "PPM", "QOI", "TGA", "TIFF", "WEBP"]
    );
    assert_eq!(
        codes("a.jpeg"),
        ["AVIF", "BMP", "GIF", "ICO", "JXL", "PDF", "PNG", "PPM", "QOI", "TGA", "TIFF", "WEBP"]
    );
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
        "Can't convert PNG to MP4. Supported formats: \
         AVIF, BMP, GIF, ICO, JPG, JXL, PDF, PPM, QOI, TGA, TIFF, WEBP."
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

#[test]
fn audio_pairs_route_to_kiln() {
    let r = Registry::builtin();
    let backend = |a: &str, b: &str| {
        r.backend(r.format(a).unwrap(), r.format(b).unwrap())
            .map(str::to_owned)
    };
    assert_eq!(backend("mp3", "flac").as_deref(), Some("audio"));
    assert_eq!(backend("flac", "m4a").as_deref(), Some("audio-mf"));
    assert_eq!(backend("wma", "mp3").as_deref(), Some("audio-mf"));
    assert_eq!(backend("mp4", "mp3").as_deref(), Some("audio"));
    assert_eq!(backend("avi", "mp3").as_deref(), Some("ffmpeg"));
    assert_eq!(backend("mp4", "webm").as_deref(), Some("ffmpeg"));
}

#[test]
fn video_among_the_four_containers_routes_to_kiln() {
    let r = Registry::builtin();
    let backend = |a: &str, b: &str| {
        r.backend(r.format(a).unwrap(), r.format(b).unwrap())
            .map(str::to_owned)
    };
    for from in ["mp4", "m4v", "mov", "mkv"] {
        for to in ["mp4", "m4v", "mov", "mkv"] {
            if from != to {
                assert_eq!(
                    backend(from, to).as_deref(),
                    Some("video"),
                    "{from} -> {to}"
                );
            }
        }
        // The other video types, animations and subtitles stay FFmpeg's.
        for to in ["webm", "avi", "gif", "webp", "apng", "srt"] {
            if r.format(from)
                .is_some_and(|f| r.targets(f).iter().any(|t| t.ext == to))
            {
                assert_eq!(
                    backend(from, to).as_deref(),
                    Some("ffmpeg"),
                    "{from} -> {to}"
                );
            }
        }
    }
    assert_eq!(backend("mp4", "mkv").as_deref(), Some("video"));
    assert_eq!(backend("avi", "mp4").as_deref(), Some("ffmpeg"));
    assert_eq!(backend("webm", "mkv").as_deref(), Some("ffmpeg"));
    assert_eq!(backend("gif", "mp4").as_deref(), Some("ffmpeg"));
}

#[test]
fn an_mp3_converts_with_no_helpers() {
    let (dir, input) = stage(&media_data("tagged.mp3"));
    let nowhere = dir.path().join("no-such-dir");
    let opts = Options {
        engines: Engines {
            ffmpeg: Some(nowhere.join("ffmpeg.exe")),
            pandoc: Some(nowhere.join("pandoc.exe")),
            libreoffice: Some(nowhere.join("soffice.exe")),
            calibre: Some(nowhere.join("ebook-convert.exe")),
        },
        temp_root: Some(dir.path().join("work")),
        ..Options::default()
    };
    let out = Kiln::new().convert(&input, "flac", &opts).unwrap();
    assert_eq!(out, dir.path().join("tagged.flac"));
    let bytes = std::fs::read(&out).unwrap();
    assert!(bytes.starts_with(b"fLaC") && bytes.len() > 1000);
}

#[test]
fn drops_say_what_is_lost() {
    let kiln = Kiln::new();
    let drops = |input: &str, target: &str, media: Option<&MediaInfo>| {
        kiln.drops(Path::new(input), target, media)
    };
    let tracks = |sound_tracks, subtitle_tracks| MediaInfo {
        sound_tracks,
        subtitle_tracks,
        ..MediaInfo::default()
    };

    // More than one sound track, and subtitles, in a video.
    let two = tracks(2, 1);
    assert_eq!(
        drops("a.mkv", "mp4", Some(&two)),
        [
            "Keeps the first of 2 sound tracks.",
            "Subtitles aren't kept."
        ]
    );
    assert_eq!(
        drops("a.mkv", "mp3", Some(&two)),
        ["Keeps the first of 2 sound tracks."]
    );
    assert_eq!(
        drops("a.avi", "webm", Some(&tracks(3, 0))),
        ["Keeps the first of 3 sound tracks."]
    );
    assert_eq!(
        drops("a.mp4", "mkv", Some(&tracks(1, 2))),
        ["Subtitles aren't kept."]
    );
    // Nothing is lost: one sound track, none, or not known.
    assert!(drops("a.mkv", "mp4", Some(&tracks(1, 0))).is_empty());
    assert!(drops("a.mkv", "mp4", Some(&tracks(0, 0))).is_empty());
    assert!(drops("a.mkv", "mp4", None).is_empty());
    // A picture, a subtitle file or a sound file has nothing to say about
    // the video's tracks: an animation is silent, and a subtitle track is
    // what the target is made of.
    assert!(drops("a.mkv", "gif", Some(&two)).is_empty());
    assert!(drops("a.mkv", "srt", Some(&two)).is_empty());
    assert!(drops("a.mp3", "wav", Some(&two)).is_empty());

    // A PDF's text laid out again keeps the text only.
    let text_only = ["Keeps the text only: no pictures, tables or layout."];
    assert_eq!(drops("a.pdf", "docx", None), text_only);
    assert_eq!(drops("a.pdf", "md", None), text_only);
    assert_eq!(drops("a.PDF", "epub", None), text_only);
    assert_eq!(drops("a.pdf", "mobi", None), text_only);
    assert_eq!(drops("a.pdf", ".odt", None), text_only);
    // Whatever else is said comes with it.
    assert_eq!(drops("a.pdf", "docx", Some(&two)), text_only);
    // A PDF's pages as pictures, its text as text, and documents that hold
    // what they hold, lose nothing worth a line.
    assert!(drops("a.pdf", "png", None).is_empty());
    assert!(drops("a.pdf", "txt", None).is_empty());
    assert!(drops("a.txt", "docx", None).is_empty());
    assert!(drops("a.docx", "mobi", None).is_empty());
    assert!(drops("a.docx", "pdf", None).is_empty());
    assert!(drops("a.png", "jpg", None).is_empty());
    // Not a pair Kiln knows, or not a format.
    assert!(drops("a.png", "mp4", None).is_empty());
    assert!(drops("a.png", "zzz", None).is_empty());
    assert!(drops("noext", "jpg", None).is_empty());
}

#[test]
fn drops_leave_out_the_first_of_note_where_kilns_engines_read_a_later_track() {
    let kiln = Kiln::new();
    let drops = |input: &str, target: &str, media: &MediaInfo| {
        kiln.drops(Path::new(input), target, Some(media))
    };
    let first_of_two = "Keeps the first of 2 sound tracks.";
    let plain = MediaInfo {
        sound_tracks: 2,
        subtitle_tracks: 1,
        ..MediaInfo::default()
    };
    let later = MediaInfo {
        later_sound: true,
        ..plain.clone()
    };
    // Kiln's video engine (MKV to MP4) and sound engines (a video to MP3,
    // or to M4A through Media Foundation) read a later track when the first
    // is one they can't name, so "the first of n" would be false; the
    // subtitles are still not kept.
    assert_eq!(drops("a.mkv", "mp4", &later), ["Subtitles aren't kept."]);
    assert!(drops("a.mkv", "mp3", &later).is_empty());
    assert!(drops("a.mp4", "m4a", &later).is_empty());
    // With the first read, as it is for most files, it is said.
    assert_eq!(
        drops("a.mkv", "mp4", &plain),
        [first_of_two, "Subtitles aren't kept."]
    );
    assert_eq!(drops("a.mkv", "mp3", &plain), [first_of_two]);
    // FFmpeg reads the first sound track whatever it is, so a route it runs
    // says so either way.
    assert_eq!(
        drops("a.mkv", "webm", &later),
        [first_of_two, "Subtitles aren't kept."]
    );
    assert_eq!(drops("a.avi", "mp4", &later)[0], first_of_two);
    // Nothing else changes: a file with one sound track says nothing of it.
    let one = MediaInfo {
        sound_tracks: 1,
        later_sound: true,
        ..MediaInfo::default()
    };
    assert!(drops("a.mkv", "mp4", &one).is_empty());
}

#[test]
fn the_notes_are_the_fixed_sentences() {
    assert_eq!(
        kiln_core::KEEPS_TEXT_ONLY,
        "Keeps the text only: no pictures, tables or layout."
    );
    assert_eq!(kiln_core::NO_SUBTITLES, "Subtitles aren't kept.");
    assert_eq!(
        kiln_core::first_of_tracks(2),
        "Keeps the first of 2 sound tracks."
    );
    assert_eq!(
        kiln_core::first_of_tracks(12),
        "Keeps the first of 12 sound tracks."
    );
}

#[test]
fn only_the_pdf_chains_are_text_only() {
    let r = Registry::builtin();
    let pdf = r.format("pdf").unwrap();
    for (from, to, steps) in r.pairs() {
        let text_only = r.text_only(from, to);
        // A PDF becomes another document type by way of its text.
        assert_eq!(
            text_only,
            from.ext == "pdf" && steps.len() > 1,
            "{} to {}",
            from.ext,
            to.ext
        );
    }
    let to = |ext: &str| r.format(ext).unwrap();
    assert!(r.text_only(pdf, to("docx")));
    assert!(r.text_only(pdf, to("azw3")));
    assert!(!r.text_only(pdf, to("png")));
    assert!(!r.text_only(to("txt"), to("docx")));
    assert!(!r.text_only(to("txt"), to("md")));
    assert!(!r.text_only(to("epub"), to("mobi")));
    assert!(!r.text_only(to("doc"), to("mobi")));
}

#[test]
fn heic_and_avif_are_read_by_the_image_backend() {
    let kiln = Kiln::new();
    let reg = kiln.registry();
    let fmt = |ext: &str| reg.format(ext).unwrap();
    for from in ["heic", "avif"] {
        for to in ["jpg", "png", "webp", "tiff", "bmp", "gif"] {
            assert_eq!(
                reg.backend(fmt(from), fmt(to)),
                Some("image"),
                "{from} to {to}"
            );
        }
    }
    // Writing AVIF stays FFmpeg's, and JPEG XL is read by FFmpeg as before.
    assert_eq!(reg.backend(fmt("png"), fmt("avif")), Some("ffmpeg"));
    assert_eq!(reg.backend(fmt("jxl"), fmt("png")), Some("ffmpeg"));
    // Further than the image backend goes, through PNG.
    for (from, to) in [
        ("heic", "pdf"),
        ("avif", "pdf"),
        ("heic", "avif"),
        ("avif", "jxl"),
    ] {
        let steps = reg.route(fmt(from), fmt(to)).unwrap();
        assert_eq!(steps.len(), 2, "{from} to {to}");
        assert_eq!(steps[0].backend, "image", "{from} to {to}");
    }
    let codes: Vec<String> = kiln
        .targets_for(Path::new("a.heic"))
        .iter()
        .map(|f| f.code())
        .collect();
    assert_eq!(
        codes,
        [
            "AVIF", "BMP", "GIF", "ICO", "JPG", "JXL", "PDF", "PNG", "PPM", "QOI", "TGA", "TIFF",
            "WEBP"
        ]
    );
}
