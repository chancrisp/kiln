//! MP4 edit lists: a picture that starts late with B-frames keeps its first
//! frame and its start through every encode path (R38), copies honour the
//! edit list (R69), and every list shape keeps each frame at its true time
//! (R75). A Matroska picture's frames take its block times (R78). The
//! sources are made here from the fixtures by Kiln's own reader and writer,
//! with every packet moved by a fixed time, and their edit lists rewritten
//! where a shape needs it.
//!
//! The ffprobe cross-checks skip with a printed line when FFmpeg isn't on
//! PATH; the rest needs only Media Foundation.

#![cfg(windows)]

mod common;

use kiln_media::video::{
    self, convert, encoder_adapter, muxer, tune, Container, EncoderChoice, JobReport, Packet,
    ReadMode, Reader, SoundChoice, VCodec, VideoSetting, VideoTarget, VideoTune,
};
use kiln_media::{MediaError, Preset, Tags};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard};

// ---------------------------------------------------------------------------
// Helpers

/// One test at a time: several use the graphics card.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner())
}

/// 100 ns ticks per second, the reader's clock.
const SECOND: i64 = 10_000_000;
/// One frame of the fixtures (30 fps), in 100 ns.
const FRAME: i64 = SECOND / 30;
/// One AAC packet of the fixtures' sound (1,024 samples at 48 kHz).
const AAC_PACKET: i64 = 1024 * SECOND / 48_000;
/// A millisecond: how near a time written into an MP4 and read back must
/// come.
const MS: i64 = SECOND / 1000;
/// How near a span in Matroska, which keeps each time to the millisecond,
/// must come: both of its ends rounded, and a tick.
const MKV_ROUNDING: i64 = 2 * MS + 1;

fn have(tool: &str) -> bool {
    Command::new(tool)
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn video(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/video")
        .join(name)
}

fn open(path: &Path, mode: ReadMode) -> Reader {
    Reader::open(File::open(path).unwrap(), mode).unwrap()
}

/// Every packet of stream `stream`, in decode order.
fn packets_of(path: &Path, stream: usize) -> Vec<Packet> {
    let mut r = open(path, ReadMode::Compressed);
    let mut out = Vec::new();
    while let Some(p) = r.next_packet().unwrap() {
        if p.stream == stream {
            out.push(p);
        }
    }
    out
}

/// Where the picture's presentation starts, as the reader says.
fn start_of(path: &Path) -> i64 {
    open(path, ReadMode::Compressed).start(0)
}

/// The decoded frames' times and lengths, in presentation order, and
/// whether they kept the decoder's times (R78: only a Matroska picture whose
/// frames and blocks didn't line up).
fn decoded(path: &Path) -> (Vec<(i64, i64)>, bool) {
    let mut r = open(path, ReadMode::Decoded { d3d: None });
    let mut out = Vec::new();
    while let Some(f) = r.next_frame().unwrap() {
        out.push((f.pts, f.duration));
    }
    (out, r.decoder_times())
}

/// The decoded frames' times and lengths, in presentation order: a
/// Matroska picture's are its block times (R78).
fn frame_times(path: &Path) -> Vec<(i64, i64)> {
    let (frames, decoder_times) = decoded(path);
    assert!(!decoder_times, "{path:?}: the frames and blocks parted");
    frames
}

/// `src`'s streams written by Kiln's writer into `c` with every packet `by`
/// later. A start after 0 is an empty edit; a start before 0 hides what
/// comes before it with the edit's media time.
fn moved(src: &Path, by: i64, out: &Path, c: Container) {
    let mut r = open(src, ReadMode::Compressed);
    let mut m = muxer(c, File::create(out).unwrap()).unwrap();
    for s in r.streams() {
        assert_eq!(m.add_stream(s.spec.clone()).unwrap(), s.index);
    }
    while let Some(mut p) = r.next_packet().unwrap() {
        p.pts += by;
        p.dts += by;
        m.write(p).unwrap();
    }
    m.finish(&Tags::default()).unwrap();
}

// The boxes of a file Kiln's writer made, where every box has a 32-bit
// size: (type, where it starts, its size).
type Bx = ([u8; 4], usize, usize);

fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(b[at..at + 4].try_into().unwrap())
}

fn kids(b: &[u8], from: usize, to: usize) -> Vec<Bx> {
    let mut out = Vec::new();
    let mut at = from;
    while at + 8 <= to {
        let size = be32(b, at) as usize;
        if size < 8 || at + size > to {
            break;
        }
        out.push((b[at + 4..at + 8].try_into().unwrap(), at, size));
        at += size;
    }
    out
}

fn kid(b: &[u8], parent: Bx, kind: &[u8; 4]) -> Bx {
    kids(b, parent.1 + 8, parent.1 + parent.2)
        .into_iter()
        .find(|k| &k.0 == kind)
        .unwrap_or_else(|| panic!("no {}", String::from_utf8_lossy(kind)))
}

fn top(b: &[u8], kind: &[u8; 4]) -> Bx {
    kids(b, 0, b.len())
        .into_iter()
        .find(|k| &k.0 == kind)
        .unwrap()
}

fn traks(b: &[u8]) -> Vec<Bx> {
    let moov = top(b, b"moov");
    kids(b, moov.1 + 8, moov.1 + moov.2)
        .into_iter()
        .filter(|k| &k.0 == b"trak")
        .collect()
}

/// Track `track`'s edit list as (duration, media time) pairs, with the
/// movie's and the track's time scales.
fn track_edits(path: &Path, track: usize) -> (Vec<(u64, i64)>, u32, u32) {
    let b = std::fs::read(path).unwrap();
    let mvhd = kid(&b, top(&b, b"moov"), b"mvhd");
    assert_eq!(b[mvhd.1 + 8], 0, "a version 0 mvhd");
    let movie = be32(&b, mvhd.1 + 20);
    let trak = traks(&b)[track];
    let mdhd = kid(&b, kid(&b, trak, b"mdia"), b"mdhd");
    assert_eq!(b[mdhd.1 + 8], 0, "a version 0 mdhd");
    let media = be32(&b, mdhd.1 + 20);
    let elst = kid(&b, kid(&b, trak, b"edts"), b"elst");
    assert_eq!(b[elst.1 + 8], 0, "a version 0 elst");
    let edits = (0..be32(&b, elst.1 + 12) as usize)
        .map(|i| {
            let e = elst.1 + 16 + i * 12;
            (be32(&b, e) as u64, be32(&b, e + 4) as i32 as i64)
        })
        .collect();
    (edits, movie, media)
}

/// The picture's edit list.
fn edits(path: &Path) -> Vec<(u64, i64)> {
    track_edits(path, 0).0
}

