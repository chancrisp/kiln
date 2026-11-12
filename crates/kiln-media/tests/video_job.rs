//! The video job (`video::convert`): copying and encoding the picture and
//! the sound, interleaving, progress, cancel and the `.part` file, checked
//! with ffprobe.
//!
//! The ffprobe checks, and the tests that make their own source files with
//! FFmpeg, skip with a printed line when FFmpeg isn't on PATH. The tests run
//! one at a time: one of them is timed, and several use the graphics card.

#![cfg(windows)]

mod common;

use kiln_media::video::{
    convert, EncoderChoice, JobReport, SoundChoice, VideoSetting, VideoTarget, VideoTune,
};
use kiln_media::{MediaError, Preset};
use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Helpers

/// One test at a time.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner())
}

fn have(tool: &str) -> bool {
    Command::new(tool)
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// True when FFmpeg and ffprobe are on PATH; says so and returns false if not.
fn ffmpeg_ready() -> bool {
    let ok = have("ffmpeg") && have("ffprobe");
    if !ok {
        eprintln!("skipped: FFmpeg isn't on PATH");
    }
    ok
}

fn video(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/video")
        .join(name)
}

fn part_of(output: &Path) -> PathBuf {
    let mut s = output.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

/// Runs FFmpeg with `args`, writing `out`.
fn ffmpeg(args: &[&str], out: &Path) {
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .arg(out)
        .status()
        .expect("ffmpeg starts");
    assert!(status.success(), "ffmpeg failed: {args:?}");
}

/// Ten seconds of 640x480 H.264 at 30 fps with AAC sound: long enough for a
/// re-encode to report progress several times.
fn long_source(dir: &Path) -> PathBuf {
    let out = dir.join("long.mp4");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=640x480:rate=30:duration=10",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=10",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-preset",
            "veryfast",
            "-crf",
            "28",
            "-g",
            "30",
            "-c:a",
            "aac",
            "-b:a",
            "96k",
            "-shortest",
        ],
        &out,
    );
    out
}

/// What ffprobe says of a file: each stream's facts (packets counted), and
/// the file's own.
struct Probe {
    streams: Vec<HashMap<String, String>>,
    format: HashMap<String, String>,
}

impl Probe {
    fn of(path: &Path) -> Probe {
        let out = Command::new("ffprobe")
            .args(["-v", "error", "-count_packets", "-show_entries"])
            .arg(
                "stream=index,codec_type,codec_name,nb_read_packets,width,height,duration,start_time\
                 :stream_side_data=rotation:format=duration:format_tags",
            )
            .args(["-of", "compact=p=0"])
            .arg(path)
            .output()
            .expect("ffprobe starts");
        assert!(
            out.status.success(),
            "ffprobe failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let mut streams = Vec::new();
        let mut format = HashMap::new();
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let map: HashMap<String, String> = line
                .split('|')
                .filter_map(|kv| kv.split_once('='))
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            if map.contains_key("index") {
                streams.push(map);
            } else {
                format.extend(map);
            }
        }
        Probe { streams, format }
    }

    fn kind(&self, kind: &str) -> Option<&HashMap<String, String>> {
        self.streams
            .iter()
            .find(|s| s.get("codec_type").map(String::as_str) == Some(kind))
    }

    fn video(&self) -> &HashMap<String, String> {
        self.kind("video").expect("a video stream")
    }

    fn sound(&self) -> Option<&HashMap<String, String>> {
        self.kind("audio")
    }

    fn duration(&self) -> f64 {
        self.format["duration"].parse().unwrap()
    }
}

/// Where the picture's last frame ends, in seconds, by its packets' times.
fn video_end(path: &Path) -> f64 {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries"])
        .args(["packet=pts_time,duration_time", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .expect("ffprobe starts");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let (pts, d) = l.split_once(',')?;
            Some(pts.trim().parse::<f64>().ok()? + d.trim().parse::<f64>().unwrap_or(0.0))
        })
        .fold(0.0, f64::max)
}

fn get<'a>(m: &'a HashMap<String, String>, key: &str) -> &'a str {
    m.get(key).map_or("", String::as_str)
}

fn packets(m: &HashMap<String, String>) -> u64 {
    get(m, "nb_read_packets").parse().unwrap()
}

fn rotation(m: &HashMap<String, String>) -> Option<String> {
    m.iter()
        .find(|(k, _)| k.ends_with("rotation"))
        .map(|(_, v)| v.clone())
}

/// Converts with no progress or cancel.
fn run(
    input: &Path,
    output: &Path,
    target: VideoTarget,
    tune: &VideoTune,
) -> Result<JobReport, MediaError> {
    convert(
        input,
        output,
        target,
        Preset::High,
        tune,
        &mut |_| {},
        &|| false,
    )
}

fn encode_at(q: u8) -> VideoTune {
    VideoTune {
        setting: Some(VideoSetting::Quality(q)),
        ..VideoTune::default()
    }
}

