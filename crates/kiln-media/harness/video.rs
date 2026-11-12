//! The video mode: holds Kiln's video encodes to FFmpeg's (libx264 and
//! libx265, at the CRF that matches Kiln's size) on VMAF, size and speed,
//! checks that copies keep every packet, and measures the sizes the
//! estimate in `video/tune.rs` is fitted to. See README.md.

mod ffmpeg;
mod fit;
mod probe;
mod rows;
mod vmaf;

pub use ffmpeg::{run as run_limited, PROBE_LIMIT, WORK_LIMIT};

use crate::report::{VideoReport, VideoRow};
use ffmpeg::FfCache;
use fit::Sample;
use kiln_media::video::{
    self, encoder_adapter, missing_extension, EncoderChoice, VCodec, VideoCodec, VideoInfo,
};
use kiln_media::Preset;
use probe::{Facts, Pkt, Tags};
use std::path::{Path, PathBuf};

/// The presets, with the CRF each search starts from: libx264 23, 20 and
/// 17, libx265 28, 24 and 20. Kiln's quality for each is the slider's mark.
const PRESETS: [(Preset, &str, f64, f64); 3] = [
    (Preset::Small, "Small", 23.0, 28.0),
    (Preset::High, "High", 20.0, 24.0),
    (Preset::Best, "Best", 17.0, 20.0),
];
const CODECS: [VCodec; 2] = [VCodec::H264, VCodec::Hevc];
/// The ends of the slider, encoded by Kiln alone for the calibration (I5).
const ENDS: [u8; 2] = [20, 90];
/// Every quality the calibration-only run encodes.
const CALIBRATION_QUALITIES: [u8; 5] = [20, 45, 65, 80, 90];

/// Kiln's VMAF may be this far under FFmpeg's at a size within `SIZE_BAND`.
pub const VMAF_TOLERANCE: f64 = 2.0;
/// How far Kiln's video stream may be from FFmpeg's, either way, for the
/// VMAF comparison to count as at a similar size.
pub const SIZE_BAND: f64 = 0.15;
/// The median absolute estimate error an encoder and codec may have.
pub const ESTIMATE_TARGET: f64 = 0.15;

/// How far Kiln's video stream is from FFmpeg's, as a fraction of FFmpeg's:
/// the one measure the CRF search aims at and the verdict reads (R83).
pub fn size_off(kiln_bytes: u64, ff_bytes: u64) -> f64 {
    kiln_bytes as f64 / ff_bytes.max(1) as f64 - 1.0
}

const VIDEO_EXTS: [&str; 5] = ["mp4", "mov", "m4v", "mkv", "webm"];

/// How an output relates to the source's dynamic range, which decides what
/// it is compared with and how FFmpeg encodes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Range {
    /// An SDR source: compared as 8-bit 4:2:0 with the clip.
    Plain,
    /// An HDR source written SDR: compared with Kiln's own q90 tone mapping
    /// (R71).
    ToneMapped,
    /// An HDR source kept HDR: compared in PQ (or HLG) at 10 bits.
    Hdr,
}

/// A clip ready to run: the file worked from (the clip, or its first
/// seconds) and what Kiln and ffprobe say it holds.
pub struct Clip {
    pub stem: String,
    pub path: PathBuf,
    pub info: VideoInfo,
    pub facts: Facts,
    /// The video packets with their payloads' MD5, and the sound's.
    pub packets: Vec<Pkt>,
    pub sound_packets: Vec<Pkt>,
    /// The decoded frames' times.
    pub frames: Vec<Pkt>,
    pub upright: (u32, u32),
    /// One frame's length, the length check's tolerance.
    pub frame: f64,
    pub sound_is_aac: bool,
    /// The label an untagged picture gets (R68's reading: BT.601 under 720
    /// lines, else BT.709); `None` for a clip that describes its colour.
    pub untagged_fix: Option<Tags>,
}

/// Where a clip's files go, how much of each clip to use, and whether files
/// are kept.
pub struct Ctx {
    pub work: PathBuf,
    pub max_secs: Option<f64>,
    /// Keep every file made (`KILN_HARNESS_VIDEO_KEEP=1`), for a closer look.
    pub keep: bool,
    /// Only these encoders and codecs, and no copies
    /// (`KILN_HARNESS_VIDEO_ONLY`); empty runs every row.
    pub only: Vec<(EncoderChoice, VCodec)>,
}