/// Gives track `track` the edit list `edits` (duration in the movie's time
/// scale, media time in the track's, -1 for an empty edit). The boxes
/// around it grow with it, and every chunk offset moves with the samples
/// after it.
fn set_edits(path: &Path, track: usize, edits: &[(u64, i64)]) {
    let mut b = std::fs::read(path).unwrap();
    let moov = top(&b, b"moov");
    let trak = traks(&b)[track];
    let edts = kid(&b, trak, b"edts");
    let elst = kid(&b, edts, b"elst");
    let mut new = Vec::new();
    new.extend(((16 + edits.len() * 12) as u32).to_be_bytes());
    new.extend(b"elst");
    new.extend([0, 0, 0, 0]);
    new.extend((edits.len() as u32).to_be_bytes());
    for &(duration, media_time) in edits {
        new.extend((duration as u32).to_be_bytes());
        new.extend((media_time as i32).to_be_bytes());
        new.extend(0x10000u32.to_be_bytes());
    }
    let delta = new.len() as i64 - elst.2 as i64;
    b.splice(elst.1..elst.1 + elst.2, new);
    for (_, at, size) in [edts, trak, moov] {
        b[at..at + 4].copy_from_slice(&((size as i64 + delta) as u32).to_be_bytes());
    }
    if top(&b, b"mdat").1 > moov.1 {
        for trak in traks(&b) {
            let stbl = kid(&b, kid(&b, kid(&b, trak, b"mdia"), b"minf"), b"stbl");
            for (kind, at, _) in kids(&b, stbl.1 + 8, stbl.1 + stbl.2) {
                let n = be32(&b, at + 12) as usize;
                for i in 0..n {
                    if &kind == b"stco" {
                        let e = at + 16 + i * 4;
                        let v = (be32(&b, e) as i64 + delta) as u32;
                        b[e..e + 4].copy_from_slice(&v.to_be_bytes());
                    } else if &kind == b"co64" {
                        let e = at + 16 + i * 8;
                        let v = u64::from_be_bytes(b[e..e + 8].try_into().unwrap());
                        b[e..e + 8].copy_from_slice(&((v as i64 + delta) as u64).to_be_bytes());
                    }
                }
            }
        }
    }
    std::fs::write(path, b).unwrap();
}

/// ffprobe's decoded frame times of the picture, in seconds (FFmpeg honours
/// the edit list), or `None` without FFmpeg.
fn ffprobe_frames(path: &Path) -> Option<Vec<f64>> {
    if !(have("ffmpeg") && have("ffprobe")) {
        eprintln!("skipped the ffprobe check: FFmpeg isn't on PATH");
        return None;
    }
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries"])
        .args(["frame=best_effort_timestamp_time", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .unwrap();
    assert!(out.status.success(), "ffprobe failed on {path:?}");
    let mut times: Vec<f64> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().trim_end_matches(',').parse().ok())
        .collect();
    times.sort_by(f64::total_cmp);
    Some(times)
}

/// FFmpeg decodes `path` with no error, or there's no FFmpeg to ask. A
/// copy that starts at an open GOP's key frame (not an IDR) gets H.264's
/// note that a picture to forget was never seen ("mmco: unref short
/// failure"), as FFmpeg's own stream copy of it does; that is no error.
fn decodes_cleanly(path: &Path) -> bool {
    if !have("ffmpeg") {
        return true;
    }
    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-f", "null", "-"])
        .output()
        .unwrap();
    let notes = String::from_utf8_lossy(&out.stderr);
    out.status.success()
        && notes
            .lines()
            .all(|l| l.contains("mmco: unref short failure") || l.contains("Last message repeated"))
}

fn ext(t: VideoTarget) -> &'static str {
    match t {
        VideoTarget::Mkv => "mkv",
        VideoTarget::Mov => "mov",
        VideoTarget::M4v => "m4v",
        VideoTarget::Mp4 => "mp4",
    }
}

fn container(t: VideoTarget) -> Container {
    match t {
        VideoTarget::Mkv => Container::Mkv,
        VideoTarget::Mov => Container::Mov,
        VideoTarget::M4v => Container::M4v,
        VideoTarget::Mp4 => Container::Mp4,
    }
}

fn original() -> VideoTune {
    VideoTune {
        setting: Some(VideoSetting::Original),
        ..VideoTune::default()
    }
}

fn run(input: &Path, output: &Path, target: VideoTarget, tune: &VideoTune) -> JobReport {
    let _ = std::fs::remove_file(output);
    convert(
        input,
        output,
        target,
        Preset::High,
        tune,
        &mut |_| {},
        &|| false,
    )
    .unwrap_or_else(|e| panic!("{output:?}: {e}"))
}

/// From the first frame's start to the last one's end, in 100 ns.
fn span(frames: &[(i64, i64)]) -> i64 {
    let start = frames.iter().map(|f| f.0).min().unwrap();
    let end = frames.iter().map(|f| f.0 + f.1).max().unwrap();
    end - start
}

fn first(frames: &[(i64, i64)]) -> i64 {
    frames.iter().map(|f| f.0).min().unwrap()
}

fn quality(q: u8, codec: VCodec, encoder: EncoderChoice) -> VideoTune {
    VideoTune {
        setting: Some(VideoSetting::Quality(q)),
        codec,
        encoder,
        ..VideoTune::default()
    }
}

/// Every frame of `out` is at `want`'s time moved by `by`, within `within`.
fn assert_frames_at(name: &str, out: &Path, want: &[(i64, i64)], by: i64, within: i64) {
    let got = frame_times(out);
    assert_eq!(got.len(), want.len(), "{name}: frames");
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        assert!(
            (g.0 - (w.0 + by)).abs() <= within,
            "{name}: frame {i} at {} (100 ns), not {}",
            g.0,
            w.0 + by
        );
    }
}

/// ffprobe sees `out`'s frames where it sees `src`'s, moved by `by`, to
/// the millisecond.
fn assert_ffprobe_frames(name: &str, src: &Path, out: &Path, by: f64) {
    if let (Some(s), Some(o)) = (ffprobe_frames(src), ffprobe_frames(out)) {
        assert_eq!(o.len(), s.len(), "{name}: FFmpeg's frames");
        for (i, (o, s)) in o.iter().zip(&s).enumerate() {
            assert!(
                (o - (s + by)).abs() <= 0.0011,
                "{name}: FFmpeg's frame {i} at {o}, not {}",
                s + by
            );
        }
    }
}

// ---------------------------------------------------------------------------
// A picture that starts late, with B-frames (R38)

/// h264.mp4 (B-frames, 60 frames at 30 fps, AAC) half a second late, in
/// `c`: in an MP4 or MOV the video track's edit list is an empty edit, then
/// the B-frame delay, as FFmpeg writes a late-starting file; in Matroska the
/// first block is at half a second.
fn late_source(dir: &Path, c: VideoTarget) -> PathBuf {
    let out = dir.join(format!("late.{}", ext(c)));
    moved(&video("h264.mp4"), SECOND / 2, &out, container(c));
    if c == VideoTarget::Mkv {
        assert_eq!(block_times(&out)[0], SECOND / 2);
        return out;
    }
    let e = edits(&out);
    assert_eq!(e.len(), 2, "an empty edit, then the media: {e:?}");
    assert_eq!(e[0].1, -1, "{e:?}");
    assert!(
        e[1].1 > 0,
        "the media starts after the B-frame delay: {e:?}"
    );
    out
}

