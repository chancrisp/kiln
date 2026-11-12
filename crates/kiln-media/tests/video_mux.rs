//! The MP4, MOV, M4V and Matroska writers (`video::muxer`).
//!
//! Round-trip tests take real packets from the fixtures in tests/data/video by
//! having FFmpeg, when it is on PATH, copy each stream out (`-c copy -f data`)
//! next to ffprobe's list of the packets (size, times, flags): exact payloads,
//! and exact times and key flags, in the container's own length-prefixed form
//! and with the codec private data the container holds. They then check the
//! output with Kiln's own reader and with ffprobe and FFmpeg's decoder.
//! Those tests skip with a printed line when FFmpeg is absent. The byte-level
//! tests use made-up payloads and always run.

use kiln_media::fuzzing::{mkv_children, mp4_children, mp4_muxer_with_bias};
use kiln_media::video::{
    muxer, probe, AudioParams, Container, Hdr, Mux, Packet, StreamCodec, StreamSpec, VideoParams,
};
use kiln_media::Tags;
use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// FFmpeg helpers

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

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data")
        .join(name)
}

fn video_fixture(name: &str) -> PathBuf {
    fixture(&format!("video/{name}"))
}

/// ffprobe's `-of flat` for stream `kind` (`v` or `a`)'s first stream, keys
/// without the prefix.
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

/// One packet as ffprobe lists it.
#[derive(Debug, Clone, PartialEq)]
struct Pkt {
    pts: Option<i64>,
    dts: Option<i64>,
    duration: i64,
    size: usize,
    key: bool,
}

fn packets(path: &Path, kind: &str) -> Vec<Pkt> {
    let out = run(Command::new("ffprobe").args([
        "-v",
        "error",
        "-select_streams",
        &format!("{kind}:0"),
        "-show_entries",
        "packet=pts,dts,duration,size,flags",
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
                pts: num("pts"),
                dts: num("dts"),
                duration: num("duration").unwrap_or(0),
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

/// A stream copied out of a fixture.
#[derive(Debug, Clone)]
struct Src {
    spec: StreamSpec,
    packets: Vec<Packet>,
}

/// The first byte string `tag` (a box type) finds in `file`, `len` bytes of
/// the box's body.
fn raw_box(file: &Path, tag: &[u8; 4], len: usize) -> Option<Vec<u8>> {
    let b = std::fs::read(file).ok()?;
    let at = b.windows(4).position(|w| w == tag)? + 4;
    b.get(at..at + len).map(<[u8]>::to_vec)
}

/// The decode times a stream with no stored ones would have had: the sorted
/// presentation times, held back by the stream's reorder depth.
fn derive_dts(pts: &[i64]) -> Vec<i64> {
    let mut sorted = pts.to_vec();
    sorted.sort_unstable();
    let depth = pts
        .iter()
        .enumerate()
        .map(|(i, p)| sorted.partition_point(|s| s < p).saturating_sub(i))
        .max()
        .unwrap_or(0);
    let step = sorted
        .windows(2)
        .map(|w| w[1] - w[0])
        .find(|&d| d > 0)
        .unwrap_or(1);
    (0..pts.len())
        .map(|i| {
            if i >= depth {
                sorted[i - depth]
            } else {
                sorted[0] - (depth - i) as i64 * step
            }
        })
        .collect()
}

/// Stream `kind` (`v` or `a`) of `path`, packet by packet. `None` if the file
/// has no such stream.
fn extract(path: &Path, kind: &str) -> Option<Src> {
    let facts = stream_facts(
        path,
        kind,
        "stream=codec_name,time_base,sample_rate,channels",
    );
    let codec_name = facts.get("codec_name")?;
    let timescale: u32 = facts["time_base"].strip_prefix("1/")?.parse().ok()?;
    let private = extradata(path, kind);
    let codec = match codec_name.as_str() {
        "h264" => StreamCodec::H264 { avcc: private },
        "hevc" => StreamCodec::Hevc { hvcc: private },
        "av1" => StreamCodec::Av1 { av1c: private },
        "aac" => StreamCodec::Aac { asc: private },
        "opus" => StreamCodec::Opus { head: private },
        "ac3" => StreamCodec::Ac3,
        other => panic!("the fixtures hold no {other}"),
    };
    let (video, audio) = if kind == "v" {
        let v = probe(path).expect("the fixture probes");
        (
            Some(VideoParams {
                width: v.width,
                height: v.height,
                rotation: v.rotation,
                hdr: v.hdr,
                bit_depth: v.bit_depth,
                mastering: raw_box(path, b"mdcv", 24).map(|b| b.try_into().unwrap()),
                content_light: raw_box(path, b"clli", 4).map(|b| b.try_into().unwrap()),
                sar: v.sar,
                tag_colour: false,
                bt601: false,
                frame_ns: None,
                stated_colour: None,
            }),
            None,
        )
    } else {
        (
            None,
            Some(AudioParams {
                rate: facts["sample_rate"].parse().unwrap(),
                channels: facts["channels"].parse().unwrap(),
            }),
        )
    };

    let list = packets(path, kind);
    let dir = tempfile::tempdir().unwrap();
    let raw = dir.path().join("stream.bin");
    run(Command::new("ffmpeg").args([
        "-v",
        "error",
        "-y",
        "-i",
        path.to_str().unwrap(),
        "-map",
        &format!("0:{kind}:0"),
        "-c",
        "copy",
        "-f",
        "data",
        raw.to_str().unwrap(),
    ]));
    let bytes = std::fs::read(&raw).unwrap();
    assert_eq!(
        bytes.len(),
        list.iter().map(|p| p.size).sum::<usize>(),
        "{path:?}: the copied bytes and the packet list disagree"
    );

    let pts: Vec<i64> = list.iter().map(|p| p.pts.expect("a pts")).collect();
    let dts: Vec<i64> = if list.iter().all(|p| p.dts.is_some()) {
        list.iter().map(|p| p.dts.unwrap()).collect()
    } else {
        derive_dts(&pts)
    };
    let mut at = 0;
    let packets = list
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let data = bytes[at..at + p.size].to_vec();
            at += p.size;
            Packet {
                stream: 0,
                pts: pts[i],
                dts: dts[i],
                duration: p.duration,
                key: p.key,
                data,
            }
        })
        .collect();
    Some(Src {
        spec: StreamSpec {
            codec,
            timescale,
            video,
            audio,
        },
        packets,
    })
}

/// `s` with every time given in `scale` ticks per second instead.
fn retimed(s: &Src, scale: u32) -> Src {
    let to = |v: i64| {
        let n = v as i128 * scale as i128;
        let d = s.spec.timescale as i128;
        (n.div_euclid(d) + i128::from(n.rem_euclid(d) * 2 >= d)) as i64
    };
    let mut out = s.clone();
    out.spec.timescale = scale;
    for p in &mut out.packets {
        p.pts = to(p.pts);
        p.dts = to(p.dts);
        p.duration = to(p.duration);
    }
    out
}

/// Writes the streams into `name` in `dir`, packets in decode-time order.
fn write(c: Container, streams: &[&Src], tags: &Tags, dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    let mut m = muxer(c, File::create(&path).unwrap()).unwrap();
    feed(&mut *m, streams);
    m.finish(tags).unwrap();
    path
}

fn feed(m: &mut dyn Mux, streams: &[&Src]) {
    let mut all = Vec::new();
    for (i, s) in streams.iter().enumerate() {
        assert_eq!(m.add_stream(s.spec.clone()).unwrap(), i);
        for p in &s.packets {
            let t = p.dts as f64 / s.spec.timescale as f64;
            all.push((
                t,
                Packet {
                    stream: i,
                    ..p.clone()
                },
            ));
        }
    }
    all.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (_, p) in all {
        m.write(p).unwrap();
    }
}

fn rescaled(v: i64, from: u32, to: u32) -> i64 {
    let n = v as i128 * to as i128;
    let d = from as i128;
    (n.div_euclid(d) + i128::from(n.rem_euclid(d) * 2 >= d)) as i64
}

/// True when FFmpeg decodes every frame of `path` without a word of complaint.
fn decodes_cleanly(path: &Path) -> bool {
    let out = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-i",
            path.to_str().unwrap(),
            "-f",
            "null",
            "-",
        ])
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&out.stderr);
    if !said.trim().is_empty() {
        eprintln!("{path:?}: {said}");
    }
    out.status.success() && said.trim().is_empty()
}

/// `ticks` of `scale` per second, plus `extra` 48 kHz samples (Opus' pre-skip),
/// as the nearest millisecond, halves up.
fn ms_with_delay(ticks: i64, scale: u32, extra: i64) -> i64 {
    let num = (ticks as i128 * 48_000 + extra as i128 * scale as i128) * 1000;
    let den = scale as i128 * 48_000;
    (num.div_euclid(den) + i128::from(num.rem_euclid(den) * 2 >= den)) as i64
}

fn pre_skip(s: &Src) -> i64 {
    match &s.spec.codec {
        StreamCodec::Opus { head } => u16::from_le_bytes([head[10], head[11]]) as i64,
        _ => 0,
    }
}

/// How far later than their own a Matroska's times must be so none is before 0.
fn mkv_shift(streams: &[&Src]) -> i64 {
    let first = streams
        .iter()
        .map(|s| {
            s.packets
                .iter()
                .map(|p| ms_with_delay(p.pts, s.spec.timescale, pre_skip(s)))
                .min()
                .unwrap()
        })
        .min()
        .unwrap();
    (-first).max(0)
}

/// The size of the NAL length prefix, and the VPS, SPS and PPS an HEVC
/// stream's hvcC holds (its SEI messages, which stay in the samples, aren't
/// counted).
fn hvcc_parameter_sets(codec: &StreamCodec) -> (usize, Vec<Vec<u8>>) {
    let StreamCodec::Hevc { hvcc } = codec else {
        panic!("not HEVC");
    };
    let length = (hvcc[21] & 3) as usize + 1;
    let mut sets: Vec<Vec<u8>> = Vec::new();
    let mut at = 23;
    for _ in 0..hvcc[22] {
        let kind = hvcc[at] & 0x3F;
        let count = u16::from_be_bytes([hvcc[at + 1], hvcc[at + 2]]);
        at += 3;
        for _ in 0..count {
            let len = u16::from_be_bytes([hvcc[at], hvcc[at + 1]]) as usize;
            if (32..=34).contains(&kind) {
                sets.push(hvcc[at + 2..at + 2 + len].to_vec());
            }
            at += 2 + len;
        }
    }
    (length, sets)
}

/// `src` (an HEVC stream) as an MP4 holds it: each sample without the NAL
/// units that are byte for byte in the hvcC, until a sample brings a parameter
/// set the hvcC doesn't have (after which nothing is taken out, and the entry
/// is `hev1`). Also says whether that happened.
fn without_repeated_parameter_sets(src: &Src) -> (Src, bool) {
    let (length, sets) = hvcc_parameter_sets(&src.spec.codec);
    let mut out = src.clone();
    let mut hev1 = false;
    for p in &mut out.packets {
        if hev1 {
            break;
        }
        let mut kept = Vec::new();
        let mut at = 0;
        let mut foreign = false;
        while at < p.data.len() {
            let len = p.data[at..at + length]
                .iter()
                .fold(0usize, |n, &b| (n << 8) | b as usize);
            let nal = &p.data[at + length..at + length + len];
            if !sets.iter().any(|s| s == nal) {
                if (32..=34).contains(&((nal[0] >> 1) & 0x3F)) {
                    foreign = true;
                }
                kept.extend_from_slice(&p.data[at..at + length + len]);
            }
            at += length + len;
        }
        if foreign {
            hev1 = true;
        } else {
            p.data = kept;
        }
    }
    (out, hev1)
}