/// Neither the output nor its `.part` is there.
fn nothing_left(output: &Path) {
    assert!(!output.exists(), "{} was left", output.display());
    let part = part_of(output);
    assert!(!part.exists(), "{} was left", part.display());
    let dir = output.parent().unwrap();
    let left: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains("kiln-mdat"))
        .collect();
    assert!(left.is_empty(), "temporary files left: {left:?}");
}

// ---------------------------------------------------------------------------
// Copying

#[test]
fn mkv_to_mp4_copies_and_is_fast() {
    let _one = serial();
    if common::no_mf() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.mp4");
    let src = video("h264.mkv");
    let start = Instant::now();
    let report = run(&src, &out, VideoTarget::Mp4, &VideoTune::default()).unwrap();
    let took = start.elapsed();
    assert!(report.video_copied, "{report:?}");
    // Opus can't go into an MP4: it is encoded as AAC.
    assert!(!report.sound_copied, "{report:?}");
    assert!(!report.hardware, "{report:?}");
    assert!(took < Duration::from_secs(2), "took {took:?}");
    assert!(!part_of(&out).exists());
    if !ffmpeg_ready() {
        return;
    }
    let (from, to) = (Probe::of(&src), Probe::of(&out));
    assert_eq!(get(to.video(), "codec_name"), "h264");
    assert_eq!(packets(to.video()), packets(from.video()));
    let sound = to.sound().expect("sound");
    assert_eq!(get(sound, "codec_name"), "aac");
    // Two seconds of 48 kHz sound in AAC frames of 1024 samples.
    let n = packets(sound);
    assert!((93..=97).contains(&n), "{n} AAC packets");
    assert!((to.duration() - from.duration()).abs() < 0.05);
}

/// The colour an MP4's `colr` (nclx) states: primaries, transfer, matrix
/// and the full range flag.
fn mp4_colr(path: &Path) -> Option<([u16; 3], bool)> {
    let b = std::fs::read(path).unwrap();
    let at = b.windows(8).position(|w| w == b"colrnclx")? + 8;
    let n = |i: usize| u16::from_be_bytes([b[at + i], b[at + i + 1]]);
    Some(([n(0), n(2), n(4)], b[at + 6] & 0x80 != 0))
}

/// Whether a Matroska file holds each of these one-byte `Colour` values:
/// primaries, transfer, matrix and range (1 limited, 2 full).
fn mkv_colour(path: &Path, [p, t, m, r]: [u8; 4]) -> bool {
    let b = std::fs::read(path).unwrap();
    let has = |id: [u8; 2], v: u8| b.windows(4).any(|w| w == [id[0], id[1], 0x81, v]);
    b.windows(2).any(|w| w == [0x55, 0xB0])
        && has([0x55, 0xBB], p)
        && has([0x55, 0xBA], t)
        && has([0x55, 0xB1], m)
        && has([0x55, 0xB9], r)
}

#[test]
fn copies_keep_the_container_s_colour() {
    let _one = serial();
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    // R94: a copy keeps the colour the source's container states (an SDR
    // colr, a Matroska Colour), as FFmpeg's -c copy does: here the BT.601
    // PAL matrix, in full range.
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("tagged.mp4");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x240:rate=30:duration=1",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-color_primaries",
            "bt470bg",
            "-color_trc",
            "smpte170m",
            "-colorspace",
            "bt470bg",
            "-color_range",
            "pc",
            "-movflags",
            "+write_colr",
        ],
        &src,
    );
    // FFmpeg states what it was given and its build passes on (the matrix
    // and the range at least).
    let stated = mp4_colr(&src).expect("the source states its colour");
    assert!(stated.0[2] == 5 && stated.1, "{stated:?}");
    let [p, t, m] = stated.0.map(|v| v as u8);
    // MP4 to MP4 and to MKV.
    let mp4 = dir.path().join("copy.mp4");
    let r = run(&src, &mp4, VideoTarget::Mp4, &VideoTune::default()).unwrap();
    assert!(r.video_copied);
    assert_eq!(mp4_colr(&mp4), Some(stated));
    let mkv = dir.path().join("copy.mkv");
    let r = run(&src, &mkv, VideoTarget::Mkv, &VideoTune::default()).unwrap();
    assert!(r.video_copied);
    assert!(mkv_colour(&mkv, [p, t, m, 2]));
    // MKV to MP4.
    let back = dir.path().join("back.mp4");
    let r = run(&mkv, &back, VideoTarget::Mp4, &VideoTune::default()).unwrap();
    assert!(r.video_copied);
    assert_eq!(mp4_colr(&back), Some(stated));
    // A source that states no colour stays untagged on a copy.
    let plain = dir.path().join("plain.mov");
    run(
        &video("h264.mp4"),
        &plain,
        VideoTarget::Mov,
        &VideoTune::default(),
    )
    .unwrap();
    assert_eq!(mp4_colr(&plain), None);
}

