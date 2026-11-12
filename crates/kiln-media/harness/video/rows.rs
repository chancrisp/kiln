//! The rows: an encode against FFmpeg's at a matched size, the encodes made
//! only for the calibration, Kiln's tone-mapped reference for an HDR clip,
//! and the copies.

use super::ffmpeg::{self, Basis, FfCache};
use super::fit::Sample;
use super::probe::{self, Tags};
use super::vmaf::{self, Model, Reference};
use super::{codec_name, encoder_name, Clip, Ctx, Range, SIZE_BAND, VMAF_TOLERANCE};
use crate::report::{CopyRow, VSide, VideoRow};
use kiln_media::video::tune::{
    can_copy_video, estimate_bytes, preset_step, sound_kbps, steps, written_fps, written_size,
};
use kiln_media::video::{
    self, EncoderChoice, Hdr, JobReport, VCodec, VideoCodec, VideoInfo, VideoSetting, VideoTarget,
    VideoTune,
};
use kiln_media::{MediaError, Preset};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::SPEED_WARN;

/// Kiln's tone-mapped q90 references for an HDR clip, by whether the
/// graphics card made them (R71).
pub type ToneRefs = HashMap<bool, Result<Reference, String>>;

/// The verdict against FFmpeg: Kiln's VMAF at least FFmpeg's − 2 at a size
/// within ±15%, or Kiln better on both. `None` when it passes.
pub fn judge(kiln_vmaf: f64, kiln_bytes: u64, ff_vmaf: f64, ff_bytes: u64) -> Option<String> {
    let off = super::size_off(kiln_bytes, ff_bytes);
    let better_both = kiln_vmaf >= ff_vmaf && kiln_bytes <= ff_bytes;
    let near = off.abs() <= SIZE_BAND && kiln_vmaf >= ff_vmaf - VMAF_TOLERANCE;
    if better_both || near {
        return None;
    }
    Some(if off.abs() <= SIZE_BAND {
        format!(
            "VMAF {kiln_vmaf:.2} is {:.2} under FFmpeg's {ff_vmaf:.2} at a size {:+.1}% from it",
            ff_vmaf - kiln_vmaf,
            off * 100.0
        )
    } else {
        format!(
            "size {:+.1}% from FFmpeg's (outside ±{:.0}%) with VMAF {kiln_vmaf:.2} against {ff_vmaf:.2}",
            off * 100.0,
            SIZE_BAND * 100.0
        )
    })
}

/// The length check: the picture's span within one frame of the source's.
fn check_length(misses: &mut Vec<String>, clip: &Clip, length: Option<f64>) {
    match (probe::span(&clip.frames), length) {
        (Some(s), Some(k)) if (k - s).abs() > clip.frame + 1e-3 => misses.push(format!(
            "length {k:.3} s is {:+.3} s from the source's {s:.3} s (one frame is {:.3} s)",
            k - s,
            clip.frame
        )),
        (_, None) => misses.push("its length can't be read".into()),
        (None, _) => misses.push("the source's length can't be read".into()),
        _ => {}
    }
}

/// How far an output's mean U or V may be from its reference's, in 8-bit
/// levels.
pub const CHROMA_TOLERANCE: f64 = 12.0;

/// The colour check against `reference` (VMAF reads only the luma): the
/// mean U and V within [`CHROMA_TOLERANCE`] of the reference's.
fn check_chroma(
    misses: &mut Vec<String>,
    out: &Path,
    tags: &Tags,
    clip: &Clip,
    reference: &Reference,
) {
    match vmaf::chroma_offset(out, tags, clip.untagged_fix.as_ref(), reference) {
        Ok((u, v)) if u.abs() <= CHROMA_TOLERANCE && v.abs() <= CHROMA_TOLERANCE => {}
        Ok((u, v)) => misses.push(format!(
            "colour: mean U is {u:+.1} and V {v:+.1} levels from the reference's (limit \
             {CHROMA_TOLERANCE:.0}); VMAF reads only the luma"
        )),
        Err(e) => misses.push(format!("the colour can't be compared: {e}")),
    }
}