#[test]
fn a_late_start_with_b_frames_is_read_from_its_first_frame() {
    let _one = serial();
    if common::no_mf() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    for c in [VideoTarget::Mp4, VideoTarget::Mov] {
        let src = late_source(dir.path(), c);
        let name = ext(c);
        let frames = frame_times(&src);
        assert_eq!(frames.len(), 60, "{name}: every frame is decoded");
        assert!(
            (first(&frames) - SECOND / 2).abs() <= 10_000,
            "{name}: the first frame is at {} (100 ns), not half a second",
            first(&frames)
        );
        let packets = packets_of(&src, 0);
        assert_eq!(packets.len(), 60);
        let first_packet = packets.iter().map(|p| p.pts).min().unwrap();
        assert!(
            (first_packet - SECOND / 2).abs() <= 10_000,
            "{name}: {first_packet}"
        );
        if let Some(ff) = ffprobe_frames(&src) {
            assert_eq!(ff.len(), 60);
            assert!((ff[0] - 0.5).abs() <= 0.001, "{name}: ffprobe {}", ff[0]);
        }
    }
}

#[test]
fn every_encode_path_keeps_a_late_first_frame_and_its_time() {
    let _one = serial();
    if common::no_mf() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mut paths = Vec::new();
    for codec in [VCodec::H264, VCodec::Hevc] {
        paths.push((codec, EncoderChoice::Processor));
        if encoder_adapter(codec, EncoderChoice::GraphicsCard).is_some() {
            paths.push((codec, EncoderChoice::GraphicsCard));
        } else {
            eprintln!("skipped {codec:?} on the graphics card: no encoder starts");
        }
    }
    let mut ran = 0;
    // From an MP4, a MOV (the same reader, QuickTime's brand) and an MKV,
    // whose first frame takes its first block's time exactly (R78).
    for from in [VideoTarget::Mp4, VideoTarget::Mov, VideoTarget::Mkv] {
        let src = late_source(dir.path(), from);
        for &(codec, encoder) in &paths {
            for target in [VideoTarget::Mp4, VideoTarget::Mkv] {
                let name = format!("{} to {codec:?}-{encoder:?}.{}", ext(from), ext(target));
                let out = dir.path().join(name.replace(' ', "_"));
                let _ = std::fs::remove_file(&out);
                let made = convert(
                    &src,
                    &out,
                    target,
                    Preset::High,
                    &quality(65, codec, encoder),
                    &mut |_| {},
                    &|| false,
                );
                match made {
                    Ok(r) => assert!(!r.video_copied, "{name}"),
                    Err(e @ MediaError::NeedsExtensionToWrite { .. }) => {
                        eprintln!("skipped {name}: {e}");
                        continue;
                    }
                    Err(e) => panic!("{name}: {e}"),
                }
                ran += 1;
                let frames = frame_times(&out);
                assert_eq!(frames.len(), 60, "{name}: every frame is kept");
                if from == VideoTarget::Mkv {
                    assert_eq!(first(&frames), SECOND / 2, "{name}: the first frame");
                } else {
                    assert!(
                        (first(&frames) - SECOND / 2).abs() <= 10_000,
                        "{name}: the first frame is at {} (100 ns), not half a second",
                        first(&frames)
                    );
                }
                if let Some(ff) = ffprobe_frames(&out) {
                    assert_eq!(ff.len(), 60, "{name}: FFmpeg's frames");
                    assert!((ff[0] - 0.5).abs() <= 0.002, "{name}: ffprobe {}", ff[0]);
                }
            }
        }
    }
    assert!(ran > 0, "no encode path ran");
}

// ---------------------------------------------------------------------------
// Copies honour the edit list (R69)

/// `src` moved `secs` earlier: its edit list hides what is now before 0.
fn cut_of(src: &Path, dir: &Path, name: &str, secs: f64) -> PathBuf {
    let out = dir.join(name);
    moved(src, -(secs * SECOND as f64) as i64, &out, Container::Mp4);
    let e = edits(&out);
    assert_eq!(e.len(), 1, "one edit: {e:?}");
    assert!(e[0].1 > 0, "the media starts after its first sample: {e:?}");
    out
}

/// h264.mp4 moved `secs` earlier. Its key frames are at 0 and 1 s, so 1.5 s
/// puts the start half a second after a key frame, and 1.0 s on one.
fn cut(dir: &Path, name: &str, secs: f64) -> PathBuf {
    cut_of(&video("h264.mp4"), dir, name, secs)
}

/// The source's picture packets a copy keeps: from the last key frame, in
/// decode order, shown at or before `start`, less those after it shown
/// before it (an open GOP's leading pictures).
fn from_the_key_frame(all: &[Packet], start: i64) -> Vec<Packet> {
    let at = all
        .iter()
        .rposition(|p| p.key && p.pts <= start + FRAME / 2)
        .expect("a key frame before the start");
    let key = all[at].pts;
    all[at..].iter().filter(|p| p.pts >= key).cloned().collect()
}

/// `out` copies `src`'s picture: the same packets, byte for byte, from the
/// key frame on, and every frame shown at the source's time.
///
/// In an MP4 the times are the source's. Matroska has no times before 0:
/// where the source's sound straddles the start, an MKV copy starts up to
/// two AAC packets late (R76), every frame and the sound moved alike, so
/// the picture's span and the A/V sync stay exact.
fn assert_copied_from_the_key_frame(src: &Path, out: &Path, name: &str, mkv: bool) {
    let want = from_the_key_frame(&packets_of(src, 0), start_of(src));
    let got = packets_of(out, 0);
    assert_eq!(got.len(), want.len(), "{name}: packets from the key frame");
    for (i, (g, w)) in got.iter().zip(&want).enumerate() {
        assert!(g.data == w.data, "{name}: packet {i} differs");
        assert_eq!(g.key, w.key, "{name}: packet {i}");
    }
    let (src_frames, out_frames) = (frame_times(src), frame_times(out));
    assert_eq!(out_frames.len(), src_frames.len(), "{name}: frames shown");
    let late = first(&out_frames) - first(&src_frames);
    if mkv {
        assert!(
            (0..=2 * AAC_PACKET + MS).contains(&late),
            "{name}: R76 allows up to two AAC packets late, not {late}"
        );
    } else {
        assert!(late.abs() <= MS, "{name}: the first frame is {late} late");
    }
    let within = if mkv { MKV_ROUNDING } else { MS };
    assert_frames_at(name, out, &src_frames, late, within);
    assert!(
        (span(&out_frames) - span(&src_frames)).abs() <= within,
        "{name}: {} long against {}",
        span(&out_frames),
        span(&src_frames)
    );
    // FFmpeg reads a list of two media edits its own way (it moves the
    // frames at the second edit about); it reads the copy as Kiln does.
    let media_edits = edits(src).iter().filter(|e| e.1 >= 0).count();
    if !mkv && media_edits == 1 {
        assert_ffprobe_frames(name, src, out, 0.0);
    }
    assert!(decodes_cleanly(out), "{name}: FFmpeg reports errors");
    // The sound kept is the source's last packets, and starts where it did
    // against the picture.
    let src_sound = packets_of(src, 1);
    if src_sound.is_empty() {
        return;
    }
    let out_sound = packets_of(out, 1);
    let kept = &src_sound[src_sound.len() - out_sound.len()..];
    for (i, (o, s)) in out_sound.iter().zip(kept).enumerate() {
        assert!(o.data == s.data, "{name}: sound packet {i} differs");
    }
    let sync_out = first(&out_frames) - out_sound[0].pts;
    let sync_src = first(&src_frames) - kept[0].pts;
    assert!(
        (sync_out - sync_src).abs() <= within,
        "{name}: the sound leads the picture by {sync_out}, the source's by {sync_src}"
    );
    // What is kept of the sound starts at most two packets before its own
    // start: the packet the decoder primes on, and the one across it.
    let sound_start = open(src, ReadMode::Compressed).start(1);
    assert!(
        (0..=2 * AAC_PACKET).contains(&(sound_start - kept[0].pts)),
        "{name}: the sound kept starts at {}, its start is {sound_start}",
        kept[0].pts
    );
}