#[test]
fn mp4_to_mkv_copies_both() {
    let _one = serial();
    if common::no_mf() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.mkv");
    let src = video("h264.mp4");
    let report = run(&src, &out, VideoTarget::Mkv, &VideoTune::default()).unwrap();
    assert_eq!(
        report,
        JobReport {
            video_copied: true,
            sound_copied: true,
            hardware: false,
            fell_back: false,
            decoder_times: false,
        }
    );
    if !ffmpeg_ready() {
        return;
    }
    let (from, to) = (Probe::of(&src), Probe::of(&out));
    assert_eq!(get(to.video(), "codec_name"), "h264");
    assert_eq!(packets(to.video()), packets(from.video()));
    let (a, b) = (from.sound().unwrap(), to.sound().unwrap());
    assert_eq!(get(b, "codec_name"), "aac");
    assert_eq!(packets(b), packets(a));
}

#[test]
fn copy_keeps_the_rotation_in_mp4() {
    let _one = serial();
    if common::no_mf() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.mov");
    let src = video("rot90.mp4");
    let report = run(&src, &out, VideoTarget::Mov, &VideoTune::default()).unwrap();
    assert!(report.video_copied && report.sound_copied, "{report:?}");
    if !ffmpeg_ready() {
        return;
    }
    let (from, to) = (Probe::of(&src), Probe::of(&out));
    assert_eq!(rotation(to.video()).as_deref(), Some("-90"));
    assert_eq!(rotation(to.video()), rotation(from.video()));
    assert_eq!(
        (get(to.video(), "width"), get(to.video(), "height")),
        ("320", "240")
    );
    assert_eq!(packets(to.video()), packets(from.video()));
}

#[test]
fn rotated_to_mkv_is_upright() {
    let _one = serial();
    if common::no_mf() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.mkv");
    let src = video("rot90.mp4");
    let report = run(&src, &out, VideoTarget::Mkv, &VideoTune::default()).unwrap();
    // MKV has no rotation: the picture is encoded upright. AAC is copied.
    assert!(!report.video_copied && report.sound_copied, "{report:?}");
    if !ffmpeg_ready() {
        return;
    }
    let to = Probe::of(&out);
    assert_eq!(
        (get(to.video(), "width"), get(to.video(), "height")),
        ("240", "320")
    );
    assert_eq!(rotation(to.video()), None);
    assert_eq!(packets(to.video()), 60);
}

#[test]
fn vfr_length_matches_within_one_frame() {
    let _one = serial();
    if common::no_mf() {
        return;
    }
    let src = video("vfr.mp4");
    let dir = tempfile::tempdir().unwrap();
    for (name, target, tune) in [
        ("copy.mkv", VideoTarget::Mkv, VideoTune::default()),
        ("encoded.mp4", VideoTarget::Mp4, encode_at(65)),
        (
            "processor.mp4",
            VideoTarget::Mp4,
            VideoTune {
                encoder: EncoderChoice::Processor,
                ..encode_at(65)
            },
        ),
    ] {
        let out = dir.path().join(name);
        let report = run(&src, &out, target, &tune).unwrap();
        assert_eq!(report.video_copied, tune.setting.is_none(), "{name}");
        if !ffmpeg_ready() {
            return;
        }
        // Where the last frame ends. (The fixture's edit list ends at 1.9 s,
        // before its last frame; R18 copies honour only an edit's start.)
        let (from, to) = (video_end(&src), video_end(&out));
        assert!(
            (to - from).abs() <= 1.0 / 30.0 + 1e-3,
            "{name}: {to} s against {from} s"
        );
        let (from, to) = (Probe::of(&src), Probe::of(&out));
        assert_eq!(packets(to.video()), packets(from.video()), "{name}");
    }
}

// ---------------------------------------------------------------------------
// Sound, and the lack of it

#[test]
fn a_silent_video_converts_silent() {
    let _one = serial();
    if common::no_mf() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let src = video("hevc.mp4");
    for (name, target) in [("out.mp4", VideoTarget::Mp4), ("out.mkv", VideoTarget::Mkv)] {
        let out = dir.path().join(name);
        let report = run(&src, &out, target, &VideoTune::default()).unwrap();
        assert!(report.video_copied && !report.sound_copied, "{report:?}");
        if !ffmpeg_ready() {
            continue;
        }
        let to = Probe::of(&out);
        assert_eq!(get(to.video(), "codec_name"), "hevc");
        assert!(to.sound().is_none(), "{name} has sound");
        assert_eq!(packets(to.video()), 60);
    }
}