/// The first-frame check against `reference`: the output must start on its
/// first frame.
fn check_first_frame(
    misses: &mut Vec<String>,
    out: &Path,
    tags: &Tags,
    clip: &Clip,
    reference: &Reference,
) {
    match vmaf::first_frame_offset(out, tags, clip.untagged_fix.as_ref(), reference) {
        Ok(0) => {}
        Ok(k) => misses.push(format!(
            "the first frame is the source's frame {} (counting from 1): {k} lost at the start, \
             and every later frame is shown {k} frame(s) early",
            k + 1
        )),
        Err(e) => misses.push(format!("the first frame can't be compared: {e}")),
    }
}

/// Kiln's quality for a preset: the slider's mark it lands on.
fn quality_of(preset: Preset, info: &VideoInfo, tune: &VideoTune) -> Option<u8> {
    let list = steps(VideoTarget::Mp4, info, tune);
    match list.get(preset_step(preset, info, tune))?.setting {
        VideoSetting::Quality(q) => Some(q),
        _ => None,
    }
}

/// The range an output is in, given the source's and its transfer.
fn range_of(clip: &Clip, tags: &Tags) -> Range {
    if clip.info.hdr == Hdr::None {
        Range::Plain
    } else if matches!(tags.transfer.as_str(), "smpte2084" | "arib-std-b67") {
        Range::Hdr
    } else {
        Range::ToneMapped
    }
}

/// The tags Kiln must write (R40, R68): BT.2020 with PQ or HLG for HDR kept;
/// BT.709 for HDR tone-mapped and for any source that describes its colour
/// or has 720 lines or more; BT.601 (SMPTE 170M) for an untagged source
/// under 720 lines. Limited range always.
fn expected_tags(clip: &Clip, range: Range) -> Tags {
    match range {
        Range::Hdr => Tags::hdr(clip.info.hdr),
        Range::Plain if clip.facts.tags.untagged() && clip.facts.height < 720 => Tags::bt601(),
        _ => Tags::bt709(),
    }
}

/// Kiln converting `clip` to `out`, which is removed first (a kept folder
/// may hold the last run's). Returns the report and the wall time.
fn kiln(
    clip: &Path,
    out: &Path,
    target: VideoTarget,
    tune: &VideoTune,
) -> (kiln_media::Result<JobReport>, f64) {
    let _ = std::fs::remove_file(out);
    let start = Instant::now();
    let made = video::convert(clip, out, target, Preset::High, tune, &mut |_| {}, &|| {
        false
    });
    (made, start.elapsed().as_secs_f64())
}

/// True for what this PC can't do at all (no encoder for the codec there),
/// which is a note, not a row.
fn cant_on_this_pc(e: &MediaError) -> bool {
    matches!(
        e,
        MediaError::NeedsExtensionToWrite { .. } | MediaError::NoHardwareEncoder { .. }
    )
}

/// What a Kiln encode measures, as a side of a row.
struct Measured {
    side: VSide,
    tags: Tags,
}

fn measure(out: &Path, wall: f64) -> Result<Measured, String> {
    let facts = probe::facts(out)?;
    let packets = probe::packets(out, "v:0", false)?;
    let frames = probe::frames(out)?;
    Ok(Measured {
        side: VSide {
            video_bytes: probe::bytes(&packets),
            file_bytes: std::fs::metadata(out).map_or(0, |m| m.len()),
            secs: wall,
            vmaf: None,
            length: probe::span(&frames),
            frames: frames.len(),
        },
        tags: facts.tags,
    })
}

/// The calibration's sample for a Kiln encode of `clip` at quality `q`.
fn sample(
    clip: &Clip,
    tune: &VideoTune,
    q: u8,
    hardware: bool,
    side: &VSide,
) -> Option<(Sample, f64)> {
    let (w, h) = written_size(&clip.info, tune);
    let fps = written_fps(&clip.info, tune).unwrap_or(30.0);
    let secs = clip.info.duration.filter(|s| *s > 0.0)?;
    let est = estimate_bytes(
        VideoTarget::Mp4,
        &VideoSetting::Quality(q),
        &clip.info,
        tune,
    )?;
    if side.file_bytes == 0 {
        return None;
    }
    let error_now = (side.file_bytes as f64 - est as f64) / side.file_bytes as f64;
    let pixels_per_sec = w as f64 * h as f64 * fps;
    Some((
        Sample {
            clip: clip.stem.clone(),
            codec: tune.codec,
            quality: q,
            hardware,
            bpp: side.video_bytes as f64 * 8.0 / (pixels_per_sec * secs),
            pixels_per_sec,
            secs,
            sound_kbps: sound_kbps(VideoTarget::Mp4, &clip.info, tune),
            file_bytes: side.file_bytes,
            source_bpp: kiln_media::video::tune::source_bpp(&clip.info),
            error_now,
        },
        error_now,
    ))
}

