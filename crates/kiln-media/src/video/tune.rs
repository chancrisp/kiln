//! How the owner wants the video written: the targets, the quality steps,
//! the presets, size estimates, Fit under and when the picture and the sound
//! can be copied as they are.
//!
//! Part 1's [`crate::tune`] does the same for sound; the names and the
//! shapes follow it.

use super::info::{VideoCodec, VideoInfo};
use crate::error::{fmt_mb, MediaError, Result};
use crate::tune::{Preset, Setting, Target, Tune};
use crate::types::{Codec, SourceInfo};

/// A video container.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum VideoTarget {
    Mp4,
    Mov,
    M4v,
    Mkv,
}

impl VideoTarget {
    /// The format's display name, as used in copy such as "MKV can't go under
    /// 2 MB for this file."
    pub fn name(self) -> &'static str {
        match self {
            VideoTarget::Mp4 => "MP4",
            VideoTarget::Mov => "MOV",
            VideoTarget::M4v => "M4V",
            VideoTarget::Mkv => "MKV",
        }
    }
}

/// The codec the picture is written in when it is encoded.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum VCodec {
    #[default]
    H264,
    Hevc,
}

/// Which encoder writes the picture.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum EncoderChoice {
    #[default]
    Automatic,
    GraphicsCard,
    Processor,
}

/// What a sink should do with the picture.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum VideoSetting {
    /// Copy the picture as it is when the rules allow it, else encode at the
    /// highest quality step.
    Original,
    /// An encoder's quality, 0 to 100; higher is better.
    Quality(u8),
    /// An average bitrate for the picture.
    Kbps(u32),
}

/// What happens to the sound.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum SoundChoice {
    /// Copy it when the target holds its codec, else encode it (AAC for MP4,
    /// MOV and M4V, Opus for MKV) at the bitrate of the tune's
    /// `sound_preset` (R95).
    #[default]
    Keep,
    /// Encode it (AAC or Opus, as above) at the step nearest this bitrate.
    /// Sound Kiln can't read is still copied when the target holds it.
    Kbps(u32),
    /// Leave the sound out (Make smaller's Sound off). Only asked for in so
    /// many words: the sound is otherwise never dropped (R14).
    Off,
}

/// The owner's tuning for one conversion. `None` setting means "follow the
/// default".
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct VideoTune {
    pub setting: Option<VideoSetting>,
    pub fit_bytes: Option<u64>,
    /// The most lines the short side may have; the picture is scaled down to
    /// it, never up.
    pub max_lines: Option<u32>,
    /// The most frames a second; the picture drops frames to it, never adds.
    pub max_fps: Option<u32>,
    pub codec: VCodec,
    pub encoder: EncoderChoice,
    pub sound: SoundChoice,
    /// The preset whose bitrate sound the job encodes under Keep is given
    /// (R95): AAC 96, 160 or 192 kbps a stereo pair, Opus 64, 96 or 160, for
    /// Small, High and Best. Set where the preset is resolved (the job's own
    /// preset, kiln-core's `video_tune`, the app's sheet), so the estimate,
    /// the labels, Fit under and FFmpeg's `-b:a` count what is written.
    pub sound_preset: Preset,
}

/// One stop on the quality slider.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct VStep {
    pub setting: VideoSetting,
    pub label: String,
}

/// The lowest, the highest and the spacing of the quality steps.
pub(crate) const QUALITY_FIRST: u8 = 20;
pub(crate) const QUALITY_LAST: u8 = 90;
pub(crate) const QUALITY_STEP: u8 = 5;
/// The three named marks.
pub(crate) const SMALL: u8 = 45;
pub(crate) const HIGH: u8 = 65;
pub(crate) const BEST: u8 = 80;

/// The least video bitrate Fit under will write.
pub(crate) const VIDEO_FLOOR_KBPS: f64 = 150.0;
/// Fit under aims 3% low for the container's overhead.
const FIT_MARGIN: f64 = 0.97;
/// `quality_under` wants the estimate this much under the limit, because a
/// quality setting only steers an encoder that ignores a bitrate.
const QUALITY_HEADROOM: f64 = 1.15;
/// The sound's bitrate when the source doesn't say.
const UNKNOWN_SOUND_KBPS: f64 = 128.0;
/// The frame rate the estimate assumes when the source doesn't say.
const UNKNOWN_FPS: f64 = 30.0;
/// A source frame rate this much over `max_fps` still counts as at it: an
/// average worked out from a container's tables (30.004) isn't over 30
/// (R31). The app offers frame rates below a source by the same rule.
pub const FPS_ALLOWANCE: f64 = 1.005;

/// The quality steps, 20 to 90.
fn qualities() -> impl DoubleEndedIterator<Item = u8> {
    (QUALITY_FIRST..=QUALITY_LAST).step_by(QUALITY_STEP as usize)
}

// ---------------------------------------------------------------------------
// Copying

/// True when the picture could be copied into `t` as it is, whatever the
/// setting: H.264 or HEVC into any target, AV1 or VP9 into MKV only, neither
/// scaled nor slowed by a limit, and not turned (MKV has no rotation, so a
/// turned picture is re-encoded upright for it). Nor, into MKV, an MP4's
/// picture whose edit list starts it more than a frame after the key frame
/// a copy would begin at: MKV has no edit list to hide those frames (R69).
fn copyable(t: VideoTarget, info: &VideoInfo, tune: &VideoTune) -> bool {
    let codec = match info.codec {
        VideoCodec::H264 | VideoCodec::Hevc => true,
        VideoCodec::Av1 | VideoCodec::Vp9 => t == VideoTarget::Mkv,
        _ => false,
    };
    // A limit at or above the source does nothing. A frame rate the file
    // doesn't state can't be shown to be under the limit.
    let lines = tune
        .max_lines
        .is_none_or(|m| m >= info.width.min(info.height));
    let fps = tune
        .max_fps
        .is_none_or(|m| info.fps.is_some_and(|f| f <= m as f64 * FPS_ALLOWANCE));
    let turned = info.rotation % 360 != 0 && t == VideoTarget::Mkv;
    let frame = 1.0 / info.fps.filter(|f| *f > 0.0).unwrap_or(30.0);
    let lead_in = t == VideoTarget::Mkv && info.lead_in.is_some_and(|l| l >= frame);
    codec && lines && fps && !turned && !lead_in
}

/// True when the picture is copied, not re-encoded: the rules in [`copyable`]
/// hold and the setting is Original.
pub fn can_copy_video(t: VideoTarget, info: &VideoInfo, tune: &VideoTune) -> bool {
    matches!(tune.setting, None | Some(VideoSetting::Original)) && copyable(t, info, tune)
}

/// True when the sound can be copied into `t`: AAC into any target; Opus,
/// Vorbis, FLAC or AC3 into MKV only.
pub fn can_copy_sound(t: VideoTarget, sound: &SourceInfo) -> bool {
    match sound.codec {
        Codec::Aac | Codec::HeAac => true,
        Codec::Opus | Codec::Vorbis | Codec::Flac | Codec::Ac3 => t == VideoTarget::Mkv,
        _ => false,
    }
}

/// What the picture is written with when the owner hasn't chosen: Original
/// where it can be copied, else the High quality step.
///
/// This judges the structural rules only and ignores `tune.setting`, unlike
/// [`can_copy_video`], which also needs the setting to be Original.
pub fn default_setting(t: VideoTarget, info: &VideoInfo, tune: &VideoTune) -> VideoSetting {
    if copyable(t, info, tune) {
        VideoSetting::Original
    } else {
        VideoSetting::Quality(HIGH)
    }
}

// ---------------------------------------------------------------------------
// The picture as written

/// The picture's size as written: upright (a source turned 90 or 270 degrees
/// swaps its sides), the short side capped at `max_lines` with the aspect
/// kept, and both sides rounded down to even (never below 2).
pub fn written_size(info: &VideoInfo, tune: &VideoTune) -> (u32, u32) {
    let (mut w, mut h) = if info.rotation % 180 == 90 {
        (info.height as u64, info.width as u64)
    } else {
        (info.width as u64, info.height as u64)
    };
    if let Some(max) = tune.max_lines {
        let max = max.max(1) as u64;
        let short = w.min(h);
        if short > max {
            w = (w * max / short).max(1);
            h = (h * max / short).max(1);
            // The short side lands on the limit; the division may fall a
            // pixel under it.
            if w <= h {
                w = max;
            } else {
                h = max;
            }
        }
    }
    let even = |n: u64| (n & !1).max(2) as u32;
    (even(w), even(h))
}

/// The frame rate as written: the source's, held to `max_fps`. `None` when
/// the file doesn't say.
pub fn written_fps(info: &VideoInfo, tune: &VideoTune) -> Option<f64> {
    let fps = info.fps?;
    Some(match tune.max_fps {
        Some(m) if fps > m as f64 * FPS_ALLOWANCE => m as f64,
        _ => fps,
    })
}

// ---------------------------------------------------------------------------
// Sizes

/// A bits-per-pixel table: H.264 spends `a + b * (q/100)^2` bits a pixel at
/// quality q, and HEVC `hevc` times that, for a source of `reference` bits
/// per pixel. A source of known bitrate scales it by its own bits per pixel
/// over `reference`, to the power `content` (R81).
#[derive(Clone, Copy, PartialEq, Debug)]
struct Table {
    a: f64,
    b: f64,
    hevc: f64,
    content: f64,
    reference: f64,
}