#[test]
fn sound_off_leaves_the_sound_out() {
    // Make smaller's Sound off: no sound track, whether the picture is
    // copied or encoded, and sound Kiln can't read (DTS) is no reason to
    // refuse when it is left out.
    let _one = serial();
    if common::no_mf() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let off = |setting| VideoTune {
        setting,
        sound: SoundChoice::Off,
        ..VideoTune::default()
    };
    for (src, name, target, setting) in [
        ("h264.mp4", "copy.mp4", VideoTarget::Mp4, None),
        (
            "h264.mp4",
            "encode.mov",
            VideoTarget::Mov,
            Some(VideoSetting::Quality(45)),
        ),
        ("h264.mkv", "copy.mkv", VideoTarget::Mkv, None),
        ("dts.mkv", "dts.mp4", VideoTarget::Mp4, None),
    ] {
        let out = dir.path().join(name);
        let report = run(&video(src), &out, target, &off(setting)).unwrap();
        assert!(!report.sound_copied, "{name}: {report:?}");
        let tracks = kiln_media::video::track_counts(&out).unwrap();
        assert_eq!(tracks.sound, 0, "{name} has sound");
        if ffmpeg_ready() {
            assert!(Probe::of(&out).sound().is_none(), "{name} has sound");
        }
    }
}

#[test]
fn a_sound_only_mp4_says_no_video() {
    let _one = serial();
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("sound.mp4");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=1",
            "-c:a",
            "aac",
        ],
        &src,
    );
    let out = dir.path().join("out.mp4");
    let r = run(&src, &out, VideoTarget::Mp4, &VideoTune::default());
    assert!(matches!(r, Err(MediaError::NoVideo)), "{r:?}");
    nothing_left(&out);
}

#[test]
fn dts_sound_into_mp4_is_refused() {
    let _one = serial();
    if common::no_mf() {
        return;
    }
    // Kiln can't read DTS and an MP4 can't hold it: R14 says refuse, never
    // drop the sound.
    let dir = tempfile::tempdir().unwrap();
    for (name, target) in [("out.mp4", VideoTarget::Mp4), ("out.mkv", VideoTarget::Mkv)] {
        let out = dir.path().join(name);
        let r = run(&video("dts.mkv"), &out, target, &VideoTune::default());
        match r {
            Err(MediaError::Unsupported { codec }) => assert_eq!(codec, "DTS"),
            other => panic!("{name}: {other:?}"),
        }
        nothing_left(&out);
    }
}

#[test]
fn a_chosen_sound_bitrate_encodes_the_sound() {
    let _one = serial();
    if common::no_mf() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.mkv");
    let tune = VideoTune {
        sound: SoundChoice::Kbps(96),
        ..VideoTune::default()
    };
    let report = run(&video("h264.mp4"), &out, VideoTarget::Mkv, &tune).unwrap();
    assert!(report.video_copied && !report.sound_copied, "{report:?}");
    if !ffmpeg_ready() {
        return;
    }
    let to = Probe::of(&out);
    let sound = to.sound().unwrap();
    assert_eq!(get(sound, "codec_name"), "opus");
    // Two seconds in 20 ms packets.
    let n = packets(sound);
    assert!((100..=102).contains(&n), "{n} Opus packets");
}

#[test]
fn kept_sound_that_cant_be_copied_follows_the_preset() {
    let _one = serial();
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    // R95: PCM, FLAC and AC-3 can't go into MP4 as they are; Keep encodes
    // them as AAC at the preset's bitrate for a stereo pair (Small 96, High
    // 160, Best 192), and the size model counts that same bitrate.
    let dir = tempfile::tempdir().unwrap();
    let src = video("h264.mp4");
    let src = src.to_str().unwrap();
    for (codec, ext) in [("pcm_s16le", "mkv"), ("flac", "mkv"), ("ac3", "mp4")] {
        let input = dir.path().join(format!("{codec}.{ext}"));
        ffmpeg(
            &[
                "-i", src, "-map", "0:v", "-map", "0:a", "-c:v", "copy", "-c:a", codec, "-ac", "2",
            ],
            &input,
        );
        let info = kiln_media::video::probe(&input).unwrap();
        for (preset, kbps) in [
            (Preset::Small, 96),
            (Preset::High, 160),
            (Preset::Best, 192),
        ] {
            let out = dir.path().join(format!("{codec}-{preset:?}.mp4"));
            let tune = VideoTune::default();
            let r = convert(
                &input,
                &out,
                VideoTarget::Mp4,
                preset,
                &tune,
                &mut |_| {},
                &|| false,
            )
            .unwrap();
            assert!(!r.sound_copied, "{codec} {preset:?}");
            let written = kiln_media::video::probe(&out).unwrap();
            let sound = written.sound.expect("sound");
            assert_eq!(sound.codec, kiln_media::Codec::Aac, "{codec} {preset:?}");
            // Windows' AAC encoder holds the bitrate it is given: what
            // ffprobe measures is within 10% of it.
            let measured = Command::new("ffprobe")
                .args(["-v", "error", "-select_streams", "a:0"])
                .args(["-show_entries", "stream=bit_rate", "-of", "csv=p=0"])
                .arg(&out)
                .output()
                .unwrap();
            let bps: f64 = String::from_utf8_lossy(&measured.stdout)
                .trim()
                .parse()
                .unwrap();
            let off = bps / 1000.0 / kbps as f64 - 1.0;
            assert!(
                off.abs() < 0.1,
                "{codec} {preset:?}: {bps} bps for {kbps} kbps"
            );
            let counted = kiln_media::video::tune::sound_kbps(
                VideoTarget::Mp4,
                &info,
                &VideoTune {
                    sound_preset: preset,
                    ..tune
                },
            );
            assert_eq!(counted, kbps as f64, "{codec} {preset:?}");
        }
    }
}