impl Ctx {
    /// Removes a file the run made, unless the run keeps them.
    fn done_with(&self, file: &Path) {
        if !self.keep {
            let _ = std::fs::remove_file(file);
        }
    }
}

fn codec_name(c: VCodec) -> &'static str {
    match c {
        VCodec::H264 => "H.264",
        VCodec::Hevc => "HEVC",
    }
}

fn encoder_name(e: EncoderChoice) -> &'static str {
    match e {
        EncoderChoice::Processor => "Processor",
        EncoderChoice::GraphicsCard => "Graphics card",
        EncoderChoice::Automatic => "Automatic",
    }
}

/// A video codec's name as people write it: "Theora", not `Other("theora")`.
fn video_codec_name(c: &VideoCodec) -> String {
    match c {
        VideoCodec::H264 => "H.264".into(),
        VideoCodec::Hevc => "HEVC".into(),
        VideoCodec::Av1 => "AV1".into(),
        VideoCodec::Vp9 => "VP9".into(),
        VideoCodec::Mpeg4 => "MPEG-4".into(),
        VideoCodec::Mjpeg => "Motion JPEG".into(),
        VideoCodec::Other(name) => {
            let mut chars = name.chars();
            chars.next().map_or_else(String::new, |f| {
                f.to_uppercase()
                    .chain(chars.flat_map(char::to_lowercase))
                    .collect()
            })
        }
    }
}

/// Every video clip in `dir`, by name; intermediate files (a `.tmp.` in the
/// name) are left out.
pub fn clips_in(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut list: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            p.is_file()
                && !name.contains(".tmp.")
                && p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| VIDEO_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        })
        .collect();
    list.sort();
    Ok(list)
}

/// Why Kiln can't decode `codec` on this PC, if it can't.
fn unreadable(codec: &VideoCodec) -> Option<String> {
    match codec {
        VideoCodec::H264 | VideoCodec::Hevc | VideoCodec::Av1 | VideoCodec::Vp9 => {
            missing_extension(codec).map(|(name, _)| format!("needs the {name} extension"))
        }
        other => Some(format!(
            "Kiln doesn't decode {} video",
            video_codec_name(other)
        )),
    }
}

/// Reads a clip and, over the cap, makes the copy of its first seconds that
/// is worked from. `None`, with a note, for a clip Kiln can't read; `None`,
/// with a miss, for one Kiln reads that can't be trimmed or read back by
/// ffprobe, since then nothing of it was measured (R72).
fn prepare(
    ctx: &Ctx,
    path: &Path,
    notes: &mut Vec<String>,
    misses: &mut Vec<String>,
) -> Option<Clip> {
    let file = path.file_name().and_then(|s| s.to_str()).unwrap_or("clip");
    let stem = crate::safe_name(path.file_stem().and_then(|s| s.to_str()).unwrap_or("clip"));
    let mut info = match video::probe(path) {
        Ok(i) => i,
        Err(e) => {
            notes.push(format!("{file}: skipped, Kiln can't read it ({e})"));
            return None;
        }
    };
    if let Some(why) = unreadable(&info.codec) {
        notes.push(format!("{file}: skipped, {why}"));
        return None;
    }
    let mut path = path.to_path_buf();
    if let Some(max) = ctx.max_secs {
        if info.duration.is_none_or(|d| d > max) {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("mp4");
            let cut = ctx.work.join(format!("{stem}.first.{ext}"));
            let made = ffmpeg::trim(&path, &cut, max)
                .and_then(|_| video::probe(&cut).map_err(|e| e.to_string()));
            match made {
                Ok(i) => {
                    notes.push(format!(
                        "{file}: first {max} s only (copied packet for packet; that copy is the source)"
                    ));
                    path = cut;
                    info = i;
                }
                Err(e) => {
                    misses.push(format!("{file}: not measured, can't trim it ({e})"));
                    return None;
                }
            }
        }
    }
    type Read = (Facts, Vec<Pkt>, Vec<Pkt>, Vec<Pkt>);
    let read = || -> Result<Read, String> {
        Ok((
            probe::facts(&path)?,
            probe::packets(&path, "v:0", true)?,
            probe::packets(&path, "a:0", true)?,
            probe::frames(&path)?,
        ))
    };
    let (facts, packets, sound_packets, frames) = match read() {
        Ok(r) => r,
        Err(e) => {
            misses.push(format!("{file}: not measured, {e}"));
            return None;
        }
    };
    let (w, h) = facts.upright();
    let frame = info
        .fps
        .filter(|f| *f > 0.0)
        .map(|f| 1.0 / f)
        .or_else(|| Some(probe::span(&frames)? / frames.len().max(1) as f64))
        .unwrap_or(1.0 / 30.0);
    let sound_is_aac = info
        .sound
        .as_ref()
        .is_some_and(|s| matches!(s.codec, kiln_media::Codec::Aac | kiln_media::Codec::HeAac));
    let untagged_fix = facts.tags.untagged().then(|| {
        if facts.height < 720 {
            Tags::bt601()
        } else {
            Tags::bt709()
        }
    });
    Some(Clip {
        stem,
        path,
        info,
        packets,
        sound_packets,
        frames,
        upright: (w & !1, h & !1),
        frame,
        sound_is_aac,
        untagged_fix,
        facts,
    })
}