fn out_name(clip: &Clip, codec: VCodec, what: &str, encoder: EncoderChoice) -> String {
    format!(
        "{}.{}.{what}.{}.kiln.mp4",
        clip.stem,
        codec_name(codec).replace('.', ""),
        crate::safe_name(encoder_name(encoder))
    )
}

/// A Kiln encode made only for the calibration (I5): quality `q`, no FFmpeg
/// and no VMAF. A Kiln error is a miss in `misses`.
pub fn calibration_encode(
    ctx: &Ctx,
    clip: &Clip,
    codec: VCodec,
    encoder: EncoderChoice,
    q: u8,
    samples: &mut Vec<Sample>,
    misses: &mut Vec<String>,
) {
    let tune = VideoTune {
        setting: Some(VideoSetting::Quality(q)),
        codec,
        encoder,
        ..VideoTune::default()
    };
    let out = ctx
        .work
        .join(out_name(clip, codec, &format!("q{q}"), encoder));
    let what = format!(
        "{} {} quality {q} on the {}",
        clip.stem,
        codec_name(codec),
        encoder_name(encoder).to_ascii_lowercase()
    );
    match kiln(&clip.path, &out, VideoTarget::Mp4, &tune) {
        (Err(e), _) if cant_on_this_pc(&e) => {}
        (Err(e), _) => misses.push(format!("{what}: Kiln failed: {e}")),
        (Ok(report), wall) => match measure(&out, wall) {
            Err(e) => misses.push(format!("{what}: can't be read back: {e}")),
            Ok(m) => {
                if let Some((s, _)) = sample(clip, &tune, q, report.hardware, &m.side) {
                    samples.push(s);
                }
            }
        },
    }
    ctx.done_with(&out);
}

/// Kiln's own tone-mapped picture of an HDR clip (R71): its q90 H.264
/// encode on the graphics card (`hardware`) or the processor, checked to
/// start on the clip's first frame. On first making it, notes how far its
/// tone curve is from Hable's (VMAF, the model that doesn't reward more
/// contrast), for the maintainer; no row is judged on that.
pub fn tone_reference(
    ctx: &Ctx,
    clip: &Clip,
    hardware: bool,
    notes: &mut Vec<String>,
) -> Result<Reference, String> {
    let encoder = if hardware {
        EncoderChoice::GraphicsCard
    } else {
        EncoderChoice::Processor
    };
    let tune = VideoTune {
        setting: Some(VideoSetting::Quality(90)),
        codec: VCodec::H264,
        encoder,
        ..VideoTune::default()
    };
    let out = ctx
        .work
        .join(out_name(clip, VCodec::H264, "toneref", encoder));
    let report = kiln(&clip.path, &out, VideoTarget::Mp4, &tune)
        .0
        .map_err(|e| format!("Kiln's q90 reference failed: {e}"))?;
    if report.hardware != hardware {
        return Err("Kiln's q90 reference came from the other encoder".into());
    }
    let facts = probe::facts(&out)?;
    if facts.tags.transfer != "bt709" {
        return Err(format!(
            "Kiln's q90 reference isn't SDR: tagged {}",
            facts.tags.label()
        ));
    }
    let hable = Reference::hable(clip);
    match vmaf::first_frame_offset(&out, &facts.tags, None, &hable) {
        Ok(0) => {}
        Ok(k) => {
            return Err(format!(
                "Kiln's q90 reference starts on the source's frame {}",
                k + 1
            ))
        }
        Err(e) => return Err(format!("Kiln's q90 reference can't be compared: {e}")),
    }
    let curve = vmaf::vmaf(&out, &facts.tags, None, &hable, Model::NoEnhancement);
    let colour = vmaf::chroma_offset(&out, &facts.tags, None, &hable).map_or_else(
        |e| format!("its colour can't be compared ({e})"),
        |(u, v)| format!("its mean U is {u:+.1} and V {v:+.1} levels from Hable's"),
    );
    notes.push(format!(
        "{}: tone curve, not judged: Kiln's q90 tone mapping on the {} against Hable's (zscale) \
         scores VMAF {} with the vmaf_v0.6.1neg model; {colour}",
        clip.stem,
        encoder_name(encoder).to_ascii_lowercase(),
        curve.map_or_else(|e| format!("n/a ({e})"), |v| format!("{v:.2}"))
    ));
    Ok(Reference::file(&out, facts.tags, clip.upright))
}