#[test]
fn tags_are_kept() {
    let _one = serial();
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    // R48: the creation time (an MP4's mvhd, a Matroska's DateUTC) and the
    // title, both ways between MP4 and MKV, besides part 1's tags.
    let dir = tempfile::tempdir().unwrap();
    for (from, ext) in [("h264.mp4", "mp4"), ("h264.mkv", "mkv")] {
        let src = dir.path().join(format!("tagged.{ext}"));
        ffmpeg(
            &[
                "-i",
                video(from).to_str().unwrap(),
                "-c",
                "copy",
                "-metadata",
                "title=Harbour at dusk",
                "-metadata",
                "creation_time=2024-05-06T07:08:09Z",
            ],
            &src,
        );
        for (name, target) in [("mp4", VideoTarget::Mp4), ("mkv", VideoTarget::Mkv)] {
            let out = dir.path().join(format!("{ext}-to.{name}"));
            run(&src, &out, target, &VideoTune::default()).unwrap();
            let to = Probe::of(&out);
            let tag = |k: &str| {
                to.format
                    .iter()
                    .find(|(key, _)| key.eq_ignore_ascii_case(&format!("tag:{k}")))
                    .map(|(_, v)| v.clone())
            };
            let what = format!("{ext} to {name}: {:?}", to.format);
            assert_eq!(tag("title").as_deref(), Some("Harbour at dusk"), "{what}");
            assert!(
                tag("creation_time").is_some_and(|t| t.starts_with("2024-05-06T07:08:09")),
                "{what}"
            );
        }
    }
}

#[test]
fn a_silent_video_keeps_its_tags() {
    let _one = serial();
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("silent.mp4");
    ffmpeg(
        &[
            "-i",
            video("hevc.mp4").to_str().unwrap(),
            "-c",
            "copy",
            "-metadata",
            "title=Silent harbour",
            "-metadata",
            "creation_time=2023-01-02T03:04:05Z",
        ],
        &src,
    );
    let out = dir.path().join("out.mkv");
    run(&src, &out, VideoTarget::Mkv, &VideoTune::default()).unwrap();
    let to = Probe::of(&out);
    let has = |k: &str, v: &str| {
        to.format
            .iter()
            .any(|(key, val)| key.eq_ignore_ascii_case(&format!("tag:{k}")) && val.starts_with(v))
    };
    assert!(has("title", "Silent harbour"), "{:?}", to.format);
    assert!(
        has("creation_time", "2023-01-02T03:04:05"),
        "{:?}",
        to.format
    );
}

fn start(m: &HashMap<String, String>) -> f64 {
    get(m, "start_time").parse().unwrap()
}

#[test]
fn re_encoded_sound_starts_where_the_source_does() {
    let _one = serial();
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    // The sound half a second after the picture, in an MP4 (AAC) and an
    // MKV (Opus), re-encoded.
    let dir = tempfile::tempdir().unwrap();
    let late = |from: &str, ext: &str| {
        let out = dir.path().join(format!("late.{ext}"));
        let src = video(from);
        let src = src.to_str().unwrap();
        ffmpeg(
            &[
                "-i",
                src,
                "-itsoffset",
                "0.5",
                "-i",
                src,
                "-map",
                "0:v",
                "-map",
                "1:a",
                "-c",
                "copy",
            ],
            &out,
        );
        out
    };
    let (mp4, mkv) = (late("h264.mp4", "mp4"), late("h264.mkv", "mkv"));
    let aac = VideoTune {
        sound: SoundChoice::Kbps(128),
        ..VideoTune::default()
    };
    let encoded = VideoTune {
        sound: SoundChoice::Kbps(128),
        ..encode_at(65)
    };
    let frame = 1024.0 / 48_000.0;
    for (src, name, target, tune) in [
        (&mp4, "aac-to-aac.mp4", VideoTarget::Mp4, aac),
        (
            &mkv,
            "opus-to-aac.mp4",
            VideoTarget::Mp4,
            VideoTune::default(),
        ),
        (&mp4, "aac-to-opus.mkv", VideoTarget::Mkv, aac),
        (&mp4, "encoded.mp4", VideoTarget::Mp4, encoded),
    ] {
        let out = dir.path().join(name);
        let report = run(src, &out, target, &tune).unwrap();
        assert!(!report.sound_copied, "{name}: {report:?}");
        let to = Probe::of(&out);
        let (v, a) = (start(to.video()), start(to.sound().unwrap()));
        assert!(v.abs() < 0.03, "{name}: the picture starts at {v}");
        assert!((a - 0.5).abs() <= frame, "{name}: the sound starts at {a}");
    }
}