#[test]
fn an_edit_lists_lead_in_is_read_from_the_sample_tables() {
    let _one = serial();
    if common::no_mf() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let lead_in = |p: &Path| video::probe(p).unwrap().lead_in;
    let between = lead_in(&cut(dir.path(), "between.mp4", 1.5)).expect("frames hidden");
    assert!((between - 0.5).abs() < 1e-3, "{between}");
    let on = lead_in(&cut(dir.path(), "on.mp4", 1.0)).expect("frames hidden");
    assert!(on.abs() < 1e-3, "{on}");
    // An edit that takes off only the B-frame delay hides nothing, with an
    // empty edit before it or not.
    assert_eq!(lead_in(&video("h264.mp4")), None);
    assert_eq!(lead_in(&late_source(dir.path(), VideoTarget::Mp4)), None);
    assert_eq!(lead_in(&video("h264.mkv")), None);
    // R75: the shapes Media Foundation handled its own way are judged by
    // the same model as the reader's.
    let two = two_media_edits(dir.path());
    let l = lead_in(&two).expect("frames hidden");
    assert!((l - 0.5).abs() < 1e-3, "two media edits: {l}");
    let empty = empty_then_trimmed(dir.path());
    let l = lead_in(&empty).expect("frames hidden");
    assert!(
        (l - 0.5).abs() < 1e-3,
        "an empty edit, then a trimmed start: {l}"
    );
}

#[test]
fn a_copy_into_mp4_starts_at_the_key_frame_before_the_start() {
    let _one = serial();
    if common::no_mf() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    for (name, secs) in [("between", 1.5), ("on", 1.0)] {
        let src = cut(dir.path(), &format!("{name}.mp4"), secs);
        for target in [VideoTarget::Mp4, VideoTarget::Mov] {
            let label = format!("{name} into {}", ext(target));
            let out = dir.path().join(format!("{name}-copy.{}", ext(target)));
            let report = run(&src, &out, target, &original());
            assert!(report.video_copied, "{label}: {report:?}");
            assert!(report.sound_copied, "{label}: {report:?}");
            assert_copied_from_the_key_frame(&src, &out, &label, false);
        }
    }
}

#[test]
fn a_copy_into_mkv_is_made_only_when_the_start_is_on_a_key_frame() {
    let _one = serial();
    if common::no_mf() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let on = cut(dir.path(), "on.mp4", 1.0);
    let info = video::probe(&on).unwrap();
    assert!(tune::can_copy_video(VideoTarget::Mkv, &info, &original()));
    let out = dir.path().join("on.mkv");
    let report = run(&on, &out, VideoTarget::Mkv, &original());
    assert!(report.video_copied, "{report:?}");
    assert_copied_from_the_key_frame(&on, &out, "on into mkv", true);

    // Half a second after the key frame: MKV can't hide that, so Original
    // encodes the picture at the top step, starting where the source does,
    // with the sound copied beside it.
    let between = cut(dir.path(), "between.mp4", 1.5);
    let info = video::probe(&between).unwrap();
    assert!(!tune::can_copy_video(VideoTarget::Mkv, &info, &original()));
    assert_eq!(
        tune::default_setting(VideoTarget::Mkv, &info, &VideoTune::default()),
        VideoSetting::Quality(65)
    );
    let out = dir.path().join("between.mkv");
    let report = run(&between, &out, VideoTarget::Mkv, &original());
    assert!(!report.video_copied, "{report:?}");
    assert!(report.sound_copied, "{report:?}");
    let (src_frames, out_frames) = (frame_times(&between), frame_times(&out));
    let late = first(&out_frames) - first(&src_frames);
    assert!(
        (0..=2 * AAC_PACKET + MS).contains(&late),
        "R76: the first frame is {late} late"
    );
    assert_frames_at("between into mkv", &out, &src_frames, late, MKV_ROUNDING);
    assert!((span(&out_frames) - span(&src_frames)).abs() <= MKV_ROUNDING);
    // The sound copied is the source's last packets, against the picture
    // where the source's were.
    let src_sound = packets_of(&between, 1);
    let sound = packets_of(&out, 1);
    let kept = &src_sound[src_sound.len() - sound.len()..];
    for (i, (o, s)) in sound.iter().zip(kept).enumerate() {
        assert!(o.data == s.data, "sound packet {i} differs");
    }
    let sync_out = first(&out_frames) - sound[0].pts;
    let sync_src = first(&src_frames) - kept[0].pts;
    assert!(
        (sync_out - sync_src).abs() <= MKV_ROUNDING,
        "the sound leads the picture by {sync_out}, the source's by {sync_src}"
    );
}

#[test]
fn an_open_gops_leading_pictures_are_left_out_of_a_copy() {
    let _one = serial();
    if common::no_mf() {
        return;
    }
    // opengop.mp4's key frame at 1 s is followed in decode order by
    // B-frames shown before it, which need the GOP before. Moved 1.5 s
    // earlier, the copy starts at that key frame, and they go with that GOP.
    let dir = tempfile::tempdir().unwrap();
    let src = cut_of(&video("opengop.mp4"), dir.path(), "open.mp4", 1.5);
    let all = packets_of(&src, 0);
    let key = all
        .iter()
        .rposition(|p| p.key && p.pts <= FRAME / 2)
        .unwrap();
    let leading = all[key..].iter().filter(|p| p.pts < all[key].pts).count();
    assert!(
        leading > 0,
        "the fixture has leading pictures after its key frame"
    );
    for target in [VideoTarget::Mp4, VideoTarget::Mov] {
        let label = format!("open GOP into {}", ext(target));
        let out = dir.path().join(format!("open-copy.{}", ext(target)));
        let report = run(&src, &out, target, &original());
        assert!(report.video_copied, "{label}: {report:?}");
        assert_copied_from_the_key_frame(&src, &out, &label, false);
        assert_eq!(packets_of(&out, 0).len(), all.len() - key - leading);
    }
}