/// The encoders that write `codec` on this PC: the processor, and the
/// graphics card when one starts; with `only`, those it names.
fn encoders(codec: VCodec, only: &[(EncoderChoice, VCodec)]) -> Vec<EncoderChoice> {
    let mut list = vec![EncoderChoice::Processor];
    if encoder_adapter(codec, EncoderChoice::GraphicsCard).is_some() {
        list.push(EncoderChoice::GraphicsCard);
    }
    list.retain(|&e| only.is_empty() || only.contains(&(e, codec)));
    list
}

/// `KILN_HARNESS_VIDEO_ONLY`: a comma-separated list of `processor-h264`,
/// `processor-hevc`, `card-h264` and `card-hevc`, the rows to run. Unknown
/// names are an error, so a typo doesn't run nothing.
pub fn only_from(text: &str) -> Result<Vec<(EncoderChoice, VCodec)>, String> {
    text.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| match s.to_ascii_lowercase().as_str() {
            "processor-h264" => Ok((EncoderChoice::Processor, VCodec::H264)),
            "processor-hevc" => Ok((EncoderChoice::Processor, VCodec::Hevc)),
            "card-h264" => Ok((EncoderChoice::GraphicsCard, VCodec::H264)),
            "card-hevc" => Ok((EncoderChoice::GraphicsCard, VCodec::Hevc)),
            _ => Err(format!(
                "KILN_HARNESS_VIDEO_ONLY: {s:?} is not processor-h264, processor-hevc, card-h264 or card-hevc"
            )),
        })
        .collect()
}

/// Every row for one clip: each codec × encoder × preset against FFmpeg,
/// the ends of the slider for the calibration, then the copies. With
/// `calibrate_only`, Kiln alone at every calibration quality.
fn run_clip(
    ctx: &Ctx,
    path: &Path,
    calibrate_only: bool,
    report: &mut VideoReport,
    samples: &mut Vec<Sample>,
) {
    let Some(clip) = prepare(ctx, path, &mut report.notes, &mut report.misses) else {
        return;
    };
    let mut cache = FfCache::default();
    let mut tones = rows::ToneRefs::new();
    for codec in CODECS {
        for encoder in encoders(codec, &ctx.only) {
            if calibrate_only {
                for q in CALIBRATION_QUALITIES {
                    eprintln!(
                        "harness: {} {} q{q} {}",
                        clip.stem,
                        codec_name(codec),
                        encoder_name(encoder)
                    );
                    rows::calibration_encode(
                        ctx,
                        &clip,
                        codec,
                        encoder,
                        q,
                        samples,
                        &mut report.misses,
                    );
                }
                continue;
            }
            for (preset, name, crf264, crf265) in PRESETS {
                let seed = if codec == VCodec::H264 {
                    crf264
                } else {
                    crf265
                };
                eprintln!(
                    "harness: {} {} {name} {}",
                    clip.stem,
                    codec_name(codec),
                    encoder_name(encoder)
                );
                if let Some(row) = rows::encode_row(
                    ctx,
                    &clip,
                    codec,
                    (preset, name, seed),
                    encoder,
                    &mut cache,
                    &mut tones,
                    samples,
                    &mut report.notes,
                ) {
                    report.rows.push(row);
                }
            }
            for q in ENDS {
                rows::calibration_encode(
                    ctx,
                    &clip,
                    codec,
                    encoder,
                    q,
                    samples,
                    &mut report.misses,
                );
            }
        }
    }
    if !calibrate_only && ctx.only.is_empty() {
        eprintln!("harness: {} copies", clip.stem);
        rows::copy_rows(ctx, &clip, &mut report.copies, &mut report.misses);
    }
}

