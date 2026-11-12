//! Reading video through Media Foundation (`video::Reader`): packets for
//! copying, checked against ffprobe's view of the same file, and decoded
//! frames, on the graphics card and off it.
//!
//! The ffprobe comparisons skip with a printed line when FFmpeg isn't on
//! PATH; so do the parts that need a graphics card or a Store extension this
//! PC lacks.

#![cfg(windows)]

mod common;

use kiln_media::video::{
    adapters, d3d, d3d_for, muxer, Container, Frame, Hdr, Packet, Pixel, ReadMode, Reader,
    StreamCodec, MAX_HELD_FRAMES,
};
use kiln_media::{MediaError, Tags};
use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Command;
use windows::core::Interface;
use windows::Win32::Media::MediaFoundation::IMFDXGIBuffer;

// ---------------------------------------------------------------------------
// Helpers

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

fn run(cmd: &mut Command) -> String {
    let out = cmd.output().expect("the tool starts");
    assert!(
        out.status.success(),
        "{cmd:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn video(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/video")
        .join(name)
}

/// ffprobe's facts of stream `kind` (`v` or `a`), keys without the prefix.
fn stream_facts(path: &Path, kind: &str, entries: &str) -> HashMap<String, String> {
    let out = run(Command::new("ffprobe").args([
        "-v",
        "error",
        "-select_streams",
        &format!("{kind}:0"),
        "-show_entries",
        entries,
        "-of",
        "flat",
        path.to_str().unwrap(),
    ]));
    out.lines()
        .filter_map(|l| {
            let (k, v) = l.split_once('=')?;
            let k = k.strip_prefix("streams.stream.0.")?;
            Some((k.to_string(), v.trim_matches('"').to_string()))
        })
        .collect()
}

/// Seconds per tick of stream `kind`'s time base.
fn time_base(path: &Path, kind: &str) -> f64 {
    let f = stream_facts(path, kind, "stream=time_base");
    let (n, d) = f["time_base"].split_once('/').unwrap();
    n.parse::<f64>().unwrap() / d.parse::<f64>().unwrap()
}

/// One packet as ffprobe lists it, times in seconds.
#[derive(Debug, Clone)]
struct Pkt {
    pts: f64,
    dts: Option<f64>,
    size: usize,
    key: bool,
}

fn ffprobe_packets(path: &Path, kind: &str) -> Vec<Pkt> {
    let tb = time_base(path, kind);
    let out = run(Command::new("ffprobe").args([
        "-v",
        "error",
        "-select_streams",
        &format!("{kind}:0"),
        "-show_entries",
        "packet=pts,dts,size,flags",
        "-of",
        "compact=p=0",
        path.to_str().unwrap(),
    ]));
    out.lines()
        .filter(|l| !l.is_empty())
        .map(|l| {
            let f: HashMap<&str, &str> = l.split('|').filter_map(|kv| kv.split_once('=')).collect();
            let num = |k: &str| f.get(k).and_then(|v| v.parse::<i64>().ok());
            Pkt {
                pts: num("pts").unwrap() as f64 * tb,
                dts: num("dts").map(|d| d as f64 * tb),
                size: num("size").unwrap() as usize,
                key: f["flags"].starts_with('K'),
            }
        })
        .collect()
}

/// The stream's codec private data, from ffprobe's hex dump of it.
fn extradata(path: &Path, kind: &str) -> Vec<u8> {
    let out = run(Command::new("ffprobe").args([
        "-v",
        "error",
        "-select_streams",
        &format!("{kind}:0"),
        "-show_streams",
        "-show_data",
        path.to_str().unwrap(),
    ]));
    let mut bytes = Vec::new();
    let mut lines = out.lines().skip_while(|l| *l != "extradata=");
    lines.next();
    for l in lines {
        let Some((offset, rest)) = l.split_once(": ") else {
            break;
        };
        if offset.len() != 8 {
            break;
        }
        let hex: String = rest
            .split("  ")
            .next()
            .unwrap()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        bytes.extend(
            (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()),
        );
    }
    bytes
}

fn secs(t: i64) -> f64 {
    t as f64 / 10_000_000.0
}

fn open(path: &Path, mode: ReadMode) -> Reader {
    Reader::open(File::open(path).unwrap(), mode).unwrap()
}

/// Every packet the reader gives, in its order.
fn all_packets(r: &mut Reader) -> Vec<Packet> {
    let mut out = Vec::new();
    while let Some(p) = r.next_packet().unwrap() {
        out.push(p);
    }
    out
}

fn of_stream(all: &[Packet], stream: usize) -> Vec<Packet> {
    all.iter().filter(|p| p.stream == stream).cloned().collect()
}