/// The media time scales an MP4 track of `src` may have: the sample rate for
/// sound, the stream's own for a picture on a clock of 90,000 or less, and
/// else one of the coarser clocks the writer picks from.
fn mp4_scales(src: &Src) -> Vec<u32> {
    if let Some(a) = &src.spec.audio {
        vec![a.rate]
    } else if src.spec.timescale <= 90_000 {
        vec![src.spec.timescale]
    } else {
        vec![90_000, 24_000, 30_000, 60_000, 120_000]
    }
}

/// Checks `out`'s stream `kind` against `src`: the same packets, in the same
/// order, with the same payloads and key flags and the same times.
/// - An MP4 track counts in its own media time scale (see [`mp4_scales`]);
///   each time is the source's, converted on its own and rounded: exact.
/// - A Matroska keeps presentation times in milliseconds, moved later by
///   `mkv_shift`, exact for picture and AAC. FFmpeg's reader takes Opus'
///   pre-skip off with a rounding of its own, and AC-3 (whose first frame
///   starts before 0) is read a millisecond off, so those two may be 1 out.
fn assert_same_packets(src: &Src, out: &Path, kind: &str, mkv: Option<i64>, same_data: bool) {
    let got = packets(out, kind);
    let facts = stream_facts(out, kind, "stream=time_base");
    let out_scale: u32 = facts["time_base"]
        .strip_prefix("1/")
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(got.len(), src.packets.len(), "{out:?} {kind}: packet count");
    if mkv.is_some() {
        assert_eq!(out_scale, 1000);
    } else {
        assert!(
            mp4_scales(src).contains(&out_scale),
            "{out:?}: time scale {out_scale}"
        );
    }
    let tolerance = i64::from(matches!(
        src.spec.codec,
        StreamCodec::Opus { .. } | StreamCodec::Ac3
    ));
    for (i, (g, p)) in got.iter().zip(&src.packets).enumerate() {
        match mkv {
            Some(shift) => {
                let want = rescaled(p.pts, src.spec.timescale, 1000) + shift;
                let got = g.pts.expect("a pts");
                assert!(
                    (got - want).abs() <= tolerance,
                    "{out:?} {kind} packet {i}: pts {got}, not {want}"
                );
            }
            None => {
                let to = |t: i64| rescaled(t, src.spec.timescale, out_scale);
                assert_eq!(g.pts, Some(to(p.pts)), "{out:?} {kind} packet {i}: pts");
                assert_eq!(g.dts, Some(to(p.dts)), "{out:?} {kind} packet {i}: dts");
            }
        }
        if kind == "v" {
            assert_eq!(g.key, p.key, "{out:?} {kind} packet {i}: key");
        }
        if same_data {
            assert_eq!(g.size, p.data.len(), "{out:?} {kind} packet {i}: size");
        }
    }
    if same_data {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("out.bin");
        run(Command::new("ffmpeg").args([
            "-v",
            "error",
            "-y",
            "-i",
            out.to_str().unwrap(),
            "-map",
            &format!("0:{kind}:0"),
            "-c",
            "copy",
            "-f",
            "data",
            raw.to_str().unwrap(),
        ]));
        let want: Vec<u8> = src.packets.iter().flat_map(|p| p.data.clone()).collect();
        assert!(
            std::fs::read(raw).unwrap() == want,
            "{out:?} {kind}: payloads"
        );
    }
}

// ---------------------------------------------------------------------------
// Box and element helpers

fn find<'a>(b: &'a [u8], path: &[&[u8; 4]]) -> Option<&'a [u8]> {
    let (first, rest) = path.split_first()?;
    let inner = mp4_children(b).into_iter().find(|(k, _)| k == *first)?.1;
    if rest.is_empty() {
        Some(inner)
    } else {
        find(inner, rest)
    }
}

fn top_level(file: &[u8]) -> Vec<String> {
    mp4_children(file)
        .into_iter()
        .map(|(k, _)| String::from_utf8_lossy(&k).into_owned())
        .collect()
}

fn traks(file: &[u8]) -> Vec<&[u8]> {
    let moov = find(file, &[b"moov"]).expect("a moov");
    mp4_children(moov)
        .into_iter()
        .filter(|(k, _)| k == b"trak")
        .map(|(_, v)| v)
        .collect()
}

fn stbl(trak: &[u8]) -> &[u8] {
    find(trak, &[b"mdia", b"minf", b"stbl"]).expect("an stbl")
}

fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(b[at..at + 4].try_into().unwrap())
}

/// The block in a SimpleBlock or a BlockGroup (the frame a video track
/// shows last, with its length, R80), when `id` is either.
fn block_of(id: u32, body: &[u8]) -> Option<&[u8]> {
    match id {
        0xA3 => Some(body),
        0xA0 => mkv_children(body)
            .into_iter()
            .find(|k| k.0 == 0xA1)
            .map(|k| k.1),
        _ => None,
    }
}

fn be64(b: &[u8], at: usize) -> u64 {
    u64::from_be_bytes(b[at..at + 8].try_into().unwrap())
}

// ---------------------------------------------------------------------------
// Made-up streams

fn avcc() -> Vec<u8> {
    // High profile, level 4.0, one SPS and one PPS of a few made-up bytes.
    vec![
        1, 0x64, 0, 0x28, 0xFF, 0xE1, 0, 4, 0x67, 0x64, 0, 0x28, 1, 0, 2, 0x68, 0xEE,
    ]
}

fn spec_h264(timescale: u32) -> StreamSpec {
    StreamSpec {
        codec: StreamCodec::H264 { avcc: avcc() },
        timescale,
        video: Some(VideoParams {
            width: 64,
            height: 48,
            rotation: 0,
            hdr: Hdr::None,
            bit_depth: 8,
            mastering: None,
            content_light: None,
            sar: (1, 1),
            tag_colour: false,
            bt601: false,
            frame_ns: None,
            stated_colour: None,
        }),
        audio: None,
    }
}

fn spec_aac(timescale: u32) -> StreamSpec {
    StreamSpec {
        codec: StreamCodec::Aac {
            asc: vec![0x11, 0x90],
        },
        timescale,
        video: None,
        audio: Some(AudioParams {
            rate: 48_000,
            channels: 2,
        }),
    }
}

fn spec_opus() -> StreamSpec {
    let mut head = b"OpusHead".to_vec();
    head.extend_from_slice(&[1, 2, 0x38, 0x01]); // version, channels, pre-skip 312
    head.extend_from_slice(&48_000u32.to_le_bytes());
    head.extend_from_slice(&[0, 0, 0]);
    StreamSpec {
        codec: StreamCodec::Opus { head },
        timescale: 48_000,
        video: None,
        audio: Some(AudioParams {
            rate: 48_000,
            channels: 2,
        }),
    }
}

/// `n` length-prefixed NAL units: a slice of `len` bytes marked by `tag`.
fn nal(tag: u8, len: usize) -> Vec<u8> {
    let mut v = (len as u32).to_be_bytes().to_vec();
    v.push(0x65);
    v.extend(std::iter::repeat_n(tag, len - 1));
    v
}

/// `frames` frames at `scale / step` per second, a key frame every `gop`.
fn frames(frames: i64, step: i64, gop: i64) -> Vec<Packet> {
    (0..frames)
        .map(|i| Packet {
            stream: 0,
            pts: i * step,
            dts: i * step,
            duration: step,
            key: i % gop == 0,
            data: nal(i as u8, 20 + i as usize),
        })
        .collect()
}

/// `frames` blocks of sound of `step` ticks each, starting at `start`.
fn blocks(frames: i64, step: i64, start: i64) -> Vec<Packet> {
    (0..frames)
        .map(|i| Packet {
            stream: 0,
            pts: start + i * step,
            dts: start + i * step,
            duration: step,
            key: true,
            data: vec![0xA0 + (i % 16) as u8; 30],
        })
        .collect()
}

fn synthetic(spec: StreamSpec, packets: Vec<Packet>) -> Src {
    Src { spec, packets }
}

fn write_to(c: Container, streams: &[&Src], tags: &Tags, dir: &TempDir, name: &str) -> Vec<u8> {
    let path = write(c, streams, tags, dir.path(), name);
    std::fs::read(path).unwrap()
}

/// The `.kiln-mdat` files in `dir`.
fn samples_files(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".kiln-mdat"))
        .collect()
}

#[test]
fn an_mp4_s_samples_wait_beside_the_path_it_is_given() {
    // R92: the samples file goes in the folder of the path the job names
    // (the .part's), whatever the open file's own path turns out to be.
    let here = tempfile::tempdir().unwrap();
    let there = tempfile::tempdir().unwrap();
    let out = File::create(here.path().join("x.mp4")).unwrap();
    let part = there.path().join("clip.mp4.part");
    let mut mux = kiln_media::video::packet::muxer_at(Container::Mp4, out, &part).unwrap();
    mux.add_stream(spec_aac(48_000)).unwrap();
    for p in blocks(4, 1024, 0) {
        mux.write(p).unwrap();
    }
    let beside = samples_files(there.path());
    assert_eq!(beside.len(), 1, "{beside:?}");
    assert!(beside[0].starts_with(".clip.mp4.part."), "{beside:?}");
    assert!(samples_files(here.path()).is_empty());
    mux.finish(&Tags::default()).unwrap();
    assert!(samples_files(there.path()).is_empty());
}

#[test]
fn cancel_during_an_mp4_s_finish_stops_within_a_piece() {
    // R92: finish copies every sample into place; it hears cancel as it
    // goes, says how far it has got, and leaves no samples file.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.mp4.part");
    let out = File::create(&path).unwrap();
    let mut mux = kiln_media::video::packet::muxer_at(Container::Mp4, out, &path).unwrap();
    mux.add_stream(spec_aac(48_000)).unwrap();
    // 48 blocks of 1 MiB: 48 MiB to copy.
    for (i, mut p) in blocks(48, 1024, 0).into_iter().enumerate() {
        p.data = vec![i as u8; 1 << 20];
        mux.write(p).unwrap();
    }
    let mut seen = Vec::new();
    let start = std::time::Instant::now();
    let r = mux.finish_watched(&Tags::default(), &mut |f| {
        seen.push(f);
        seen.len() == 3
    });
    assert!(matches!(r, Err(kiln_media::MediaError::Cancelled)), "{r:?}");
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
    // Two pieces copied before the third look stopped it.
    assert_eq!(seen.len(), 3, "{seen:?}");
    assert_eq!(seen[0], 0.0);
    assert!(seen.windows(2).all(|w| w[0] < w[1]), "{seen:?}");
    assert!(seen[2] < 0.1, "{seen:?}");
    assert!(samples_files(dir.path()).is_empty());
    // Not cancelled, the same writer reports up to the whole copy.
    let path = dir.path().join("small.mp4");
    let mut mux =
        kiln_media::video::packet::muxer_at(Container::Mp4, File::create(&path).unwrap(), &path)
            .unwrap();
    mux.add_stream(spec_aac(48_000)).unwrap();
    for (i, mut p) in blocks(3, 1024, 0).into_iter().enumerate() {
        p.data = vec![i as u8; 1 << 20];
        mux.write(p).unwrap();
    }
    let mut seen = Vec::new();
    mux.finish_watched(&Tags::default(), &mut |f| {
        seen.push(f);
        false
    })
    .unwrap();
    assert_eq!(seen.last(), Some(&1.0), "{seen:?}");
    assert!(samples_files(dir.path()).is_empty());
}

// ---------------------------------------------------------------------------
// MP4: bytes

