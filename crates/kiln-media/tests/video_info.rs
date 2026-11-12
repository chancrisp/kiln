//! `video::probe`: a video's facts from its container, against the fixtures in
//! tests/data/video (`make.ps1` there writes them; they are checked in) and the
//! audio-and-video ones in tests/data.

use kiln_media::sym::takes_first_sound;
use kiln_media::video::{probe, track_counts, Hdr, TrackCounts, VideoCodec, VideoInfo};
use kiln_media::{Codec, MediaError};
use std::path::{Path, PathBuf};
use std::process::Command;

fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join(name)
}

fn video(name: &str) -> PathBuf {
    data(&format!("video/{name}"))
}

fn facts(path: &Path) -> VideoInfo {
    probe(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn near(a: Option<f64>, b: f64, tolerance: f64) -> bool {
    a.is_some_and(|a| (a - b).abs() <= tolerance)
}

/// A copy of `bytes` named `name` in a fresh temp folder.
fn written(name: &str, bytes: &[u8]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    (dir, path)
}

fn damaged(r: kiln_media::Result<VideoInfo>) -> bool {
    matches!(r, Err(MediaError::Damaged { ref code }) if code == "video")
}

#[test]
fn h264_in_mp4_gives_its_picture_and_its_sound() {
    let v = facts(&video("h264.mp4"));
    assert_eq!((v.width, v.height), (320, 240));
    assert_eq!(v.codec, VideoCodec::H264);
    assert_eq!(v.profile.as_deref(), Some("High"));
    assert_eq!(v.bit_depth, 8);
    assert_eq!(v.rotation, 0);
    assert!(near(v.fps, 30.0, 0.01), "{:?}", v.fps);
    assert!(!v.variable_rate);
    assert!(near(v.duration, 2.0, 0.04), "{:?}", v.duration);
    assert_eq!(v.hdr, Hdr::None);
    // ffprobe says 190,572 bits per second for the video stream.
    let kbps = v.video_kbps.expect("a bit rate");
    assert!((180..=200).contains(&kbps), "{kbps}");
    assert_eq!(v.sound_unreadable, None);
    let sound = v.sound.expect("sound");
    assert!([44_100, 48_000].contains(&sound.rate), "{}", sound.rate);
    assert_eq!(sound.codec, Codec::Aac);
}

#[test]
fn sound_kiln_cannot_read_does_not_stop_the_picture() {
    // H.264 with DTS sound: part 1 says "Unsupported" for the sound.
    let v = facts(&video("dts.mkv"));
    assert_eq!((v.width, v.height), (64, 64));
    assert_eq!(v.codec, VideoCodec::H264);
    assert!(v.sound.is_none());
    assert_eq!(v.sound_unreadable.as_deref(), Some("DTS"));
    // A silent video has neither.
    let v = facts(&video("hevc.mp4"));
    assert!(v.sound.is_none());
    assert_eq!(v.sound_unreadable, None);
}

#[test]
fn a_display_matrix_that_turns_it_90_degrees_clockwise_reads_90() {
    let v = facts(&video("rot90.mp4"));
    assert_eq!(v.rotation, 90);
    // The size is the coded one, before the rotation.
    assert_eq!((v.width, v.height), (320, 240));
    assert_eq!(facts(&video("h264.mp4")).rotation, 0);
}

#[test]
fn hevc_gives_its_profile_and_bit_depth() {
    let v = facts(&video("hevc.mp4"));
    assert_eq!(v.codec, VideoCodec::Hevc);
    assert_eq!(v.profile.as_deref(), Some("Main"));
    assert_eq!(v.bit_depth, 8);
    assert_eq!(v.hdr, Hdr::None);
    assert!(near(v.fps, 30.0, 0.01));
    assert!(v.sound.is_none(), "silent");
}

#[test]
fn hdr10_hevc_is_pq_at_10_bits() {
    let v = facts(&video("hevc10_hdr.mp4"));
    assert_eq!(v.codec, VideoCodec::Hevc);
    assert_eq!(v.profile.as_deref(), Some("Main 10"));
    assert_eq!(v.bit_depth, 10);
    // The file has no colr box: the transfer function is in the SPS's VUI.
    assert_eq!(v.hdr, Hdr::Pq);
    assert!(v.colour_tagged, "the VUI describes the colour");
}

#[test]
fn a_file_that_describes_no_colour_is_untagged() {
    // R68: FFmpeg's testsrc2 encodes say nothing of their colour.
    for name in ["h264.mp4", "h264.mkv", "hevc.mp4"] {
        assert!(!facts(&video(name)).colour_tagged, "{name}");
    }
}

#[test]
fn matroska_gives_its_tracks() {
    let v = facts(&video("h264.mkv"));
    assert_eq!((v.width, v.height), (320, 240));
    assert_eq!(v.codec, VideoCodec::H264);
    assert_eq!(v.profile.as_deref(), Some("High"));
    assert_eq!(v.bit_depth, 8);
    assert!(near(v.fps, 30.0, 0.01), "{:?}", v.fps);
    assert!(!v.variable_rate);
    assert!(near(v.duration, 2.008, 0.01), "{:?}", v.duration);
    assert_eq!(v.rotation, 0);
    assert_eq!(v.hdr, Hdr::None);
    assert_eq!(v.sound.expect("sound").codec, Codec::Opus);

    let v = facts(&video("av1.mkv"));
    assert_eq!(v.codec, VideoCodec::Av1);
    assert_eq!(v.profile.as_deref(), Some("Main"));
    assert_eq!((v.width, v.height), (320, 240));
    assert!(v.sound.is_none());
}

#[test]
fn webm_with_a_codec_kiln_does_not_name_is_other() {
    let v = facts(&data("av.webm"));
    assert_eq!(v.codec, VideoCodec::Other("vp8".into()));
    assert_eq!((v.width, v.height), (64, 64));
    assert_eq!(v.sound.expect("sound").codec, Codec::Vorbis);
}

#[test]
fn quicktime_and_the_small_mp4_and_mkv_fixtures() {
    for name in ["av.mov", "av.mp4", "silent.mp4", "av.mkv"] {
        let v = facts(&data(name));
        assert_eq!((v.width, v.height), (64, 64), "{name}");
        assert_eq!(v.codec, VideoCodec::H264, "{name}");
        assert_eq!(v.profile.as_deref(), Some("Constrained Baseline"), "{name}");
        assert!(near(v.fps, 10.0, 0.01), "{name}: {:?}", v.fps);
        assert!(near(v.duration, 1.0, 0.05), "{name}: {:?}", v.duration);
        assert_eq!(v.sound.is_some(), !name.starts_with("silent"), "{name}");
    }
}

#[test]
fn frames_of_different_lengths_are_variable_rate() {
    let v = facts(&video("vfr.mp4"));
    assert!(v.variable_rate);
    // 45 frames in 1.9 s.
    assert!(near(v.fps, 450.0 / 19.0, 0.01), "{:?}", v.fps);
    assert!(near(v.duration, 1.9, 0.04), "{:?}", v.duration);
}

#[test]
fn an_odd_sized_picture_keeps_its_coded_size() {
    // AV1 in MP4, so its facts come from the av01 box and need no extension.
    let v = facts(&video("odd.mp4"));
    assert_eq!((v.width, v.height), (885, 1919));
    assert_eq!(v.codec, VideoCodec::Av1);
    assert!(near(v.fps, 10.0, 0.01));
}

#[test]
fn a_file_with_only_sound_has_no_video() {
    let bytes = std::fs::read(data("tone_aac.m4a")).unwrap();
    let (_dir, path) = written("tone.mp4", &bytes);
    assert!(matches!(probe(&path), Err(MediaError::NoVideo)));
    let bytes = std::fs::read(data("tone_opus.webm")).unwrap();
    let (_dir, path) = written("tone.mkv", &bytes);
    assert!(matches!(probe(&path), Err(MediaError::NoVideo)));
    assert_eq!(
        MediaError::NoVideo.to_string(),
        "This file has no video to convert."
    );
}

#[test]
fn a_cut_short_file_is_damaged() {
    for (name, keep) in [
        ("h264.mp4", 600),
        ("h264.mp4", 40),
        ("h264.mkv", 100),
        ("h264.mkv", 20),
    ] {
        let bytes = std::fs::read(video(name)).unwrap();
        let (_dir, path) = written(name, &bytes[..keep]);
        assert!(damaged(probe(&path)), "{name} cut to {keep}");
    }
    assert_eq!(
        MediaError::Damaged {
            code: "video".into()
        }
        .to_string(),
        "This video is damaged and can't be read."
    );
}

#[test]
fn something_that_is_not_a_video_is_damaged() {
    let (_dir, path) = written("empty.mp4", b"");
    assert!(damaged(probe(&path)));
    let (_dir, path) = written("text.mkv", b"not a video at all, just words");
    assert!(damaged(probe(&path)));
    let wav = std::fs::read(data("tone.wav")).unwrap();
    let (_dir, path) = written("tone.mp4", &wav);
    assert!(damaged(probe(&path)));
}

#[test]
fn a_missing_file_is_a_read_error() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        probe(&dir.path().join("gone.mp4")),
        Err(MediaError::Read(_))
    ));
}