/// The video mode over `dir`: every clip, then the calibration. Each clip's
/// files go in a folder of their own, removed when the clip is done unless
/// `keep` names a folder to keep them in; else the folders sit in a temp
/// folder under the build's own. With `calibrate_only`, Kiln alone encodes
/// every clip at every calibration quality and only the calibration is
/// reported.
pub fn run_folder(
    dir: &Path,
    max_secs: Option<f64>,
    keep: Option<PathBuf>,
    calibrate_only: bool,
    only: &[(EncoderChoice, VCodec)],
) -> Result<VideoReport, String> {
    let clips = clips_in(dir).map_err(|e| format!("can't read {}: {e}", dir.display()))?;
    let mut report = VideoReport::default();
    if !only.is_empty() {
        let names: Vec<String> = only
            .iter()
            .map(|&(e, c)| format!("{} {}", codec_name(c), encoder_name(e)))
            .collect();
        // A filtered run never reads as a whole one.
        report.misses.push(format!(
            "partial run (KILN_HARNESS_VIDEO_ONLY): only {}, and no copies",
            names.join(", ")
        ));
    }
    if !calibrate_only {
        match ffmpeg::missing_features() {
            Ok(m) if m.is_empty() => {}
            Ok(m) => report.misses.push(format!(
                "FFmpeg lacks {}, so those comparisons can't be made (R72)",
                m.join(", ")
            )),
            Err(e) => report
                .misses
                .push(format!("FFmpeg's features can't be listed: {e}")),
        }
    }
    let tmp = Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(tmp).map_err(|e| e.to_string())?;
    let temp = tempfile::Builder::new()
        .prefix("video-harness")
        .tempdir_in(tmp)
        .map_err(|e| e.to_string())?;
    let root = match &keep {
        Some(folder) => {
            report
                .notes
                .push(format!("Files made are kept in {}", folder.display()));
            folder.clone()
        }
        None => temp.path().to_path_buf(),
    };
    let hardware: Vec<&str> = CODECS
        .iter()
        .filter(|&&c| encoder_adapter(c, EncoderChoice::GraphicsCard).is_some())
        .map(|&c| codec_name(c))
        .collect();
    report.notes.push(if hardware.is_empty() {
        "Graphics card encoders: none on this PC, so only the processor's rows".into()
    } else {
        format!("Graphics card encoders: {}", hardware.join(", "))
    });
    let mut samples = Vec::new();
    for path in &clips {
        eprintln!("harness: {}", path.display());
        let stem = crate::safe_name(path.file_stem().and_then(|s| s.to_str()).unwrap_or("clip"));
        let ctx = Ctx {
            work: root.join(&stem),
            max_secs,
            keep: keep.is_some(),
            only: only.to_vec(),
        };
        if let Err(e) = std::fs::create_dir_all(&ctx.work) {
            report.misses.push(format!("{stem}: no work folder ({e})"));
            continue;
        }
        let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_clip(&ctx, path, calibrate_only, &mut report, &mut samples)
        }));
        if let Err(panic) = ran {
            let mut row = VideoRow::new(&stem, "every codec", "every preset", "every encoder");
            row.misses.push(format!(
                "the harness panicked: {}",
                crate::panic_text(&*panic)
            ));
            row.error = true;
            report.rows.push(row);
        }
        // M1: a clip's files go when it is done.
        if !ctx.keep {
            let _ = std::fs::remove_dir_all(&ctx.work);
        }
    }
    // R72: clips given and nothing measured is a miss, not a pass.
    if calibrate_only && samples.is_empty() {
        report.misses.push(format!(
            "no Kiln encode was measured from the clips in {}",
            dir.display()
        ));
    }
    if !calibrate_only && report.rows.is_empty() {
        report.misses.push(format!(
            "no encode rows were made from the clips in {}",
            dir.display()
        ));
    }
    report.encodes = samples.len();
    let mut misses = Vec::new();
    report.calibration = fit::calibration(&samples, &mut misses);
    report.misses.extend(misses);
    Ok(report)
}