/// Checks a stream's packets against ffprobe's: the same count, sizes and
/// key flags (for the picture), and each presentation time within 1 ms.
fn assert_matches_ffprobe(path: &Path, kind: &str, got: &[Packet]) {
    let want = ffprobe_packets(path, kind);
    assert_eq!(got.len(), want.len(), "{path:?} {kind}: packet count");
    for (i, (g, w)) in got.iter().zip(&want).enumerate() {
        assert_eq!(g.data.len(), w.size, "{path:?} {kind} packet {i}: size");
        if kind == "v" {
            assert_eq!(g.key, w.key, "{path:?} {kind} packet {i}: key");
        }
        assert!(
            (secs(g.pts) - w.pts).abs() <= 0.001,
            "{path:?} {kind} packet {i}: pts {} s, ffprobe {} s",
            secs(g.pts),
            w.pts
        );
    }
}

/// Decode times that a muxer takes: rising (strictly, for the picture),
/// never after the presentation time, and the streams within half a second
/// of each other.
fn assert_decode_order(all: &[Packet]) {
    let mut last: HashMap<usize, i64> = HashMap::new();
    let mut furthest = i64::MIN;
    for (i, p) in all.iter().enumerate() {
        if let Some(&l) = last.get(&p.stream) {
            if p.stream == 0 {
                assert!(p.dts > l, "packet {i}: dts {} after {l}", p.dts);
            } else {
                assert!(p.dts >= l, "packet {i}: dts {} after {l}", p.dts);
            }
        }
        assert!(p.dts <= p.pts, "packet {i}: dts {} > pts {}", p.dts, p.pts);
        assert!(
            p.dts >= furthest.saturating_sub(5_000_000),
            "packet {i} (stream {}) at {} s is well behind {} s",
            p.stream,
            secs(p.dts),
            secs(furthest)
        );
        furthest = furthest.max(p.dts);
        last.insert(p.stream, p.dts);
    }
}

// ---------------------------------------------------------------------------
// Compressed

#[test]
fn compressed_packets_of_h264_mp4_match_the_file() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    let path = video("h264.mp4");
    let mut r = open(&path, ReadMode::Compressed);
    assert_eq!(r.streams().len(), 2);
    assert_eq!(r.sound_unavailable(), None);
    let v = r.streams()[0].spec.clone();
    let a = r.streams()[1].spec.clone();
    assert_eq!((v.timescale, a.timescale), (10_000_000, 10_000_000));
    let StreamCodec::H264 { avcc } = &v.codec else {
        panic!("not H.264: {:?}", v.codec);
    };
    let StreamCodec::Aac { asc } = &a.codec else {
        panic!("not AAC: {:?}", a.codec);
    };
    // The avcC is the file's own; its profile and level are ffprobe's.
    assert_eq!(avcc, &extradata(&path, "v"));
    let facts = stream_facts(&path, "v", "stream=profile,level");
    assert_eq!(facts["profile"], "High");
    assert_eq!(avcc[1], 100);
    assert_eq!(avcc[3].to_string(), facts["level"]);
    assert_eq!(asc, &extradata(&path, "a"));
    let params = a.audio.as_ref().unwrap();
    assert_eq!((params.rate, params.channels), (48_000, 2));

    let all = all_packets(&mut r);
    assert_decode_order(&all);
    let pics = of_stream(&all, 0);
    assert_eq!(pics.len(), 60);
    // Key frames where the GOPs start, sizes byte for byte (the in-band
    // parameter sets Media Foundation adds are taken back out), and the edit
    // list applied: the first picture shows at 0.
    assert_matches_ffprobe(&path, "v", &pics);
    let keys: Vec<usize> = (0..60).filter(|&i| pics[i].key).collect();
    assert_eq!(keys, vec![0, 30]);
    assert_eq!(pics[0].pts, 0);
    // Decode times as the file has them: two frames before the first shown.
    for (i, (g, w)) in pics.iter().zip(ffprobe_packets(&path, "v")).enumerate() {
        let want = w.dts.unwrap();
        assert!(
            (secs(g.dts) - want).abs() <= 0.001,
            "packet {i}: dts {} s, ffprobe {want} s",
            secs(g.dts)
        );
    }
    // The samples are length-prefixed.
    let first = &pics[0].data;
    let len = u32::from_be_bytes(first[..4].try_into().unwrap()) as usize;
    assert!(len + 4 <= first.len());

    // The sound: ffprobe's packets (the priming edit puts the first at
    // -1,024 samples), lasting as long as the stream within one AAC frame.
    let sound = of_stream(&all, 1);
    assert_matches_ffprobe(&path, "a", &sound);
    let total: i64 = sound.iter().map(|p| p.duration).sum();
    let stream: f64 = stream_facts(&path, "a", "stream=duration")["duration"]
        .parse()
        .unwrap();
    let frame = 1024.0 / 48_000.0;
    assert!(
        (secs(total) - stream).abs() <= frame,
        "sound packets last {} s, the stream {stream} s",
        secs(total)
    );
    assert!(r.position() > 1.9, "position {}", r.position());
    // The end stays the end.
    assert!(r.next_packet().unwrap().is_none());
}