#[test]
fn each_mp4_kind_writes_its_brands() {
    let dir = tempfile::tempdir().unwrap();
    let v = synthetic(spec_h264(15_360), frames(3, 512, 3));
    let expect: [(Container, Vec<u8>); 3] = [
        (
            Container::Mp4,
            [
                &[0, 0, 0, 0x20][..],
                b"ftyp",
                b"isom",
                &[0, 0, 0, 0],
                b"isom",
                b"iso2",
                b"avc1",
                b"mp41",
            ]
            .concat(),
        ),
        (
            Container::Mov,
            [
                &[0, 0, 0, 0x14][..],
                b"ftyp",
                b"qt  ",
                &[0, 0, 0, 0],
                b"qt  ",
            ]
            .concat(),
        ),
        (
            Container::M4v,
            [
                &[0, 0, 0, 0x1C][..],
                b"ftyp",
                b"M4V ",
                &[0, 0, 0, 0],
                b"M4V ",
                b"M4A ",
                b"mp42",
            ]
            .concat(),
        ),
    ];
    for (c, want) in expect {
        let file = write_to(c, &[&v], &Tags::default(), &dir, "brands.bin");
        assert_eq!(&file[..want.len()], &want[..], "{c:?}");
    }
}

#[test]
fn the_moov_comes_before_the_mdat_and_the_samples_follow_it() {
    let dir = tempfile::tempdir().unwrap();
    let v = synthetic(spec_h264(15_360), frames(40, 512, 30));
    let a = synthetic(spec_aac(48_000), blocks(60, 1024, 0));
    let file = write_to(
        Container::Mp4,
        &[&v, &a],
        &Tags::default(),
        &dir,
        "order.mp4",
    );
    assert_eq!(top_level(&file), ["ftyp", "moov", "mdat"]);
    assert_eq!(traks(&file).len(), 2);
    // The first video chunk is the first thing in the mdat.
    let mdat_at = file.len() - find(&file, &[b"mdat"]).unwrap().len();
    let first = be32(find(stbl(traks(&file)[0]), &[b"stco"]).unwrap(), 8) as usize;
    assert_eq!(first, mdat_at);
    assert_eq!(&file[first + 4..first + 5], &[0x65]);
    // Nothing is left beside the output.
    let left: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, ["order.mp4"]);
}

#[test]
fn the_avcc_is_carried_whole_in_an_avc1_entry() {
    let dir = tempfile::tempdir().unwrap();
    let v = synthetic(spec_h264(15_360), frames(3, 512, 3));
    let file = write_to(Container::Mp4, &[&v], &Tags::default(), &dir, "avcc.mp4");
    let stsd = find(stbl(traks(&file)[0]), &[b"stsd"]).unwrap();
    assert_eq!(be32(stsd, 4), 1);
    let entry = &stsd[8..];
    assert_eq!(&entry[4..8], b"avc1");
    // 6 reserved bytes, reference 1, 16 zero bytes, 64 x 48, 72 dpi twice.
    assert_eq!(&entry[8 + 6..8 + 8], &[0, 1]);
    assert_eq!(&entry[8 + 24..8 + 28], &[0, 64, 0, 48]);
    assert_eq!(&entry[8 + 28..8 + 36], &[0, 0x48, 0, 0, 0, 0x48, 0, 0]);
    assert_eq!(&entry[8 + 74..8 + 78], &[0, 0x18, 0xFF, 0xFF]);
    let want = [&[0, 0, 0, 25][..], b"avcC", &avcc()].concat();
    assert_eq!(&entry[8 + 78..], &want[..]);
}

#[test]
fn a_key_frame_every_frame_has_no_stss_and_a_mixed_stream_has_one() {
    let dir = tempfile::tempdir().unwrap();
    let all = synthetic(spec_h264(15_360), frames(4, 512, 1));
    let file = write_to(Container::Mp4, &[&all], &Tags::default(), &dir, "keys.mp4");
    assert!(find(stbl(traks(&file)[0]), &[b"stss"]).is_none());
    let some = synthetic(spec_h264(15_360), frames(70, 512, 30));
    let file = write_to(
        Container::Mp4,
        &[&some],
        &Tags::default(),
        &dir,
        "keys2.mp4",
    );
    let stss = find(stbl(traks(&file)[0]), &[b"stss"]).unwrap();
    // Samples 1, 31 and 61.
    assert_eq!(
        (be32(stss, 4), be32(stss, 8), be32(stss, 12), be32(stss, 16)),
        (3, 1, 31, 61)
    );
}

/// Every sample of a track, found the way a player finds them: `stsc` says
/// how many samples are in each chunk, `stco` or `co64` where it starts, and
/// `stsz` how big each sample is.
fn samples_of(file: &[u8], trak: &[u8]) -> Vec<Vec<u8>> {
    let sbl = stbl(trak);
    let stsz = find(sbl, &[b"stsz"]).unwrap();
    let sizes: Vec<usize> = (0..be32(stsz, 8) as usize)
        .map(|i| be32(stsz, 12 + i * 4) as usize)
        .collect();
    let stsc = find(sbl, &[b"stsc"]).unwrap();
    let runs: Vec<(u32, u32)> = (0..be32(stsc, 4) as usize)
        .map(|i| (be32(stsc, 8 + i * 12), be32(stsc, 12 + i * 12)))
        .collect();
    let offsets: Vec<u64> = match (find(sbl, &[b"stco"]), find(sbl, &[b"co64"])) {
        (Some(c), _) => (0..be32(c, 4) as usize)
            .map(|i| be32(c, 8 + i * 4) as u64)
            .collect(),
        (_, Some(c)) => (0..be32(c, 4) as usize)
            .map(|i| be64(c, 8 + i * 8))
            .collect(),
        _ => panic!("no chunk offsets"),
    };
    let mut out = Vec::new();
    let mut next = 0;
    for (i, &offset) in offsets.iter().enumerate() {
        let per = runs.iter().rev().find(|r| r.0 as usize <= i + 1).unwrap().1;
        let mut at = offset as usize;
        for _ in 0..per {
            out.push(file[at..at + sizes[next]].to_vec());
            at += sizes[next];
            next += 1;
        }
    }
    assert_eq!(next, sizes.len(), "the chunks hold every sample");
    out
}

#[test]
fn streams_may_arrive_in_any_order_and_every_sample_is_found_where_the_tables_say() {
    let dir = tempfile::tempdir().unwrap();
    let v = synthetic(spec_h264(30_000), frames(90, 1000, 30));
    let a = synthetic(spec_aac(48_000), blocks(140, 1024, 0));
    // The sound 0.4 s behind the picture; the sound ahead of it; and all the
    // sound before any picture, far outside any window.
    for (name, sound_delay) in [
        ("behind.mp4", 0.4),
        ("ahead.mp4", -0.4),
        ("first.mp4", -100.0),
    ] {
        let path = dir.path().join(name);
        let mut m = muxer(Container::Mp4, File::create(&path).unwrap()).unwrap();
        m.add_stream(v.spec.clone()).unwrap();
        m.add_stream(a.spec.clone()).unwrap();
        let mut all = Vec::new();
        for p in &v.packets {
            all.push((
                p.dts as f64 / 30_000.0,
                Packet {
                    stream: 0,
                    ..p.clone()
                },
            ));
        }
        for p in &a.packets {
            all.push((
                p.dts as f64 / 48_000.0 + sound_delay,
                Packet {
                    stream: 1,
                    ..p.clone()
                },
            ));
        }
        all.sort_by(|x, y| x.0.total_cmp(&y.0));
        for (_, p) in all {
            m.write(p).unwrap();
        }
        m.finish(&Tags::default()).unwrap();
        let file = std::fs::read(&path).unwrap();
        let t = traks(&file);
        let want_v: Vec<_> = v.packets.iter().map(|p| p.data.clone()).collect();
        let want_a: Vec<_> = a.packets.iter().map(|p| p.data.clone()).collect();
        assert_eq!(samples_of(&file, t[0]), want_v, "{name}");
        assert_eq!(samples_of(&file, t[1]), want_a, "{name}");
    }

    // The same for Matroska: every block of each track, in order.
    for (name, sound_delay) in [("behind.mkv", 0.4), ("ahead.mkv", -0.4)] {
        let path = dir.path().join(name);
        let mut m = muxer(Container::Mkv, File::create(&path).unwrap()).unwrap();
        m.add_stream(v.spec.clone()).unwrap();
        m.add_stream(a.spec.clone()).unwrap();
        let mut all = Vec::new();
        for p in &v.packets {
            all.push((
                p.dts as f64 / 30_000.0,
                Packet {
                    stream: 0,
                    ..p.clone()
                },
            ));
        }
        for p in &a.packets {
            all.push((
                p.dts as f64 / 48_000.0 + sound_delay,
                Packet {
                    stream: 1,
                    ..p.clone()
                },
            ));
        }
        all.sort_by(|x, y| x.0.total_cmp(&y.0));
        for (_, p) in all {
            m.write(p).unwrap();
        }
        m.finish(&Tags::default()).unwrap();
        let file = std::fs::read(&path).unwrap();
        let mut got: [Vec<Vec<u8>>; 2] = [Vec::new(), Vec::new()];
        for (id, cluster) in mkv_children(mkv_children(&file)[1].1) {
            if id != 0x1F43_B675 {
                continue;
            }
            for (id, body) in mkv_children(cluster) {
                if let Some(block) = block_of(id, body) {
                    got[(block[0] - 0x81) as usize].push(block[4..].to_vec());
                }
            }
        }
        let want_v: Vec<_> = v.packets.iter().map(|p| p.data.clone()).collect();
        let want_a: Vec<_> = a.packets.iter().map(|p| p.data.clone()).collect();
        assert_eq!(got[0], want_v, "{name}");
        assert_eq!(got[1], want_a, "{name}");
    }
}

#[test]
fn chunks_hold_half_a_second_of_one_stream_and_alternate_between_streams() {
    let dir = tempfile::tempdir().unwrap();
    // Two seconds of 30 fps video and of 48 kHz sound in 1024-sample blocks.
    let v = synthetic(spec_h264(30_000), frames(60, 1000, 30));
    let a = synthetic(spec_aac(48_000), blocks(94, 1024, 0));
    let file = write_to(
        Container::Mp4,
        &[&v, &a],
        &Tags::default(),
        &dir,
        "chunks.mp4",
    );
    let t = traks(&file);
    let chunk_table = |trak: &[u8]| {
        let stco = find(stbl(trak), &[b"stco"]).unwrap();
        (0..be32(stco, 4) as usize)
            .map(|i| be32(stco, 8 + i * 4))
            .collect::<Vec<_>>()
    };
    let (vc, ac) = (chunk_table(t[0]), chunk_table(t[1]));
    assert_eq!(vc.len(), 4, "four half seconds of video");
    // 94 blocks of 1024 samples; a chunk runs until a block starts 24,000
    // samples (half a second) after its first: blocks 0-23, 24-47, 48-71, 72-93.
    assert_eq!(ac.len(), 4, "audio chunks");
    // Video chunk of 15 frames, then 15, 15, 15.
    let stsc = find(stbl(t[0]), &[b"stsc"]).unwrap();
    assert_eq!((be32(stsc, 4), be32(stsc, 8), be32(stsc, 12)), (1, 1, 15));
    // In the file they take turns.
    let mut order: Vec<(u32, char)> = vc
        .iter()
        .map(|&o| (o, 'v'))
        .chain(ac.iter().map(|&o| (o, 'a')))
        .collect();
    order.sort();
    let kinds: String = order.iter().map(|x| x.1).collect();
    assert!(
        kinds.starts_with("vava") || kinds.starts_with("vaav") || kinds.starts_with("avva"),
        "{kinds}"
    );
    assert!(!kinds.contains("vvv") && !kinds.contains("aaa"), "{kinds}");
}