/// The graphics card's encoders, fitted by the quality harness (Task 10) to
/// the measured size of every encode it made on them, so that the median
/// error per codec is least, and fitted again with each encoder's own
/// settings (Task 10d, R85). The reference is the median of the harness
/// clips' own bits per pixel. See harness/README.md.
const CARD: Table = Table {
    a: 0.0051,
    b: 0.121,
    hevc: 0.94,
    content: 0.02,
    reference: 0.107,
};

/// Windows' software encoders on the processor, fitted the same way. They
/// spend more than the graphics card's at the same quality (R70); with its
/// settings (R85) the H.264 encoder now spends less than the HEVC one. The
/// source's own bits per pixel told nothing more of their size (k = 0).
const PROCESSOR: Table = Table {
    a: 0.0098,
    b: 0.113,
    hevc: 1.28,
    content: 0.0,
    reference: 0.107,
};

/// The table for the encoder the plan resolves to (R70): the processor's for
/// Processor, and for Graphics card or Automatic when no graphics card
/// encoder for `codec` starts on this PC; else the graphics card's.
///
/// It never starts anything (R82): off the unit tests it reads the answer
/// [`super::encode::encoder_adapter`] has already found, and until a job or
/// the app's worker has asked, it takes the graphics card's table,
/// Automatic's usual path. The app works its labels out again when its
/// worker's answer lands.
fn table_for(codec: VCodec, encoder: EncoderChoice) -> Table {
    if encoder != EncoderChoice::Processor && card_encodes(codec, encoder) {
        CARD
    } else {
        PROCESSOR
    }
}

#[cfg(not(test))]
fn card_encodes(codec: VCodec, encoder: EncoderChoice) -> bool {
    super::encode::encoder_adapter_known(codec, encoder).is_none_or(|card| card.is_some())
}

// The unit tests choose whether a graphics card encodes, per test thread, so
// they read the same on every PC.
#[cfg(test)]
thread_local! {
    static CARD_ENCODES: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
fn card_encodes(_codec: VCodec, _encoder: EncoderChoice) -> bool {
    CARD_ENCODES.with(std::cell::Cell::get)
}

/// Bits per pixel an encoder spends at a quality, by `table`, for a source
/// of `source` bits per pixel when that is known.
fn bits_per_pixel(table: Table, codec: VCodec, quality: u8, source: Option<f64>) -> f64 {
    let q = quality.min(100) as f64 / 100.0;
    let h264 = table.a + table.b * q * q;
    let content = source.map_or(1.0, |s| (s / table.reference).powf(table.content));
    content
        * match codec {
            VCodec::H264 => h264,
            VCodec::Hevc => h264 * table.hevc,
        }
}

/// The source picture's own bits per pixel: its video bitrate over its
/// width × height × frame rate. `None` when the file gives no bitrate or no
/// frame rate (R81: the table alone then).
pub fn source_bpp(info: &VideoInfo) -> Option<f64> {
    let kbps = info.video_kbps.filter(|&k| k > 0)? as f64;
    let fps = info.fps.filter(|f| f.is_finite() && *f > 0.0)?;
    let pixels = info.width as f64 * info.height as f64 * fps;
    (pixels > 0.0).then(|| kbps * 1000.0 / pixels)
}

/// Bits per pixel at a quality for `info`'s picture: the table's, capped by
/// the source's own when the file gives it (ENG-14). A source already
/// written in fewer bits a pixel than the step spends (a screen recording,
/// say) is re-encoded in about as few, so an estimate at the source's own
/// picture size and frame rate is never above the source's picture. At a
/// smaller size or frame rate the cap is [`source_cap`]'s looser one.
fn capped_bpp(q: u8, info: &VideoInfo, tune: &VideoTune) -> f64 {
    let source = source_bpp(info);
    let table = bits_per_pixel(table_for(tune.codec, tune.encoder), tune.codec, q, source);
    let Some(s) = source else {
        return table;
    };
    let (w, h) = written_size(info, tune);
    let fps = written_fps(info, tune).unwrap_or(0.0);
    let own = info.width as f64 * info.height as f64 * info.fps.unwrap_or(0.0);
    table.min(source_cap(s, own, w as f64 * h as f64 * fps))
}

/// The most bits a pixel a picture written at `written` pixels a second
/// is estimated at, from a source of `source_bpp` bits a pixel at `source`
/// pixels a second: the source's own at its own size and frame rate. A
/// smaller picture, or fewer frames, needs more bits a pixel, so the cap
/// grows with the square root of how many fewer pixels a second there are:
/// half the pixels is capped at about 0.7 of the source's rate.
pub fn source_cap(source_bpp: f64, source: f64, written: f64) -> f64 {
    if written > 0.0 && written < source {
        source_bpp * (source / written).sqrt()
    } else {
        source_bpp
    }
}

/// The picture's bitrate in kbps at a quality, as written.
fn quality_kbps(q: u8, info: &VideoInfo, tune: &VideoTune) -> f64 {
    let (w, h) = written_size(info, tune);
    let fps = written_fps(info, tune).unwrap_or_else(|| {
        // No frame rate in the file: 30, or the limit when that is lower.
        tune.max_fps
            .map_or(UNKNOWN_FPS, |m| UNKNOWN_FPS.min(m as f64))
    });
    capped_bpp(q, info, tune) * w as f64 * h as f64 * fps / 1000.0
}

/// How the job encodes `sound` into `t` when it doesn't copy it: AAC into
/// MP4, MOV and M4V, Opus into MKV, at the chosen bitrate (the step the
/// encoder lists nearest it), or for Keep at the bitrate of
/// `tune.sound_preset` (R95). The encoder, the sound as written (its rate
/// and channels), and the setting.
pub(crate) fn sound_encoding(
    t: VideoTarget,
    sound: &SourceInfo,
    tune: &VideoTune,
) -> Result<(Target, SourceInfo, Setting)> {
    let audio = if t == VideoTarget::Mkv {
        Target::Opus
    } else {
        Target::M4a
    };
    let asked = Tune {
        kbps: match tune.sound {
            SoundChoice::Kbps(k) => Some(k),
            SoundChoice::Keep | SoundChoice::Off => None,
        },
        ..Tune::default()
    };
    let written = crate::table::written(audio, &asked, sound);
    let setting = crate::tune::chosen_setting(audio, tune.sound_preset, &asked, &written)?;
    Ok((audio, written, setting))
}

/// The sound's bitrate in kbps the labels, the estimate and Fit under count
/// for `t` (R90): 0 when the file is silent or the sound is left out; the source's (else 128) when
/// Keep copies it; else the bitrate the sound encoder is given
/// ([`sound_encoding`]). Sound Kiln can't read is copied or refused (R14),
/// so it counts the chosen bitrate, else 128. Another engine writing to a
/// limit gives the sound this bitrate.
pub fn sound_kbps(t: VideoTarget, info: &VideoInfo, tune: &VideoTune) -> f64 {
    let chosen = match tune.sound {
        SoundChoice::Kbps(k) => k as f64,
        SoundChoice::Keep => UNKNOWN_SOUND_KBPS,
        SoundChoice::Off => return 0.0,
    };
    match (&info.sound, &info.sound_unreadable) {
        (None, None) => 0.0,
        (None, Some(_)) => chosen,
        (Some(s), _) if tune.sound == SoundChoice::Keep && can_copy_sound(t, s) => {
            s.kbps.map_or(UNKNOWN_SOUND_KBPS, f64::from)
        }
        (Some(s), _) => sound_encoding(t, s, tune).map_or(chosen, |(_, _, setting)| {
            crate::tune::setting_kbps(&setting)
        }),
    }
}

/// The length in seconds, when the file says and it is worth dividing by.
fn seconds(info: &VideoInfo) -> Option<f64> {
    info.duration.filter(|d| d.is_finite() && *d > 0.0)
}

/// Bytes for a picture and a sound at these bitrates over `secs`.
fn bytes_of(t: VideoTarget, video_kbps: f64, info: &VideoInfo, tune: &VideoTune, secs: f64) -> u64 {
    ((video_kbps + sound_kbps(t, info, tune)) * 1000.0 / 8.0 * secs).round() as u64
}

/// The expected size of the written file. `None` when the length is unknown,
/// or when copying a picture whose bitrate the container doesn't give.
///
/// A quality step is worked out from the picture as written (size, frame rate
/// and the codec in `tune`); a copy uses the source's own bitrate; Original
/// where the copy isn't possible is the highest quality step. The sound is
/// added at its bitrate.
pub fn estimate_bytes(
    t: VideoTarget,
    s: &VideoSetting,
    info: &VideoInfo,
    tune: &VideoTune,
) -> Option<u64> {
    let secs = seconds(info)?;
    let video_kbps = match *s {
        VideoSetting::Original if copyable(t, info, tune) => info.video_kbps? as f64,
        VideoSetting::Original => quality_kbps(QUALITY_LAST, info, tune),
        VideoSetting::Quality(q) => quality_kbps(q, info, tune),
        VideoSetting::Kbps(k) => k as f64,
    };
    Some(bytes_of(t, video_kbps, info, tune, secs))
}

/// What a short sample encode of the file found ([`super::sample`]): the
/// picture's bitrate at a quality step, and that over the estimate's own
/// figure for the step, by which every quality step's estimate for the file
/// is scaled.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Measured {
    /// The quality step the sample was encoded at.
    pub quality: u8,
    /// The picture's bitrate the sample came to, in kbps.
    pub kbps: f64,
    /// `kbps` over the capped table's bitrate for the step.
    pub scale: f64,
}