#[test]
fn vfr_timestamps_are_kept() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    let path = video("vfr.mp4");
    let mut r = open(&path, ReadMode::Compressed);
    let pics = all_packets(&mut r);
    let want = ffprobe_packets(&path, "v");
    assert_eq!(pics.len(), want.len());
    let mut got: Vec<f64> = pics.iter().map(|p| secs(p.pts)).collect();
    let mut want: Vec<f64> = want.iter().map(|p| p.pts).collect();
    got.sort_by(f64::total_cmp);
    want.sort_by(f64::total_cmp);
    let steps = |v: &[f64]| v.windows(2).map(|w| w[1] - w[0]).collect::<Vec<_>>();
    for (i, (g, w)) in steps(&got).iter().zip(steps(&want)).enumerate() {
        assert!((g - w).abs() <= 0.001, "step {i}: {g} s, ffprobe {w} s");
    }
    // Both rates are there: 33 ms and 67 ms steps.
    assert!(steps(&got).iter().any(|s| (s - 1.0 / 30.0).abs() < 0.001));
    assert!(steps(&got).iter().any(|s| (s - 1.0 / 15.0).abs() < 0.001));
    assert_decode_order(&pics);
}

#[test]
fn matroska_packets_match_the_file() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    // H.264 with Opus: the Opus packets start before 0 by the pre-skip, as
    // FFmpeg reads them (the writer puts it back as CodecDelay).
    let path = video("h264.mkv");
    let mut r = open(&path, ReadMode::Compressed);
    assert_eq!(r.streams().len(), 2);
    let StreamCodec::Opus { head } = &r.streams()[1].spec.codec else {
        panic!("not Opus");
    };
    assert_eq!(head, &extradata(&path, "a"));
    assert_eq!(r.info().fps.map(|f| f.round()), Some(30.0));
    let all = all_packets(&mut r);
    assert_decode_order(&all);
    assert_matches_ffprobe(&path, "v", &of_stream(&all, 0));
    let sound = of_stream(&all, 1);
    assert_matches_ffprobe(&path, "a", &sound);
    assert_eq!(sound[0].pts, -65_000);

    // AV1: the av1C is the file's.
    let path = video("av1.mkv");
    let mut r = open(&path, ReadMode::Compressed);
    let StreamCodec::Av1 { av1c } = &r.streams()[0].spec.codec else {
        panic!("not AV1");
    };
    assert_eq!(av1c, &extradata(&path, "v"));
    let all = all_packets(&mut r);
    assert_decode_order(&all);
    assert_matches_ffprobe(&path, "v", &all);
}

#[test]
fn hevc_copies_with_the_files_hvcc_and_hdr() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    for name in ["hevc.mp4", "hevc10_hdr.mp4"] {
        let path = video(name);
        let mut r = open(&path, ReadMode::Compressed);
        let spec = r.streams()[0].spec.clone();
        let StreamCodec::Hevc { hvcc } = &spec.codec else {
            panic!("{name}: not HEVC");
        };
        assert_eq!(hvcc, &extradata(&path, "v"), "{name}");
        let all = all_packets(&mut r);
        assert_decode_order(&all);
        assert_matches_ffprobe(&path, "v", &all);
        let v = spec.video.unwrap();
        if name == "hevc10_hdr.mp4" {
            assert_eq!((v.hdr, v.bit_depth), (Hdr::Pq, 10));
            // max-cll 1000,400.
            assert_eq!(v.content_light, Some([0x03, 0xE8, 0x01, 0x90]));
            assert!(v.mastering.is_some());
        } else {
            assert_eq!((v.hdr, v.bit_depth), (Hdr::None, 8));
        }
    }
}

#[test]
fn every_fixture_reads_compressed_in_decode_order() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    for name in [
        "h264.mp4",
        "h264.mkv",
        "hevc.mp4",
        "hevc10_hdr.mp4",
        "rot90.mp4",
        "vfr.mp4",
        "odd.mp4",
        "av1.mkv",
        "dts.mkv",
    ] {
        let path = video(name);
        let mut r = open(&path, ReadMode::Compressed);
        let v = r.streams()[0].spec.video.clone().unwrap();
        let info = r.info().clone();
        assert_eq!((v.width, v.height), (info.width, info.height), "{name}");
        if name == "rot90.mp4" {
            assert_eq!(v.rotation, 90);
        }
        let all = all_packets(&mut r);
        assert_decode_order(&all);
        assert_matches_ffprobe(&path, "v", &of_stream(&all, 0));
    }
}

#[test]
fn a_sound_codec_that_cant_be_copied_leaves_the_picture() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    // DTS: Media Foundation lists it, but no muxer takes it.
    let path = video("dts.mkv");
    let mut r = open(&path, ReadMode::Compressed);
    assert_eq!(r.streams().len(), 1);
    assert_eq!(r.sound_unavailable(), Some("DTS"));
    let all = all_packets(&mut r);
    assert!(all.iter().all(|p| p.stream == 0));
    assert_matches_ffprobe(&path, "v", &all);
}