#[test]
fn offsets_past_4_gib_use_co64() {
    let dir = tempfile::tempdir().unwrap();
    let v = synthetic(spec_h264(15_360), frames(70, 512, 30));
    let bias = (1u64 << 32) + 8;
    for (bias, wide) in [(bias, true), (0, false)] {
        let path = dir.path().join(format!("wide{wide}.mp4"));
        let mut m =
            mp4_muxer_with_bias(Container::Mp4, File::create(&path).unwrap(), bias).unwrap();
        feed(&mut *m, &[&v]);
        m.finish(&Tags::default()).unwrap();
        let file = std::fs::read(&path).unwrap();
        let sbl = stbl(traks(&file)[0]);
        let mdat_at = (file.len() - find(&file, &[b"mdat"]).unwrap().len()) as u64;
        if wide {
            assert!(find(sbl, &[b"stco"]).is_none());
            let co64 = find(sbl, &[b"co64"]).expect("a co64");
            assert_eq!(be32(co64, 4), 5, "70 frames in 2.3 s: five half seconds");
            assert_eq!(be64(co64, 8), bias + mdat_at);
            assert!(be64(co64, 16) > be64(co64, 8));
        } else {
            assert!(find(sbl, &[b"co64"]).is_none());
            assert_eq!(be32(find(sbl, &[b"stco"]).unwrap(), 8) as u64, mdat_at);
        }
    }
}

#[test]
fn a_late_starting_track_gets_an_empty_edit_and_an_early_one_skips_its_priming() {
    let dir = tempfile::tempdir().unwrap();
    let v = synthetic(spec_h264(15_360), frames(30, 512, 30));
    // Sound that starts a quarter second in; and sound with one block of
    // priming before time 0.
    let late = synthetic(spec_aac(48_000), blocks(40, 1024, 12_000));
    let primed = synthetic(spec_aac(48_000), blocks(40, 1024, -1024));
    let file = write_to(
        Container::Mp4,
        &[&v, &late, &primed],
        &Tags::default(),
        &dir,
        "edits.mp4",
    );
    let t = traks(&file);
    let elst = |trak: &[u8]| {
        let e = find(trak, &[b"edts", b"elst"]).unwrap();
        let n = be32(e, 4) as usize;
        (0..n)
            .map(|i| (be32(e, 8 + i * 12), be32(e, 12 + i * 12) as i32))
            .collect::<Vec<_>>()
    };
    // Video starts at 0: one edit, from media time 0.
    assert_eq!(elst(t[0]), [(1000, 0)]);
    // 12,000 / 48,000 s = 250 ms of nothing, then 40 blocks (853 ms).
    assert_eq!(elst(t[1]), [(250, -1), (853, 0)]);
    // The priming block is skipped: the media starts 1024 ticks in, and 39
    // blocks are shown (832 ms).
    assert_eq!(elst(t[2]), [(832, 1024)]);
}

#[test]
fn b_frames_write_a_signed_ctts_and_ordinary_streams_none() {
    let dir = tempfile::tempdir().unwrap();
    // Decode order I P B B: shown 0, 3, 1, 2 (in steps of 512), decoded two
    // steps early, as an encoder with two frames of delay writes them.
    let mut p = Vec::new();
    for (i, (pts, dts)) in [(0, -2), (3, -1), (1, 0), (2, 1)].into_iter().enumerate() {
        p.push(Packet {
            stream: 0,
            pts: pts * 512,
            dts: dts * 512,
            duration: 512,
            key: i == 0,
            data: nal(i as u8, 30),
        });
    }
    let v = synthetic(spec_h264(15_360), p);
    let file = write_to(Container::Mp4, &[&v], &Tags::default(), &dir, "ctts.mp4");
    let sbl = stbl(traks(&file)[0]);
    let ctts = find(sbl, &[b"ctts"]).expect("a ctts");
    assert_eq!(ctts[0], 1, "version 1: signed");
    // pts - dts: 2, 4, 1, 1 steps of 512 -> runs (1, 1024), (1, 2048), (2, 512).
    assert_eq!(be32(ctts, 4), 3);
    assert_eq!((be32(ctts, 8), be32(ctts, 12)), (1, 1024));
    assert_eq!((be32(ctts, 16), be32(ctts, 20)), (1, 2048));
    assert_eq!((be32(ctts, 24), be32(ctts, 28)), (2, 512));
    // stts starts at 0.
    let stts = find(sbl, &[b"stts"]).unwrap();
    assert_eq!((be32(stts, 8), be32(stts, 12)), (4, 512));
    // The first shown frame is two steps into the media (the first decode
    // time is 0 there), and it shows at 0: one edit.
    let e = find(traks(&file)[0], &[b"edts", b"elst"]).unwrap();
    assert_eq!(be32(e, 4), 1);
    assert_eq!(be32(e, 12) as i32, 1024);

    let plain = synthetic(spec_h264(15_360), frames(5, 512, 5));
    let file = write_to(
        Container::Mp4,
        &[&plain],
        &Tags::default(),
        &dir,
        "noctts.mp4",
    );
    assert!(find(stbl(traks(&file)[0]), &[b"ctts"]).is_none());
}

#[test]
fn packets_out_of_decode_order_or_for_nothing_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let v = synthetic(spec_h264(15_360), frames(3, 512, 3));
    let mut m = muxer(
        Container::Mp4,
        File::create(dir.path().join("a.mp4")).unwrap(),
    )
    .unwrap();
    m.add_stream(v.spec.clone()).unwrap();
    let mut p = v.packets[1].clone();
    m.write(p.clone()).unwrap();
    p.dts -= 1;
    assert!(m.write(p).is_err());
    let mut stray = v.packets[0].clone();
    stray.stream = 5;
    assert!(m.write(stray).is_err());
    // A stream the container can't hold; one with nothing to describe it.
    assert!(m.add_stream(spec_opus()).is_err());
    let mut bare = spec_h264(15_360);
    bare.video = None;
    assert!(m.add_stream(bare).is_err());
    // A picture's decode times never repeat, in either writer; a sound's may.
    for c in [Container::Mp4, Container::Mkv] {
        let mut m = muxer(c, File::create(dir.path().join("dup.bin")).unwrap()).unwrap();
        m.add_stream(v.spec.clone()).unwrap();
        m.add_stream(spec_aac(48_000)).unwrap();
        let p = v.packets[1].clone();
        m.write(p.clone()).unwrap();
        assert!(m.write(p.clone()).is_err(), "{c:?}: equal picture times");
        // After a refusal the muxer is done.
        assert!(
            m.write(v.packets[2].clone()).is_err(),
            "{c:?}: failed state"
        );
        assert!(m.finish(&Tags::default()).is_err(), "{c:?}: failed state");
        // Sound with equal times is let through; going back is not.
        let mut m = muxer(c, File::create(dir.path().join("dup.bin")).unwrap()).unwrap();
        m.add_stream(spec_aac(48_000)).unwrap();
        let mut a = Packet {
            stream: 0,
            ..blocks(2, 1024, 1024)[1].clone()
        };
        m.write(a.clone()).unwrap();
        m.write(a.clone()).unwrap();
        a.dts -= 1;
        assert!(m.write(a).is_err(), "{c:?}: sound going back");
    }
    // Nothing written at all.
    let empty = muxer(
        Container::Mp4,
        File::create(dir.path().join("b.mp4")).unwrap(),
    )
    .unwrap();
    assert!(empty.finish(&Tags::default()).is_err());
}

#[test]
fn an_abandoned_or_failed_mp4_leaves_no_temporary_file() {
    let dir = tempfile::tempdir().unwrap();
    let v = synthetic(spec_h264(15_360), frames(40, 512, 30));
    {
        let mut m = muxer(
            Container::Mp4,
            File::create(dir.path().join("x.mp4")).unwrap(),
        )
        .unwrap();
        m.add_stream(v.spec.clone()).unwrap();
        for p in &v.packets {
            m.write(p.clone()).unwrap();
        }
        let temps = std::fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(temps, 2, "the output and its temporary file");
    }
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    // A finish that fails.
    let mut m = muxer(
        Container::Mp4,
        File::create(dir.path().join("y.mp4")).unwrap(),
    )
    .unwrap();
    m.add_stream(v.spec.clone()).unwrap();
    m.add_stream(spec_aac(48_000)).unwrap(); // gets no packets
    for p in &v.packets {
        m.write(p.clone()).unwrap();
    }
    assert!(m.finish(&Tags::default()).is_err());
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        2,
        "x.mp4 and y.mp4"
    );
}

#[test]
fn hdr_and_rotation_boxes_are_written_from_the_params() {
    let dir = tempfile::tempdir().unwrap();
    let mut spec = spec_h264(15_360);
    let v = spec.video.as_mut().unwrap();
    v.hdr = Hdr::Hlg;
    v.rotation = 270;
    v.mastering = Some(*b"0123456789abcdefghijklmn");
    v.content_light = Some([3, 232, 1, 144]);
    let s = synthetic(spec, frames(3, 512, 3));
    let file = write_to(Container::Mov, &[&s], &Tags::default(), &dir, "hdr.mov");
    let trak = traks(&file)[0];
    let stsd = find(stbl(trak), &[b"stsd"]).unwrap();
    let entry = &stsd[8 + 8 + 78..];
    let kids: Vec<_> = mp4_children(entry);
    let names: Vec<_> = kids
        .iter()
        .map(|(k, _)| String::from_utf8_lossy(k).into_owned())
        .collect();
    assert_eq!(names, ["avcC", "colr", "mdcv", "clli"]);
    // nclx, primaries 9, transfer 18 (HLG), matrix 9, limited range.
    assert_eq!(kids[1].1, [b'n', b'c', b'l', b'x', 0, 9, 0, 18, 0, 9, 0]);
    assert_eq!(kids[2].1, b"0123456789abcdefghijklmn");
    assert_eq!(kids[3].1, [3, 232, 1, 144]);
    // 270 degrees clockwise: a=0, b=-1, c=1, d=0.
    let tkhd = find(trak, &[b"tkhd"]).unwrap();
    let m: Vec<u32> = (0..9).map(|i| be32(tkhd, 40 + i * 4)).collect();
    assert_eq!(m, [0, 0xFFFF_0000, 0, 0x10000, 0, 0, 0, 0, 0x4000_0000]);
}

#[test]
fn hevc_is_always_hvc1_with_its_parameter_sets_only_in_the_hvcc() {
    let dir = tempfile::tempdir().unwrap();
    let (vps, sps, pps) = ([0x40u8, 1, 9], [0x42u8, 1, 8, 8], [0x44u8, 1, 7]);
    let mut hvcc = vec![0u8; 23];
    hvcc[0] = 1;
    hvcc[21] = 3;
    hvcc[22] = 3;
    for (kind, nal) in [(32u8, &vps[..]), (33, &sps[..]), (34, &pps[..])] {
        hvcc.push(0x80 | kind);
        hvcc.extend_from_slice(&1u16.to_be_bytes());
        hvcc.extend_from_slice(&(nal.len() as u16).to_be_bytes());
        hvcc.extend_from_slice(nal);
    }
    let prefixed = |nals: &[&[u8]]| -> Vec<u8> {
        nals.iter()
            .flat_map(|n| {
                (n.len() as u32)
                    .to_be_bytes()
                    .into_iter()
                    .chain(n.iter().copied())
            })
            .collect()
    };
    let idr = [0x26u8, 1, 0xAA, 0xBB, 0xCC];
    let trail = [0x02u8, 1, 0x11];
    let mut spec = spec_h264(15_360);
    spec.codec = StreamCodec::Hevc { hvcc: hvcc.clone() };
    let packets = vec![
        Packet {
            stream: 0,
            pts: 0,
            dts: 0,
            duration: 512,
            key: true,
            data: prefixed(&[&vps, &sps, &pps, &idr]),
        },
        Packet {
            stream: 0,
            pts: 512,
            dts: 512,
            duration: 512,
            key: false,
            data: prefixed(&[&trail]),
        },
    ];
    let file = write_to(
        Container::Mp4,
        &[&synthetic(spec, packets)],
        &Tags::default(),
        &dir,
        "hvc1.mp4",
    );
    let stsd = find(stbl(traks(&file)[0]), &[b"stsd"]).unwrap();
    assert_eq!(&stsd[12..16], b"hvc1");
    assert_eq!(find(&stsd[8 + 8 + 78..], &[b"hvcC"]).unwrap(), &hvcc[..]);
    // The first sample is the IDR alone; the second is as it was.
    let mdat = find(&file, &[b"mdat"]).unwrap();
    assert_eq!(mdat, [prefixed(&[&idr]), prefixed(&[&trail])].concat());
    let sizes = find(stbl(traks(&file)[0]), &[b"stsz"]).unwrap();
    assert_eq!((be32(sizes, 12), be32(sizes, 16)), (9, 7));
}