// ---------------------------------------------------------------------------
// Sound and subtitle tracks

fn have(tool: &str) -> bool {
    Command::new(tool)
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// True when FFmpeg is on PATH; says so and returns false if not.
fn ffmpeg_ready() -> bool {
    let ok = have("ffmpeg");
    if !ok {
        eprintln!("skipped: FFmpeg isn't on PATH");
    }
    ok
}

/// A one-second file of `container`'s kind in `dir`: a picture, then `sound`
/// sound tracks, then a subtitle track when `subtitles` is set, then a
/// chapter list when `chapters` is.
fn muxed(dir: &Path, name: &str, sound: usize, subtitles: Option<&str>, chapters: bool) -> PathBuf {
    std::fs::write(
        dir.join("words.srt"),
        "1\n00:00:00,000 --> 00:00:00,500\nhello\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("chapters.txt"),
        ";FFMETADATA1\n\
         [CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=500\ntitle=One\n\
         [CHAPTER]\nTIMEBASE=1/1000\nSTART=500\nEND=1000\ntitle=Two\n",
    )
    .unwrap();
    let mut args: Vec<String> = vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-y".into(),
        "-f".into(),
        "lavfi".into(),
        "-i".into(),
        "testsrc=size=64x48:rate=10:duration=1".into(),
    ];
    for n in 0..sound {
        args.extend([
            "-f".into(),
            "lavfi".into(),
            "-i".into(),
            format!("sine=frequency={}:duration=1", 440 * (n + 1)),
        ]);
    }
    let mut maps: Vec<String> = vec!["-map".into(), "0:v".into()];
    for n in 0..sound {
        maps.extend(["-map".into(), format!("{}:a", n + 1)]);
    }
    let mut next = sound + 1;
    if subtitles.is_some() {
        args.extend(["-i".into(), dir.join("words.srt").display().to_string()]);
        maps.extend(["-map".into(), format!("{next}:s")]);
        next += 1;
    }
    if chapters {
        args.extend(["-i".into(), dir.join("chapters.txt").display().to_string()]);
        maps.extend([
            "-map_metadata".into(),
            next.to_string(),
            "-map_chapters".into(),
            next.to_string(),
        ]);
    }
    args.extend(maps);
    // WebM holds only VP8, VP9 or AV1, and Vorbis or Opus.
    let codecs = if name.ends_with(".webm") {
        ["-c:v", "libvpx", "-c:a", "libopus"]
    } else {
        ["-c:v", "mpeg4", "-c:a", "aac"]
    };
    args.extend(codecs.map(String::from));
    if let Some(codec) = subtitles {
        args.extend(["-c:s".into(), codec.into()]);
    }
    let out = dir.join(name);
    args.push(out.display().to_string());
    let made = Command::new("ffmpeg").args(&args).output().unwrap();
    assert!(
        made.status.success(),
        "ffmpeg {args:?}: {}",
        String::from_utf8_lossy(&made.stderr)
    );
    out
}