/// Each sound codec the reader copies, in the containers FFmpeg puts it in,
/// against ffprobe: the same packets at the same times, and the codec
/// private data the file holds.
#[test]
fn sound_packets_of_each_codec_match_ffprobe() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let src = video("h264.mp4");
    for (name, encoder) in [
        ("aac.mkv", "aac"),
        ("ac3.mkv", "ac3"),
        ("ac3.mp4", "ac3"),
        ("flac.mkv", "flac"),
        ("flac.mp4", "flac"),
        ("opus.mp4", "libopus"),
        ("vorbis.mkv", "libvorbis"),
    ] {
        let path = dir.path().join(name);
        let made = Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-i"])
            .arg(&src)
            .args(["-f", "lavfi", "-i", "sine=d=2:r=48000"])
            .args(["-map", "0:v", "-c:v", "copy", "-map", "1:a", "-ac", "2"])
            .args(["-c:a", encoder, "-strict", "-2"])
            .arg(&path)
            .output()
            .unwrap();
        if !made.status.success() {
            eprintln!("skipped {name}: this FFmpeg can't make it");
            continue;
        }
        let mut r = open(&path, ReadMode::Compressed);
        assert_eq!(r.sound_unavailable(), None, "{name}");
        assert_eq!(r.streams().len(), 2, "{name}");
        let spec = r.streams()[1].spec.clone();
        let private = extradata(&path, "a");
        match &spec.codec {
            StreamCodec::Aac { asc } => assert_eq!(asc, &private, "{name}"),
            StreamCodec::Opus { head } => assert_eq!(head, &private, "{name}"),
            StreamCodec::Vorbis { private: p } => assert_eq!(p, &private, "{name}"),
            // The whole fLaC header, of which ffprobe shows the STREAMINFO.
            StreamCodec::Flac { streaminfo } => {
                assert!(streaminfo.starts_with(b"fLaC"), "{name}");
                assert_eq!(&streaminfo[8..42], &private[..], "{name}");
            }
            StreamCodec::Ac3 => {}
            other => panic!("{name}: {other:?}"),
        }
        let all = all_packets(&mut r);
        assert_decode_order(&all);
        assert_matches_ffprobe(&path, "a", &of_stream(&all, 1));
    }
}

#[test]
fn the_reader_lets_go_of_the_file() {
    if common::no_mf() {
        return;
    }
    use std::os::windows::fs::OpenOptionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("copy.mp4");
    std::fs::copy(video("h264.mp4"), &path).unwrap();
    for decoded in [false, true] {
        if decoded {
            let mut r = open(&path, ReadMode::Decoded { d3d: None });
            r.next_frame().unwrap().unwrap();
        } else {
            let mut r = open(&path, ReadMode::Compressed);
            r.next_packet().unwrap().unwrap();
        }
        // No handle is left: the file opens with no sharing at all.
        let only = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path);
        assert!(only.is_ok(), "{only:?}");
    }
    // A file it can't read is let go of too.
    std::fs::write(&path, b"not a video at all").unwrap();
    assert!(matches!(
        Reader::open(File::open(&path).unwrap(), ReadMode::Compressed),
        Err(MediaError::Damaged { code }) if code == "video"
    ));
    std::fs::remove_file(&path).unwrap();
}

/// What the reader copies, written by Kiln's own muxers, is what ffprobe
/// finds in the source: same packets, same times.
#[test]
fn copied_packets_make_the_same_file() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    for (name, c, ext) in [
        ("h264.mp4", Container::Mp4, "mp4"),
        ("h264.mkv", Container::Mkv, "mkv"),
        ("hevc10_hdr.mp4", Container::Mkv, "mkv"),
    ] {
        let src = video(name);
        let mut r = open(&src, ReadMode::Compressed);
        let out = dir.path().join(format!("copy-of-{name}.{ext}"));
        let mut m = muxer(c, File::create(&out).unwrap()).unwrap();
        for s in r.streams() {
            assert_eq!(m.add_stream(s.spec.clone()).unwrap(), s.index);
        }
        while let Some(p) = r.next_packet().unwrap() {
            m.write(p).unwrap();
        }
        m.finish(&Tags::default()).unwrap();
        for kind in ["v", "a"] {
            if kind == "a" && r.streams().len() < 2 {
                continue;
            }
            let want = ffprobe_packets(&src, kind);
            let got = ffprobe_packets(&out, kind);
            assert_eq!(got.len(), want.len(), "{name} {kind}");
            for (i, (g, w)) in got.iter().zip(&want).enumerate() {
                assert_eq!(g.size, w.size, "{name} {kind} packet {i}: size");
                assert!(
                    (g.pts - w.pts).abs() <= 0.0011,
                    "{name} {kind} packet {i}: pts {} s, source {} s",
                    g.pts,
                    w.pts
                );
            }
        }
        let decoded = Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(&out)
            .args(["-f", "null", "-"])
            .output()
            .unwrap();
        assert!(
            decoded.status.success() && decoded.stderr.is_empty(),
            "{name}: {}",
            String::from_utf8_lossy(&decoded.stderr)
        );
    }
}

// ---------------------------------------------------------------------------
// Decoded