#[test]
fn a_parameter_set_the_hvcc_lacks_stops_the_stripping_and_makes_the_entry_hev1() {
    let dir = tempfile::tempdir().unwrap();
    let (vps, sps_a, sps_b, pps) = (
        [0x40u8, 1, 9],
        [0x42u8, 1, 8, 8],
        [0x42u8, 1, 5, 5],
        [0x44u8, 1, 7],
    );
    let mut hvcc = vec![0u8; 23];
    hvcc[0] = 1;
    hvcc[21] = 3;
    hvcc[22] = 3;
    for (kind, nal) in [(32u8, &vps[..]), (33, &sps_a[..]), (34, &pps[..])] {
        hvcc.push(0x80 | kind);
        hvcc.extend_from_slice(&1u16.to_be_bytes());
        hvcc.extend_from_slice(&(nal.len() as u16).to_be_bytes());
        hvcc.extend_from_slice(nal);
    }
    let prefixed = |nals: &[&[u8]]| -> Vec<u8> {
        nals.iter()
            .flat_map(|n| {
                (n.len() as u32)
                    .to_be_bytes()
                    .into_iter()
                    .chain(n.iter().copied())
            })
            .collect()
    };
    let (idr, trail) = ([0x26u8, 1, 0xAA, 0xBB], [0x02u8, 1, 0x11]);
    let mut spec = spec_h264(15_360);
    spec.codec = StreamCodec::Hevc { hvcc };
    // A (the hvcC's), then B with the same id, then A again.
    let samples = [
        prefixed(&[&vps, &sps_a, &pps, &idr]),
        prefixed(&[&sps_b, &trail]),
        prefixed(&[&sps_a, &trail]),
        prefixed(&[&trail]),
    ];
    let packets: Vec<Packet> = samples
        .iter()
        .enumerate()
        .map(|(i, data)| Packet {
            stream: 0,
            pts: i as i64 * 512,
            dts: i as i64 * 512,
            duration: 512,
            key: i == 0,
            data: data.clone(),
        })
        .collect();
    let file = write_to(
        Container::Mp4,
        &[&synthetic(spec, packets)],
        &Tags::default(),
        &dir,
        "hev1.mp4",
    );
    let stsd = find(stbl(traks(&file)[0]), &[b"stsd"]).unwrap();
    assert_eq!(&stsd[12..16], b"hev1");
    // The first sample was stripped, before B came; B's and the second A are
    // both kept, so the frames after B decode with B and then with A.
    let want = [
        prefixed(&[&idr]),
        samples[1].clone(),
        samples[2].clone(),
        samples[3].clone(),
    ];
    assert_eq!(samples_of(&file, traks(&file)[0]), want);
}

#[test]
fn tags_go_in_a_udta_only_when_there_are_some() {
    let dir = tempfile::tempdir().unwrap();
    let v = synthetic(spec_h264(15_360), frames(3, 512, 3));
    let tags = Tags {
        title: Some("A title".into()),
        ..Tags::default()
    };
    let file = write_to(Container::Mp4, &[&v], &tags, &dir, "tags.mp4");
    let udta = find(&file, &[b"moov", b"udta"]).expect("a udta");
    assert!(udta.windows(7).any(|w| w == b"A title"));
    let file = write_to(Container::Mp4, &[&v], &Tags::default(), &dir, "notags.mp4");
    assert!(find(&file, &[b"moov", b"udta"]).is_none());
}

#[test]
fn a_10_mhz_clock_goes_to_the_sample_rate_or_90_khz_in_an_mp4_and_to_milliseconds_in_a_matroska() {
    let dir = tempfile::tempdir().unwrap();
    // 30 fps in 100 ns ticks: 333,333 and 333,334 alternating, the way a
    // Media Foundation source steps; sound at 1024 / 48000 s.
    let video: Vec<Packet> = (0..60i64)
        .map(|i| {
            let t = (i * 10_000_000 + 15) / 30;
            let next = ((i + 1) * 10_000_000 + 15) / 30;
            Packet {
                stream: 0,
                pts: t,
                dts: t,
                duration: next - t,
                key: i % 30 == 0,
                data: nal(i as u8, 25),
            }
        })
        .collect();
    let sound: Vec<Packet> = (0..94i64)
        .map(|i| {
            let t = (i * 1024 * 10_000_000 + 24_000) / 48_000;
            let next = ((i + 1) * 1024 * 10_000_000 + 24_000) / 48_000;
            Packet {
                stream: 0,
                pts: t,
                dts: t,
                duration: next - t,
                key: true,
                data: vec![7; 20],
            }
        })
        .collect();
    let v = synthetic(spec_h264(10_000_000), video.clone());
    let a = synthetic(spec_aac(10_000_000), sound.clone());

    let file = write_to(
        Container::Mp4,
        &[&v, &a],
        &Tags::default(),
        &dir,
        "tenmhz.mp4",
    );
    // Picture goes to 90,000 and sound to its sample rate; every frame is the
    // same length there, so each stts is one run.
    let t = traks(&file);
    for (trak, (scale, step, count)) in t.iter().zip([(90_000, 3000, 60), (48_000, 1024, 94)]) {
        let mdhd = find(trak, &[b"mdia", b"mdhd"]).unwrap();
        let long = mdhd[0] == 1;
        let (got_scale, duration) = if long {
            (be32(mdhd, 20), be64(mdhd, 24))
        } else {
            (be32(mdhd, 12), be32(mdhd, 16) as u64)
        };
        assert_eq!(got_scale, scale);
        assert_eq!(duration, step as u64 * count as u64);
        let stts = find(stbl(trak), &[b"stts"]).unwrap();
        assert_eq!(be32(stts, 4), 1, "one run");
        assert_eq!((be32(stts, 8), be32(stts, 12)), (count, step));
    }

    let file = write_to(
        Container::Mkv,
        &[&v, &a],
        &Tags::default(),
        &dir,
        "tenmhz.mkv",
    );
    let segment = &mkv_children(&file)[1].1;
    let mut stamps = Vec::new();
    for (id, body) in mkv_children(segment) {
        if id != 0x1F43_B675 {
            continue;
        }
        let kids = mkv_children(body);
        let cluster_ts = kids
            .iter()
            .find(|k| k.0 == 0xE7)
            .map(|k| k.1.iter().fold(0i64, |a, &b| a << 8 | b as i64))
            .unwrap();
        for (id, body) in kids {
            if let Some(block) = block_of(id, body).filter(|b| b[0] == 0x81) {
                let rel = i16::from_be_bytes([block[1], block[2]]) as i64;
                stamps.push(cluster_ts + rel);
            }
        }
    }
    let want: Vec<i64> = video
        .iter()
        .map(|p| rescaled(p.pts, 10_000_000, 1000))
        .collect();
    assert_eq!(stamps, want);
}

/// The `stts` steps of a track, one per sample, and its media time scale.
fn track_steps(trak: &[u8]) -> (u32, Vec<i64>) {
    let mdhd = find(trak, &[b"mdia", b"mdhd"]).unwrap();
    let scale = if mdhd[0] == 1 {
        be32(mdhd, 20)
    } else {
        be32(mdhd, 12)
    };
    let stts = find(stbl(trak), &[b"stts"]).unwrap();
    let mut steps = Vec::new();
    for i in 0..be32(stts, 4) as usize {
        for _ in 0..be32(stts, 8 + i * 8) {
            steps.push(be32(stts, 12 + i * 8) as i64);
        }
    }
    (scale, steps)
}

/// `n` frames of one-byte-ish payloads whose 100 ns times are
/// `round(i * num * 10^7 / den)`.
fn clocked_frames(n: i64, num: i64, den: i64) -> Vec<Packet> {
    let at = |i: i64| (i * num * 10_000_000 * 2 + den) / (2 * den);
    (0..n)
        .map(|i| Packet {
            stream: 0,
            pts: at(i),
            dts: at(i),
            duration: at(i + 1) - at(i),
            key: i % 60 == 0,
            data: vec![0, 0, 0, 1, 0x65],
        })
        .collect()
}

#[test]
fn two_hours_at_ntsc_rates_get_a_frame_rate_clock_and_a_one_run_stts_with_no_drift() {
    let dir = tempfile::tempdir().unwrap();
    // 59.94 fps: 1001 / 60000 s a frame, which 90,000 can't hold evenly
    // (1501.5), so the writer picks 60,000. 23.976 fps goes to 24,000.
    for (num, den, frames, scale, step) in [
        (1001, 60_000, 431_568, 60_000u32, 1001i64),
        (1001, 24_000, 172_627, 24_000, 1001),
        (1, 60, 432_000, 90_000, 1500),
    ] {
        let v = synthetic(spec_h264(10_000_000), clocked_frames(frames, num, den));
        let file = write_to(Container::Mp4, &[&v], &Tags::default(), &dir, "long.mp4");
        let (got_scale, steps) = track_steps(traks(&file)[0]);
        assert_eq!(got_scale, scale);
        assert_eq!(steps.len() as i64, frames);
        assert!(steps.iter().all(|&d| d == step), "{num}/{den}");
        let stts = find(stbl(traks(&file)[0]), &[b"stts"]).unwrap();
        assert_eq!(be32(stts, 4), 1, "one run");
        // The moov stays small: a run, a ctts-free table, and 4-byte sizes.
        let moov = find(&file, &[b"moov"]).unwrap();
        assert!(moov.len() < frames as usize * 5 + 4096, "{}", moov.len());
    }
}

#[test]
fn uneven_times_are_converted_one_by_one_so_nothing_adds_up() {
    let dir = tempfile::tempdir().unwrap();
    // Frames at irregular 100 ns times; no clock holds them evenly, so 90,000.
    let mut t = 0i64;
    let mut state = 12_345u64;
    let packets: Vec<Packet> = (0..5000)
        .map(|i| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let step = 150_000 + (state >> 40) as i64 % 400_000;
            let p = Packet {
                stream: 0,
                pts: t,
                dts: t,
                duration: step,
                key: i % 50 == 0,
                data: vec![0, 0, 0, 1, 0x65],
            };
            t += step;
            p
        })
        .collect();
    let v = synthetic(spec_h264(10_000_000), packets.clone());
    let file = write_to(Container::Mp4, &[&v], &Tags::default(), &dir, "uneven.mp4");
    let (scale, steps) = track_steps(traks(&file)[0]);
    assert_eq!(scale, 90_000);
    // Summing the steps lands on each time's own conversion, to the tick.
    let mut sum = 0;
    for (p, step) in packets.iter().zip(&steps) {
        assert_eq!(sum, rescaled(p.dts - packets[0].dts, 10_000_000, 90_000));
        sum += step;
    }
    let last = packets.last().unwrap();
    assert_eq!(sum, rescaled(last.dts + last.duration, 10_000_000, 90_000));
}