fn counts(sound: u16, subtitles: u16) -> Option<TrackCounts> {
    Some(TrackCounts { sound, subtitles })
}

#[test]
fn track_counts_reads_every_track() {
    // The fixtures: one sound track each, or none.
    assert_eq!(track_counts(&video("h264.mp4")), counts(1, 0));
    assert_eq!(track_counts(&video("h264.mkv")), counts(1, 0));
    assert_eq!(track_counts(&video("hevc.mp4")), counts(0, 0));
    assert_eq!(track_counts(&video("prores.mov")), counts(1, 0));
    assert_eq!(track_counts(&data("tone_opus.webm")), counts(1, 0));

    // An MKV with two sound tracks and a subtitle track (type 17).
    if !ffmpeg_ready() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mkv = muxed(dir.path(), "two.mkv", 2, Some("srt"), false);
    assert_eq!(track_counts(&mkv), counts(2, 1));
    let webm = muxed(dir.path(), "two.webm", 2, Some("webvtt"), false);
    assert_eq!(track_counts(&webm), counts(2, 1));
    // The same in an MP4 and an M4V, whose subtitle track is `sbtl`.
    for name in ["two.mp4", "two.m4v"] {
        let mp4 = muxed(dir.path(), name, 2, Some("mov_text"), false);
        assert_eq!(track_counts(&mp4), counts(2, 1), "{name}");
    }
    // A MOV counts its sound tracks the same way.
    let mov = muxed(dir.path(), "two.mov", 2, None, false);
    assert_eq!(track_counts(&mov), counts(2, 0));
    // No subtitles and one sound track is what it says.
    let one = muxed(dir.path(), "one.mkv", 1, None, false);
    assert_eq!(track_counts(&one), counts(1, 0));
}