/// Decodes every frame of `path`; the frames, the format after the last,
/// and whether every frame stayed on the graphics card.
fn decode_all(path: &Path, on_card: bool) -> (usize, kiln_media::video::FrameFormat, bool) {
    let d3d = if on_card { d3d() } else { None };
    let mut r = open(path, ReadMode::Decoded { d3d });
    assert_eq!(r.d3d().is_some(), on_card);
    let mut n = 0;
    let mut textures = true;
    let mut last = i64::MIN;
    while let Some(f) = r.next_frame().unwrap() {
        n += 1;
        assert!(f.pts > last, "frame {n}: {} after {last}", f.pts);
        last = f.pts;
        // SAFETY: plain calls on a sample the reader gave.
        let buffer = unsafe { f.sample.GetBufferByIndex(0) }.unwrap();
        textures &= buffer.cast::<IMFDXGIBuffer>().is_ok();
        assert_eq!(unsafe { f.sample.GetSampleTime() }.unwrap(), f.pts);
    }
    (n, r.frame_format().unwrap().clone(), textures)
}

#[test]
fn decoded_frames_count_and_size() {
    if common::no_mf() {
        return;
    }
    let path = video("h264.mp4");
    let (n, format, textures) = decode_all(&path, false);
    assert_eq!(n, 60);
    assert_eq!((format.width, format.height), (320, 240));
    assert_eq!((format.shown.width, format.shown.height), (320, 240));
    assert_eq!(format.pixel, Pixel::Nv12);
    assert!(!textures, "frames decoded without a card are in memory");

    if d3d().is_none() {
        eprintln!("skipped the graphics card half: no card makes a video device");
        return;
    }
    let (n, format, textures) = decode_all(&path, true);
    assert_eq!(n, 60);
    assert_eq!((format.width, format.height), (320, 240));
    // On the card, nothing is copied to memory.
    assert!(textures, "every frame is a texture on the card");
}

#[test]
fn hevc_reads_or_says_what_is_missing() {
    if common::no_mf() {
        return;
    }
    let path = video("hevc.mp4");
    match Reader::open(File::open(&path).unwrap(), ReadMode::Decoded { d3d: None }) {
        Ok(_) => {}
        Err(MediaError::NeedsExtension {
            codec: "HEVC",
            product: "9N4WGH0Z6VHQ",
        }) => {
            eprintln!("skipped: the HEVC Video Extensions aren't installed");
            return;
        }
        Err(e) => panic!("{e}"),
    }
    // In memory, and on the card when there is one.
    let (n, format, textures) = decode_all(&path, false);
    assert_eq!(n, 60);
    assert_eq!((format.shown.width, format.shown.height), (320, 240));
    assert!(!textures);
    if d3d().is_some() {
        let (n, format, textures) = decode_all(&path, true);
        assert_eq!(n, 60);
        assert_eq!((format.width, format.height), (320, 240));
        assert!(textures, "every HEVC frame is a texture on the card");
    } else {
        eprintln!("skipped the graphics card half: no card makes a video device");
    }
    // The 10-bit file decodes, to P010, in memory and on the card.
    let hdr = video("hevc10_hdr.mp4");
    for on_card in [false, true] {
        if on_card && d3d().is_none() {
            continue;
        }
        let (n, format, _) = decode_all(&hdr, on_card);
        assert_eq!(n, 60);
        assert_eq!(format.pixel, Pixel::P010, "on the card: {on_card}");
    }
}

/// A padded frame says which part of it is the picture (R4: the processor
/// crops to it).
#[test]
fn odd_frames_say_what_part_is_picture() {
    if common::no_mf() {
        return;
    }
    let path = video("odd.mp4");
    let mut r = match Reader::open(File::open(&path).unwrap(), ReadMode::Decoded { d3d: None }) {
        Ok(r) => r,
        Err(MediaError::NeedsExtension { codec: "AV1", .. }) => {
            eprintln!("skipped: the AV1 Video Extension isn't installed");
            return;
        }
        Err(e) => panic!("{e}"),
    };
    let mut n = 0;
    while r.next_frame().unwrap().is_some() {
        n += 1;
    }
    assert_eq!(n, 10);
    let f = r.frame_format().unwrap();
    assert_eq!((f.shown.x, f.shown.y), (0, 0));
    assert_eq!((f.shown.width, f.shown.height), (885, 1919));
    assert!(f.width >= 886 && f.height >= 1920, "{f:?}");
}

#[test]
fn reading_the_wrong_way_is_refused() {
    if common::no_mf() {
        return;
    }
    let mut r = open(&video("h264.mp4"), ReadMode::Compressed);
    assert!(r.next_frame().is_err());
    assert!(r.frame_format().is_none());
    let mut r = open(&video("h264.mp4"), ReadMode::Decoded { d3d: None });
    assert!(r.next_packet().is_err());
    // A file with no picture.
    let sound = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/tone_aac.m4a");
    assert!(matches!(
        Reader::open(File::open(&sound).unwrap(), ReadMode::Compressed),
        Err(MediaError::NoVideo)
    ));
}