// ---------------------------------------------------------------------------
// Matroska: bytes

#[test]
fn the_ebml_header_is_matroska_version_4_readable_by_2() {
    let dir = tempfile::tempdir().unwrap();
    let v = synthetic(spec_h264(1000), frames(3, 33, 3));
    let file = write_to(Container::Mkv, &[&v], &Tags::default(), &dir, "head.mkv");
    let want = [
        0x1A, 0x45, 0xDF, 0xA3, 0xA3, // EBML, 35 bytes
        0x42, 0x86, 0x81, 0x01, // EBMLVersion 1
        0x42, 0xF7, 0x81, 0x01, // EBMLReadVersion 1
        0x42, 0xF2, 0x81, 0x04, // EBMLMaxIDLength 4
        0x42, 0xF3, 0x81, 0x08, // EBMLMaxSizeLength 8
        0x42, 0x82, 0x88, b'm', b'a', b't', b'r', b'o', b's', b'k', b'a', // DocType
        0x42, 0x87, 0x81, 0x04, // DocTypeVersion 4
        0x42, 0x85, 0x81, 0x02, // DocTypeReadVersion 2
    ];
    assert_eq!(&file[..want.len()], &want[..]);
    // Then the Segment, whose size was patched to the rest of the file.
    assert_eq!(&file[want.len()..want.len() + 4], &[0x18, 0x53, 0x80, 0x67]);
    assert_eq!(file[want.len() + 4], 0x01);
    let size = u64::from_be_bytes([
        0,
        file[want.len() + 5],
        file[want.len() + 6],
        file[want.len() + 7],
        file[want.len() + 8],
        file[want.len() + 9],
        file[want.len() + 10],
        file[want.len() + 11],
    ]);
    assert_eq!(size as usize, file.len() - want.len() - 12);
}

fn uint_of(b: &[u8]) -> u64 {
    b.iter().fold(0, |a, &x| a << 8 | x as u64)
}

#[test]
fn a_matroska_has_its_seek_head_info_tracks_clusters_and_cues_in_order() {
    let dir = tempfile::tempdir().unwrap();
    // Six seconds of 30 fps with a key frame each second, and Opus in step.
    let v = synthetic(
        spec_h264(1000),
        (0..180)
            .map(|i| {
                let t = i * 1000 / 30;
                Packet {
                    stream: 0,
                    pts: t,
                    dts: t,
                    duration: 33,
                    key: i % 30 == 0,
                    data: nal(i as u8, 22),
                }
            })
            .collect(),
    );
    let a = synthetic(spec_opus(), blocks(300, 960, 0));
    let file = write_to(
        Container::Mkv,
        &[&v, &a],
        &Tags {
            title: Some("Hi".into()),
            ..Tags::default()
        },
        &dir,
        "layout.mkv",
    );
    let top = mkv_children(&file);
    assert_eq!(
        top.iter().map(|t| t.0).collect::<Vec<_>>(),
        [0x1A45_DFA3, 0x1853_8067]
    );
    let seg = top[1].1;
    let kids = mkv_children(seg);
    let ids: Vec<u32> = kids.iter().map(|k| k.0).collect();
    // SeekHead, Info, Tracks, three clusters (0, 2 and 4 s), Cues, Tags.
    assert_eq!(
        ids,
        [
            0x114D_9B74,
            0x1549_A966,
            0x1654_AE6B,
            0x1F43_B675,
            0x1F43_B675,
            0x1F43_B675,
            0x1C53_BB6B,
            0x1254_C367
        ]
    );
    // Each cluster starts on a key frame of the video.
    let mut cluster_pos = Vec::new();
    let mut at = 0usize;
    for (id, body) in &kids {
        let head = seg.len() - seg[at..].len();
        let _ = head;
        if *id == 0x1F43_B675 {
            cluster_pos.push(at);
            let c = mkv_children(body);
            let ts = uint_of(c.iter().find(|k| k.0 == 0xE7).unwrap().1);
            let first = c.iter().find(|k| k.0 == 0xA3).unwrap().1;
            assert_eq!(ts % 2000, 0);
            assert_eq!(first[0], 0x81, "the first block is the video's");
            assert_eq!(first[3] & 0x80, 0x80, "and a key frame");
        }
        // Element header: ID bytes and a size of 1, 2, 4 or 8 bytes.
        let id_len = if *id > 0xFF_FFFF {
            4
        } else if *id > 0xFFFF {
            3
        } else if *id > 0xFF {
            2
        } else {
            1
        };
        let size_len = (seg[at + id_len]).leading_zeros() as usize + 1;
        at += id_len + size_len + body.len();
    }
    // The cues point at those clusters.
    let cues = mkv_children(kids[6].1);
    assert_eq!(cues.len(), 3);
    let mut points = Vec::new();
    for (id, point) in &cues {
        assert_eq!(*id, 0xBB);
        let p = mkv_children(point);
        let time = uint_of(p.iter().find(|k| k.0 == 0xB3).unwrap().1);
        let pos = mkv_children(p.iter().find(|k| k.0 == 0xB7).unwrap().1);
        assert_eq!(
            uint_of(pos.iter().find(|k| k.0 == 0xF7).unwrap().1),
            1,
            "video track"
        );
        points.push((
            time,
            uint_of(pos.iter().find(|k| k.0 == 0xF1).unwrap().1) as usize,
        ));
    }
    // Segment positions: the cluster's element, counted from the segment's data.
    let want: Vec<(u64, usize)> = cluster_pos
        .iter()
        .enumerate()
        .map(|(i, &p)| (i as u64 * 2000, p))
        .collect();
    assert_eq!(points, want);
    // The seek head points at Info, Tracks, Cues and Tags.
    let mut seeks = Vec::new();
    for (id, body) in mkv_children(kids[0].1) {
        assert_eq!(id, 0x4DBB);
        let s = mkv_children(body);
        seeks.push((uint_of(s[0].1), uint_of(s[1].1) as usize));
    }
    assert_eq!(seeks.len(), 4);
    for (id, pos) in seeks {
        let start = &seg[pos..];
        assert!(
            start.starts_with(
                &id.to_be_bytes()[id.to_be_bytes().iter().take_while(|&&b| b == 0).count()..]
            ),
            "{id:X} at {pos}"
        );
    }
    // The duration is the end of the latest block shown. Opus' last packet ends
    // at 6000 ms, and its block times carry the 312-sample pre-skip (6.5 ms):
    // 6006.5 rounds up to 6007. (The picture ends at 5999.)
    let info = mkv_children(kids[1].1);
    let d = info.iter().find(|k| k.0 == 0x4489).unwrap().1;
    let ms = f64::from_be_bytes(d.try_into().unwrap());
    assert_eq!(ms, 6007.0);
    // Tracks: numbers 1 and 2, with the codec ids and private data.
    let tracks = mkv_children(kids[2].1);
    assert_eq!(tracks.len(), 2);
    let ids: Vec<String> = tracks
        .iter()
        .map(|(_, t)| {
            let e = mkv_children(t);
            String::from_utf8(e.iter().find(|k| k.0 == 0x86).unwrap().1.to_vec()).unwrap()
        })
        .collect();
    assert_eq!(ids, ["V_MPEG4/ISO/AVC", "A_OPUS"]);
    let private = mkv_children(tracks[0].1)
        .into_iter()
        .find(|k| k.0 == 0x63A2)
        .unwrap()
        .1;
    assert_eq!(private, &avcc()[..]);
    let opus = mkv_children(tracks[1].1);
    // 312 samples of pre-skip at 48 kHz = 6,500,000 ns.
    assert_eq!(
        uint_of(opus.iter().find(|k| k.0 == 0x56AA).unwrap().1),
        6_500_000
    );
    assert_eq!(
        uint_of(opus.iter().find(|k| k.0 == 0x56BB).unwrap().1),
        80_000_000
    );
}

#[test]
fn a_matroska_stream_with_no_video_cues_every_cluster() {
    let dir = tempfile::tempdir().unwrap();
    let a = synthetic(spec_opus(), blocks(300, 960, 0)); // 6 s
    let file = write_to(Container::Mkv, &[&a], &Tags::default(), &dir, "audio.mkv");
    let kids = mkv_children(mkv_children(&file)[1].1);
    let clusters = kids.iter().filter(|k| k.0 == 0x1F43_B675).count();
    assert_eq!(clusters, 3);
    assert_eq!(
        mkv_children(kids.iter().find(|k| k.0 == 0x1C53_BB6B).unwrap().1).len(),
        3
    );
}

#[test]
fn a_packet_before_the_start_once_the_times_are_settled_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let v = synthetic(spec_h264(1000), frames(100, 33, 30));
    let mut m = muxer(
        Container::Mkv,
        File::create(dir.path().join("neg.mkv")).unwrap(),
    )
    .unwrap();
    m.add_stream(v.spec.clone()).unwrap();
    m.add_stream(spec_aac(48_000)).unwrap();
    // The sound starts 21 ms early, so everything moves 21 ms later.
    for p in blocks(3, 1024, -1024) {
        m.write(Packet { stream: 1, ..p }).unwrap();
    }
    // Over two seconds of picture closes the first cluster and settles it.
    for p in &v.packets {
        m.write(p.clone()).unwrap();
    }
    // Sound at -100 ms is earlier than anything the move covers.
    let early = Packet {
        stream: 1,
        pts: -4800,
        dts: 2_000_000,
        duration: 1024,
        key: true,
        data: vec![1; 10],
    };
    assert!(m.write(early).is_err());
    assert!(m.finish(&Tags::default()).is_err());
}

#[test]
fn colour_for_matroska_comes_from_the_params() {
    let dir = tempfile::tempdir().unwrap();
    let mut spec = spec_h264(1000);
    let v = spec.video.as_mut().unwrap();
    v.hdr = Hdr::Pq;
    v.bit_depth = 10;
    // Green (0.265, 0.69), blue (0.15, 0.06), red (0.68, 0.32), white
    // (0.3127, 0.329), 1000 nits down to 0.0001.
    let mut m = Vec::new();
    for x in [13250u16, 34500, 7500, 3000, 34000, 16000, 15635, 16450] {
        m.extend_from_slice(&x.to_be_bytes());
    }
    m.extend_from_slice(&10_000_000u32.to_be_bytes());
    m.extend_from_slice(&1u32.to_be_bytes());
    v.mastering = Some(m.try_into().unwrap());
    v.content_light = Some([0x03, 0xE8, 0x01, 0x90]);
    let s = synthetic(spec, frames(3, 33, 3));
    let file = write_to(Container::Mkv, &[&s], &Tags::default(), &dir, "colour.mkv");
    let kids = mkv_children(mkv_children(&file)[1].1);
    let track = mkv_children(mkv_children(kids[2].1)[0].1);
    let video = mkv_children(track.iter().find(|k| k.0 == 0xE0).unwrap().1);
    let colour = mkv_children(video.iter().find(|k| k.0 == 0x55B0).unwrap().1);
    let get = |id: u32| uint_of(colour.iter().find(|k| k.0 == id).unwrap().1);
    assert_eq!(get(0x55BA), 16, "PQ");
    assert_eq!((get(0x55BB), get(0x55B1), get(0x55B2)), (9, 9, 10));
    assert_eq!((get(0x55BC), get(0x55BD)), (1000, 400));
    let mastering = mkv_children(colour.iter().find(|k| k.0 == 0x55D0).unwrap().1);
    let f = |id: u32| {
        f64::from_be_bytes(
            mastering
                .iter()
                .find(|k| k.0 == id)
                .unwrap()
                .1
                .try_into()
                .unwrap(),
        )
    };
    assert_eq!((f(0x55D1), f(0x55D2)), (0.68, 0.32), "red");
    assert_eq!((f(0x55D3), f(0x55D4)), (0.265, 0.69), "green");
    assert_eq!((f(0x55D5), f(0x55D6)), (0.15, 0.06), "blue");
    assert_eq!((f(0x55D7), f(0x55D8)), (0.3127, 0.329), "white point");
    assert_eq!((f(0x55D9), f(0x55DA)), (1000.0, 0.0001));
}