/// The handler type of each track of an MP4 or MOV, in order, read where
/// it is: `moov/trak/mdia/hdlr`, after the box's version, flags and
/// pre-defined fields.
fn handlers(file: &[u8]) -> Vec<[u8; 4]> {
    fn children(b: &[u8]) -> Vec<([u8; 4], &[u8])> {
        let mut out = vec![];
        let mut rest = b;
        while rest.len() >= 8 {
            let size = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
            if size < 8 || size > rest.len() {
                break;
            }
            out.push((rest[4..8].try_into().unwrap(), &rest[8..size]));
            rest = &rest[size..];
        }
        out
    }
    fn find<'a>(b: &'a [u8], kind: &[u8; 4]) -> Option<([u8; 4], &'a [u8])> {
        children(b).into_iter().find(|(k, _)| k == kind)
    }
    let Some((_, moov)) = find(file, b"moov") else {
        return vec![];
    };
    children(moov)
        .into_iter()
        .filter(|(k, _)| k == b"trak")
        .filter_map(|(_, trak)| {
            let (_, mdia) = find(trak, b"mdia")?;
            let (_, hdlr) = find(mdia, b"hdlr")?;
            hdlr.get(8..12)?.try_into().ok()
        })
        .collect()
}

#[test]
fn a_chapter_track_is_not_a_subtitle_track() {
    // A QuickTime chapter list is a `text` track: not something to say
    // Kiln drops.
    if !ffmpeg_ready() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mov = muxed(dir.path(), "chapters.mov", 1, None, true);
    let bytes = std::fs::read(&mov).unwrap();
    assert!(
        handlers(&bytes).contains(b"text"),
        "the file should hold a text track: {:?}",
        handlers(&bytes)
            .iter()
            .map(|h| String::from_utf8_lossy(h).into_owned())
            .collect::<Vec<_>>()
    );
    assert_eq!(track_counts(&mov), counts(1, 0));
}