// ---------------------------------------------------------------------------
// Edit lists Media Foundation handled its own way (R75)

/// between.mp4's edit (the start half a second after a key frame) as two
/// media edits, one after the other: the same presentation. Left to it,
/// Media Foundation applied both, and dropped the key frame before them.
fn two_media_edits(dir: &Path) -> PathBuf {
    let out = cut(dir, "two.mp4", 1.5);
    let (e, movie, media) = track_edits(&out, 0);
    let (duration, at) = e[0];
    let half = duration / 2;
    let half_media = (half as i64 * media as i64) / movie as i64;
    set_edits(&out, 0, &[(half, at), (duration - half, at + half_media)]);
    out
}

/// between.mp4's edit after an empty edit of half a second: its frames are
/// shown half a second later. Left to it, Media Foundation gave the hidden
/// frames' times as the start.
fn empty_then_trimmed(dir: &Path) -> PathBuf {
    let out = cut(dir, "empty-trimmed.mp4", 1.5);
    let (e, movie, _) = track_edits(&out, 0);
    set_edits(&out, 0, &[(movie as u64 / 2, -1), e[0]]);
    out
}

/// `src`, presented as `reference` is but `by` later, keeps every frame at
/// its time when read, encoded (MP4 and MKV) and copied into MP4; MKV
/// copies only when the start is on a key frame (here it isn't). FFmpeg's
/// own times are compared where it reads `src` as Kiln does (`ffmpeg_src`):
/// with two media edits it moves the frames at the second edit about.
fn assert_every_frame_kept(
    dir: &Path,
    name: &str,
    src: &Path,
    reference: &Path,
    by: i64,
    ffmpeg_src: bool,
) {
    let want = frame_times(reference);
    let secs = by as f64 / SECOND as f64;
    assert_eq!(want.len(), 15, "the reference shows half a second");
    assert_eq!(packets_of(src, 0).len(), 60, "{name}: every sample is read");
    assert_frames_at(&format!("{name} read"), src, &want, by, 0);
    if ffmpeg_src {
        assert_ffprobe_frames(name, reference, src, secs);
    }

    for target in [VideoTarget::Mp4, VideoTarget::Mkv] {
        let label = format!("{name} encoded into {}", ext(target));
        let out = dir.join(format!("{name}-q65.{}", ext(target)));
        let report = run(
            src,
            &out,
            target,
            &quality(65, VCodec::H264, EncoderChoice::Processor),
        );
        assert!(!report.video_copied, "{label}");
        // The sound is copied beside the picture: into MKV, everything may
        // start up to two AAC packets late where it straddles the start
        // (R76), the frames moved alike.
        let late = first(&frame_times(&out)) - (first(&want) + by);
        if target == VideoTarget::Mkv {
            assert!(
                (0..=2 * AAC_PACKET + MS).contains(&late),
                "{label}: R76 allows up to two AAC packets late, not {late}"
            );
            assert_frames_at(&label, &out, &want, by + late, MKV_ROUNDING);
        } else {
            assert_frames_at(&label, &out, &want, by, MS);
            assert_ffprobe_frames(&label, reference, &out, secs);
        }
    }

    let label = format!("{name} copied into mp4");
    let out = dir.join(format!("{name}-copy.mp4"));
    let report = run(src, &out, VideoTarget::Mp4, &original());
    assert!(report.video_copied, "{label}: {report:?}");
    assert_copied_from_the_key_frame(src, &out, &label, false);
    assert_frames_at(&label, &out, &want, by, MS);

    let info = video::probe(src).unwrap();
    assert!(!tune::can_copy_video(VideoTarget::Mkv, &info, &original()));
}

#[test]
fn a_list_of_two_media_edits_keeps_every_frame_at_its_time() {
    let _one = serial();
    if common::no_mf() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let reference = cut(dir.path(), "one.mp4", 1.5);
    let two = two_media_edits(dir.path());
    assert_eq!(edits(&two).len(), 2);
    assert_every_frame_kept(dir.path(), "two edits", &two, &reference, 0, false);
}

#[test]
fn an_empty_edit_before_a_trimmed_start_keeps_every_frame_at_its_time() {
    let _one = serial();
    if common::no_mf() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let reference = cut(dir.path(), "one.mp4", 1.5);
    let src = empty_then_trimmed(dir.path());
    let e = edits(&src);
    assert_eq!((e.len(), e[0].1), (2, -1), "{e:?}");
    assert_eq!(start_of(&src), SECOND / 2);
    assert_every_frame_kept(
        dir.path(),
        "empty then trimmed",
        &src,
        &reference,
        SECOND / 2,
        true,
    );
}

// ---------------------------------------------------------------------------
// Sound beside an edit list

#[test]
fn re_encoded_sound_beside_a_lead_in_starts_with_the_picture() {
    let _one = serial();
    if common::no_mf() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let between = cut(dir.path(), "between.mp4", 1.5);
    let src_frames = frame_times(&between);
    let tune = VideoTune {
        setting: Some(VideoSetting::Original),
        sound: SoundChoice::Kbps(128),
        ..VideoTune::default()
    };
    // MP4: the picture copied, the sound encoded to AAC. MKV: the picture
    // encoded (R69), the sound to Opus.
    for (target, copied) in [(VideoTarget::Mp4, true), (VideoTarget::Mkv, false)] {
        let name = format!("between into {}", ext(target));
        let out = dir.path().join(format!("between-sound.{}", ext(target)));
        let report = run(&between, &out, target, &tune);
        assert_eq!(report.video_copied, copied, "{name}: {report:?}");
        assert!(!report.sound_copied, "{name}: {report:?}");
        let frames = frame_times(&out);
        assert_eq!(frames.len(), src_frames.len(), "{name}");
        assert!(
            (span(&frames) - span(&src_frames)).abs() <= FRAME,
            "{name}: {} long against {}",
            span(&frames),
            span(&src_frames)
        );
        let sound = packets_of(&out, 1);
        let gap = sound[0].pts - first(&frames);
        assert!(
            gap.abs() <= 2 * AAC_PACKET,
            "{name}: the sound starts {gap} from the picture"
        );
    }
}