#[test]
fn pixel_aspect_and_bt709_are_written_when_the_params_say() {
    let dir = tempfile::tempdir().unwrap();
    let entry_boxes = |sar: (u32, u32), tag_colour: bool, name: &str| {
        let mut spec = spec_h264(15_360);
        let v = spec.video.as_mut().unwrap();
        v.sar = sar;
        v.tag_colour = tag_colour;
        let s = synthetic(spec, frames(3, 512, 3));
        let file = write_to(Container::Mp4, &[&s], &Tags::default(), &dir, name);
        let trak = traks(&file)[0];
        let stsd = find(stbl(trak), &[b"stsd"]).unwrap();
        mp4_children(&stsd[8 + 8 + 78..])
            .into_iter()
            .map(|(k, b)| (String::from_utf8_lossy(&k).into_owned(), b.to_vec()))
            .collect::<Vec<_>>()
    };
    // Square pixels and a copy's colour: nothing added.
    let plain = entry_boxes((1, 1), false, "plain.mp4");
    assert_eq!(plain.len(), 1, "{plain:?}");
    // 8:6 is 4:3 in lowest terms; BT.709 (1, 1, 1) in limited range.
    let kids = entry_boxes((8, 6), true, "ana.mp4");
    let names: Vec<_> = kids.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(names, ["avcC", "colr", "pasp"]);
    assert_eq!(kids[1].1, [b'n', b'c', b'l', b'x', 0, 1, 0, 1, 0, 1, 0]);
    assert_eq!(kids[2].1, [0, 0, 0, 4, 0, 0, 0, 3]);

    // Matroska: the display size stretches the width, and the colour is
    // described in a Colour element.
    let mut spec = spec_h264(1000);
    let v = spec.video.as_mut().unwrap();
    v.sar = (4, 3);
    v.tag_colour = true;
    let s = synthetic(spec, frames(3, 33, 3));
    let file = write_to(Container::Mkv, &[&s], &Tags::default(), &dir, "ana.mkv");
    let kids = mkv_children(mkv_children(&file)[1].1);
    let track = mkv_children(mkv_children(kids[2].1)[0].1);
    let video = mkv_children(track.iter().find(|k| k.0 == 0xE0).unwrap().1);
    let get = |list: &[(u32, &[u8])], id: u32| uint_of(list.iter().find(|k| k.0 == id).unwrap().1);
    // 64x48 at 4:3: shown 85 (85.3) by 48.
    assert_eq!((get(&video, 0x54B0), get(&video, 0x54BA)), (85, 48));
    let colour = mkv_children(video.iter().find(|k| k.0 == 0x55B0).unwrap().1);
    assert_eq!(
        (
            get(&colour, 0x55B1),
            get(&colour, 0x55B9),
            get(&colour, 0x55BA),
            get(&colour, 0x55BB)
        ),
        (1, 1, 1, 1),
        "matrix, range, transfer, primaries"
    );
}

#[test]
fn bt601_is_written_when_the_params_say() {
    // R68: an untagged source under 720 lines is written as players read
    // it, BT.601 (SMPTE 170M primaries, transfer and matrix, 6 in H.273).
    let dir = tempfile::tempdir().unwrap();
    let spec = || {
        let mut spec = spec_h264(15_360);
        let v = spec.video.as_mut().unwrap();
        v.tag_colour = true;
        v.bt601 = true;
        spec
    };
    let s = synthetic(spec(), frames(3, 512, 3));
    let file = write_to(Container::Mp4, &[&s], &Tags::default(), &dir, "sd.mp4");
    let trak = traks(&file)[0];
    let stsd = find(stbl(trak), &[b"stsd"]).unwrap();
    let kids = mp4_children(&stsd[8 + 8 + 78..]);
    let colr = kids.iter().find(|(k, _)| k == b"colr").expect("a colr");
    assert_eq!(colr.1, [b'n', b'c', b'l', b'x', 0, 6, 0, 6, 0, 6, 0]);

    let s = synthetic(spec(), frames(3, 512, 3));
    let file = write_to(Container::Mkv, &[&s], &Tags::default(), &dir, "sd.mkv");
    let kids = mkv_children(mkv_children(&file)[1].1);
    let track = mkv_children(mkv_children(kids[2].1)[0].1);
    let video = mkv_children(track.iter().find(|k| k.0 == 0xE0).unwrap().1);
    let get = |list: &[(u32, &[u8])], id: u32| uint_of(list.iter().find(|k| k.0 == id).unwrap().1);
    let colour = mkv_children(video.iter().find(|k| k.0 == 0x55B0).unwrap().1);
    assert_eq!(
        (
            get(&colour, 0x55B1),
            get(&colour, 0x55B9),
            get(&colour, 0x55BA),
            get(&colour, 0x55BB)
        ),
        (6, 1, 6, 6),
        "matrix, range, transfer, primaries"
    );

    // BT.601 is for standard range only: HDR keeps its own description.
    let mut hdr = spec();
    hdr.video.as_mut().unwrap().hdr = Hdr::Pq;
    let s = synthetic(hdr, frames(3, 512, 3));
    let file = write_to(Container::Mp4, &[&s], &Tags::default(), &dir, "hdr.mp4");
    let trak = traks(&file)[0];
    let stsd = find(stbl(trak), &[b"stsd"]).unwrap();
    let kids = mp4_children(&stsd[8 + 8 + 78..]);
    let colr = kids.iter().find(|(k, _)| k == b"colr").expect("a colr");
    assert_eq!(colr.1, [b'n', b'c', b'l', b'x', 0, 9, 0, 16, 0, 9, 0]);
}

// ---------------------------------------------------------------------------
// Round trips through FFmpeg

#[test]
fn an_mp4_with_one_h264_and_one_aac_stream_reads_back() {
    if !ffmpeg_ready() {
        return;
    }
    let src = video_fixture("h264.mp4");
    let (v, a) = (extract(&src, "v").unwrap(), extract(&src, "a").unwrap());
    assert_eq!(v.packets.len(), 60);
    let dir = tempfile::tempdir().unwrap();
    let out = write(
        Container::Mp4,
        &[&v, &a],
        &Tags::default(),
        dir.path(),
        "out.mp4",
    );

    // Kiln's own reader.
    let info = probe(&out).unwrap();
    assert_eq!((info.width, info.height, info.rotation), (320, 240, 0));
    assert_eq!(info.bit_depth, 8);
    assert!(info.sound.is_some());
    // moov before mdat.
    assert_eq!(
        top_level(&std::fs::read(&out).unwrap()),
        ["ftyp", "moov", "mdat"]
    );
    // FFmpeg's.
    let counts = run(Command::new("ffprobe").args([
        "-v",
        "error",
        "-count_packets",
        "-show_entries",
        "stream=codec_name,nb_read_packets",
        "-of",
        "csv=p=0",
        out.to_str().unwrap(),
    ]));
    let lines: Vec<&str> = counts.lines().collect();
    assert_eq!(
        lines,
        ["h264,60", format!("aac,{}", a.packets.len()).as_str()]
    );
    assert_same_packets(&v, &out, "v", None, true);
    assert_same_packets(&a, &out, "a", None, true);
    assert!(decodes_cleanly(&out));
}

#[test]
fn b_frames_write_ctts_and_play_in_order() {
    if !ffmpeg_ready() {
        return;
    }
    let v = extract(&video_fixture("h264.mp4"), "v").unwrap();
    // The fixture has B-frames: pts and dts differ.
    assert!(v.packets.iter().any(|p| p.pts != p.dts));
    assert!(v.packets.iter().any(|p| p.dts < 0));
    let dir = tempfile::tempdir().unwrap();
    let out = write(Container::Mp4, &[&v], &Tags::default(), dir.path(), "b.mp4");
    let file = std::fs::read(&out).unwrap();
    assert!(find(stbl(traks(&file)[0]), &[b"ctts"]).is_some());
    let got = packets(&out, "v");
    let pts: Vec<_> = got.iter().map(|p| p.pts.unwrap()).collect();
    let want: Vec<_> = v.packets.iter().map(|p| p.pts).collect();
    assert_eq!(pts, want);
    let mut shown = pts.clone();
    shown.sort_unstable();
    assert_eq!(shown[0], 0, "the first frame shows at 0");
    assert!(decodes_cleanly(&out));
}

#[test]
fn every_fixture_stream_survives_every_container_it_fits() {
    if !ffmpeg_ready() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    // (fixture, containers). AV1 only goes to Matroska in this piece.
    let cases: [(&str, &[Container]); 7] = [
        (
            "h264.mp4",
            &[
                Container::Mp4,
                Container::Mov,
                Container::M4v,
                Container::Mkv,
            ],
        ),
        ("h264.mkv", &[Container::Mp4, Container::Mkv]),
        (
            "hevc.mp4",
            &[Container::Mp4, Container::Mov, Container::Mkv],
        ),
        ("vfr.mp4", &[Container::Mp4, Container::Mkv]),
        ("av1.mkv", &[Container::Mkv]),
        ("odd.mp4", &[Container::Mkv]),
        ("dts.mkv", &[Container::Mkv]),
    ];
    for (name, containers) in cases {
        let src = video_fixture(name);
        let v = extract(&src, "v").unwrap();
        // The sound is AAC, Opus or (dts.mkv) DTS, which has no stream type here.
        let a = if name == "dts.mkv" {
            None
        } else {
            extract(&src, "a")
        };
        for &c in containers {
            let mkv = c == Container::Mkv;
            let ext = if mkv { "mkv" } else { "mp4" };
            let mut streams = vec![&v];
            if let Some(a) = &a {
                // Opus can't go into an MP4 here.
                if !(matches!(a.spec.codec, StreamCodec::Opus { .. }) && !mkv) {
                    streams.push(a);
                }
            }
            let out = write(
                c,
                &streams,
                &Tags::default(),
                dir.path(),
                &format!("{name}.{c:?}.{ext}"),
            );
            let shift = mkv.then(|| mkv_shift(&streams));
            if matches!(v.spec.codec, StreamCodec::Hevc { .. }) && !mkv {
                // An MP4 takes the samples' copies of the hvcC's parameter sets out.
                let (expected, hev1) = without_repeated_parameter_sets(&v);
                assert_same_packets(&expected, &out, "v", shift, true);
                let tag = stream_facts(&out, "v", "stream=codec_tag_string");
                assert_eq!(tag["codec_tag_string"], if hev1 { "hev1" } else { "hvc1" });
            } else {
                assert_same_packets(&v, &out, "v", shift, true);
            }
            if streams.len() > 1 {
                assert_same_packets(a.as_ref().unwrap(), &out, "a", shift, true);
            }
            assert!(decodes_cleanly(&out), "{name} as {c:?}");
            let info = probe(&out).unwrap();
            assert_eq!(
                (info.width, info.height),
                (
                    v.spec.video.as_ref().unwrap().width,
                    v.spec.video.as_ref().unwrap().height
                )
            );
        }
    }
}