#[test]
fn track_counts_is_none_for_what_it_cannot_read() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(track_counts(&dir.path().join("gone.mp4")), None);
    let (_dir, path) = written("empty.mp4", b"");
    assert_eq!(track_counts(&path), None);
    let (_dir, path) = written("text.mkv", b"not a video at all, just words");
    assert_eq!(track_counts(&path), None);
    // Headers cut short.
    let bytes = std::fs::read(video("h264.mkv")).unwrap();
    let (_dir, path) = written("short.mkv", &bytes[..100]);
    assert_eq!(track_counts(&path), None);
    let wav = std::fs::read(data("tone.wav")).unwrap();
    let (_dir, path) = written("tone.mp4", &wav);
    assert_eq!(track_counts(&path), None);
}

/// A one-second file named `name` in `dir` with a picture and one sound
/// track for each of `codecs` (FFmpeg encoder names), in that order.
fn with_sound(dir: &Path, name: &str, codecs: &[&str]) -> PathBuf {
    let mut args: Vec<String> = ["-hide_banner", "-loglevel", "error", "-y"]
        .map(String::from)
        .to_vec();
    args.extend(["-f", "lavfi", "-i", "testsrc=size=64x48:rate=10:duration=1"].map(String::from));
    for n in 0..codecs.len() {
        args.extend(["-f".into(), "lavfi".into(), "-i".into()]);
        args.push(format!("sine=frequency={}:duration=1", 440 * (n + 1)));
    }
    args.extend(["-map".into(), "0:v".into()]);
    for n in 0..codecs.len() {
        args.extend(["-map".into(), format!("{}:a", n + 1)]);
    }
    args.extend(["-c:v", "mpeg4"].map(String::from));
    for (n, codec) in codecs.iter().enumerate() {
        args.extend([format!("-c:a:{n}"), (*codec).to_owned()]);
    }
    let out = dir.join(name);
    args.push(out.display().to_string());
    let made = Command::new("ffmpeg").args(&args).output().unwrap();
    assert!(
        made.status.success(),
        "ffmpeg {args:?}: {}",
        String::from_utf8_lossy(&made.stderr)
    );
    out
}

#[test]
fn takes_first_sound_follows_the_sound_engines_pick() {
    // The engine reads the first sound track whose codec Symphonia names.
    for path in [video("h264.mp4"), video("h264.mkv"), data("tone_opus.webm")] {
        assert_eq!(takes_first_sound(&path), Some(true), "{}", path.display());
    }
    if !ffmpeg_ready() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    for ext in ["mkv", "mp4"] {
        // Two it can name: the first.
        let plain = with_sound(dir.path(), &format!("plain.{ext}"), &["aac", "aac"]);
        assert_eq!(takes_first_sound(&plain), Some(true), "{ext}");
        // AC-3 first, which it can't name: the second.
        let skipped = with_sound(dir.path(), &format!("skipped.{ext}"), &["ac3", "aac"]);
        assert_eq!(takes_first_sound(&skipped), Some(false), "{ext}");
        // Only AC-3: it names none, and Media Foundation reads the first.
        let only = with_sound(dir.path(), &format!("only.{ext}"), &["ac3"]);
        assert_eq!(takes_first_sound(&only), Some(true), "{ext}");
        // A sound track it can name, then one it can't.
        let last = with_sound(dir.path(), &format!("last.{ext}"), &["aac", "ac3"]);
        assert_eq!(takes_first_sound(&last), Some(true), "{ext}");
    }
    // What it can't read as a video container.
    assert_eq!(takes_first_sound(&dir.path().join("gone.mkv")), None);
    let (_dir, path) = written("empty.mp4", b"");
    assert_eq!(takes_first_sound(&path), None);
    let wav = std::fs::read(data("tone.wav")).unwrap();
    let (_dir, path) = written("tone.mp4", &wav);
    assert_eq!(takes_first_sound(&path), None);
}