#[test]
fn sound_after_an_empty_edit_and_aac_priming_keeps_its_order() {
    let _one = serial();
    if common::no_mf() {
        return;
    }
    // The sound half a second late, with its priming (1,024 samples) taken
    // off by the edit's media time, as encoders write AAC: an empty edit,
    // then media time 1,024. Media Foundation gave the priming packet and
    // the next the same time.
    let dir = tempfile::tempdir().unwrap();
    let src = late_source(dir.path(), VideoTarget::Mp4);
    let (e, movie, media) = track_edits(&src, 1);
    assert_eq!(media, 48_000);
    let shown = e.last().unwrap().0;
    set_edits(&src, 1, &[(movie as u64 / 2, -1), (shown, 1024)]);
    let check = |path: &Path, name: &str| {
        let sound = packets_of(path, 1);
        for (i, w) in sound.windows(2).enumerate() {
            assert!(
                w[1].pts > w[0].pts,
                "{name}: sound packet {} at {} after {}",
                i + 1,
                w[1].pts,
                w[0].pts
            );
        }
        assert!(
            (sound[0].pts - (SECOND / 2 - AAC_PACKET)).abs() <= 1,
            "{name}: the priming packet is at {}",
            sound[0].pts
        );
    };
    check(&src, "the source");
    let out = dir.path().join("late-copy.mp4");
    let report = run(&src, &out, VideoTarget::Mp4, &original());
    assert!(report.video_copied && report.sound_copied, "{report:?}");
    check(&out, "its copy");
}

#[test]
fn re_encoded_aac_after_an_empty_edit_hides_its_priming() {
    let _one = serial();
    if common::no_mf() {
        return;
    }
    // The sound half a second late (an empty edit, then the priming taken
    // off), encoded again to AAC beside the picture copied: the sound is
    // presented from half a second, as the source's is, with whatever the
    // new encoder primes on hidden by the edit list. (Windows' encoder
    // takes its own delay out, so here there is none to hide.)
    let dir = tempfile::tempdir().unwrap();
    let src = late_source(dir.path(), VideoTarget::Mp4);
    let (e, movie, _) = track_edits(&src, 1);
    set_edits(
        &src,
        1,
        &[(movie as u64 / 2, -1), (e.last().unwrap().0, 1024)],
    );
    let source_start = open(&src, ReadMode::Compressed).start(1);
    assert_eq!(source_start, SECOND / 2);
    let out = dir.path().join("late-aac.mp4");
    let tune = VideoTune {
        setting: Some(VideoSetting::Original),
        sound: SoundChoice::Kbps(128),
        ..VideoTune::default()
    };
    let report = run(&src, &out, VideoTarget::Mp4, &tune);
    assert!(report.video_copied && !report.sound_copied, "{report:?}");
    let (e, _, media) = track_edits(&out, 1);
    assert_eq!(e.len(), 2, "an empty edit, then the sound: {e:?}");
    assert_eq!(e[0].1, -1, "{e:?}");
    assert!(e[1].1 >= 0, "{e:?}");
    assert_eq!(
        open(&out, ReadMode::Compressed).start(1),
        source_start,
        "the sound is presented from where the source's is"
    );
    let sound = packets_of(&out, 1);
    assert_eq!(
        sound[0].pts + e[1].1 * SECOND / media as i64,
        source_start,
        "the first packet's priming, if any, ends at the start"
    );
}

// ---------------------------------------------------------------------------
// A Matroska picture's frames take its block times (R78)

/// The picture's block times as Kiln's compressed reader gives them, in
/// presentation order.
fn block_times(path: &Path) -> Vec<i64> {
    let mut t: Vec<i64> = packets_of(path, 0).iter().map(|p| p.pts).collect();
    t.sort_unstable();
    t
}