#[test]
fn a_device_can_be_made_on_each_card() {
    let cards = adapters();
    if cards.is_empty() {
        eprintln!("skipped: no graphics card");
        return;
    }
    let any = d3d().expect("a card is listed, so a device is made");
    assert!(cards.contains(any.adapter()));
    for c in &cards {
        if let Some(d) = d3d_for(c.luid) {
            assert_eq!(d.adapter(), c);
            // Clones share the device.
            let again = d.clone();
            assert_eq!(again.device(), d.device());
        }
    }
    assert!(d3d_for(0).is_none());
}

// ---------------------------------------------------------------------------
// Files cut short, and frames held

/// The first `percent` of the file at `src`, as `name` in `dir`.
fn cut_short(dir: &Path, src: &Path, name: &str, percent: u64) -> PathBuf {
    let whole = std::fs::read(src).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, &whole[..whole.len() * percent as usize / 100]).unwrap();
    path
}

/// Has FFmpeg make `out` in `dir` with `args` (inputs and options), or
/// says why not and gives None.
fn make(dir: &Path, out: &str, args: &[&str]) -> Option<PathBuf> {
    let path = dir.join(out);
    let made = Command::new("ffmpeg")
        .args(["-v", "error", "-y"])
        .args(args)
        .arg(&path)
        .output()
        .unwrap();
    if !made.status.success() {
        eprintln!(
            "skipped {out}: FFmpeg couldn't make it: {}",
            String::from_utf8_lossy(&made.stderr)
        );
        return None;
    }
    Some(path)
}

/// Reads `path` to its end in `mode`, packets or frames.
fn read_to_end(path: &Path, decoded: bool) -> Result<usize, MediaError> {
    let mode = if decoded {
        ReadMode::Decoded { d3d: None }
    } else {
        ReadMode::Compressed
    };
    let mut r = Reader::open(File::open(path).unwrap(), mode)?;
    let mut n = 0;
    if decoded {
        while r.next_frame()?.is_some() {
            n += 1;
        }
    } else {
        while r.next_packet()?.is_some() {
            n += 1;
        }
    }
    Ok(n)
}

fn assert_damaged(path: &Path, what: &str) {
    for decoded in [false, true] {
        let got = read_to_end(path, decoded);
        assert!(
            matches!(&got, Err(MediaError::Damaged { code }) if code == "video"),
            "{what}, decoded {decoded}: {got:?}"
        );
    }
}

#[test]
fn a_file_cut_short_is_damaged_not_ended() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let h264 = video("h264.mp4");
    // An MP4 with its moov first, so a cut leaves the tables whole and only
    // the samples short: the sample tables say where the data must reach.
    let fast = make(
        d,
        "fast.mp4",
        &[
            "-i",
            h264.to_str().unwrap(),
            "-c",
            "copy",
            "-movflags",
            "+faststart",
        ],
    )
    .unwrap();
    assert_eq!(read_to_end(&fast, false).unwrap(), 60 + 95);
    for percent in [60, 98] {
        let cut = cut_short(d, &fast, &format!("fast-{percent}.mp4"), percent);
        assert_damaged(&cut, &format!("faststart MP4 cut to {percent}%"));
    }
    // The sound's last stsc run grown by one sample past what stsz lists,
    // and the last byte cut: the samples the tables agree on still bound it.
    let mut bytes = std::fs::read(&fast).unwrap();
    let find = |b: &[u8], what: &[u8], from: usize| {
        from + b[from..]
            .windows(what.len())
            .position(|w| w == what)
            .unwrap()
    };
    let stsc = find(&bytes, b"stsc", find(&bytes, b"soun", 0));
    let runs = u32::from_be_bytes(bytes[stsc + 8..stsc + 12].try_into().unwrap()) as usize;
    let per = stsc + 12 + (runs - 1) * 12 + 4;
    let n = u32::from_be_bytes(bytes[per..per + 4].try_into().unwrap());
    bytes[per..per + 4].copy_from_slice(&(n + 1).to_be_bytes());
    bytes.pop();
    let patched = d.join("fast-stsc.mp4");
    std::fs::write(&patched, &bytes).unwrap();
    assert_damaged(&patched, "MP4 with a long stsc run, cut by a byte");
    // The fixture keeps its moov at the end: cut, it has none.
    assert_damaged(
        &cut_short(d, &h264, "h264-60.mp4", 60),
        "MP4 without its moov",
    );

    // A Matroska with sizes everywhere, cut: its Segment runs past the end.
    let mkv = video("h264.mkv");
    assert_damaged(&cut_short(d, &mkv, "h264-60.mkv", 60), "Matroska");
    // Written live to a pipe, FFmpeg leaves the Segment's size unknown and
    // writes no Duration; its Clusters still have sizes, and the last one
    // cut runs past the end.
    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&mkv)
        .args(["-c", "copy", "-live", "1", "-f", "matroska", "-"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let piped = d.join("piped.mkv");
    std::fs::write(&piped, &out.stdout).unwrap();
    let duration = run(Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "csv=p=0",
        ])
        .arg(&piped));
    assert_eq!(duration.trim(), "N/A", "the piped file has no Duration");
    let segment = out
        .stdout
        .windows(4)
        .position(|w| w == [0x18, 0x53, 0x80, 0x67])
        .unwrap();
    assert_eq!(
        out.stdout[segment + 4],
        0x01,
        "the Segment's size is unknown"
    );
    assert!(out.stdout[segment + 5..segment + 12]
        .iter()
        .all(|&b| b == 0xFF));
    assert_eq!(read_to_end(&piped, false).unwrap(), 60 + 101);
    assert_eq!(read_to_end(&piped, true).unwrap(), 60);
    assert_damaged(
        &cut_short(d, &piped, "piped-60.mkv", 60),
        "a piped Matroska with no Segment size or Duration",
    );
}