#[test]
fn both_streams_starting_late_stay_in_step() {
    let _one = serial();
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    // Picture and sound both start 10 s in.
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("offset10.mkv");
    ffmpeg(
        &[
            "-itsoffset",
            "10",
            "-i",
            video("h264.mkv").to_str().unwrap(),
            "-c",
            "copy",
        ],
        &src,
    );
    let opus = VideoTune {
        sound: SoundChoice::Kbps(96),
        ..VideoTune::default()
    };
    for (name, target, tune) in [
        ("copied.mp4", VideoTarget::Mp4, VideoTune::default()),
        ("opus.mkv", VideoTarget::Mkv, opus),
        ("encoded.mp4", VideoTarget::Mp4, encode_at(65)),
    ] {
        let out = dir.path().join(name);
        let report = run(&src, &out, target, &tune).unwrap();
        assert!(!report.sound_copied, "{name}: {report:?}");
        let to = Probe::of(&out);
        let (v, a) = (start(to.video()), start(to.sound().unwrap()));
        assert!((v - 10.0).abs() < 0.03, "{name}: the picture starts at {v}");
        assert!(
            (a - v).abs() <= 1024.0 / 48_000.0,
            "{name}: sound {a}, picture {v}"
        );
    }
}

#[test]
fn a_damaged_file_says_so_whether_copied_or_encoded() {
    let _one = serial();
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    // A Matroska cut off half way.
    let dir = tempfile::tempdir().unwrap();
    let whole = dir.path().join("whole.mkv");
    let long = long_source(dir.path());
    ffmpeg(&["-i", long.to_str().unwrap(), "-c", "copy"], &whole);
    let bytes = std::fs::read(&whole).unwrap();
    let src = dir.path().join("cut.mkv");
    std::fs::write(&src, &bytes[..bytes.len() / 2]).unwrap();
    for (name, tune) in [
        ("copy.mp4", VideoTune::default()),
        ("encode.mp4", encode_at(65)),
    ] {
        let out = dir.path().join(name);
        let r = run(&src, &out, VideoTarget::Mp4, &tune);
        assert!(
            matches!(&r, Err(MediaError::Damaged { code }) if code == "video"),
            "{name}: {r:?}"
        );
        nothing_left(&out);
    }
}

// ---------------------------------------------------------------------------
// The job itself

#[test]
fn an_output_that_exists_is_left_alone() {
    let _one = serial();
    if common::no_mf() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.mp4");
    std::fs::write(&out, b"mine").unwrap();
    let r = run(
        &video("h264.mp4"),
        &out,
        VideoTarget::Mp4,
        &VideoTune::default(),
    );
    assert!(
        matches!(&r, Err(MediaError::Io(e)) if e.kind() == std::io::ErrorKind::AlreadyExists),
        "{r:?}"
    );
    assert_eq!(std::fs::read(&out).unwrap(), b"mine");
    assert!(!part_of(&out).exists());
}

#[test]
fn cancel_leaves_no_file() {
    let _one = serial();
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let src = long_source(dir.path());
    let out = dir.path().join("out.mkv");
    let calls = Cell::new(0u32);
    let r = convert(
        &src,
        &out,
        VideoTarget::Mkv,
        Preset::High,
        &encode_at(65),
        &mut |_| calls.set(calls.get() + 1),
        &|| calls.get() >= 3,
    );
    assert!(matches!(r, Err(MediaError::Cancelled)), "{r:?}");
    assert_eq!(calls.get(), 3);
    nothing_left(&out);
}

#[test]
fn cancel_during_an_mp4_s_finish_leaves_nothing() {
    let _one = serial();
    if common::no_mf() {
        return;
    }
    // R92: the last looks at cancel are the MP4 writer's, while it copies
    // the samples into place. A cancel there ends the job at once, and
    // neither the .part nor the samples file is left.
    let dir = tempfile::tempdir().unwrap();
    let src = video("h264.mkv");
    let out = dir.path().join("out.mp4");
    let calls = Cell::new(0u32);
    let beside = Cell::new(false);
    convert(
        &src,
        &out,
        VideoTarget::Mp4,
        Preset::High,
        &VideoTune::default(),
        &mut |_| {},
        &|| {
            calls.set(calls.get() + 1);
            // The samples wait beside the .part.
            let names: Vec<String> = std::fs::read_dir(dir.path())
                .unwrap()
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            if names
                .iter()
                .any(|n| n.starts_with(".out.mp4.part.") && n.ends_with(".kiln-mdat"))
            {
                beside.set(true);
            }
            false
        },
    )
    .unwrap();
    assert!(beside.get(), "no samples file beside the .part");
    let all = calls.get();
    assert!(all >= 2, "{all}");
    std::fs::remove_file(&out).unwrap();
    // Again, cancelled at the writer's first look.
    let calls = Cell::new(0u32);
    let start = Instant::now();
    let r = convert(
        &src,
        &out,
        VideoTarget::Mp4,
        Preset::High,
        &VideoTune::default(),
        &mut |_| {},
        &|| {
            calls.set(calls.get() + 1);
            calls.get() >= all - 1
        },
    );
    assert!(matches!(r, Err(MediaError::Cancelled)), "{r:?}");
    assert_eq!(calls.get(), all - 1);
    assert!(start.elapsed() < Duration::from_secs(10));
    nothing_left(&out);
}