/// The capped table's picture bitrate in kbps at quality `q`, as written.
pub(crate) fn table_kbps(q: u8, info: &VideoInfo, tune: &VideoTune) -> f64 {
    quality_kbps(q, info, tune)
}

/// [`estimate_bytes`], with a quality step's picture scaled by what a
/// sample encode of the file measured (spec 3.3). A copy and a bitrate are
/// what they are; with no sample this is `estimate_bytes`.
pub fn estimate_measured(
    t: VideoTarget,
    s: &VideoSetting,
    info: &VideoInfo,
    tune: &VideoTune,
    measured: Option<&Measured>,
) -> Option<u64> {
    let Some(m) = measured.filter(|m| m.scale.is_finite() && m.scale > 0.0) else {
        return estimate_bytes(t, s, info, tune);
    };
    let secs = seconds(info)?;
    let video_kbps = match *s {
        VideoSetting::Original if copyable(t, info, tune) => info.video_kbps? as f64,
        VideoSetting::Original => quality_kbps(QUALITY_LAST, info, tune) * m.scale,
        VideoSetting::Quality(q) => quality_kbps(q, info, tune) * m.scale,
        VideoSetting::Kbps(k) => k as f64,
    };
    Some(bytes_of(t, video_kbps, info, tune, secs))
}

// ---------------------------------------------------------------------------
// Labels and steps

/// "about 6 Mbps" from 1000 kbps up, else "about 840 kbps".
fn rate_text(kbps: f64) -> String {
    if kbps >= 1000.0 {
        format!("about {} Mbps", (kbps / 1000.0).round() as u32)
    } else {
        format!("about {} kbps", kbps.round() as u32)
    }
}

/// "about 84 MB", in part 1's megabytes.
fn size_text(bytes: u64) -> String {
    let mb = fmt_mb(bytes);
    format!("about {} MB", mb.strip_suffix(".0").unwrap_or(&mb))
}

/// The name of a quality: Small, High or Best on the three marks, else its
/// step number on the slider (`Step 1` is the lowest).
fn quality_name(q: u8) -> String {
    match q {
        SMALL => "Small".into(),
        HIGH => "High".into(),
        BEST => "Best".into(),
        _ if (QUALITY_FIRST..=QUALITY_LAST).contains(&q)
            && (q - QUALITY_FIRST) % QUALITY_STEP == 0 =>
        {
            format!("Step {}", (q - QUALITY_FIRST) / QUALITY_STEP + 1)
        }
        _ => format!("Quality {q}"),
    }
}

/// A setting's label in its parts, for a caller that writes them its own
/// way (the app writes sizes as it writes them everywhere).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LabelParts {
    /// "High", "Step 9", "Original", or for a bitrate "2500 kbps".
    pub name: String,
    /// What Original does: "copies the video" or "highest quality".
    pub what: Option<&'static str>,
    /// A quality step's bitrate: "about 6 Mbps".
    pub rate: Option<String>,
    /// The file's size in bytes, when the length is known; not for Original.
    pub size: Option<u64>,
}