#[test]
fn a_fragmented_mp4_cut_short_is_damaged() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let h264 = video("h264.mp4");
    let frag = make(
        d,
        "frag.mp4",
        &[
            "-i",
            h264.to_str().unwrap(),
            "-c",
            "copy",
            "-movflags",
            "frag_keyframe+empty_moov",
        ],
    )
    .unwrap();
    let pictures = ffprobe_packets(&frag, "v").len();
    let sounds = ffprobe_packets(&frag, "a").len();
    assert_eq!(read_to_end(&frag, false).unwrap(), pictures + sounds);
    assert_eq!(read_to_end(&frag, true).unwrap(), pictures);
    // Cut inside the last mdat, and inside the middle of the file.
    for percent in [60, 98] {
        let cut = cut_short(d, &frag, &format!("frag-{percent}.mp4"), percent);
        assert_damaged(&cut, &format!("fragmented MP4 cut to {percent}%"));
    }
}

#[test]
fn a_matroska_cut_short_probes_with_its_sound_unreadable() {
    if common::no_mf() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let cut = cut_short(dir.path(), &video("h264.mkv"), "h264-60.mkv", 60);
    // The sound reader fails on this file (the registry's Symphonia panics;
    // the vendored one errs): the probe keeps the picture and says the sound
    // can't be read.
    let info = kiln_media::video::probe(&cut).unwrap();
    assert!(info.sound.is_none());
    assert!(info.sound_unreadable.is_some(), "{info:?}");
    assert_damaged(&cut, "Matroska cut to 60%");
}

#[test]
fn sound_that_outlasts_the_picture_is_no_damage() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let h264 = video("h264.mp4");
    // 2 s of picture with 2.2 s of sound, and 1 s with 3 s.
    for (name, picture, sound) in [("long-a.mkv", "2", "2.2"), ("long-b.mkv", "1", "3")] {
        let Some(path) = make(
            dir.path(),
            name,
            &[
                "-t",
                picture,
                "-i",
                h264.to_str().unwrap(),
                "-f",
                "lavfi",
                "-i",
                &format!("sine=d={sound}:r=48000"),
                "-map",
                "0:v",
                "-map",
                "1:a",
                "-c:v",
                "copy",
                "-c:a",
                "aac",
            ],
        ) else {
            continue;
        };
        let pictures = ffprobe_packets(&path, "v").len();
        let mut r = open(&path, ReadMode::Compressed);
        let all = all_packets(&mut r);
        assert_eq!(of_stream(&all, 0).len(), pictures, "{name}");
        assert_matches_ffprobe(&path, "a", &of_stream(&all, 1));
        assert_eq!(read_to_end(&path, true).unwrap(), pictures, "{name}");
    }
}

#[test]
fn an_hevc_mp4_starting_on_a_cra_decodes() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    // Cut at the second GOP of an open-GOP stream: it starts on a CRA, and
    // the decoder rightly drops the RASL pictures after it.
    let src = video("hevc.mp4");
    let path = make(
        dir.path(),
        "cra.mp4",
        &["-ss", "1", "-i", src.to_str().unwrap(), "-c", "copy"],
    )
    .unwrap();
    let mut r = open(&path, ReadMode::Compressed);
    let all = all_packets(&mut r);
    assert_matches_ffprobe(&path, "v", &all);
    let first = &all[0].data;
    let mut at = 0;
    let mut kinds = Vec::new();
    while at + 4 < first.len() {
        let len = u32::from_be_bytes(first[at..at + 4].try_into().unwrap()) as usize;
        kinds.push((first[at + 4] >> 1) & 0x3F);
        at += 4 + len;
    }
    assert!(kinds.contains(&21), "the first picture is a CRA: {kinds:?}");
    match Reader::open(File::open(&path).unwrap(), ReadMode::Decoded { d3d: None }) {
        Err(MediaError::NeedsExtension { .. }) => {
            eprintln!("skipped decoding: the HEVC Video Extensions aren't installed")
        }
        Err(e) => panic!("{e}"),
        Ok(_) => {
            let n = read_to_end(&path, true).unwrap();
            assert!(n > 0 && n <= all.len(), "{n} of {}", all.len());
        }
    }
}