// ------------------------------------------------------------------- checks

pub use fit::the_bpp_fit_finds_a_table_with_no_error;
pub use rows::the_verdict_is_read_right;

pub fn vmaf_spans_and_packets_are_read_right() {
    let log = "frame=  10 fps=0.0\n[Parsed_libvmaf_4 @ 0000029c] VMAF score: 95.868295\n";
    assert_eq!(vmaf::vmaf_of(log), Some(95.868295));
    assert_eq!(vmaf::vmaf_of("no score here"), None);
    let p = |pts: f64, dur: f64| Pkt {
        pts: Some(pts),
        dur: Some(dur),
        ..Pkt::default()
    };
    // Decode order, B-frames out of order: from 0 to the last shown's end.
    let packets = [p(0.0, 0.04), p(0.12, 0.04), p(0.04, 0.04), p(0.08, 0.04)];
    assert!((probe::span(&packets).unwrap() - 0.16).abs() < 1e-9);
    assert_eq!(probe::span(&[]), None);
    // The nearest picture by mean squared difference.
    let pictures = [vec![0u8, 0], vec![10, 9], vec![50, 50]];
    assert_eq!(vmaf::nearest(&[10, 10], &pictures), Some(1));
    assert_eq!(vmaf::nearest(&[10, 10], &[]), None);

    // A copy may drop hidden lead-in from the front, nothing else.
    let h = |t: f64, hash: &str, key: bool| Pkt {
        pts: Some(t),
        dur: Some(0.04),
        hash: hash.into(),
        key,
        ..Pkt::default()
    };
    let src = [
        h(-0.12, "a", true),
        h(-0.08, "b", false),
        h(-0.04, "c", true),
        h(0.0, "d", false),
        h(0.04, "e", false),
    ];
    let shown = [p(0.0, 0.04), p(0.04, 0.04)];
    assert!((probe::hidden_lead_in(&src, &shown) - 0.04).abs() < 1e-9);
    assert_eq!(probe::compare_packets(&src, &src, 0.0), (0, 0, None));
    let (dropped, differ, problem) = probe::compare_packets(&src, &src[2..], 0.0);
    assert_eq!((dropped, differ, problem), (2, 0, None));
    // Dropping a shown packet, changing a payload, or losing the tail: misses.
    assert!(probe::compare_packets(&src, &src[4..], 0.0).2.is_some());
    let mut changed = src.to_vec();
    changed[3].hash = "x".into();
    assert_eq!(probe::compare_packets(&src, &changed, 0.0).1, 1);
    assert!(probe::compare_packets(&src, &src[..4], 0.0).2.is_some());

    // Tags: untagged is nothing said of the colour; R68's labels.
    assert!(Tags::default().untagged());
    assert!(!Tags::bt601().untagged());
    assert_eq!(Tags::bt601().matrix(), "smpte170m");
    assert_eq!(
        Tags::bt709().setparams(),
        "setparams=range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709"
    );
    assert_eq!(
        video_codec_name(&VideoCodec::Other("theora".into())),
        "Theora"
    );
}

pub fn the_crf_search_steps_toward_the_size() {
    let ln = |v: f64| v.ln();
    // Nothing known: the seed.
    assert_eq!(ffmpeg::next_crf(&[], ln(1000.0), 23.0), Some(23.0));
    // Too big at 23: up by the halving slope, 6 CRF per halving.
    let c = ffmpeg::next_crf(&[(23.0, ln(2000.0))], ln(1000.0), 23.0).unwrap();
    assert!((c - 29.0).abs() < 0.01, "{c}");
    // One over and one under: log-linear between them.
    let c = ffmpeg::next_crf(&[(20.0, ln(4000.0)), (26.0, ln(1000.0))], ln(2000.0), 23.0).unwrap();
    assert!((c - 23.0).abs() < 0.01, "{c}");
    // Already at 51 and still too big: nowhere to go.
    assert_eq!(
        ffmpeg::next_crf(&[(51.0, ln(5000.0))], ln(1000.0), 23.0),
        None
    );
    // R83: the band is Kiln's size against FFmpeg's, as the verdict reads
    // it: FFmpeg at 870 against Kiln's 1000 is +14.9%, in the band, and the
    // verdict agrees; at 869 it is out for both.
    assert!(size_off(1000, 870) <= SIZE_BAND);
    assert!(rows::judge(90.0, 1000, 90.0, 870).is_none());
    assert!(size_off(1000, 869) > SIZE_BAND);
    assert!(rows::judge(90.0, 1000, 90.0, 869).is_some());
}