/// One encode row: Kiln at the preset's quality, then FFmpeg at the CRF
/// whose size matches Kiln's (R67), both scored against the same reference.
/// `None` when this PC has no encoder for it (a note says so).
#[allow(clippy::too_many_arguments)]
pub fn encode_row(
    ctx: &Ctx,
    clip: &Clip,
    codec: VCodec,
    preset: (Preset, &'static str, f64),
    encoder: EncoderChoice,
    cache: &mut FfCache,
    tones: &mut ToneRefs,
    samples: &mut Vec<Sample>,
    notes: &mut Vec<String>,
) -> Option<VideoRow> {
    let (preset, preset_name, seed) = preset;
    let mut row = VideoRow::new(
        &clip.stem,
        codec_name(codec),
        preset_name,
        encoder_name(encoder),
    );
    let base = VideoTune {
        codec,
        encoder,
        ..VideoTune::default()
    };
    let Some(q) = quality_of(preset, &clip.info, &base) else {
        row.misses
            .push("the preset lands on no quality step".into());
        row.error = true;
        return Some(row);
    };
    let tune = VideoTune {
        setting: Some(VideoSetting::Quality(q)),
        ..base
    };
    let library = if codec == VCodec::H264 {
        "x264"
    } else {
        "x265"
    };
    row.setting = format!("Quality {q}");
    row.source_length = probe::span(&clip.frames);
    row.source_frames = clip.frames.len();

    // Kiln.
    let out = ctx.work.join(out_name(clip, codec, preset_name, encoder));
    let (made, wall) = kiln(&clip.path, &out, VideoTarget::Mp4, &tune);
    let report = match made {
        Ok(r) => r,
        Err(e) if cant_on_this_pc(&e) => {
            let note = format!(
                "{} on the {}: skipped on this PC ({e})",
                codec_name(codec),
                encoder_name(encoder).to_ascii_lowercase()
            );
            if !notes.contains(&note) {
                notes.push(note);
            }
            return None;
        }
        Err(e) => {
            row.misses.push(format!("Kiln failed: {e}"));
            row.error = true;
            return Some(row);
        }
    };
    row.hardware = report.hardware;
    // R72: a graphics-card row the processor wrote measures the wrong thing.
    if encoder == EncoderChoice::GraphicsCard && (!report.hardware || report.fell_back) {
        row.misses
            .push("the graphics card was asked for, but the processor wrote it".into());
    }
    if encoder == EncoderChoice::Processor && report.hardware {
        row.misses
            .push("the processor was asked for, but a graphics card wrote it".into());
    }
    let m = match measure(&out, wall) {
        Ok(m) => m,
        Err(e) => {
            row.misses
                .push(format!("Kiln's file can't be read back: {e}"));
            row.error = true;
            return Some(row);
        }
    };
    row.kiln = m.side;
    let range = range_of(clip, &m.tags);

    // M9 (R35): HEVC from HDR stays HDR on the graphics card; everything
    // else from HDR is tone-mapped to SDR.
    if clip.info.hdr != Hdr::None {
        let want = if codec == VCodec::Hevc && report.hardware {
            Range::Hdr
        } else {
            Range::ToneMapped
        };
        if range != want {
            row.misses.push(format!(
                "the HDR clip was written {range:?}, where R35 says {want:?}"
            ));
        }
        row.warnings.push(match range {
            Range::Hdr => "HDR kept, compared in PQ at 10 bits: VMAF is indicative".into(),
            _ => "HDR tone-mapped, compared with Kiln's own q90 tone mapping (R71)".to_string(),
        });
    }
    // R73: the tags are Kiln's own, checked here, not overwritten.
    let want = expected_tags(clip, range);
    if m.tags.clone().or_tv() != want {
        row.misses.push(format!(
            "tagged {}, where R40/R68 say {}",
            m.tags.label(),
            want.label()
        ));
    }

    // What it is compared with, and what FFmpeg encodes from.
    let source = Reference::source(clip, range == Range::Hdr);
    let (reference, basis, input) = match range {
        Range::ToneMapped => {
            let made = tones
                .entry(report.hardware)
                .or_insert_with(|| tone_reference(ctx, clip, report.hardware, notes));
            match made {
                Ok(r) => (r.clone(), Basis::ToneRef(report.hardware), r.path.clone()),
                Err(e) => {
                    row.misses
                        .push(format!("no tone-mapped reference to compare with: {e}"));
                    ctx.done_with(&out);
                    return Some(row);
                }
            }
        }
        r => (source.clone(), Basis::Source(r), clip.path.clone()),
    };

    // The picture: VMAF, length, frames and the first frame.
    match vmaf::vmaf(
        &out,
        &m.tags,
        clip.untagged_fix.as_ref(),
        &reference,
        Model::Default,
    ) {
        Ok(v) => row.kiln.vmaf = Some(v),
        Err(e) => row.misses.push(format!("Kiln's VMAF not measured: {e}")),
    }
    check_length(&mut row.misses, clip, row.kiln.length);
    if row.kiln.frames != clip.frames.len() {
        row.warnings.push(format!(
            "{} frames against the source's {}",
            row.kiln.frames,
            clip.frames.len()
        ));
    }
    let first_against = if range == Range::ToneMapped {
        Reference::hable(clip)
    } else {
        source
    };
    check_first_frame(&mut row.misses, &out, &m.tags, clip, &first_against);
    // The colour against the clip (through Hable's curve when tone-mapped:
    // Kiln's own reference could carry the same fault).
    check_chroma(&mut row.misses, &out, &m.tags, clip, &first_against);

    // The estimate.
    if let Some((s, error)) = sample(clip, &tune, q, report.hardware, &row.kiln) {
        row.estimate_error = Some(error);
        samples.push(s);
    }

    // FFmpeg at Kiln's size.
    let hdr = (range == Range::Hdr).then_some(clip.info.hdr);
    match ffmpeg::matched(
        cache,
        &ctx.work,
        &clip.stem,
        codec,
        basis,
        &input,
        hdr,
        clip.sound_is_aac,
        row.kiln.video_bytes,
        seed,
    ) {
        Err(e) => row.misses.push(format!("FFmpeg not compared: {e}")),
        Ok(found) => {
            let _ = write_setting(&mut row.setting, library, &found);
            let path = found.enc.path.clone();
            let ff = match measure(&path, found.enc.secs) {
                Ok(f) => Some(f),
                Err(e) => {
                    row.misses
                        .push(format!("FFmpeg's file can't be read back: {e}"));
                    None
                }
            };
            if let Some(f) = ff {
                let mut side = f.side;
                let score = cache
                    .vmaf
                    .entry(path.clone())
                    .or_insert_with(|| {
                        vmaf::vmaf(
                            &path,
                            &f.tags,
                            clip.untagged_fix.as_ref(),
                            &reference,
                            Model::Default,
                        )
                    })
                    .clone();
                match score {
                    Ok(v) => side.vmaf = Some(v),
                    Err(e) => row.misses.push(format!("FFmpeg's VMAF not measured: {e}")),
                }
                row.size_off = Some(super::size_off(row.kiln.video_bytes, side.video_bytes));
                if let (Some(k), Some(v)) = (row.kiln.vmaf, side.vmaf) {
                    if let Some(miss) = judge(k, row.kiln.video_bytes, v, side.video_bytes) {
                        row.misses.push(miss);
                    }
                }
                if row.kiln.secs > SPEED_WARN * side.secs {
                    row.warnings.push(format!(
                        "Kiln took {:.1}x FFmpeg's time",
                        row.kiln.secs / side.secs.max(1e-9)
                    ));
                }
                row.ffmpeg = Some(side);
            }
        }
    }
    ctx.done_with(&out);
    Some(row)
}

fn write_setting(setting: &mut String, library: &str, found: &ffmpeg::Matched) -> std::fmt::Result {
    use std::fmt::Write;
    write!(
        setting,
        " / {library} CRF {:.2} ({} encode{})",
        found.enc.crf,
        found.probes,
        if found.probes == 1 { "" } else { "s" }
    )
}

/// A copy: `from` converted to `target` with Original, which must copy the
/// picture. Every video packet's payload, and the sound's when it is copied,
/// must be the source's from some packet on, any dropped from the front
/// being hidden lead-in (R69). The picture's span (R76) and first frame are
/// the source's.
fn copy_row(
    ctx: &Ctx,
    clip: &Clip,
    from: &Path,
    from_name: &str,
    target: VideoTarget,
) -> (CopyRow, Option<PathBuf>) {
    let mut row = CopyRow {
        clip: clip.stem.clone(),
        route: format!("{from_name} → {}", target.name()),
        source_packets: clip.packets.len(),
        source_length: probe::span(&clip.frames),
        has_sound: clip.info.sound.is_some() || clip.info.sound_unreadable.is_some(),
        ..CopyRow::default()
    };
    let out = ctx.work.join(format!(
        "{}.copy.{}.{}",
        clip.stem,
        from_name.to_ascii_lowercase(),
        target.name().to_ascii_lowercase()
    ));
    let tune = VideoTune {
        setting: Some(VideoSetting::Original),
        ..VideoTune::default()
    };
    let (made, wall) = kiln(from, &out, target, &tune);
    row.secs = wall;
    let report = match made {
        Ok(r) => r,
        Err(e) => {
            row.misses.push(format!("Kiln failed: {e}"));
            return (row, None);
        }
    };
    row.video_copied = report.video_copied;
    row.sound_copied = report.sound_copied;
    if !report.video_copied {
        row.misses
            .push("Original re-encoded the picture instead of copying it".into());
    }
    let shown = probe::first_shown(&clip.frames).unwrap_or(0.0);
    match probe::packets(&out, "v:0", true) {
        Err(e) => row.misses.push(format!("can't be read back: {e}")),
        Ok(p) => {
            row.packets = p.len();
            let (dropped, differ, problem) = probe::compare_packets(&clip.packets, &p, shown);
            row.dropped = dropped;
            row.mismatched = differ;
            row.misses.extend(problem.map(|p| format!("picture: {p}")));
        }
    }
    if report.sound_copied {
        match probe::packets(&out, "a:0", true) {
            Err(e) => row
                .misses
                .push(format!("its sound can't be read back: {e}")),
            Ok(p) => {
                // R76: an MKV copy may start up to two AAC packets late;
                // MP4 and MOV keep the edit, and so every packet.
                let one = clip
                    .sound_packets
                    .iter()
                    .find_map(|p| p.dur)
                    .unwrap_or(0.025);
                let late = if target == VideoTarget::Mkv {
                    2.0 * one
                } else {
                    0.0
                };
                let (_, differ, problem) =
                    probe::compare_packets(&clip.sound_packets, &p, shown + late);
                row.sound_packets = (clip.sound_packets.len(), p.len());
                row.sound_mismatched = differ;
                row.misses.extend(problem.map(|p| format!("sound: {p}")));
            }
        }
    }
    match probe::frames(&out) {
        Err(e) => row.misses.push(format!("its frames can't be read: {e}")),
        Ok(f) => {
            row.length = probe::span(&f);
            check_length(&mut row.misses, clip, row.length);
        }
    }
    match probe::facts(&out) {
        Err(e) => row.misses.push(format!("can't be read back: {e}")),
        Ok(facts) => {
            let source = Reference::source(clip, clip.info.hdr != Hdr::None);
            check_first_frame(&mut row.misses, &out, &facts.tags, clip, &source);
        }
    }
    (row, Some(out))
}

/// R74: an MP4 whose edit list starts it off a key frame can't copy into
/// MKV (R69), so Original encodes it there at the top step; that encode
/// must keep the source's length and first frame.
fn mkv_encode_row(ctx: &Ctx, clip: &Clip, from_name: &str) -> CopyRow {
    let mut row = CopyRow {
        clip: clip.stem.clone(),
        route: format!("{from_name} → MKV, encoded (R69)"),
        encoded: true,
        source_packets: clip.packets.len(),
        source_length: probe::span(&clip.frames),
        has_sound: clip.info.sound.is_some() || clip.info.sound_unreadable.is_some(),
        ..CopyRow::default()
    };
    let out = ctx.work.join(format!("{}.encode.mkv", clip.stem));
    let tune = VideoTune {
        setting: Some(VideoSetting::Original),
        ..VideoTune::default()
    };
    let (made, wall) = kiln(&clip.path, &out, VideoTarget::Mkv, &tune);
    row.secs = wall;
    let report = match made {
        Ok(r) => r,
        Err(e) => {
            row.misses.push(format!("Kiln failed: {e}"));
            return row;
        }
    };
    row.video_copied = report.video_copied;
    row.sound_copied = report.sound_copied;
    if report.video_copied {
        row.misses
            .push("the picture was copied into MKV, though its edit starts off a key frame".into());
    }
    match probe::frames(&out) {
        Err(e) => row.misses.push(format!("its frames can't be read: {e}")),
        Ok(f) => {
            row.packets = f.len();
            row.length = probe::span(&f);
            check_length(&mut row.misses, clip, row.length);
        }
    }
    match probe::facts(&out) {
        Err(e) => row.misses.push(format!("can't be read back: {e}")),
        Ok(facts) => {
            let source = Reference::source(clip, false);
            check_first_frame(&mut row.misses, &out, &facts.tags, clip, &source);
        }
    }
    ctx.done_with(&out);
    row
}

/// The copies for one clip (R74): into MKV and back where MKV can copy it;
/// into MOV and back to MP4 where it can't (a turned picture, or an edit
/// list that starts it off a key frame), with the MKV encode checked
/// instead; MKV sources go to MP4 and back. Kiln's `can_copy_video` must
/// agree with the edit list ffprobe sees: misses go to `misses`.
pub fn copy_rows(ctx: &Ctx, clip: &Clip, rows: &mut Vec<CopyRow>, misses: &mut Vec<String>) {
    if !matches!(clip.info.codec, VideoCodec::H264 | VideoCodec::Hevc) {
        return;
    }
    let from = clip
        .path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("mp4")
        .to_ascii_uppercase();
    let turned = clip.info.rotation % 360 != 0;
    let lead_in = probe::hidden_lead_in(&clip.packets, &clip.frames);
    let off_key = lead_in >= clip.frame - 1e-3;
    let original = VideoTune {
        setting: Some(VideoSetting::Original),
        ..VideoTune::default()
    };
    let mkv_copies = can_copy_video(VideoTarget::Mkv, &clip.info, &original);
    if from != "MKV" && !turned {
        if off_key && mkv_copies {
            misses.push(format!(
                "{}: Kiln says MKV can copy it, but its edit list starts it {lead_in:.3} s after \
                 the key frame (R69)",
                clip.stem
            ));
        }
        if !off_key && !mkv_copies {
            misses.push(format!(
                "{}: Kiln says MKV can't copy it, but it starts on a key frame",
                clip.stem
            ));
        }
    }
    let first = match from.as_str() {
        "MKV" => VideoTarget::Mp4,
        _ if turned || off_key => VideoTarget::Mov,
        _ => VideoTarget::Mkv,
    };
    let second = if first == VideoTarget::Mkv {
        VideoTarget::Mp4
    } else if from == "MKV" {
        VideoTarget::Mkv
    } else {
        VideoTarget::Mp4
    };
    let (row, made) = copy_row(ctx, clip, &clip.path, &from, first);
    rows.push(row);
    if let Some(made) = made {
        let (row, again) = copy_row(ctx, clip, &made, first.name(), second);
        rows.push(row);
        ctx.done_with(&made);
        if let Some(again) = again {
            ctx.done_with(&again);
        }
    }
    if from != "MKV" && !turned && off_key {
        rows.push(mkv_encode_row(ctx, clip, &from));
    }
}

// ------------------------------------------------------------------- checks

pub fn the_verdict_is_read_right() {
    // Within ±15% and within 2: a pass; better on both: a pass.
    assert_eq!(judge(93.5, 1_100, 95.0, 1_000), None);
    assert_eq!(judge(96.0, 600, 95.0, 1_000), None);
    // 2.5 under at the same size, and 30% larger: misses.
    assert!(judge(92.5, 1_000, 95.0, 1_000).is_some());
    assert!(judge(96.0, 1_300, 95.0, 1_000).is_some());
    // Smaller by 30% but worse: outside the band and not better on both.
    assert!(judge(94.5, 700, 95.0, 1_000).is_some());
}