/// ffprobe's times of the picture's blocks, in 100 ns, in presentation
/// order, or `None` without FFmpeg.
fn ffprobe_blocks(path: &Path) -> Option<Vec<i64>> {
    if !have("ffprobe") {
        eprintln!("skipped the ffprobe check: FFmpeg isn't on PATH");
        return None;
    }
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries"])
        .args(["packet=pts_time", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .unwrap();
    assert!(out.status.success(), "ffprobe failed on {path:?}");
    let mut t: Vec<i64> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().trim_end_matches(',').parse::<f64>().ok())
        .map(|s| (s * SECOND as f64).round() as i64)
        .collect();
    t.sort_unstable();
    Some(t)
}

/// Two B-frames for the processor's H.264 encoder while it lives, through
/// the debug build's `KILN_H264_B_FRAMES` (R85 gives it none), gone again
/// when dropped, a panic included. Another test's processor H.264 encode
/// running meanwhile would only get B-frames too, which every reader takes.
struct BFrames;

impl BFrames {
    fn two() -> BFrames {
        std::env::set_var("KILN_H264_B_FRAMES", "2");
        BFrames
    }
}

impl Drop for BFrames {
    fn drop(&mut self) {
        std::env::remove_var("KILN_H264_B_FRAMES");
    }
}

#[test]
fn a_reordered_encode_into_mp4_starts_at_0() {
    let _one = serial();
    if common::no_mf() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("b-frames.mp4");
    {
        let _b = BFrames::two();
        run(
            &video("h264.mp4"),
            &out,
            VideoTarget::Mp4,
            &quality(65, VCodec::H264, EncoderChoice::Processor),
        );
    }
    // Reordered: the packets' times go back somewhere, and the MP4 says so
    // with a ctts.
    let packets = packets_of(&out, 0);
    assert!(
        packets.windows(2).any(|w| w[1].pts < w[0].pts),
        "no B-frames"
    );
    let bytes = std::fs::read(&out).unwrap();
    assert!(bytes.windows(4).any(|w| w == b"ctts"), "no ctts");
    // And the picture starts at 0, as the source's does (R38).
    assert_eq!(frame_times(&out)[0].0, 0);
    if have("ffprobe") {
        let o = Command::new("ffprobe")
            .args(["-v", "error", "-select_streams", "v:0", "-show_entries"])
            .args(["stream=start_time", "-of", "csv=p=0"])
            .arg(&out)
            .output()
            .unwrap();
        let start: f64 = String::from_utf8_lossy(&o.stdout)
            .trim()
            .trim_end_matches(',')
            .parse()
            .unwrap();
        assert_eq!(start, 0.0, "ffprobe's start_time");
    } else {
        eprintln!("skipped the ffprobe check: FFmpeg isn't on PATH");
    }
}

/// Matroska pictures with B-frames: h264.mkv (its first block at 0), the
/// same half a second late, and Kiln's own encode of between.mp4 on the
/// processor (its first block 28 ms in, R76), whose first frame Media
/// Foundation's decoder gave at 27.0 ms, every time. Kiln gives the
/// processor's H.264 encoder no B-frames (R85), so that encode asks for two
/// through the debug build's `KILN_H264_B_FRAMES`, as it had before.
fn mkv_sources(dir: &Path) -> Vec<(&'static str, PathBuf)> {
    let late = dir.join("late-blocks.mkv");
    moved(&video("h264.mkv"), SECOND / 2, &late, Container::Mkv);
    let between = cut(dir, "between.mp4", 1.5);
    let encoded = dir.join("encoded.mkv");
    {
        let _b = BFrames::two();
        run(
            &between,
            &encoded,
            VideoTarget::Mkv,
            &quality(65, VCodec::H264, EncoderChoice::Processor),
        );
    }
    let sources = vec![
        ("h264.mkv", video("h264.mkv")),
        ("late.mkv", late),
        ("encoded.mkv", encoded),
    ];
    for (name, src) in &sources {
        let packets = packets_of(src, 0);
        assert!(
            packets.windows(2).any(|w| w[1].pts < w[0].pts),
            "{name} has B-frames"
        );
    }
    sources
}

#[test]
fn an_mkv_pictures_frames_take_its_block_times() {
    let _one = serial();
    if common::no_mf() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let sources = mkv_sources(dir.path());
    let starts: Vec<i64> = sources.iter().map(|(_, s)| block_times(s)[0]).collect();
    assert_eq!(starts[..2], [0, SECOND / 2]);
    assert!(starts[2] > 0, "encoded.mkv's first block is after 0");
    for (name, src) in &sources {
        // A copy needs nothing: the compressed reader's times are the
        // blocks', as FFmpeg reads them.
        let blocks = block_times(src);
        if let Some(ff) = ffprobe_blocks(src) {
            assert_eq!(blocks, ff, "{name}: the packets' times");
        }
        // Decoded, every frame takes its block's time, read after read.
        for read in 0..3 {
            let (frames, decoder_times) = decoded(src);
            assert!(!decoder_times, "{name}");
            let times: Vec<i64> = frames.iter().map(|f| f.0).collect();
            assert_eq!(times, blocks, "{name}: read {read}");
        }
    }

    let mut paths = vec![(VCodec::H264, EncoderChoice::Processor)];
    for codec in [VCodec::H264, VCodec::Hevc] {
        if encoder_adapter(codec, EncoderChoice::GraphicsCard).is_some() {
            paths.push((codec, EncoderChoice::GraphicsCard));
        }
    }
    for (name, src) in &sources {
        let want = block_times(src);
        for &(codec, encoder) in &paths {
            for target in [VideoTarget::Mp4, VideoTarget::Mkv] {
                let label = format!("{name} to {codec:?}-{encoder:?}.{}", ext(target));
                let out = dir.path().join(label.replace(' ', "_"));
                let made = convert(
                    src,
                    &out,
                    target,
                    Preset::High,
                    &quality(65, codec, encoder),
                    &mut |_| {},
                    &|| false,
                );
                let report = match made {
                    Ok(r) => r,
                    Err(e @ MediaError::NeedsExtensionToWrite { .. }) => {
                        eprintln!("skipped {label}: {e}");
                        continue;
                    }
                    Err(e) => panic!("{label}: {e}"),
                };
                assert!(!report.video_copied && !report.decoder_times, "{label}");
                let got: Vec<i64> = frame_times(&out).iter().map(|f| f.0).collect();
                assert_eq!(got, want, "{label}: every frame at its block's time");
            }
        }
        // Copied into MP4, the frames keep the same times.
        let out = dir.path().join(format!("{name}-copy.mp4"));
        let report = run(src, &out, VideoTarget::Mp4, &original());
        assert!(report.video_copied && !report.decoder_times, "{report:?}");
        let got: Vec<i64> = frame_times(&out).iter().map(|f| f.0).collect();
        assert_eq!(got, want, "{name} copied");
    }
}

#[test]
fn an_mkv_picture_that_starts_before_a_key_frame_keeps_the_decoders_times() {
    let _one = serial();
    if common::no_mf() {
        return;
    }
    // h264.mkv less its first picture block: its first GOP has no key
    // frame, so the decoder gives the 30 frames from the key frame at 1 s,
    // of 59 blocks. Taken by number, each frame would be a second early;
    // they keep the decoder's times, which are the blocks' from the key
    // frame on, and the report says so.
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("no-key.mkv");
    let mut r = open(&video("h264.mkv"), ReadMode::Compressed);
    let mut m = muxer(Container::Mkv, File::create(&src).unwrap()).unwrap();
    for s in r.streams() {
        m.add_stream(s.spec.clone()).unwrap();
    }
    let mut first_picture = true;
    while let Some(p) = r.next_packet().unwrap() {
        if p.stream == 0 && std::mem::take(&mut first_picture) {
            continue;
        }
        m.write(p).unwrap();
    }
    m.finish(&Tags::default()).unwrap();
    drop(r);
    let blocks = block_times(&src);
    assert_eq!(blocks.len(), 59);
    let (frames, decoder_times) = decoded(&src);
    assert!(decoder_times, "the frames and blocks part");
    let times: Vec<i64> = frames.iter().map(|f| f.0).collect();
    let from_the_key: Vec<i64> = blocks.iter().copied().filter(|&t| t >= SECOND).collect();
    assert_eq!(times, from_the_key);

    let out = dir.path().join("no-key.mp4");
    let report = run(
        &src,
        &out,
        VideoTarget::Mp4,
        &quality(65, VCodec::H264, EncoderChoice::Processor),
    );
    assert!(report.decoder_times, "{report:?}");
    let got: Vec<i64> = frame_times(&out).iter().map(|f| f.0).collect();
    assert_eq!(got, from_the_key);
}

/// h264.mkv's streams written again into `out` by Kiln's writer, each
/// packet through `change` (which may drop it), and `extra` after them.
fn rewritten(
    out: &Path,
    mut change: impl FnMut(usize, Packet) -> Option<Packet>,
    extra: Option<Packet>,
) {
    let mut r = open(&video("h264.mkv"), ReadMode::Compressed);
    let mut m = muxer(Container::Mkv, File::create(out).unwrap()).unwrap();
    for s in r.streams() {
        m.add_stream(s.spec.clone()).unwrap();
    }
    let mut n = 0;
    while let Some(p) = r.next_packet().unwrap() {
        let picture = p.stream == 0;
        if let Some(p) = change(n, p) {
            m.write(p).unwrap();
        }
        n += picture as usize;
    }
    if let Some(p) = extra {
        m.write(p).unwrap();
    }
    m.finish(&Tags::default()).unwrap();
}

/// ffprobe's length of the frame shown last, in 100 ns, or `None` without
/// FFmpeg.
fn ffprobe_last_length(path: &Path) -> Option<i64> {
    if !have("ffprobe") {
        eprintln!("skipped the ffprobe check: FFmpeg isn't on PATH");
        return None;
    }
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries"])
        .args(["packet=pts_time,duration_time", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .unwrap();
    assert!(out.status.success(), "ffprobe failed on {path:?}");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut f = l.trim().trim_end_matches(',').split(',');
            let pts: f64 = f.next()?.parse().ok()?;
            let length: f64 = f.next()?.parse().ok()?;
            Some((pts, length))
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, l)| (l * SECOND as f64).round() as i64)
}

/// The length of the frame shown last, as Kiln decodes it.
fn last_length(path: &Path) -> i64 {
    let frames = frame_times(path);
    frames.iter().max_by_key(|f| f.0).unwrap().1
}

#[test]
fn a_constant_rate_mkv_says_its_frame_length() {
    let _one = serial();
    if common::no_mf() {
        return;
    }
    // h264.mkv says its rate (DefaultDuration): every frame keeps the
    // frame's length, and a copy into MKV says the same length.
    let src = video("h264.mkv");
    let (fixed, _) = decoded(&src);
    assert!(fixed.iter().all(|f| f.1 == FRAME), "{fixed:?}");
    let dir = tempfile::tempdir().unwrap();
    for (name, from) in [("h264.mkv", src), ("h264.mp4", video("h264.mp4"))] {
        let out = dir.path().join(format!("{name}-copy.mkv"));
        let report = run(&from, &out, VideoTarget::Mkv, &original());
        assert!(report.video_copied, "{name}: {report:?}");
        let (want, got) = (video::probe(&from).unwrap(), video::probe(&out).unwrap());
        assert!(!got.variable_rate, "{name}: the copy says its rate");
        let ns = |fps: Option<f64>| (1e9 / fps.unwrap()).round();
        assert_eq!(
            ns(got.fps),
            ns(want.fps),
            "{name}: the source's frame length"
        );
        let frames = frame_times(&out);
        assert!(frames.iter().all(|f| f.1 == FRAME), "{name}: {frames:?}");
        // The last frame's BlockDuration is to the millisecond.
        if let Some(ff) = ffprobe_last_length(&out) {
            assert!(
                (ff - last_length(&out)).abs() <= MS,
                "{name}: FFmpeg's {ff}"
            );
        }
    }
    // An encode of a constant rate says it too.
    let out = dir.path().join("h264.mkv-q65.mkv");
    run(
        &video("h264.mkv"),
        &out,
        VideoTarget::Mkv,
        &quality(65, VCodec::H264, EncoderChoice::Processor),
    );
    assert!(!video::probe(&out).unwrap().variable_rate);
}

#[test]
fn a_variable_rate_mkv_gives_its_last_frame_a_length() {
    let _one = serial();
    if common::no_mf() {
        return;
    }
    // vfr.mp4 (B-frames, its last frame 66.7 ms) copied into MKV: no
    // DefaultDuration, each frame lasting until the next block, and the
    // last its BlockDuration, which FFmpeg reads as Kiln does.
    let dir = tempfile::tempdir().unwrap();
    let src = video("vfr.mp4");
    let out = dir.path().join("vfr.mkv");
    let report = run(&src, &out, VideoTarget::Mkv, &original());
    assert!(report.video_copied, "{report:?}");
    assert!(video::probe(&out).unwrap().variable_rate);
    let blocks = block_times(&out);
    let frames = frame_times(&out);
    for (i, w) in frames.windows(2).enumerate() {
        assert_eq!(w[0].1, w[1].0 - w[0].0, "frame {i}'s length");
    }
    let times: Vec<i64> = frames.iter().map(|f| f.0).collect();
    assert_eq!(times, blocks);
    // The frame in a BlockGroup is still read as no key frame.
    let keys = |p: &Path| packets_of(p, 0).iter().map(|p| p.key).collect::<Vec<_>>();
    assert_eq!(keys(&out), keys(&src));
    let last = last_length(&out);
    assert!(
        (last - last_length(&src)).abs() <= MS,
        "the source's last length, to the millisecond: {last}"
    );
    if let Some(ff) = ffprobe_last_length(&out) {
        assert_eq!(ff, last, "FFmpeg reads the BlockDuration as Kiln does");
    }

    // Shaped as another writer may leave it, with no BlockDuration either:
    // the last frame lasts the gap before it (R80).
    let shown_last = packets_of(&out, 0)
        .into_iter()
        .max_by_key(|p| p.pts)
        .unwrap();
    let mut bytes = std::fs::read(&out).unwrap();
    let at = bytes
        .windows(shown_last.data.len())
        .position(|w| w == shown_last.data)
        .expect("the last frame's block")
        + shown_last.data.len();
    assert_eq!(bytes[at], 0x9B, "a BlockDuration after the block");
    bytes[at] = 0xEC; // Void
    let bare = dir.path().join("bare.mkv");
    std::fs::write(&bare, bytes).unwrap();
    let frames = frame_times(&bare);
    let n = frames.len();
    assert_eq!(frames[n - 1].1, frames[n - 1].0 - frames[n - 2].0);
}

#[test]
fn a_laced_picture_block_leaves_the_frames_from_it_the_decoders_times() {
    let _one = serial();
    if common::no_mf() {
        return;
    }
    // The 11th picture block made a laced block of one frame (Xiph lacing,
    // a frame count of 0 then the frame): the blocks can't be timed one by
    // one from there.
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("laced.mkv");
    let mut eleventh = Vec::new();
    rewritten(
        &src,
        |n, mut p| {
            if p.stream == 0 && n == 10 {
                eleventh = p.data.clone();
                p.data.insert(0, 0);
            }
            Some(p)
        },
        None,
    );
    let mut bytes = std::fs::read(&src).unwrap();
    let marked: Vec<u8> = [&[0u8][..], &eleventh[..32]].concat();
    let at = bytes
        .windows(marked.len())
        .position(|w| w == marked)
        .expect("the block");
    assert_eq!(bytes[at - 4], 0x81, "track 1's SimpleBlock");
    bytes[at - 1] |= 0x02;
    std::fs::write(&src, bytes).unwrap();

    let (frames, decoder_times) = decoded(&src);
    assert!(decoder_times, "the frames keep the decoder's times from it");
    let times: Vec<i64> = frames.iter().map(|f| f.0).collect();
    assert_eq!(times, block_times(&video("h264.mkv")));
}

#[test]
fn a_block_left_over_after_the_last_frame_is_noted() {
    let _one = serial();
    if common::no_mf() {
        return;
    }
    // A last picture block holding only an access unit delimiter: no
    // picture comes of it. Every frame lines up with its block; the blocks
    // are one more.
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("one-over.mkv");
    let delimiter = Packet {
        stream: 0,
        pts: 2 * SECOND,
        dts: 2 * SECOND,
        duration: FRAME,
        key: false,
        data: vec![0, 0, 0, 2, 0x09, 0xF0],
    };
    rewritten(&src, |_, p| Some(p), Some(delimiter));
    assert_eq!(block_times(&src).len(), 61);
    let (frames, decoder_times) = decoded(&src);
    assert!(decoder_times, "the counts differ");
    let times: Vec<i64> = frames.iter().map(|f| f.0).collect();
    assert_eq!(
        times,
        block_times(&video("h264.mkv")),
        "every frame lined up"
    );
    let out = dir.path().join("one-over.mp4");
    let report = run(
        &src,
        &out,
        VideoTarget::Mp4,
        &quality(65, VCodec::H264, EncoderChoice::Processor),
    );
    assert!(report.decoder_times, "{report:?}");
}