impl LabelParts {
    /// The parts joined as the label reads: "High · about 6 Mbps · about
    /// 84 MB", with the size in `size_text`'s words.
    fn joined(&self, size_text: impl Fn(u64) -> String) -> String {
        std::iter::once(self.name.clone())
            .chain(self.what.map(str::to_owned))
            .chain(self.rate.clone())
            .chain(self.size.map(size_text))
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

/// The parts of a setting's label into `t`.
fn parts_of(t: VideoTarget, s: &VideoSetting, info: &VideoInfo, tune: &VideoTune) -> LabelParts {
    let copies = copyable(t, info, tune);
    // The size part is left out when the length is unknown.
    let size = |kbps: f64| seconds(info).map(|secs| bytes_of(t, kbps, info, tune, secs));
    match *s {
        VideoSetting::Original => LabelParts {
            name: "Original".into(),
            what: Some(if copies {
                "copies the video"
            } else {
                "highest quality"
            }),
            rate: None,
            size: None,
        },
        VideoSetting::Quality(q) => {
            let kbps = quality_kbps(q, info, tune);
            LabelParts {
                name: quality_name(q),
                what: None,
                rate: Some(rate_text(kbps)),
                size: size(kbps),
            }
        }
        VideoSetting::Kbps(k) => LabelParts {
            name: format!("{k} kbps"),
            what: None,
            rate: None,
            size: size(k as f64),
        },
    }
}

/// The slider's stops, Small to Best and then Original last: quality 20 to
/// 90 in steps of 5, and Original, which copies the picture where it can.
///
/// Original's label says whether the picture is copied into `t`.
pub fn steps(t: VideoTarget, info: &VideoInfo, tune: &VideoTune) -> Vec<VStep> {
    qualities()
        .map(VideoSetting::Quality)
        .chain([VideoSetting::Original])
        .map(|setting| VStep {
            label: label_of(t, &setting, info, tune),
            setting,
        })
        .collect()
}

/// The step the preset lands on: Small, High and Best are the marks at
/// quality 45, 65 and 80.
pub fn preset_step(p: Preset, _info: &VideoInfo, _tune: &VideoTune) -> usize {
    let q = match p {
        Preset::Small => SMALL,
        Preset::High => HIGH,
        Preset::Best => BEST,
    };
    ((q - QUALITY_FIRST) / QUALITY_STEP) as usize
}

/// The label for any setting, including Advanced choices that aren't on the
/// slider (for example "2500 kbps"). Original's label says whether the
/// picture is copied into `t`.
pub fn label_of(t: VideoTarget, s: &VideoSetting, info: &VideoInfo, tune: &VideoTune) -> String {
    parts_of(t, s, info, tune).joined(size_text)
}

/// [`label_of`] in its parts: the name, what Original does, the bitrate and
/// the size in bytes.
pub fn label_parts(
    t: VideoTarget,
    s: &VideoSetting,
    info: &VideoInfo,
    tune: &VideoTune,
) -> LabelParts {
    parts_of(t, s, info, tune)
}

// ---------------------------------------------------------------------------
// Fit under

/// The setting that writes a file under `max_bytes`: a video bitrate of
/// `(max_bytes * 8 / length - the sound's bitrate) * 0.97`, aiming 3% low for
/// the container's overhead.
///
/// Below 150 kbps of video the answer is `TooSmall`. When `max_bytes` covers
/// the best the picture can be (a copy where it copies, else the top quality
/// step) that is the answer, not a bitrate padded to fill the limit. A copy
/// of a picture whose bitrate is unknown is judged by the top quality step's
/// size.
///
/// When the length is unknown no honest bitrate can be worked out, so this
/// returns the default setting unchanged; the app hides Fit under then.
pub fn fit_under(
    t: VideoTarget,
    max_bytes: u64,
    info: &VideoInfo,
    tune: &VideoTune,
) -> Result<VideoSetting> {
    let Some(secs) = seconds(info) else {
        return Ok(default_setting(t, info, tune));
    };
    let sound = sound_kbps(t, info, tune);
    // The smallest limit that leaves the floor for the picture.
    let min_bytes = ((VIDEO_FLOOR_KBPS / FIT_MARGIN + sound) * 1000.0 / 8.0 * secs).ceil() as u64;
    if max_bytes < min_bytes {
        return Err(MediaError::TooSmall {
            format: t.name().to_string(),
            min_bytes,
        });
    }
    // A limit that holds the best the picture can be gets that: Original
    // where it copies, else the top quality step.
    let best = if copyable(t, info, tune) {
        VideoSetting::Original
    } else {
        VideoSetting::Quality(QUALITY_LAST)
    };
    // A copy whose bitrate the container doesn't give has no estimate; the
    // top quality step's stands in for it, so a huge limit still stops here.
    let ceiling = estimate_bytes(t, &best, info, tune)
        .or_else(|| estimate_bytes(t, &VideoSetting::Quality(QUALITY_LAST), info, tune));
    if ceiling.is_some_and(|b| b <= max_bytes) {
        return Ok(best);
    }
    let kbps = (max_bytes as f64 * 8.0 / secs / 1000.0 - sound) * FIT_MARGIN;
    Ok(VideoSetting::Kbps(
        kbps.floor().clamp(VIDEO_FLOOR_KBPS, u32::MAX as f64) as u32,
    ))
}

/// The highest quality step whose estimate, with 15% headroom, is no more than
/// `max_bytes`; `None` when none is, or when the length is unknown.
///
/// For an encoder that ignores a bitrate and is steered by quality alone,
/// where [`fit_under`]'s bitrate would not hold.
pub fn quality_under(
    t: VideoTarget,
    max_bytes: u64,
    info: &VideoInfo,
    tune: &VideoTune,
) -> Option<u8> {
    qualities().rev().find(|&q| {
        estimate_bytes(t, &VideoSetting::Quality(q), info, tune)
            .is_some_and(|b| b as f64 * QUALITY_HEADROOM <= max_bytes as f64)
    })
}

/// The setting the picture of `info` is written into `t` with (R44): what
/// Fit under finds for `tune.fit_bytes`; else `tune.setting`; else the
/// preset's: High is Original where the picture can be copied, else the High
/// step; Small is quality 45; Best is Original where it can be copied, else
/// quality 80.
///
/// This is the first pass's choice; a job whose Fit under pass lands over the
/// limit picks again (R45). Part 1's [`crate::tune::chosen_setting`] does the
/// same for sound.
pub fn chosen_setting(
    t: VideoTarget,
    preset: Preset,
    tune: &VideoTune,
    info: &VideoInfo,
) -> Result<VideoSetting> {
    if let Some(bytes) = tune.fit_bytes {
        return fit_under(t, bytes, info, tune);
    }
    if let Some(s) = tune.setting {
        return Ok(s);
    }
    let copies = default_setting(t, info, tune) == VideoSetting::Original;
    Ok(match preset {
        Preset::High => default_setting(t, info, tune),
        Preset::Small => VideoSetting::Quality(SMALL),
        Preset::Best if copies => VideoSetting::Original,
        Preset::Best => VideoSetting::Quality(BEST),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Channels, Tags};
    use crate::video::Hdr;

    fn video(width: u32, height: u32) -> VideoInfo {
        VideoInfo {
            width,
            height,
            rotation: 0,
            fps: Some(30.0),
            variable_rate: false,
            codec: VideoCodec::H264,
            profile: None,
            bit_depth: 8,
            hdr: Hdr::None,
            duration: Some(120.0),
            video_kbps: Some(8000),
            sound: None,
            sound_unreadable: None,
            sar: (1, 1),
            mastering: None,
            content_light: None,
            created: None,
            title: None,
            lead_in: None,
            colour_tagged: false,
        }
    }

    fn sound(codec: Codec, kbps: Option<u32>) -> SourceInfo {
        SourceInfo {
            codec,
            rate: 48000,
            channels: Channels {
                count: 2,
                mask: 0x3,
            },
            bits: None,
            frames: None,
            kbps,
            tags: Tags::default(),
        }
    }

    /// H.264's bits per pixel at quality `q` by `table`, for `info`'s
    /// source.
    fn h264_bpp(table: Table, q: u8, info: &VideoInfo) -> f64 {
        bits_per_pixel(table, VCodec::H264, q, source_bpp(info))
    }

    #[test]
    fn the_source_s_own_bits_per_pixel_scale_the_table() {
        // R81: the table times (source bpp / reference) to the power k.
        let table = Table {
            a: 0.01,
            b: 0.1,
            hevc: 0.7,
            content: 0.5,
            reference: 0.1,
        };
        let plain = bits_per_pixel(table, VCodec::H264, 50, None);
        assert!((plain - 0.035).abs() < 1e-12, "{plain}");
        let busy = bits_per_pixel(table, VCodec::H264, 50, Some(0.4));
        assert!((busy - 2.0 * plain).abs() < 1e-12, "{busy}");
        let calm = bits_per_pixel(table, VCodec::Hevc, 50, Some(0.025));
        assert!((calm - 0.5 * 0.7 * plain).abs() < 1e-12, "{calm}");
        // 8000 kbps over 1920x1080 at 30 fps.
        let got = source_bpp(&clip()).unwrap();
        assert!((got - 8_000_000.0 / (1920.0 * 1080.0 * 30.0)).abs() < 1e-12);
        // No bitrate or no frame rate: the table alone.
        for info in [
            VideoInfo {
                video_kbps: None,
                ..clip()
            },
            VideoInfo {
                fps: None,
                ..clip()
            },
        ] {
            assert_eq!(source_bpp(&info), None);
        }
    }

    fn clip() -> VideoInfo {
        video(1920, 1080)
    }

    const ALL: [VideoTarget; 4] = [
        VideoTarget::Mp4,
        VideoTarget::Mov,
        VideoTarget::M4v,
        VideoTarget::Mkv,
    ];

    #[test]
    fn original_is_the_default_when_the_video_can_be_copied() {
        let tune = VideoTune::default();
        assert_eq!(
            default_setting(VideoTarget::Mkv, &clip(), &tune),
            VideoSetting::Original
        );
        let hevc = VideoInfo {
            codec: VideoCodec::Hevc,
            ..clip()
        };
        assert_eq!(
            default_setting(VideoTarget::Mp4, &hevc, &tune),
            VideoSetting::Original
        );
        // Not for a codec the target can't hold, or one Kiln doesn't copy.
        let av1 = VideoInfo {
            codec: VideoCodec::Av1,
            ..clip()
        };
        assert_eq!(
            default_setting(VideoTarget::Mp4, &av1, &tune),
            VideoSetting::Quality(65)
        );
        assert_eq!(
            default_setting(VideoTarget::Mkv, &av1, &tune),
            VideoSetting::Original
        );
        let mpeg4 = VideoInfo {
            codec: VideoCodec::Mpeg4,
            ..clip()
        };
        for t in ALL {
            assert_eq!(default_setting(t, &mpeg4, &tune), VideoSetting::Quality(65));
        }
    }

    #[test]
    fn a_rotated_source_to_mkv_cannot_copy() {
        let turned = VideoInfo {
            rotation: 90,
            ..clip()
        };
        let tune = VideoTune::default();
        assert!(!can_copy_video(VideoTarget::Mkv, &turned, &tune));
        assert!(can_copy_video(VideoTarget::Mp4, &turned, &tune));
        assert!(can_copy_video(VideoTarget::Mov, &turned, &tune));
        assert_eq!(
            default_setting(VideoTarget::Mkv, &turned, &tune),
            VideoSetting::Quality(65)
        );
        assert!(can_copy_video(VideoTarget::Mkv, &clip(), &tune));
    }

    #[test]
    fn a_hidden_lead_in_stops_the_copy_into_mkv_only() {
        // R69: an edit list that starts the picture a frame or more after
        // the key frame a copy begins at can't be kept in MKV.
        let tune = VideoTune::default();
        let cut = |l| VideoInfo {
            lead_in: Some(l),
            ..clip()
        };
        for l in [1.0 / 30.0, 0.5, 8.08] {
            assert!(!can_copy_video(VideoTarget::Mkv, &cut(l), &tune), "{l}");
            assert_eq!(
                default_setting(VideoTarget::Mkv, &cut(l), &tune),
                VideoSetting::Quality(65)
            );
            for t in [VideoTarget::Mp4, VideoTarget::Mov, VideoTarget::M4v] {
                assert!(can_copy_video(t, &cut(l), &tune), "{l} into {t:?}");
            }
        }
        // On the key frame, or within a frame of it, MKV copies.
        for l in [0.0, 0.02] {
            assert!(can_copy_video(VideoTarget::Mkv, &cut(l), &tune), "{l}");
        }
        // A frame is the file's own: at 10 fps, 0.05 s is within it.
        let slow = VideoInfo {
            fps: Some(10.0),
            ..cut(0.05)
        };
        assert!(can_copy_video(VideoTarget::Mkv, &slow, &tune));
    }

    #[test]
    fn capping_resolution_or_frame_rate_stops_the_copy() {
        let t = VideoTarget::Mp4;
        let none = VideoTune::default();
        assert!(can_copy_video(t, &clip(), &none));
        let lines = |m| VideoTune {
            max_lines: Some(m),
            ..none
        };
        assert!(!can_copy_video(t, &clip(), &lines(720)));
        assert!(can_copy_video(t, &clip(), &lines(1080)));
        assert!(can_copy_video(t, &clip(), &lines(2160)));
        let fps = |m| VideoTune {
            max_fps: Some(m),
            ..none
        };
        assert!(!can_copy_video(t, &clip(), &fps(24)));
        assert!(can_copy_video(t, &clip(), &fps(30)));
        assert!(can_copy_video(t, &clip(), &fps(60)));
        // A frame rate the file doesn't state can't be shown to be under it.
        let unknown = VideoInfo {
            fps: None,
            ..clip()
        };
        assert!(!can_copy_video(t, &unknown, &fps(60)));
        assert!(can_copy_video(t, &unknown, &none));
    }

    #[test]
    fn a_chosen_quality_stops_the_copy() {
        let t = VideoTarget::Mp4;
        let quality = VideoTune {
            setting: Some(VideoSetting::Quality(65)),
            ..VideoTune::default()
        };
        assert!(!can_copy_video(t, &clip(), &quality));
        let original = VideoTune {
            setting: Some(VideoSetting::Original),
            ..VideoTune::default()
        };
        assert!(can_copy_video(t, &clip(), &original));
    }

    #[test]
    fn sound_copies_by_codec_and_target() {
        for t in ALL {
            assert!(can_copy_sound(t, &sound(Codec::Aac, None)), "{t:?}");
            assert!(can_copy_sound(t, &sound(Codec::HeAac, None)), "{t:?}");
            assert!(!can_copy_sound(t, &sound(Codec::Mp3, None)), "{t:?}");
            assert!(
                !can_copy_sound(t, &sound(Codec::Other("dts".into()), None)),
                "{t:?}"
            );
        }
        for codec in [Codec::Opus, Codec::Vorbis, Codec::Flac, Codec::Ac3] {
            assert!(can_copy_sound(
                VideoTarget::Mkv,
                &sound(codec.clone(), None)
            ));
            for t in [VideoTarget::Mp4, VideoTarget::Mov, VideoTarget::M4v] {
                assert!(!can_copy_sound(t, &sound(codec.clone(), None)), "{codec:?}");
            }
        }
    }

    #[test]
    fn written_size_is_upright_capped_and_even() {
        let none = VideoTune::default();
        assert_eq!(written_size(&video(885, 1919), &none), (884, 1918));
        let turned = VideoInfo {
            rotation: 90,
            ..clip()
        };
        let cap = |m| VideoTune {
            max_lines: Some(m),
            ..none
        };
        assert_eq!(written_size(&turned, &cap(720)), (720, 1280));
        assert_eq!(written_size(&turned, &none), (1080, 1920));
        let upside_down = VideoInfo {
            rotation: 180,
            ..clip()
        };
        assert_eq!(written_size(&upside_down, &none), (1920, 1080));
        assert_eq!(written_size(&video(3840, 2160), &cap(1080)), (1920, 1080));
        // A limit above the source never enlarges.
        assert_eq!(written_size(&clip(), &cap(2160)), (1920, 1080));
        // An odd result rounds down: 1000x563 at 281 lines is 499x281.
        assert_eq!(written_size(&video(1000, 563), &cap(281)), (498, 280));
    }

    #[test]
    fn odd_sizes_never_leave_an_odd_side() {
        for n in 1..=4000u32 {
            for (w, h) in [(n, 1080), (1920, n), (n, n), (n, 4001 - n)] {
                for rotation in [0, 90] {
                    for max_lines in [None, Some(481), Some(720)] {
                        let info = VideoInfo {
                            rotation,
                            ..video(w, h)
                        };
                        let tune = VideoTune {
                            max_lines,
                            ..VideoTune::default()
                        };
                        let (ow, oh) = written_size(&info, &tune);
                        assert!(ow % 2 == 0 && oh % 2 == 0, "{w}x{h} -> {ow}x{oh}");
                        if let Some(m) = max_lines {
                            assert!(ow.min(oh) <= m, "{w}x{h} at {m} -> {ow}x{oh}");
                        }
                        // A side of 1 is skipped here: probe rejects a zero
                        // or one-pixel size, and even() floors a side at 2,
                        // which is larger than the source.
                        if w >= 2 && h >= 2 {
                            // Never larger than the source's upright size.
                            let (uw, uh) = if rotation == 90 { (h, w) } else { (w, h) };
                            assert!(ow <= uw && oh <= uh, "{w}x{h} -> {ow}x{oh}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn frame_rate_is_held_to_the_limit_and_unknown_stays_unknown() {
        let tune = VideoTune {
            max_fps: Some(24),
            ..VideoTune::default()
        };
        assert_eq!(written_fps(&clip(), &tune), Some(24.0));
        assert_eq!(written_fps(&clip(), &VideoTune::default()), Some(30.0));
        let slow = VideoInfo {
            fps: Some(15.0),
            ..clip()
        };
        assert_eq!(written_fps(&slow, &tune), Some(15.0));
        let unknown = VideoInfo {
            fps: None,
            ..clip()
        };
        assert_eq!(written_fps(&unknown, &tune), None);
    }

    #[test]
    fn steps_are_ordered_and_end_with_original() {
        let list = steps(VideoTarget::Mp4, &clip(), &VideoTune::default());
        assert_eq!(list.len(), 16);
        assert_eq!(list[15].setting, VideoSetting::Original);
        let qualities: Vec<u8> = list[..15]
            .iter()
            .map(|s| match s.setting {
                VideoSetting::Quality(q) => q,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(qualities.first(), Some(&20));
        assert_eq!(qualities.last(), Some(&90));
        assert!(qualities.windows(2).all(|w| w[1] == w[0] + 5));
    }

    #[test]
    fn presets_land_on_the_marks() {
        let tune = VideoTune::default();
        let list = steps(VideoTarget::Mp4, &clip(), &tune);
        let at = |p| list[preset_step(p, &clip(), &tune)].setting;
        assert_eq!(at(Preset::Small), VideoSetting::Quality(45));
        assert_eq!(at(Preset::High), VideoSetting::Quality(65));
        assert_eq!(at(Preset::Best), VideoSetting::Quality(80));
    }

    #[test]
    fn labels_name_the_marks_and_number_the_rest() {
        let info = clip();
        let tune = VideoTune::default();
        let list = steps(VideoTarget::Mp4, &info, &tune);
        // 1080p30 at quality 65 for 120 s with no sound, by the processor's
        // table (no graphics card encodes on the test thread).
        let kbps = h264_bpp(PROCESSOR, 65, &info) * 1920.0 * 1080.0 * 30.0 / 1000.0;
        let bytes = (kbps * 1000.0 / 8.0 * 120.0).round() as u64;
        assert_eq!(
            list[9].label,
            format!("High · {} · {}", rate_text(kbps), size_text(bytes))
        );
        assert!(list[5].label.starts_with("Small · "), "{}", list[5].label);
        assert!(list[12].label.starts_with("Best · "), "{}", list[12].label);
        assert!(list[0].label.starts_with("Step 1 · "), "{}", list[0].label);
        assert!(list[1].label.starts_with("Step 2 · "), "{}", list[1].label);
        assert!(
            list[14].label.starts_with("Step 15 · "),
            "{}",
            list[14].label
        );
        assert_eq!(list[15].label, "Original · copies the video");
        // Under 1 Mbps the rate is in kbps: 640x360 at quality 20.
        let kbps = h264_bpp(PROCESSOR, 20, &video(640, 360)) * 640.0 * 360.0 * 30.0 / 1000.0;
        assert!(kbps < 1000.0);
        let small = steps(VideoTarget::Mp4, &video(640, 360), &tune);
        let want = format!("Step 1 · about {} kbps · about ", kbps.round() as u32);
        assert!(small[0].label.starts_with(&want), "{}", small[0].label);
    }

    #[test]
    fn original_reads_highest_quality_when_it_cannot_copy() {
        let av1 = VideoInfo {
            codec: VideoCodec::Av1,
            ..clip()
        };
        let tune = VideoTune::default();
        let list = steps(VideoTarget::Mp4, &av1, &tune);
        assert_eq!(list[15].label, "Original · highest quality");
        // Into MKV the same file copies.
        assert_eq!(
            steps(VideoTarget::Mkv, &av1, &tune)[15].label,
            "Original · copies the video"
        );
        // And it encodes at the top step: the same size as Quality(90).
        assert_eq!(
            estimate_bytes(VideoTarget::Mp4, &VideoSetting::Original, &av1, &tune),
            estimate_bytes(VideoTarget::Mp4, &VideoSetting::Quality(90), &av1, &tune)
        );
    }

    #[test]
    fn an_unknown_length_drops_the_size_from_a_label() {
        let info = VideoInfo {
            duration: None,
            ..clip()
        };
        let tune = VideoTune::default();
        let kbps = h264_bpp(PROCESSOR, 65, &info) * 1920.0 * 1080.0 * 30.0 / 1000.0;
        assert_eq!(
            steps(VideoTarget::Mp4, &info, &tune)[9].label,
            format!("High · {}", rate_text(kbps))
        );
        assert_eq!(
            label_of(VideoTarget::Mp4, &VideoSetting::Kbps(2500), &info, &tune),
            "2500 kbps"
        );
        assert_eq!(
            label_of(VideoTarget::Mp4, &VideoSetting::Kbps(2500), &clip(), &tune),
            "2500 kbps · about 37.5 MB"
        );
        assert!(
            label_of(VideoTarget::Mp4, &VideoSetting::Quality(62), &info, &tune)
                .starts_with("Quality 62 · about ")
        );
    }

    #[test]
    fn label_parts_are_the_labels_own() {
        let t = VideoTarget::Mp4;
        let tune = VideoTune::default();
        let info = clip();
        let p = label_parts(t, &VideoSetting::Kbps(2500), &info, &tune);
        assert_eq!(p.name, "2500 kbps");
        assert_eq!((p.what, p.rate.as_deref()), (None, None));
        assert_eq!(
            p.size,
            estimate_bytes(t, &VideoSetting::Kbps(2500), &info, &tune)
        );
        let high = label_parts(t, &VideoSetting::Quality(HIGH), &info, &tune);
        assert_eq!(high.name, "High");
        assert!(high
            .rate
            .as_deref()
            .is_some_and(|r| r.starts_with("about ")));
        let original = label_parts(t, &VideoSetting::Original, &info, &tune);
        assert_eq!(
            (original.name.as_str(), original.what, original.size),
            ("Original", Some("copies the video"), None)
        );
        // Joined, they are the label.
        for s in [
            VideoSetting::Kbps(2500),
            VideoSetting::Quality(HIGH),
            VideoSetting::Original,
        ] {
            assert_eq!(
                label_parts(t, &s, &info, &tune).joined(size_text),
                label_of(t, &s, &info, &tune)
            );
        }
    }

    #[test]
    fn estimates_grow_with_quality_and_size() {
        let t = VideoTarget::Mp4;
        let tune = VideoTune::default();
        let at = |info: &VideoInfo, tune: &VideoTune, q| {
            estimate_bytes(t, &VideoSetting::Quality(q), info, tune).unwrap()
        };
        let sizes: Vec<u64> = qualities().map(|q| at(&clip(), &tune, q)).collect();
        assert!(sizes.windows(2).all(|w| w[1] > w[0]), "{sizes:?}");
        assert!(at(&video(3840, 2160), &tune, 65) > at(&clip(), &tune, 65));
        let longer = VideoInfo {
            duration: Some(240.0),
            ..clip()
        };
        assert!(at(&longer, &tune, 65) > at(&clip(), &tune, 65));
        let hevc = VideoTune {
            codec: VCodec::Hevc,
            ..tune
        };
        // The table's HEVC factor of H.264 for the picture alone: on the
        // processor HEVC is the larger since R85 (CABAC for H.264).
        for q in qualities() {
            assert!(at(&clip(), &hevc, q) > at(&clip(), &tune, q));
        }
        let ratio = at(&clip(), &hevc, 65) as f64 / at(&clip(), &tune, 65) as f64;
        assert!((ratio - PROCESSOR.hevc).abs() < 1e-6, "{ratio}");
        // A frame rate limit and a line limit both shrink it.
        let slow = VideoTune {
            max_fps: Some(15),
            ..tune
        };
        assert!(at(&clip(), &slow, 65) < at(&clip(), &tune, 65));
        let small = VideoTune {
            max_lines: Some(720),
            ..tune
        };
        assert!(at(&clip(), &small, 65) < at(&clip(), &tune, 65));
    }

    #[test]
    fn the_table_follows_the_encoder_the_plan_resolves() {
        // R70: the processor's table for Processor, and for Graphics card or
        // Automatic when no graphics card encoder starts; else the card's.
        let t = VideoTarget::Mp4;
        let q65 = |encoder| {
            let tune = VideoTune {
                encoder,
                ..VideoTune::default()
            };
            estimate_bytes(t, &VideoSetting::Quality(65), &clip(), &tune).unwrap()
        };
        let bytes = |table: Table| {
            let bpp = h264_bpp(table, 65, &clip());
            (bpp * 1920.0 * 1080.0 * 30.0 / 1000.0 * 1000.0 / 8.0 * 120.0).round() as u64
        };
        CARD_ENCODES.with(|c| c.set(false));
        for e in [
            EncoderChoice::Processor,
            EncoderChoice::Automatic,
            EncoderChoice::GraphicsCard,
        ] {
            assert_eq!(q65(e), bytes(PROCESSOR), "{e:?} with no card");
        }
        CARD_ENCODES.with(|c| c.set(true));
        assert_eq!(q65(EncoderChoice::Processor), bytes(PROCESSOR));
        assert_eq!(q65(EncoderChoice::Automatic), bytes(CARD));
        assert_eq!(q65(EncoderChoice::GraphicsCard), bytes(CARD));
        CARD_ENCODES.with(|c| c.set(false));
        assert_ne!(CARD, PROCESSOR);
    }

    #[test]
    fn the_estimate_is_the_table_times_pixels_frames_and_seconds() {
        // 1920x1080 at 30 fps for 120 s at quality 65, no sound.
        let bpp = h264_bpp(PROCESSOR, 65, &clip());
        let want = bpp * 1920.0 * 1080.0 * 30.0 * 120.0 / 8.0;
        let got = estimate_bytes(
            VideoTarget::Mp4,
            &VideoSetting::Quality(65),
            &clip(),
            &VideoTune::default(),
        )
        .unwrap();
        assert!((got as f64 - want).abs() <= 1.0, "{got} {want}");
    }

    #[test]
    fn copy_estimates_use_the_source_bitrate_and_the_sound_adds_its_own() {
        let t = VideoTarget::Mp4;
        let tune = VideoTune::default();
        let silent = estimate_bytes(t, &VideoSetting::Original, &clip(), &tune).unwrap();
        assert_eq!(silent, 8000 * 1000 / 8 * 120);
        let unknown = VideoInfo {
            video_kbps: None,
            ..clip()
        };
        assert_eq!(
            estimate_bytes(t, &VideoSetting::Original, &unknown, &tune),
            None
        );
        // Keep: the source sound's bitrate when known, else 128.
        let with = |kbps| VideoInfo {
            sound: Some(sound(Codec::Aac, kbps)),
            ..clip()
        };
        let q = VideoSetting::Quality(65);
        let base = estimate_bytes(t, &q, &clip(), &tune).unwrap();
        let sound_bytes = |kbps: u64| kbps * 1000 / 8 * 120;
        assert_eq!(
            estimate_bytes(t, &q, &with(Some(192)), &tune).unwrap(),
            base + sound_bytes(192)
        );
        assert_eq!(
            estimate_bytes(t, &q, &with(None), &tune).unwrap(),
            base + sound_bytes(128)
        );
        // Kbps(k) uses k, whatever the source had.
        let chosen = VideoTune {
            sound: SoundChoice::Kbps(96),
            ..tune
        };
        assert_eq!(
            estimate_bytes(t, &q, &with(Some(192)), &chosen).unwrap(),
            base + sound_bytes(96)
        );
        // A silent file adds nothing, even when asked for a sound bitrate.
        assert_eq!(estimate_bytes(t, &q, &clip(), &chosen).unwrap(), base);
        // Sound Kiln can't read is still sound: 128.
        let unreadable = VideoInfo {
            sound_unreadable: Some("DTS".into()),
            ..clip()
        };
        assert_eq!(
            estimate_bytes(t, &q, &unreadable, &tune).unwrap(),
            base + sound_bytes(128)
        );
    }

    #[test]
    fn a_rate_too_fast_to_time_has_no_frame_length() {
        // A frame length that rounds to 0 ns is none (MKV's DefaultDuration
        // would say frames take no time).
        let at = |fps| VideoInfo {
            fps: Some(fps),
            ..clip()
        };
        assert_eq!(at(25.0).frame_ns(), Some(40_000_000));
        assert_eq!(at(1e12).frame_ns(), None);
    }

    #[test]
    fn keep_counts_the_source_only_when_the_sound_is_copied() {
        // R90: PCM, FLAC and AC-3 can't go into MP4 as they are, so Keep
        // encodes them as AAC at the High preset's 160 kbps for stereo, and
        // that is what is counted, not the source's bitrate.
        let q = VideoSetting::Quality(65);
        let tune = VideoTune::default();
        let with = |codec, kbps| VideoInfo {
            sound: Some(sound(codec, Some(kbps))),
            ..clip()
        };
        let pcm = with(
            Codec::Pcm {
                bits: 16,
                float: false,
            },
            1536,
        );
        let flac = with(Codec::Flac, 900);
        let ac3 = with(Codec::Ac3, 448);
        for info in [&pcm, &flac, &ac3] {
            assert_eq!(sound_kbps(VideoTarget::Mp4, info, &tune), 160.0);
            assert_eq!(sound_kbps(VideoTarget::Mov, info, &tune), 160.0);
        }
        // MKV holds FLAC and AC-3 as they are: their own bitrate. PCM
        // becomes Opus at the High preset's 96.
        assert_eq!(sound_kbps(VideoTarget::Mkv, &flac, &tune), 900.0);
        assert_eq!(sound_kbps(VideoTarget::Mkv, &ac3, &tune), 448.0);
        assert_eq!(sound_kbps(VideoTarget::Mkv, &pcm, &tune), 96.0);
        // AAC copies everywhere.
        let aac = with(Codec::Aac, 256);
        for t in ALL {
            assert_eq!(sound_kbps(t, &aac, &tune), 256.0);
        }
        // A chosen bitrate is the step the encoder lists nearest it.
        let chosen = VideoTune {
            sound: SoundChoice::Kbps(128),
            ..tune
        };
        assert_eq!(sound_kbps(VideoTarget::Mp4, &pcm, &chosen), 128.0);
        // The estimate counts it too.
        let base = estimate_bytes(VideoTarget::Mp4, &q, &clip(), &tune).unwrap();
        assert_eq!(
            estimate_bytes(VideoTarget::Mp4, &q, &pcm, &tune).unwrap(),
            base + 160 * 1000 / 8 * 120
        );
    }

    #[test]
    fn keep_encodes_at_the_presets_bitrate() {
        // R95: sound Keep can't copy follows the preset, a stereo pair at
        // AAC 96, 160 or 192 and Opus 64, 96 or 160.
        let with = |codec, kbps| VideoInfo {
            sound: Some(sound(codec, Some(kbps))),
            ..clip()
        };
        let pcm = with(
            Codec::Pcm {
                bits: 16,
                float: false,
            },
            1536,
        );
        let flac = with(Codec::Flac, 900);
        let ac3 = with(Codec::Ac3, 448);
        for (preset, aac, opus) in [
            (Preset::Small, 96.0, 64.0),
            (Preset::High, 160.0, 96.0),
            (Preset::Best, 192.0, 160.0),
        ] {
            let tune = VideoTune {
                sound_preset: preset,
                ..VideoTune::default()
            };
            for info in [&pcm, &flac, &ac3] {
                assert_eq!(sound_kbps(VideoTarget::Mp4, info, &tune), aac, "{preset:?}");
                // The estimate counts the same.
                let q = VideoSetting::Quality(65);
                let silent = estimate_bytes(VideoTarget::Mp4, &q, &clip(), &tune).unwrap();
                let bytes = estimate_bytes(VideoTarget::Mp4, &q, info, &tune).unwrap();
                assert_eq!(bytes - silent, (aac * 1000.0 / 8.0 * 120.0) as u64);
            }
            assert_eq!(
                sound_kbps(VideoTarget::Mkv, &pcm, &tune),
                opus,
                "{preset:?}"
            );
            // Copied sound counts its own bitrate whatever the preset.
            assert_eq!(sound_kbps(VideoTarget::Mkv, &flac, &tune), 900.0);
            // A chosen bitrate is the chosen bitrate.
            let chosen = VideoTune {
                sound: SoundChoice::Kbps(128),
                ..tune
            };
            assert_eq!(sound_kbps(VideoTarget::Mp4, &pcm, &chosen), 128.0);
        }
    }

    #[test]
    fn fit_under_reaches_a_limit_that_holds_the_encoded_sound() {
        // A 2-minute clip with PCM sound (1536 kbps) into MP4: the sound is
        // written as 160 kbps AAC, so a 6 MB limit leaves room for the
        // picture (240 kbps of video after the margin). Counting the PCM
        // would have refused anything under about 25 MB.
        let info = VideoInfo {
            sound: Some(sound(
                Codec::Pcm {
                    bits: 16,
                    float: false,
                },
                Some(1536),
            )),
            ..clip()
        };
        let tune = VideoTune::default();
        let t = VideoTarget::Mp4;
        let Ok(VideoSetting::Kbps(k)) = fit_under(t, 6_000_000, &info, &tune) else {
            panic!("expected a bitrate");
        };
        assert_eq!(
            k,
            ((6_000_000.0 * 8.0 / 120.0 / 1000.0 - 160.0) * 0.97) as u32
        );
        // TooSmall's floor counts the same sound.
        let Err(MediaError::TooSmall { min_bytes, .. }) = fit_under(t, 1, &info, &tune) else {
            panic!("expected TooSmall");
        };
        let want = (150.0 / 0.97 + 160.0) * 1000.0 / 8.0 * 120.0;
        assert!((min_bytes as f64 - want).abs() <= 1.0, "{min_bytes}");
    }

    #[test]
    fn an_unknown_length_has_no_estimate_and_fit_under_returns_the_default() {
        let info = VideoInfo {
            duration: None,
            ..clip()
        };
        let tune = VideoTune::default();
        for s in [
            VideoSetting::Original,
            VideoSetting::Quality(65),
            VideoSetting::Kbps(4000),
        ] {
            assert_eq!(estimate_bytes(VideoTarget::Mp4, &s, &info, &tune), None);
        }
        assert_eq!(
            fit_under(VideoTarget::Mp4, 10, &info, &tune).unwrap(),
            VideoSetting::Original
        );
        let av1 = VideoInfo {
            codec: VideoCodec::Av1,
            ..info
        };
        assert_eq!(
            fit_under(VideoTarget::Mp4, 10, &av1, &tune).unwrap(),
            VideoSetting::Quality(65)
        );
        assert_eq!(quality_under(VideoTarget::Mp4, u64::MAX, &av1, &tune), None);
    }

    #[test]
    fn an_unknown_frame_rate_estimates_at_30() {
        let t = VideoTarget::Mp4;
        let tune = VideoTune::default();
        let unknown = VideoInfo {
            fps: None,
            ..clip()
        };
        let q = VideoSetting::Quality(65);
        assert_eq!(
            estimate_bytes(t, &q, &unknown, &tune),
            estimate_bytes(t, &q, &clip(), &tune)
        );
    }

    #[test]
    fn fit_under_25_mb_for_a_two_minute_1080p_clip() {
        let want = (25_000_000.0 * 8.0 / 120.0 - 128_000.0) / 1000.0 * 0.97;
        let near = |s: VideoSetting| match s {
            VideoSetting::Kbps(k) => assert!((k as f64 - want).abs() <= 1.0, "{k} vs {want}"),
            other => panic!("{other:?}"),
        };
        let info = VideoInfo {
            sound: Some(sound(Codec::Aac, Some(128))),
            ..clip()
        };
        let chosen = VideoTune {
            sound: SoundChoice::Kbps(128),
            ..VideoTune::default()
        };
        let keep = VideoTune::default();
        near(fit_under(VideoTarget::Mp4, 25_000_000, &info, &chosen).unwrap());
        near(fit_under(VideoTarget::Mp4, 25_000_000, &info, &keep).unwrap());
        // Keep with an unknown sound bitrate is 128 too.
        let unknown = VideoInfo {
            sound: Some(sound(Codec::Aac, None)),
            ..clip()
        };
        near(fit_under(VideoTarget::Mp4, 25_000_000, &unknown, &keep).unwrap());
        // The result lands under the limit.
        let VideoSetting::Kbps(k) = fit_under(VideoTarget::Mp4, 25_000_000, &info, &keep).unwrap()
        else {
            unreachable!()
        };
        let bytes = estimate_bytes(VideoTarget::Mp4, &VideoSetting::Kbps(k), &info, &keep).unwrap();
        assert!(bytes < 25_000_000, "{bytes}");
    }

    #[test]
    fn fit_under_too_small_says_so() {
        let info = VideoInfo {
            sound: Some(sound(Codec::Aac, Some(128))),
            ..clip()
        };
        let tune = VideoTune::default();
        let t = VideoTarget::Mkv;
        let Err(MediaError::TooSmall { format, min_bytes }) = fit_under(t, 1_000_000, &info, &tune)
        else {
            panic!("expected TooSmall");
        };
        assert_eq!(format, "MKV");
        // 150 kbps of video (3% over for the container) and 128 of sound.
        let want = (150.0 / 0.97 + 128.0) * 1000.0 / 8.0 * 120.0;
        assert!((min_bytes as f64 - want).abs() <= 1.0, "{min_bytes}");
        // That limit is the smallest that works, and it gives the floor.
        assert_eq!(
            fit_under(t, min_bytes, &info, &tune).unwrap(),
            VideoSetting::Kbps(150)
        );
        assert!(fit_under(t, min_bytes - 1, &info, &tune).is_err());
        for t in ALL {
            let Err(e) = fit_under(t, 1, &info, &tune) else {
                panic!("expected TooSmall");
            };
            assert!(
                e.to_string()
                    .starts_with(&format!("{} can't go under ", t.name())),
                "{e}"
            );
        }
        assert_eq!(
            fit_under(t, 1, &info, &tune).unwrap_err().to_string(),
            format!("MKV can't go under {} MB for this file.", fmt_mb(min_bytes))
        );
    }

    #[test]
    fn quality_under_picks_the_highest_step_with_headroom() {
        let t = VideoTarget::Mp4;
        let tune = VideoTune::default();
        let info = clip();
        let est = |q| estimate_bytes(t, &VideoSetting::Quality(q), &info, &tune).unwrap();
        // Room for exactly quality 50 and its 15% headroom.
        let limit = (est(50) as f64 * 1.15).ceil() as u64;
        assert_eq!(quality_under(t, limit, &info, &tune), Some(50));
        assert_eq!(quality_under(t, limit - 1, &info, &tune), Some(45));
        assert_eq!(quality_under(t, u64::MAX, &info, &tune), Some(90));
        assert_eq!(quality_under(t, est(20), &info, &tune), None);
        assert_eq!(quality_under(t, 0, &info, &tune), None);
    }

    #[test]
    fn target_names() {
        assert_eq!(ALL.map(VideoTarget::name), ["MP4", "MOV", "M4V", "MKV"]);
    }

    #[test]
    fn a_rotated_source_labels_original_by_the_target() {
        let turned = VideoInfo {
            rotation: 90,
            ..clip()
        };
        let tune = VideoTune::default();
        let last = |t| steps(t, &turned, &tune).pop().unwrap().label;
        assert_eq!(last(VideoTarget::Mkv), "Original · highest quality");
        assert_eq!(last(VideoTarget::Mp4), "Original · copies the video");
        assert_eq!(
            label_of(VideoTarget::Mkv, &VideoSetting::Original, &turned, &tune),
            "Original · highest quality"
        );
        assert_eq!(
            label_of(VideoTarget::Mov, &VideoSetting::Original, &turned, &tune),
            "Original · copies the video"
        );
    }

    #[test]
    fn a_frame_rate_a_hair_over_the_cap_still_copies() {
        let t = VideoTarget::Mp4;
        let tune = VideoTune {
            max_fps: Some(30),
            ..VideoTune::default()
        };
        let near = VideoInfo {
            fps: Some(30.004),
            ..clip()
        };
        assert!(can_copy_video(t, &near, &tune));
        assert_eq!(written_fps(&near, &tune), Some(30.004));
        // A real excess does not: 30.2 is over the 0.5% allowance.
        let over = VideoInfo {
            fps: Some(30.2),
            ..clip()
        };
        assert!(!can_copy_video(t, &over, &tune));
        assert_eq!(written_fps(&over, &tune), Some(30.0));
        // 29.97 under a cap of 30 is under, as before.
        let ntsc = VideoInfo {
            fps: Some(29.97),
            ..clip()
        };
        assert!(can_copy_video(t, &ntsc, &tune));
        assert_eq!(written_fps(&ntsc, &tune), Some(29.97));
    }

    #[test]
    fn a_turn_of_180_copies_into_mp4_and_not_into_mkv() {
        let upside_down = VideoInfo {
            rotation: 180,
            ..clip()
        };
        let tune = VideoTune::default();
        assert!(can_copy_video(VideoTarget::Mp4, &upside_down, &tune));
        assert!(can_copy_video(VideoTarget::Mov, &upside_down, &tune));
        assert!(!can_copy_video(VideoTarget::Mkv, &upside_down, &tune));
    }

    #[test]
    fn a_zero_length_is_an_unknown_length() {
        let info = VideoInfo {
            duration: Some(0.0),
            ..clip()
        };
        let tune = VideoTune::default();
        assert_eq!(
            estimate_bytes(VideoTarget::Mp4, &VideoSetting::Quality(65), &info, &tune),
            None
        );
        assert_eq!(
            fit_under(VideoTarget::Mp4, 1, &info, &tune).unwrap(),
            VideoSetting::Original
        );
        assert_eq!(
            quality_under(VideoTarget::Mp4, u64::MAX, &info, &tune),
            None
        );
        let label = &steps(VideoTarget::Mp4, &info, &tune)[9].label;
        assert!(!label.contains(" MB"), "{label}");
    }

    #[test]
    fn fit_under_has_a_ceiling() {
        let tune = VideoTune::default();
        // A copy: 8000 kbps for 120 s is 120 MB, so 200 MB and u64::MAX copy.
        for max in [200_000_000, u64::MAX] {
            assert_eq!(
                fit_under(VideoTarget::Mp4, max, &clip(), &tune).unwrap(),
                VideoSetting::Original,
                "{max}"
            );
        }
        // Just under the size of the copy is a bitrate.
        let copy =
            estimate_bytes(VideoTarget::Mp4, &VideoSetting::Original, &clip(), &tune).unwrap();
        assert_eq!(
            fit_under(VideoTarget::Mp4, copy, &clip(), &tune).unwrap(),
            VideoSetting::Original
        );
        assert!(matches!(
            fit_under(VideoTarget::Mp4, copy - 1, &clip(), &tune).unwrap(),
            VideoSetting::Kbps(_)
        ));
        // A picture that cannot copy gets the top quality step.
        let av1 = VideoInfo {
            codec: VideoCodec::Av1,
            ..clip()
        };
        assert_eq!(
            fit_under(VideoTarget::Mp4, u64::MAX, &av1, &tune).unwrap(),
            VideoSetting::Quality(90)
        );
        // A copy whose bitrate is unknown has no estimate of its own, so the
        // top quality step's stands in for it.
        let unknown = VideoInfo {
            video_kbps: None,
            ..clip()
        };
        assert_eq!(
            fit_under(VideoTarget::Mp4, u64::MAX, &unknown, &tune).unwrap(),
            VideoSetting::Original
        );
        let top = estimate_bytes(
            VideoTarget::Mp4,
            &VideoSetting::Quality(90),
            &unknown,
            &tune,
        )
        .unwrap();
        assert_eq!(
            fit_under(VideoTarget::Mp4, top, &unknown, &tune).unwrap(),
            VideoSetting::Original
        );
        assert!(matches!(
            fit_under(VideoTarget::Mp4, top - 1, &unknown, &tune).unwrap(),
            VideoSetting::Kbps(_)
        ));
    }

    #[test]
    fn what_fit_under_picks_fits_under_the_limit() {
        let sounds = [
            SoundChoice::Keep,
            SoundChoice::Kbps(64),
            SoundChoice::Kbps(320),
        ];
        for duration in [0.5, 7.0, 120.0, 3600.0] {
            for sound_choice in sounds {
                for codec in [VCodec::H264, VCodec::Hevc] {
                    for (with_sound, video_kbps) in [
                        (None, Some(8000)),
                        (Some(sound(Codec::Aac, Some(192))), Some(8000)),
                        // A copy whose bitrate the container doesn't give.
                        (None, None),
                        (Some(sound(Codec::Aac, None)), None),
                    ] {
                        let info = VideoInfo {
                            duration: Some(duration),
                            sound: with_sound,
                            video_kbps,
                            ..clip()
                        };
                        let tune = VideoTune {
                            sound: sound_choice,
                            codec,
                            ..VideoTune::default()
                        };
                        for max in [
                            1_000,
                            50_000,
                            1_000_000,
                            25_000_000,
                            500_000_000,
                            50_000_000_000,
                            u64::MAX,
                        ] {
                            for t in ALL {
                                let Ok(setting) = fit_under(t, max, &info, &tune) else {
                                    continue;
                                };
                                // A copy of unknown bitrate has no estimate.
                                if let Some(bytes) = estimate_bytes(t, &setting, &info, &tune) {
                                    assert!(
                                        bytes <= max,
                                        "{t:?} {duration}s {sound_choice:?} {codec:?}: {bytes} > {max}"
                                    );
                                }
                                // A bitrate is never above the top quality's.
                                if let VideoSetting::Kbps(k) = setting {
                                    let top = quality_kbps(QUALITY_LAST, &info, &tune);
                                    assert!(k as f64 <= top, "{t:?} {max}: {k} > {top}");
                                }
                                if info.video_kbps.is_none() && max == u64::MAX {
                                    assert_eq!(setting, VideoSetting::Original, "{t:?}");
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn fit_under_for_hevc_counts_the_same_bitrate() {
        // The bitrate is the picture's, whichever codec writes it.
        let hevc = VideoTune {
            codec: VCodec::Hevc,
            ..VideoTune::default()
        };
        let h264 = VideoTune::default();
        assert_eq!(
            fit_under(VideoTarget::Mp4, 25_000_000, &clip(), &hevc).unwrap(),
            fit_under(VideoTarget::Mp4, 25_000_000, &clip(), &h264).unwrap()
        );
        // And quality_under finds a lower step for HEVC, which the
        // processor's table makes the larger (R85).
        let qh = quality_under(VideoTarget::Mp4, 60_000_000, &clip(), &hevc).unwrap();
        let q2 = quality_under(VideoTarget::Mp4, 60_000_000, &clip(), &h264).unwrap();
        assert!(qh < q2, "{qh} {q2}");
    }

    #[test]
    fn sound_off_counts_no_sound() {
        let loud = VideoInfo {
            sound: Some(sound(Codec::Aac, Some(256))),
            sound_unreadable: None,
            ..clip()
        };
        let off = VideoTune {
            sound: SoundChoice::Off,
            ..VideoTune::default()
        };
        let silent = VideoInfo {
            sound: None,
            ..loud.clone()
        };
        for t in ALL {
            assert_eq!(sound_kbps(t, &loud, &off), 0.0, "{t:?}");
            let s = VideoSetting::Kbps(2000);
            assert_eq!(
                estimate_bytes(t, &s, &loud, &off),
                estimate_bytes(t, &s, &silent, &VideoTune::default())
            );
        }
        // Unreadable sound left out counts nothing either.
        let dts = VideoInfo {
            sound: None,
            sound_unreadable: Some("DTS".into()),
            ..clip()
        };
        assert_eq!(sound_kbps(VideoTarget::Mp4, &dts, &off), 0.0);
    }

    #[test]
    fn a_flat_low_rate_source_is_never_estimated_over_its_own_size() {
        // A screen recording: 1080p at 30 fps in 1000 kbps, 0.016 bits a
        // pixel, far under the table at any step (ENG-14: 26x high).
        let screen = VideoInfo {
            video_kbps: Some(1000),
            sound: Some(sound(Codec::Aac, Some(128))),
            ..clip()
        };
        let secs = screen.duration.unwrap();
        let source = ((1000.0 + 128.0) * 1000.0 / 8.0 * secs).round() as u64;
        for card in [false, true] {
            CARD_ENCODES.with(|c| c.set(card));
            for codec in [VCodec::H264, VCodec::Hevc] {
                let tune = VideoTune {
                    codec,
                    ..VideoTune::default()
                };
                let every = qualities()
                    .map(VideoSetting::Quality)
                    .chain([VideoSetting::Original]);
                for s in every {
                    for t in ALL {
                        let got = estimate_bytes(t, &s, &screen, &tune).unwrap();
                        assert!(
                            got <= source,
                            "{card} {codec:?} {s:?} {t:?}: {got} > {source}"
                        );
                    }
                }
            }
        }
        CARD_ENCODES.with(|c| c.set(false));
        // Under the cap, the step's own bits a pixel: a richer source keeps
        // the table's figure, as before.
        let tune = VideoTune::default();
        let rich = clip();
        let table = h264_bpp(PROCESSOR, 45, &rich) * 1920.0 * 1080.0 * 30.0 / 1000.0;
        let got = estimate_bytes(VideoTarget::Mp4, &VideoSetting::Quality(45), &rich, &tune);
        assert_eq!(got, Some((table * 1000.0 / 8.0 * 120.0).round() as u64));
        // At the cap the step is the source's own bits a pixel, so a picture
        // made smaller is smaller still.
        let small = VideoTune {
            max_lines: Some(720),
            ..tune
        };
        let q = VideoSetting::Quality(65);
        let full = estimate_bytes(VideoTarget::Mp4, &q, &screen, &tune).unwrap();
        let less = estimate_bytes(VideoTarget::Mp4, &q, &screen, &small).unwrap();
        assert!(less < full, "{less} {full}");
    }

    #[test]
    fn a_smaller_picture_of_a_low_rate_source_spends_more_bits_a_pixel() {
        // The cap is the source's own bits a pixel at its own picture size
        // and frame rate. A smaller picture or fewer frames need more bits
        // a pixel, so there the cap is looser, though still under the
        // source's whole rate.
        let screen = VideoInfo {
            video_kbps: Some(1000),
            sound: None,
            ..clip()
        };
        let tune = VideoTune::default();
        let q = VideoSetting::Quality(65);
        let full = estimate_bytes(VideoTarget::Mp4, &q, &screen, &tune).unwrap() as f64;
        let share = |w: f64, h: f64, fps: f64| w * h * fps / (1920.0 * 1080.0 * 30.0);
        let small = VideoTune {
            max_lines: Some(720),
            ..tune
        };
        let less = estimate_bytes(VideoTarget::Mp4, &q, &screen, &small).unwrap() as f64;
        let pixels = share(1280.0, 720.0, 30.0);
        assert!(less > full * pixels * 1.2, "{less} {full}");
        assert!(less < full, "{less} {full}");
        let slow = VideoTune {
            max_fps: Some(15),
            ..tune
        };
        let fewer = estimate_bytes(VideoTarget::Mp4, &q, &screen, &slow).unwrap() as f64;
        assert!(
            fewer > full * share(1920.0, 1080.0, 15.0) * 1.2,
            "{fewer} {full}"
        );
        assert!(fewer < full, "{fewer} {full}");
    }
}