#[test]
fn a_matroskas_own_hdr_sei_stays() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let src = video("hevc10_hdr.mp4");
    let path = make(
        dir.path(),
        "hdr.mkv",
        &["-i", src.to_str().unwrap(), "-c", "copy"],
    )
    .unwrap();
    // The first key packet, byte for byte as the file holds it.
    let raw = dir.path().join("first.bin");
    run(Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-i"])
        .arg(&path)
        .args(["-map", "0:v", "-c", "copy", "-frames:v", "1", "-f", "data"])
        .arg(&raw));
    let want = std::fs::read(&raw).unwrap();
    let mut r = open(&path, ReadMode::Compressed);
    let first = r.next_packet().unwrap().unwrap();
    assert!(first.key);
    assert_eq!(first.data.len(), want.len());
    assert_eq!(first.data, want);
    // And all of them, by size.
    let mut r = open(&path, ReadMode::Compressed);
    assert_matches_ffprobe(&path, "v", &all_packets(&mut r));
}

#[test]
fn five_one_aac_keeps_its_sound() {
    if common::no_mf() || !ffmpeg_ready() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let h264 = video("h264.mp4");
    let Some(path) = make(
        dir.path(),
        "aac51.mp4",
        &[
            "-i",
            h264.to_str().unwrap(),
            "-f",
            "lavfi",
            "-i",
            "sine=d=2:r=48000",
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-c:v",
            "copy",
            "-ac",
            "6",
            "-c:a",
            "aac",
        ],
    ) else {
        return;
    };
    let r = open(&path, ReadMode::Compressed);
    assert_eq!(r.sound_unavailable(), None);
    assert_eq!(r.streams()[1].spec.audio.as_ref().unwrap().channels, 6);
}

/// Decodes `path` on the card holding `held` frames at a time (the oldest
/// let go as each new one comes). `Err` says what went wrong: the reader's
/// own error or panic, or a read that never came back.
fn decodes_holding(path: &Path, held: usize) -> Result<(), String> {
    enum Said {
        Frame,
        Done,
        Failed(String),
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let path = path.to_path_buf();
    std::thread::spawn(move || {
        let run = || -> Result<(), MediaError> {
            let mut r = Reader::open(File::open(&path).unwrap(), ReadMode::Decoded { d3d: d3d() })?;
            let mut kept: std::collections::VecDeque<Frame> = Default::default();
            while let Some(f) = r.next_frame()? {
                kept.push_back(f);
                if kept.len() > held {
                    kept.pop_front();
                }
                let _ = tx.send(Said::Frame);
            }
            Ok(())
        };
        let said = match run() {
            Ok(()) => Said::Done,
            Err(e) => Said::Failed(format!("{e:?}")),
        };
        let _ = tx.send(said);
    });
    loop {
        match rx.recv_timeout(std::time::Duration::from_secs(20)) {
            Ok(Said::Done) => return Ok(()),
            Ok(Said::Frame) => {}
            Ok(Said::Failed(e)) => return Err(format!("the reader failed: {e}")),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err("the reader thread panicked".into())
            }
            // A stalled decoder: the thread is left behind, stuck.
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                return Err("the decoder stalled".into())
            }
        }
    }
}

#[test]
fn frames_up_to_the_limit_can_be_held_on_the_card() {
    if common::no_mf() {
        return;
    }
    if d3d().is_none() {
        eprintln!("skipped: no card makes a video device");
        return;
    }
    // MAX_HELD_FRAMES alive at once: the ones held and the one being read.
    for name in ["h264.mp4", "hevc.mp4", "av1.mkv"] {
        if let Err(e) = decodes_holding(&video(name), MAX_HELD_FRAMES - 1) {
            panic!("{name}, holding {}: {e}", MAX_HELD_FRAMES - 1);
        }
    }
}

#[test]
fn a_reader_of_frames_moves_to_a_time() {
    // The estimate's sample starts in the middle (spec 3.3). h264.mp4 is
    // two seconds at 30 fps with a key frame each second.
    if common::no_mf() {
        return;
    }
    const SECOND: i64 = 10_000_000;
    for mkv in [false, true] {
        let path = video(if mkv { "h264.mkv" } else { "h264.mp4" });
        let mut whole = open(&path, ReadMode::Decoded { d3d: None });
        let mut all = 0;
        while whole.next_frame().unwrap().is_some() {
            all += 1;
        }
        let mut r = open(&path, ReadMode::Decoded { d3d: None });
        r.seek(1.5).unwrap();
        let first = r.next_frame().unwrap().unwrap();
        // From the key frame at or before the time asked: one second in.
        assert!(
            (SECOND - SECOND / 10..=SECOND * 3 / 2).contains(&first.pts),
            "{mkv}: {}",
            first.pts
        );
        drop(first);
        let mut rest = 1;
        while r.next_frame().unwrap().is_some() {
            rest += 1;
        }
        assert!(rest < all && rest >= all / 4, "{mkv}: {rest} of {all}");
    }
    // A reader of packets doesn't move.
    let mut packets = open(&video("h264.mp4"), ReadMode::Compressed);
    assert!(packets.seek(1.0).is_err());
}