#[test]
fn a_10_mhz_stream_plays_the_same_times_through_ffprobe() {
    if !ffmpeg_ready() {
        return;
    }
    let src = video_fixture("h264.mp4");
    let v = retimed(&extract(&src, "v").unwrap(), 10_000_000);
    let a = retimed(&extract(&src, "a").unwrap(), 10_000_000);
    let dir = tempfile::tempdir().unwrap();
    let out = write(
        Container::Mp4,
        &[&v, &a],
        &Tags::default(),
        dir.path(),
        "x.mp4",
    );
    assert_same_packets(&v, &out, "v", None, true);
    assert_same_packets(&a, &out, "a", None, true);
    assert!(decodes_cleanly(&out));
    let out = write(
        Container::Mkv,
        &[&v, &a],
        &Tags::default(),
        dir.path(),
        "x.mkv",
    );
    let shift = Some(mkv_shift(&[&v, &a]));
    assert_same_packets(&v, &out, "v", shift, true);
    assert_same_packets(&a, &out, "a", shift, true);
    assert!(decodes_cleanly(&out));
}

#[test]
fn an_mkv_with_h264_and_opus_reads_back() {
    if !ffmpeg_ready() {
        return;
    }
    let src = video_fixture("h264.mkv");
    let (v, a) = (extract(&src, "v").unwrap(), extract(&src, "a").unwrap());
    let dir = tempfile::tempdir().unwrap();
    let out = write(
        Container::Mkv,
        &[&v, &a],
        &Tags::default(),
        dir.path(),
        "out.mkv",
    );
    let names = run(Command::new("ffprobe").args([
        "-v",
        "error",
        "-count_packets",
        "-show_entries",
        "stream=codec_name,nb_read_packets",
        "-of",
        "csv=p=0",
        out.to_str().unwrap(),
    ]));
    assert_eq!(
        names.lines().collect::<Vec<_>>(),
        [
            format!("h264,{}", v.packets.len()),
            format!("opus,{}", a.packets.len())
        ]
    );
    let secs: f64 = run(Command::new("ffprobe").args([
        "-v",
        "error",
        "-show_entries",
        "format=duration",
        "-of",
        "csv=p=0",
        out.to_str().unwrap(),
    ]))
    .trim()
    .parse()
    .unwrap();
    // The Info duration: the latest end of any block, in milliseconds, moved
    // by the shift. (ffprobe prints it to the microsecond.)
    let end_ms = [&v, &a]
        .iter()
        .map(|s| {
            let last = s.packets.last().unwrap();
            ms_with_delay(last.pts + last.duration, s.spec.timescale, pre_skip(s))
        })
        .max()
        .unwrap();
    let want = (end_ms + mkv_shift(&[&v, &a])) as f64 / 1000.0;
    assert!((secs - want).abs() < 0.0005, "{secs}, not {want}");
    // Seeking to 1 s finds a key frame through the cues: the first frame out
    // is at or before the second, and decoding goes on from it.
    let seek = run(Command::new("ffprobe").args([
        "-v",
        "error",
        "-read_intervals",
        "1%+#2",
        "-select_streams",
        "v:0",
        "-show_entries",
        "packet=pts_time,flags",
        "-of",
        "csv=p=0",
        out.to_str().unwrap(),
    ]));
    let first: Vec<&str> = seek.lines().next().unwrap().split(',').collect();
    let t: f64 = first[0].parse().unwrap();
    assert!(t <= 1.0 && first[1].starts_with('K'), "{seek}");
    let frame = run(Command::new("ffmpeg").args([
        "-v",
        "error",
        "-copyts",
        "-ss",
        "1",
        "-i",
        out.to_str().unwrap(),
        "-map",
        "0:v:0",
        "-frames:v",
        "1",
        "-f",
        "framemd5",
        "-",
    ]));
    let line = frame
        .lines()
        .find(|l| !l.starts_with('#'))
        .expect("a frame after the seek");
    // framemd5: stream, dts, pts, duration, size, hash; in the time base its
    // "#tb" line gives. The frame is at 1 s.
    let tb = frame
        .lines()
        .find_map(|l| l.strip_prefix("#tb 0: "))
        .unwrap();
    let (num, den) = tb.split_once('/').unwrap();
    let pts: f64 = line.split(',').nth(2).unwrap().trim().parse().unwrap();
    let secs = pts * num.parse::<f64>().unwrap() / den.parse::<f64>().unwrap();
    assert!((secs - 1.0).abs() < 0.001, "{line} at {secs}");
    let shift = Some(mkv_shift(&[&v, &a]));
    assert_same_packets(&v, &out, "v", shift, true);
    assert_same_packets(&a, &out, "a", shift, true);
    assert!(decodes_cleanly(&out));
}

#[test]
fn hdr_hevc_keeps_its_colour_tags() {
    if !ffmpeg_ready() {
        return;
    }
    let mut v = extract(&video_fixture("hevc10_hdr.mp4"), "v").unwrap();
    // The fixture carries its HDR10 metadata in the bitstream (SEI) and has no
    // mdcv or clli boxes, so the params get what make.ps1 gave x265.
    let params = v.spec.video.as_mut().unwrap();
    assert_eq!(params.hdr, Hdr::Pq);
    let mut m = Vec::new();
    for x in [13250u16, 34500, 7500, 3000, 34000, 16000, 15635, 16450] {
        m.extend_from_slice(&x.to_be_bytes());
    }
    m.extend_from_slice(&10_000_000u32.to_be_bytes());
    m.extend_from_slice(&1u32.to_be_bytes());
    params.mastering = Some(m.try_into().unwrap());
    params.content_light = Some([0x03, 0xE8, 0x01, 0x90]);
    let dir = tempfile::tempdir().unwrap();
    for c in [Container::Mp4, Container::Mkv] {
        let ext = if c == Container::Mkv { "mkv" } else { "mp4" };
        let out = write(
            c,
            &[&v],
            &Tags::default(),
            dir.path(),
            &format!("hdr.{ext}"),
        );
        let facts = stream_facts(
            &out,
            "v",
            "stream=pix_fmt,color_transfer,color_primaries,color_space,codec_tag_string",
        );
        assert_eq!(facts["color_transfer"], "smpte2084", "{c:?}");
        assert_eq!(facts["color_primaries"], "bt2020", "{c:?}");
        assert_eq!(facts["color_space"], "bt2020nc", "{c:?}");
        assert_eq!(facts["pix_fmt"], "yuv420p10le", "{c:?}");
        if c != Container::Mkv {
            assert_eq!(facts["codec_tag_string"], "hvc1");
        }
        // Kiln's reader agrees, and the mastering and light levels survived.
        let info = probe(&out).unwrap();
        assert_eq!((info.hdr, info.bit_depth), (Hdr::Pq, 10), "{c:?}");
        // The stream's own side data is what the container holds, not the
        // bitstream's SEI.
        let side = run(Command::new("ffprobe").args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream_side_data",
            "-of",
            "flat",
            out.to_str().unwrap(),
        ]));
        // Matroska keeps the chromaticities as decimals, which FFmpeg reduces.
        let (red_x, max_lum) = if c == Container::Mkv {
            ("17/25", "1000/1")
        } else {
            ("34000/50000", "10000000/10000")
        };
        for want in [
            "max_content=1000".to_string(),
            "max_average=400".to_string(),
            format!("red_x=\"{red_x}\""),
            format!("max_luminance=\"{max_lum}\""),
        ] {
            assert!(side.contains(&want), "{c:?}: no {want} in {side}");
        }
        if c != Container::Mkv {
            assert_eq!(
                raw_box(&out, b"mdcv", 24),
                v.spec.video.as_ref().unwrap().mastering.map(|m| m.to_vec())
            );
            assert_eq!(
                raw_box(&out, b"clli", 4),
                Some(vec![0x03, 0xE8, 0x01, 0x90])
            );
        }
        let shift = (c == Container::Mkv).then(|| mkv_shift(&[&v]));
        if c == Container::Mkv {
            assert_same_packets(&v, &out, "v", shift, true);
        } else {
            // The fixture's samples carry no parameter sets (FFmpeg's MP4
            // writer took them out). Put the hvcC's in front of every key
            // frame, as an encoder that repeats its headers does: the writer
            // takes them out again, leaving the fixture's own samples.
            assert_same_packets(&v, &out, "v", shift, true);
            let (length, sets) = hvcc_parameter_sets(&v.spec.codec);
            let mut repeated = v.clone();
            for p in repeated.packets.iter_mut().filter(|p| p.key) {
                let mut data = Vec::new();
                for set in &sets {
                    data.extend_from_slice(&(set.len() as u32).to_be_bytes()[4 - length..]);
                    data.extend_from_slice(set);
                }
                data.extend_from_slice(&p.data);
                p.data = data;
            }
            assert!(repeated.packets[0].data.len() > v.packets[0].data.len());
            let again = write(
                c,
                &[&repeated],
                &Tags::default(),
                dir.path(),
                "hdr_repeated.mp4",
            );
            assert_same_packets(&v, &again, "v", shift, true);
            let tag = stream_facts(&again, "v", "stream=codec_tag_string");
            assert_eq!(tag["codec_tag_string"], "hvc1");
            assert!(decodes_cleanly(&again));
        }
        assert!(decodes_cleanly(&out), "{c:?}");
    }
}

#[test]
fn a_rotated_mp4_carries_its_matrix() {
    if !ffmpeg_ready() {
        return;
    }
    let v = extract(&video_fixture("rot90.mp4"), "v").unwrap();
    assert_eq!(v.spec.video.as_ref().unwrap().rotation, 90);
    let dir = tempfile::tempdir().unwrap();
    for c in [Container::Mp4, Container::Mov, Container::Mkv] {
        let ext = if c == Container::Mkv { "mkv" } else { "mp4" };
        let out = write(
            c,
            &[&v],
            &Tags::default(),
            dir.path(),
            &format!("rot.{c:?}.{ext}"),
        );
        let side = run(Command::new("ffprobe").args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream_side_data",
            "-of",
            "flat",
            out.to_str().unwrap(),
        ]));
        assert!(side.contains("rotation=-90"), "{c:?}: {side}");
        if c != Container::Mkv {
            assert_eq!(probe(&out).unwrap().rotation, 90, "{c:?}");
        }
    }
}

#[test]
fn ac3_goes_into_mp4_with_the_dac3_of_its_first_frame_and_into_matroska() {
    if !ffmpeg_ready() {
        return;
    }
    let src = fixture("ac3.mp4");
    let Some(a) = extract(&src, "a") else {
        eprintln!("skipped: the AC-3 fixture has no sound stream");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let out = write(
        Container::Mp4,
        &[&a],
        &Tags::default(),
        dir.path(),
        "ac3.mp4",
    );
    assert_eq!(
        stream_facts(&out, "a", "stream=codec_name")["codec_name"],
        "ac3"
    );
    assert_same_packets(&a, &out, "a", None, true);
    // The fixture's dac3 box (three bytes) and ours are the same.
    assert_eq!(raw_box(&src, b"dac3", 3), raw_box(&out, b"dac3", 3));
    assert!(raw_box(&out, b"dac3", 3).is_some());
    let out = write(
        Container::Mkv,
        &[&a],
        &Tags::default(),
        dir.path(),
        "ac3.mkv",
    );
    assert_eq!(
        stream_facts(&out, "a", "stream=codec_name")["codec_name"],
        "ac3"
    );
    assert_same_packets(&a, &out, "a", Some(mkv_shift(&[&a])), true);
}

#[test]
fn matroska_tags_are_written() {
    if !ffmpeg_ready() {
        return;
    }
    let v = extract(&video_fixture("h264.mp4"), "v").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let tags = Tags {
        title: Some("A title".into()),
        artist: Some("Someone".into()),
        ..Tags::default()
    };
    let out = write(Container::Mkv, &[&v], &tags, dir.path(), "tagged.mkv");
    let shown = run(Command::new("ffprobe").args([
        "-v",
        "error",
        "-show_entries",
        "format_tags",
        "-of",
        "flat",
        out.to_str().unwrap(),
    ]));
    assert!(
        shown.contains("A title") && shown.contains("Someone"),
        "{shown}"
    );
}