#[test]
fn progress_rises_to_one() {
    let _one = serial();
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let src = long_source(dir.path());
    for (name, tune) in [
        ("encoded.mp4", encode_at(65)),
        ("copied.mp4", VideoTune::default()),
    ] {
        let out = dir.path().join(name);
        let mut seen = Vec::new();
        convert(
            &src,
            &out,
            VideoTarget::Mp4,
            Preset::High,
            &tune,
            &mut |p| seen.push(p),
            &|| false,
        )
        .unwrap();
        assert!(seen.windows(2).all(|w| w[0] <= w[1]), "{name}: {seen:?}");
        assert!(seen.iter().all(|p| (0.0..=1.0).contains(p)), "{seen:?}");
        assert_eq!(seen.last(), Some(&1.0), "{name}");
        assert_eq!(seen.iter().filter(|&&p| p == 1.0).count(), 1, "{seen:?}");
        if tune.setting.is_some() {
            // An encode takes long enough to report along the way.
            assert!(seen.len() >= 3, "{name}: {seen:?}");
            assert!(seen.iter().any(|&p| p > 0.0 && p < 1.0), "{seen:?}");
        }
    }
}

/// Converts with Fit under at `limit` and checks R45: the file is under the
/// limit, or the job said TooSmall (with a size over the limit) and left
/// nothing. Gives the report, or `None` for TooSmall.
fn fit(src: &Path, out: &Path, limit: u64, tune: VideoTune) -> Option<JobReport> {
    let tune = VideoTune {
        fit_bytes: Some(limit),
        ..tune
    };
    let mut seen = Vec::new();
    let r = convert(
        src,
        out,
        VideoTarget::Mp4,
        Preset::High,
        &tune,
        &mut |p| seen.push(p),
        &|| false,
    );
    assert!(seen.windows(2).all(|w| w[0] <= w[1]), "{seen:?}");
    match r {
        Ok(report) => {
            let size = std::fs::metadata(out).unwrap().len();
            assert!(size <= limit, "{size} bytes over {limit}");
            assert!(!part_of(out).exists());
            assert_eq!(seen.last(), Some(&1.0));
            Some(report)
        }
        Err(MediaError::TooSmall { min_bytes, .. }) => {
            assert!(min_bytes > limit, "{min_bytes} for {limit}");
            nothing_left(out);
            None
        }
        Err(e) => panic!("{e:?}"),
    }
}

#[test]
fn fit_under_on_the_processor_lands_under() {
    let _one = serial();
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    // Microsoft's H.264 encoder follows a bitrate in CBR (R49), so Fit under
    // gives it one, corrected pass by pass when it lands over (R45).
    let dir = tempfile::tempdir().unwrap();
    let src = long_source(dir.path());
    let processor = VideoTune {
        encoder: EncoderChoice::Processor,
        ..VideoTune::default()
    };
    // Under the source, so encoded, and under the limit.
    let out = dir.path().join("under.mp4");
    let r = fit(&src, &out, 800_000, processor).expect("fits");
    assert!(!r.video_copied && !r.hardware, "{r:?}");
    // Smaller: under the limit, or TooSmall.
    let out = dir.path().join("small.mp4");
    fit(&src, &out, 400_000, processor);
    // Scaled down, so encoded whatever the limit, and with room to spare.
    let out = dir.path().join("roomy.mp4");
    let scaled = VideoTune {
        max_lines: Some(360),
        ..processor
    };
    let r = fit(&src, &out, 1_500_000, scaled).expect("fits");
    assert!(!r.video_copied && !r.hardware, "{r:?}");
}

#[test]
fn fit_under_on_the_graphics_card_lands_under() {
    let _one = serial();
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let src = long_source(dir.path());
    let out = dir.path().join("out.mp4");
    if let Some(r) = fit(&src, &out, 600_000, VideoTune::default()) {
        assert!(!r.video_copied, "{r:?}");
    }
}