pub fn only_the_rows_asked_for_are_run() {
    use EncoderChoice::{GraphicsCard, Processor};
    assert_eq!(only_from(""), Ok(vec![]));
    assert_eq!(
        only_from("processor-h264, CARD-hevc"),
        Ok(vec![
            (Processor, VCodec::H264),
            (GraphicsCard, VCodec::Hevc)
        ])
    );
    assert!(only_from("processor-av1").is_err());
    // Empty runs everything this PC has; a list only what it names.
    assert!(encoders(VCodec::H264, &[]).contains(&Processor));
    assert_eq!(
        encoders(VCodec::H264, &[(Processor, VCodec::Hevc)]),
        Vec::<EncoderChoice>::new()
    );
    assert_eq!(
        encoders(VCodec::Hevc, &[(Processor, VCodec::Hevc)]),
        vec![Processor]
    );
}

/// I4: a two-second clip FFmpeg makes goes through one encode row and the
/// copy rows. It checks that everything is measured, not how good it is.
/// Skipped without FFmpeg or with an FFmpeg that lacks what the mode needs.
pub fn a_tiny_clip_goes_through_the_video_rows() {
    if ffmpeg::version().is_none() {
        println!("    skipped: FFmpeg not found");
        return;
    }
    match ffmpeg::missing_features() {
        Ok(m) if m.is_empty() => {}
        Ok(m) => {
            println!("    skipped: FFmpeg lacks {}", m.join(", "));
            return;
        }
        Err(e) => {
            println!("    skipped: {e}");
            return;
        }
    }
    // Kiln's video engine is Media Foundation's; its AAC encoder stands in
    // for it being there.
    if !cfg!(windows) || !kiln_media::mf_codecs().aac_enc {
        println!("    skipped: no Media Foundation");
        return;
    }
    let tmp = Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(tmp).unwrap();
    let dir = tempfile::Builder::new()
        .prefix("video-check")
        .tempdir_in(tmp)
        .unwrap();
    let clip_path = dir.path().join("tiny.mp4");
    ffmpeg::test_clip(&clip_path).expect("FFmpeg makes the test clip");
    let ctx = Ctx {
        work: dir.path().join("work"),
        max_secs: None,
        keep: false,
        only: Vec::new(),
    };
    std::fs::create_dir_all(&ctx.work).unwrap();
    let (mut notes, mut prepared) = (Vec::new(), Vec::new());
    let clip = prepare(&ctx, &clip_path, &mut notes, &mut prepared).expect("the test clip reads");
    assert!(prepared.is_empty(), "{prepared:?}");
    let (mut cache, mut tones, mut samples) = (FfCache::default(), rows::ToneRefs::new(), vec![]);
    let Some(row) = rows::encode_row(
        &ctx,
        &clip,
        VCodec::H264,
        (Preset::Small, "Small", 23.0),
        EncoderChoice::Processor,
        &mut cache,
        &mut tones,
        &mut samples,
        &mut notes,
    ) else {
        println!("    skipped: this PC's processor has no H.264 encoder");
        return;
    };
    assert!(!row.error, "{:?}", row.misses);
    assert!(row.kiln.vmaf.is_some(), "{:?}", row.misses);
    let ff = row.ffmpeg.as_ref().expect("FFmpeg matched Kiln's size");
    assert!(ff.vmaf.is_some(), "{:?}", row.misses);
    assert!(row.setting.contains("CRF"), "{}", row.setting);
    assert!(row.size_off.is_some_and(|o| o.abs() <= SIZE_BAND));
    assert_eq!(samples.len(), 1);
    let (mut copies, mut misses) = (Vec::new(), Vec::new());
    rows::copy_rows(&ctx, &clip, &mut copies, &mut misses);
    assert!(misses.is_empty(), "{misses:?}");
    assert_eq!(copies.len(), 2);
    for c in &copies {
        assert!(c.misses.is_empty(), "{}: {:?}", c.route, c.misses);
        assert!(c.video_copied && c.sound_copied, "{}", c.route);
    }
}