#[test]
fn a_copy_fit_under_chose_that_lands_over_is_encoded_again() {
    let _one = serial();
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    // Loud noise at 320 kbps, which the size model (not knowing a video's
    // sound bitrate) puts at 128: Fit under answers Original, and the copy
    // lands over.
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("noisy.mp4");
    let long = long_source(dir.path());
    ffmpeg(
        &[
            "-i",
            long.to_str().unwrap(),
            "-f",
            "lavfi",
            "-i",
            "anoisesrc=color=white:sample_rate=48000:duration=10",
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-c:v",
            "copy",
            "-c:a",
            "aac",
            "-b:a",
            "320k",
            "-ac",
            "2",
        ],
        &src,
    );
    let info = kiln_media::video::probe(&src).unwrap();
    let tune = VideoTune::default();
    let limit = kiln_media::video::tune::estimate_bytes(
        VideoTarget::Mp4,
        &VideoSetting::Original,
        &info,
        &tune,
    )
    .expect("an estimate");
    let asked = VideoTune {
        fit_bytes: Some(limit),
        ..tune
    };
    assert_eq!(
        kiln_media::video::tune::fit_under(VideoTarget::Mp4, limit, &info, &asked).unwrap(),
        VideoSetting::Original
    );
    let copy = dir.path().join("copy.mp4");
    run(&src, &copy, VideoTarget::Mp4, &tune).unwrap();
    assert!(std::fs::metadata(&copy).unwrap().len() > limit);

    let out = dir.path().join("fit.mp4");
    if let Some(r) = fit(&src, &out, limit, tune) {
        assert!(!r.video_copied, "{r:?}");
    }
}

#[test]
fn a_fragmented_mp4_reports_progress_by_bytes() {
    let _one = serial();
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    // A fragmented MP4 states no length (R22), so progress is the share of
    // the file read (R43).
    // Several MB, so the share read moves along as the job goes.
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("fragmented.mp4");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=1280x720:rate=30:duration=10",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=10",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-preset",
            "veryfast",
            "-crf",
            "18",
            "-g",
            "30",
            "-c:a",
            "aac",
            "-shortest",
            "-movflags",
            "frag_keyframe+empty_moov",
        ],
        &src,
    );
    assert!(std::fs::metadata(&src).unwrap().len() > 3_000_000);
    assert_eq!(kiln_media::video::probe(&src).unwrap().duration, None);
    let out = dir.path().join("out.mkv");
    let mut seen = Vec::new();
    convert(
        &src,
        &out,
        VideoTarget::Mkv,
        Preset::High,
        &encode_at(65),
        &mut |p| seen.push(p),
        &|| false,
    )
    .unwrap();
    assert!(seen.windows(2).all(|w| w[0] <= w[1]), "{seen:?}");
    assert_eq!(seen.last(), Some(&1.0));
    let mut between: Vec<f32> = seen
        .iter()
        .copied()
        .filter(|&p| p > 0.0 && p < 1.0)
        .collect();
    between.dedup();
    assert!(between.len() >= 3, "{seen:?}");
    assert_eq!(packets(Probe::of(&out).video()), 300);
}

#[test]
fn ac3_in_mkv_is_copied_into_mkv_and_refused_for_mp4() {
    let _one = serial();
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    // R50: part 1 can't read AC-3 or E-AC-3 out of a Matroska file. MKV
    // takes AC-3 as it is; MP4 gets part 1's sentence, not "damaged".
    let dir = tempfile::tempdir().unwrap();
    let src = video("h264.mp4");
    let src = src.to_str().unwrap();
    for (codec, name, start) in [
        ("ac3", "AC3", "0"),
        ("ac3", "AC3", "0.5"),
        ("eac3", "EAC3", "0.5"),
    ] {
        let file = dir.path().join(format!("{codec}-{start}.mkv"));
        ffmpeg(
            &[
                "-i",
                src,
                "-itsoffset",
                start,
                "-i",
                src,
                "-map",
                "0:v",
                "-map",
                "1:a",
                "-c:v",
                "copy",
                "-c:a",
                codec,
            ],
            &file,
        );
        let out = dir.path().join(format!("{codec}-{start}.mp4"));
        let r = run(&file, &out, VideoTarget::Mp4, &VideoTune::default());
        match r {
            Err(MediaError::Unsupported { codec: c }) => assert_eq!(c, name, "{file:?}"),
            other => panic!("{file:?} to MP4: {other:?}"),
        }
        nothing_left(&out);
        let out = dir.path().join(format!("{codec}-{start}-to.mkv"));
        let r = run(&file, &out, VideoTarget::Mkv, &VideoTune::default());
        if codec == "ac3" {
            let report = r.unwrap();
            assert!(report.video_copied && report.sound_copied, "{report:?}");
            let sound = Probe::of(&out);
            assert_eq!(get(sound.sound().unwrap(), "codec_name"), "ac3");
        } else {
            // E-AC-3 has no stream type here: refused for MKV too.
            assert!(matches!(&r, Err(MediaError::Unsupported { .. })), "{r:?}");
            nothing_left(&out);
        }
    }
}
