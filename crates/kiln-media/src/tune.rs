//! How the owner wants the audio written: the targets, every setting, the
//! slider steps for each format, the presets, size estimates and Fit under.

use crate::error::{MediaError, Result};
use crate::types::{Codec, SourceInfo};

/// An output format.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Target {
    Wav,
    Aiff,
    Flac,
    WavPack,
    Mp3,
    Aac,
    M4a,
    Ogg,
    Opus,
    Wma,
    Ac3,
}

impl Target {
    /// The target for a file extension (case-insensitive, with or without the dot).
    pub fn from_ext(ext: &str) -> Option<Target> {
        match ext.trim_start_matches('.').to_ascii_lowercase().as_str() {
            "wav" => Some(Target::Wav),
            "aiff" | "aif" => Some(Target::Aiff),
            "flac" => Some(Target::Flac),
            "wv" => Some(Target::WavPack),
            "mp3" => Some(Target::Mp3),
            "aac" => Some(Target::Aac),
            "m4a" => Some(Target::M4a),
            "ogg" => Some(Target::Ogg),
            "opus" => Some(Target::Opus),
            "wma" => Some(Target::Wma),
            "ac3" => Some(Target::Ac3),
            _ => None,
        }
    }

    /// The lower-case file extension, without the dot.
    pub fn ext(self) -> &'static str {
        match self {
            Target::Wav => "wav",
            Target::Aiff => "aiff",
            Target::Flac => "flac",
            Target::WavPack => "wv",
            Target::Mp3 => "mp3",
            Target::Aac => "aac",
            Target::M4a => "m4a",
            Target::Ogg => "ogg",
            Target::Opus => "opus",
            Target::Wma => "wma",
            Target::Ac3 => "ac3",
        }
    }

    /// The upper-case code, e.g. "MP3".
    pub fn code(self) -> &'static str {
        match self {
            Target::Wav => "WAV",
            Target::Aiff => "AIFF",
            Target::Flac => "FLAC",
            Target::WavPack => "WV",
            Target::Mp3 => "MP3",
            Target::Aac => "AAC",
            Target::M4a => "M4A",
            Target::Ogg => "OGG",
            Target::Opus => "OPUS",
            Target::Wma => "WMA",
            Target::Ac3 => "AC3",
        }
    }

    /// The format's display name, as used in copy such as "Vorbis can't go under 2 MB".
    pub fn name(self) -> &'static str {
        match self {
            Target::Ogg => "Vorbis",
            Target::Opus => "Opus",
            Target::WavPack => "WavPack",
            other => other.code(),
        }
    }

    /// True for the formats that keep every sample.
    pub fn lossless(self) -> bool {
        matches!(
            self,
            Target::Wav | Target::Aiff | Target::Flac | Target::WavPack
        )
    }
}

/// The three named quality presets. High is the default.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Preset {
    Small,
    #[default]
    High,
    Best,
}

/// What a sink should write.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Setting {
    /// Plain PCM: integer at `bits` (16, 24 or 32), or IEEE float when `float`.
    Pcm { bits: u16, float: bool },
    /// LAME variable bitrate. `tenths` is the V number times ten: V0 = 0, V9 = 90.
    LameVbr { tenths: u8 },
    /// LAME constant bitrate.
    LameCbr { kbps: u32 },
    /// LAME average bitrate.
    LameAbr { kbps: u32 },
    /// Vorbis quality times ten, -10..=100.
    VorbisQ { tenths: i16 },
    /// Vorbis managed (average) bitrate.
    VorbisAbr { kbps: u32 },
    /// Opus at a bitrate, tuned for speech or for music.
    Opus { kbps: u32, speech: bool },
    /// A constant bitrate: AAC, WMA and AC3.
    Kbps { kbps: u32 },
    /// FLAC compression level 0 to 8. `bits` overrides the source depth.
    FlacLevel { level: u8, bits: Option<u16> },
    /// WavPack mode, 0 fast to 3 very high. `bits` overrides the source depth.
    WavPack { mode: u8, bits: Option<u16> },
}

/// One stop on a quality slider.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Step {
    pub setting: Setting,
    pub label: String,
    pub kbps: Option<u32>,
}

/// Whether the owner wants the channels kept, or folded to stereo or mono.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum ChannelChoice {
    #[default]
    Keep,
    Stereo,
    Mono,
}

/// The owner's tuning for one conversion. `None` setting means "follow the preset".
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Tune {
    pub setting: Option<Setting>,
    pub fit_bytes: Option<u64>,
    pub rate: Option<u32>,
    pub channels: ChannelChoice,
    pub bits: Option<u16>,
    /// A bitrate to write at when `setting` is unset, as the format's step
    /// nearest to it: what a saved default or the command line asks for.
    /// Fit under comes first; lossless formats ignore it.
    pub kbps: Option<u32>,
}

/// The setting `t` is written with for `info` (the audio as written, after
/// the rate and channel limits): Tune's own setting, else Fit under's, else
/// for WAV and AIFF the PCM Tune's depth asks for, else the step nearest
/// Tune's bitrate, else the preset's. A bitrate for AAC, M4A, WMA or AC3 that
/// this sound's list lacks becomes the nearest it has: a setting worked out
/// for one file of several (stereo, 44.1 kHz) may not be one Windows takes
/// for another (mono, 48 kHz, 5.1).
pub fn chosen_setting(
    t: Target,
    preset: Preset,
    tune: &Tune,
    info: &SourceInfo,
) -> Result<Setting> {
    Ok(match (tune.setting, tune.fit_bytes) {
        (Some(s), _) => listed(t, s, info),
        (None, Some(bytes)) if !t.lossless() => fit_under(t, bytes, info)?,
        (None, _) if matches!(t, Target::Wav | Target::Aiff) => pcm_setting(tune.bits, info),
        (None, _) => {
            let near = tune.kbps.filter(|_| !t.lossless()).and_then(|k| {
                steps(t, info)
                    .get(nearest_step(t, k, info))
                    .map(|s| s.setting)
            });
            near.unwrap_or_else(|| preset_setting(t, preset, info))
        }
    })
}

/// `s`, or for a bitrate `t`'s list for `info` lacks, the step nearest it.
/// AAC's lists are totals, so a stereo bitrate meets a 5.1 list at three
/// times itself, as the presets do (and a 5.1 total meets stereo at a
/// third): the High of a stereo stand-in, 160, is 480 for 5.1, not 128.
fn listed(t: Target, s: Setting, info: &SourceInfo) -> Setting {
    let Setting::Kbps { kbps } = s else {
        return s;
    };
    if !matches!(t, Target::Aac | Target::M4a | Target::Wma | Target::Ac3) {
        return s;
    }
    let list = steps(t, info);
    if list.is_empty() || list.iter().any(|x| x.setting == s) {
        return s;
    }
    let stereo_top = crate::mf::aac::KBPS[crate::mf::aac::KBPS.len() - 1];
    let wanted = match t {
        Target::Aac | Target::M4a if aac_pairs(info) > 1 && kbps <= stereo_top => {
            kbps * aac_pairs(info)
        }
        Target::Aac | Target::M4a if aac_pairs(info) == 1 && kbps > stereo_top => kbps / 3,
        _ => kbps,
    };
    list[nearest_step(t, wanted, info)].setting
}

/// WAV and AIFF: the depth Tune asks for (32 is float), else the source's,
/// 17 to 24 bits in 24 and float in float.
fn pcm_setting(tune_bits: Option<u16>, info: &SourceInfo) -> Setting {
    let float = |bits| Setting::Pcm { bits, float: true };
    let int = |bits| Setting::Pcm { bits, float: false };
    match tune_bits {
        Some(32) => float(32),
        Some(b) => int(b),
        None => match (&info.codec, info.bits) {
            (Codec::Pcm { float: true, .. }, _) => float(32),
            // 17 to 24 bits (20-bit, say) in 24, as for FLAC and WavPack.
            (_, Some(b)) if b > 24 => int(32),
            (_, Some(b)) if b > 16 => int(24),
            _ => int(16),
        },
    }
}

/// The constant bitrates Advanced offers for MP3, which are not on the slider.
pub const MP3_CBR_KBPS: [u32; 14] = [
    32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
];

const OPUS_KBPS: [u32; 21] = [
    6, 8, 12, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 256, 320, 384, 450, 510,
];

/// MP3 VBR kbps for a stereo pair, V0 to V9. Between whole levels the slider's
/// half steps are interpolated. Both this table and `VORBIS_KBPS` are the
/// median bitrate Task 16's harness measured on ten music clips (piano, rock,
/// orchestral, percussion, harpsichord and 5.1 film audio, a minute of each,
/// with 5.1 divided by its channel scale); speech is far lower and was left
/// out (R42). The estimates and the "about N kbps" labels both read these.
const MP3_VBR_KBPS: [u32; 10] = [244, 205, 173, 154, 134, 109, 92, 81, 79, 61];
/// Vorbis kbps for a stereo pair at q -1 to q 10, measured as above.
const VORBIS_KBPS: [u32; 12] = [43, 58, 74, 89, 110, 121, 143, 167, 194, 217, 290, 418];

const MP3_ABR_MAX: u32 = 320;
const VORBIS_ABR_MAX: u32 = 500;
const OPUS_MAX: u32 = 510;
const MP3_FLOOR: u32 = 32;
const VORBIS_FLOOR: u32 = 45;
const OPUS_FLOOR: u32 = 6;
/// Fit under aims 2% below the limit to leave room for tags and container overhead.
const FIT_MARGIN: f64 = 0.98;

/// Linear interpolation in a table whose entries are one whole unit apart;
/// `pos_tenths` is the position in tenths of a unit.
fn interpolate(table: &[u32], pos_tenths: usize) -> f64 {
    let i = pos_tenths / 10;
    let rem = (pos_tenths % 10) as f64;
    let a = table[i.min(table.len() - 1)] as f64;
    let b = table[(i + 1).min(table.len() - 1)] as f64;
    a + (b - a) * rem / 10.0
}

/// A number given in tenths, written with a decimal only when it isn't whole.
fn fmt_tenths(tenths: i32) -> String {
    let sign = if tenths < 0 { "-" } else { "" };
    let a = tenths.abs();
    if a % 10 == 0 {
        format!("{sign}{}", a / 10)
    } else {
        format!("{sign}{}.{}", a / 10, a % 10)
    }
}

fn pick<T>(p: Preset, small: T, high: T, best: T) -> T {
    match p {
        Preset::Small => small,
        Preset::High => high,
        Preset::Best => best,
    }
}

/// Bitrate of a lossy setting at 44.1 kHz stereo, before any channel scaling.
pub(crate) fn setting_kbps(s: &Setting) -> f64 {
    match *s {
        Setting::LameVbr { tenths } => interpolate(&MP3_VBR_KBPS, tenths as usize),
        Setting::VorbisQ { tenths } => {
            interpolate(&VORBIS_KBPS, (tenths.clamp(-10, 100) + 10) as usize)
        }
        Setting::LameCbr { kbps }
        | Setting::LameAbr { kbps }
        | Setting::VorbisAbr { kbps }
        | Setting::Opus { kbps, .. }
        | Setting::Kbps { kbps } => kbps as f64,
        Setting::Pcm { .. } | Setting::FlacLevel { .. } | Setting::WavPack { .. } => 0.0,
    }
}

/// VBR sizes scale with the channel count: mono x 0.6, more than two x channels / 2.
/// Public only for the quality harness's calibration; not a stable API.
#[doc(hidden)]
pub fn channel_scale(channels: u16) -> f64 {
    match channels {
        1 => 0.6,
        0 | 2 => 1.0,
        n => n as f64 / 2.0,
    }
}

fn pcm_bytes(info: &SourceInfo, bits: u16) -> Option<u64> {
    let frames = info.frames?;
    Some(frames * info.channels.count as u64 * bits as u64 / 8 + 44)
}

fn label_text(s: &Setting) -> String {
    match *s {
        Setting::Pcm { bits, float: true } => format!("{bits}-bit float"),
        Setting::Pcm { bits, float: false } => format!("{bits}-bit"),
        Setting::LameVbr { tenths } => format!(
            "V{} · about {} kbps",
            fmt_tenths(tenths as i32),
            setting_kbps(s).round() as u32
        ),
        Setting::LameCbr { kbps } => format!("{kbps} kbps CBR"),
        Setting::LameAbr { kbps } | Setting::VorbisAbr { kbps } => format!("{kbps} kbps ABR"),
        Setting::VorbisQ { tenths } => format!(
            "q{} · about {} kbps",
            fmt_tenths(tenths as i32),
            setting_kbps(s).round() as u32
        ),
        Setting::Opus { kbps, .. } | Setting::Kbps { kbps } => format!("{kbps} kbps"),
        Setting::FlacLevel { level: 8, .. } => "Level 8 · smallest".to_string(),
        Setting::FlacLevel { level: 0, .. } => "Level 0 · fastest".to_string(),
        Setting::FlacLevel { level, .. } => format!("Level {level}"),
        Setting::WavPack { mode, .. } => match mode {
            0 => "Fast",
            1 => "Normal",
            2 => "High",
            _ => "Very high",
        }
        .to_string(),
    }
}

/// The label for any setting, including Advanced choices that aren't on the
/// slider (for example "320 kbps CBR").
pub fn label_of(_t: Target, s: &Setting, _info: &SourceInfo) -> String {
    label_text(s)
}

fn step_of(setting: Setting) -> Step {
    let kbps = match setting {
        Setting::Pcm { .. } | Setting::FlacLevel { .. } | Setting::WavPack { .. } => None,
        _ => Some(setting_kbps(&setting).round() as u32),
    };
    Step {
        setting,
        label: label_text(&setting),
        kbps,
    }
}

/// AAC's channel pairs for a source: one for mono and stereo, three for
/// anything more, which is written as 5.1 (Windows' encoder takes no 3 to 5
/// channels, and 6 at most).
fn aac_pairs(info: &SourceInfo) -> u32 {
    if info.channels.count > 2 {
        3
    } else {
        1
    }
}

/// AAC's bitrates for a source, as totals: 96 to 192 kbps for mono and
/// stereo, and for more channels the 5.1 totals Windows' encoder takes at the
/// rate it will be written at (44.1 kHz, else 48 kHz).
fn aac_kbps(info: &SourceInfo) -> Vec<u32> {
    use crate::mf::aac;
    if aac_pairs(info) == 1 {
        return aac::KBPS.to_vec();
    }
    let rate = if info.rate == 44_100 { 44_100 } else { 48_000 };
    aac::bitrates(rate, 6)
}

/// WMA's bitrates for a source: what Windows' encoder lists for mono or
/// stereo (more channels are folded to stereo) at the source's rate, else at
/// 44.1 kHz; the fixed list when Media Foundation can't be asked.
fn wma_kbps(info: &SourceInfo) -> Vec<u32> {
    use crate::mf::wma;
    let ch = if info.channels.count == 1 { 1 } else { 2 };
    [info.rate, 44_100]
        .into_iter()
        .map(|rate| wma::bitrates(rate, ch))
        .find(|l| !l.is_empty())
        .unwrap_or_else(|| wma::FALLBACK_KBPS.to_vec())
}

/// AC3's bitrates for a source, at the 48 kHz AC3 is written at: what
/// Windows' encoder takes for mono or stereo, and every bitrate from 96 kbps
/// for surround, which another encoder serves.
fn ac3_kbps(info: &SourceInfo) -> Vec<u32> {
    use crate::mf::ac3;
    match info.channels.count {
        1 | 2 => ac3::bitrates(48_000, info.channels.count),
        0 => ac3::bitrates(48_000, 2),
        _ => ac3::SURROUND_KBPS.to_vec(),
    }
}

fn kbps_steps(t: Target, list: &[u32]) -> Vec<Step> {
    list.iter()
        .map(|&kbps| {
            step_of(match t {
                Target::Opus => Setting::Opus {
                    kbps,
                    speech: false,
                },
                _ => Setting::Kbps { kbps },
            })
        })
        .collect()
}

/// The slider steps for a target, ordered smallest file to best sound.
/// Empty for WAV and AIFF.
pub fn steps(t: Target, info: &SourceInfo) -> Vec<Step> {
    match t {
        Target::Wav | Target::Aiff => Vec::new(),
        Target::Mp3 => (0..=18u8)
            .map(|i| step_of(Setting::LameVbr { tenths: 90 - i * 5 }))
            .collect(),
        Target::Ogg => (-10..=100i16)
            .map(|tenths| step_of(Setting::VorbisQ { tenths }))
            .collect(),
        Target::Opus => kbps_steps(t, &OPUS_KBPS),
        Target::Aac | Target::M4a => kbps_steps(t, &aac_kbps(info)),
        Target::Wma => kbps_steps(t, &wma_kbps(info)),
        Target::Ac3 => kbps_steps(t, &ac3_kbps(info)),
        Target::Flac => (0..=8u8)
            .rev()
            .map(|level| step_of(Setting::FlacLevel { level, bits: None }))
            .collect(),
        Target::WavPack => (0..=3u8)
            .rev()
            .map(|mode| step_of(Setting::WavPack { mode, bits: None }))
            .collect(),
    }
}

/// The index into [`steps`] a preset lands on. 0 when the target has no steps.
pub fn preset_step(t: Target, p: Preset, info: &SourceInfo) -> usize {
    let wanted = match t {
        Target::Mp3 => Setting::LameVbr {
            tenths: pick(p, 50, 20, 0),
        },
        Target::Ogg => Setting::VorbisQ {
            tenths: pick(p, 30, 50, 70),
        },
        Target::Flac => Setting::FlacLevel {
            level: pick(p, 8, 5, 5),
            bits: None,
        },
        Target::WavPack => Setting::WavPack {
            mode: pick(p, 2, 1, 1),
            bits: None,
        },
        Target::Opus => return nearest_step(t, pick(p, 64, 96, 160), info),
        // Per stereo pair: 5.1 aims for three times as much.
        Target::Aac | Target::M4a => {
            let kbps = pick(p, 96, 160, 192) * aac_pairs(info);
            return nearest_step(t, kbps, info);
        }
        Target::Wma => return nearest_step(t, pick(p, 96, 128, 192), info),
        Target::Ac3 => {
            let kbps = match info.channels.count {
                1 => pick(p, 96, 128, 192),
                0 | 2 => pick(p, 192, 256, 448),
                _ => pick(p, 384, 448, 640),
            };
            return nearest_step(t, kbps, info);
        }
        Target::Wav | Target::Aiff => return 0,
    };
    steps(t, info)
        .iter()
        .position(|s| s.setting == wanted)
        .unwrap_or(0)
}

/// The index into [`steps`] whose bitrate is closest to `kbps`; the lower step
/// wins a tie. 0 when the target's steps carry no bitrate.
pub fn nearest_step(t: Target, kbps: u32, info: &SourceInfo) -> usize {
    steps(t, info)
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.kbps.map(|k| (i, k.abs_diff(kbps))))
        .min_by_key(|&(_, d)| d)
        .map_or(0, |(i, _)| i)
}

/// The setting a preset stands for. WAV and AIFF have no steps, so they get PCM
/// at the source depth (16, 24 or 32 bits; 16 when unknown).
pub(crate) fn preset_setting(t: Target, p: Preset, info: &SourceInfo) -> Setting {
    match steps(t, info).get(preset_step(t, p, info)) {
        Some(s) => s.setting,
        None => Setting::Pcm {
            bits: match info.bits {
                Some(24) => 24,
                Some(32) => 32,
                _ => 16,
            },
            float: false,
        },
    }
}

/// The expected size of the written file. `None` when the length is unknown and
/// the size depends on it.
///
/// MP3 VBR and Vorbis quality sizes scale with the channel count; CBR, ABR and
/// the other bitrate formats use the bitrate as given.
pub fn estimate_bytes(_t: Target, s: &Setting, info: &SourceInfo) -> Option<u64> {
    // The depth FLAC and WavPack write: the setting's, else the source's
    // integer depth up to 24, else 16 (a float source too), as the sinks do.
    let lossless = |ratio: f64, bits: Option<u16>| {
        let source = match info.codec {
            Codec::Pcm { float: true, .. } => None,
            _ => info.bits,
        };
        let pcm = pcm_bytes(info, crate::pcm::lossless_bits(bits, source))?;
        Some((pcm as f64 * ratio).round() as u64)
    };
    match *s {
        Setting::Pcm { bits, .. } => pcm_bytes(info, bits),
        Setting::FlacLevel { bits, .. } => lossless(0.58, bits),
        Setting::WavPack { bits, .. } => lossless(0.56, bits),
        Setting::LameVbr { .. } | Setting::VorbisQ { .. } => {
            let secs = info.duration_secs()?;
            let scale = channel_scale(info.channels.count);
            Some((setting_kbps(s) * 1000.0 / 8.0 * secs * scale).round() as u64)
        }
        Setting::LameCbr { .. }
        | Setting::LameAbr { .. }
        | Setting::VorbisAbr { .. }
        | Setting::Opus { .. }
        | Setting::Kbps { .. } => {
            let secs = info.duration_secs()?;
            Some((setting_kbps(s) * 1000.0 / 8.0 * secs).round() as u64)
        }
    }
}

/// The setting with the highest quality that still writes a file under
/// `max_bytes`, aiming 2% low for tags and container overhead.
///
/// MP3 gets `LameAbr`, Vorbis `VorbisAbr`, Opus its own bitrate, and AAC, WMA
/// and AC3 the highest listed step that fits. Below the format's floor the
/// answer is `TooSmall`. For AAC, WMA and AC3 the floor is the lowest listed
/// bitrate: AAC 96 (for 5.1, the lowest total Windows lists), WMA what
/// Windows lists, AC3 64 for mono, 128 for stereo and 96 for surround.
///
/// When the length is unknown no honest bitrate can be worked out, so this
/// returns the High preset's setting unchanged. The same goes for the
/// lossless formats, which have no bitrate to trade. The app hides Fit under
/// when the length is unknown.
pub fn fit_under(t: Target, max_bytes: u64, info: &SourceInfo) -> Result<Setting> {
    let secs = match info.duration_secs() {
        Some(d) if d > 0.0 => d,
        _ => return Ok(preset_setting(t, Preset::High, info)),
    };
    let listed = steps(t, info);
    let floor = match t {
        Target::Mp3 => MP3_FLOOR,
        Target::Ogg => VORBIS_FLOOR,
        Target::Opus => OPUS_FLOOR,
        Target::Aac | Target::M4a | Target::Wma | Target::Ac3 => {
            listed.first().and_then(|s| s.kbps).unwrap_or(0)
        }
        Target::Wav | Target::Aiff | Target::Flac | Target::WavPack => {
            return Ok(preset_setting(t, Preset::High, info));
        }
    };
    let kbps = max_bytes as f64 * 8.0 * FIT_MARGIN / secs / 1000.0;
    if kbps < floor as f64 {
        let min_bytes = (floor as f64 * 1000.0 * secs / 8.0 / FIT_MARGIN).ceil() as u64;
        return Err(MediaError::TooSmall {
            format: t.name().to_string(),
            min_bytes,
        });
    }
    let whole = kbps.floor() as u32;
    Ok(match t {
        Target::Mp3 => Setting::LameAbr {
            kbps: whole.min(MP3_ABR_MAX),
        },
        Target::Ogg => Setting::VorbisAbr {
            kbps: whole.min(VORBIS_ABR_MAX),
        },
        Target::Opus => Setting::Opus {
            kbps: whole.min(OPUS_MAX),
            speech: false,
        },
        _ => listed
            .iter()
            .rev()
            .find(|s| s.kbps.is_some_and(|k| k <= whole))
            .map_or_else(|| listed[0].setting, |s| s.setting),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::MediaError;
    use crate::types::{Channels, Codec, Tags};

    fn info(secs: u64, channels: u16) -> SourceInfo {
        SourceInfo {
            codec: Codec::Flac,
            rate: 44100,
            channels: Channels {
                count: channels,
                mask: 0,
            },
            bits: Some(16),
            frames: Some(secs * 44100),
            kbps: None,
            tags: Tags::default(),
        }
    }

    fn song() -> SourceInfo {
        info(221, 2)
    }

    fn at(t: Target, p: Preset, i: &SourceInfo) -> Step {
        steps(t, i)[preset_step(t, p, i)].clone()
    }

    const ALL: [Target; 11] = [
        Target::Wav,
        Target::Aiff,
        Target::Flac,
        Target::WavPack,
        Target::Mp3,
        Target::Aac,
        Target::M4a,
        Target::Ogg,
        Target::Opus,
        Target::Wma,
        Target::Ac3,
    ];

    #[test]
    fn mp3_has_19_vbr_steps_small_to_best() {
        let s = steps(Target::Mp3, &song());
        assert_eq!(s.len(), 19);
        assert_eq!(s[0].setting, Setting::LameVbr { tenths: 90 });
        assert_eq!(s[18].setting, Setting::LameVbr { tenths: 0 });
        assert_eq!(s[0].label, "V9 · about 61 kbps");
        assert_eq!(s[1].label, "V8.5 · about 70 kbps");
    }

    #[test]
    fn presets_land_on_their_steps() {
        let i = song();
        let v = |p| at(Target::Mp3, p, &i).setting;
        assert_eq!(v(Preset::Small), Setting::LameVbr { tenths: 50 });
        assert_eq!(v(Preset::High), Setting::LameVbr { tenths: 20 });
        assert_eq!(v(Preset::Best), Setting::LameVbr { tenths: 0 });
        assert_eq!(
            at(Target::Mp3, Preset::High, &i).label,
            "V2 · about 173 kbps"
        );
        assert_eq!(at(Target::Opus, Preset::High, &i).kbps, Some(96));
        assert_eq!(at(Target::Opus, Preset::Small, &i).kbps, Some(64));
        assert_eq!(at(Target::Opus, Preset::Best, &i).kbps, Some(160));
        assert_eq!(
            at(Target::Ogg, Preset::High, &i).setting,
            Setting::VorbisQ { tenths: 50 }
        );
        assert_eq!(
            at(Target::Ogg, Preset::High, &i).label,
            "q5 · about 143 kbps"
        );
        assert_eq!(
            at(Target::Ogg, Preset::Small, &i).setting,
            Setting::VorbisQ { tenths: 30 }
        );
        assert_eq!(
            at(Target::Ogg, Preset::Best, &i).setting,
            Setting::VorbisQ { tenths: 70 }
        );
        for t in [Target::Aac, Target::M4a] {
            assert_eq!(at(t, Preset::Small, &i).kbps, Some(96));
            assert_eq!(at(t, Preset::High, &i).kbps, Some(160));
            assert_eq!(at(t, Preset::Best, &i).kbps, Some(192));
        }
        assert_eq!(at(Target::Wma, Preset::Small, &i).kbps, Some(96));
        assert_eq!(at(Target::Wma, Preset::High, &i).kbps, Some(128));
        assert_eq!(at(Target::Wma, Preset::Best, &i).kbps, Some(192));
        assert_eq!(at(Target::Ac3, Preset::Small, &i).kbps, Some(192));
        assert_eq!(at(Target::Ac3, Preset::High, &i).kbps, Some(256));
        assert_eq!(at(Target::Ac3, Preset::Best, &i).kbps, Some(448));
        let mono = info(221, 1);
        assert_eq!(at(Target::Ac3, Preset::Small, &mono).kbps, Some(96));
        assert_eq!(at(Target::Ac3, Preset::High, &mono).kbps, Some(128));
        assert_eq!(at(Target::Ac3, Preset::Best, &mono).kbps, Some(192));
        let surround = info(221, 6);
        assert_eq!(at(Target::Ac3, Preset::Small, &surround).kbps, Some(384));
        assert_eq!(at(Target::Ac3, Preset::High, &surround).kbps, Some(448));
        assert_eq!(at(Target::Ac3, Preset::Best, &surround).kbps, Some(640));
    }

    #[test]
    fn lossless_presets_and_labels() {
        let i = song();
        let flac = steps(Target::Flac, &i);
        assert_eq!(flac.len(), 9);
        assert_eq!(flac[0].label, "Level 8 · smallest");
        assert_eq!(flac[8].label, "Level 0 · fastest");
        assert_eq!(flac[3].label, "Level 5");
        assert!(flac.iter().all(|s| s.kbps.is_none()));
        let lv = |p| at(Target::Flac, p, &i).setting;
        assert_eq!(
            lv(Preset::Small),
            Setting::FlacLevel {
                level: 8,
                bits: None
            }
        );
        assert_eq!(
            lv(Preset::High),
            Setting::FlacLevel {
                level: 5,
                bits: None
            }
        );
        assert_eq!(
            lv(Preset::Best),
            Setting::FlacLevel {
                level: 5,
                bits: None
            }
        );

        let wv: Vec<String> = steps(Target::WavPack, &i)
            .into_iter()
            .map(|s| s.label)
            .collect();
        assert_eq!(wv, ["Very high", "High", "Normal", "Fast"]);
        let m = |p| at(Target::WavPack, p, &i).setting;
        assert_eq!(
            m(Preset::Small),
            Setting::WavPack {
                mode: 2,
                bits: None
            }
        );
        assert_eq!(
            m(Preset::High),
            Setting::WavPack {
                mode: 1,
                bits: None
            }
        );
        assert_eq!(
            m(Preset::Best),
            Setting::WavPack {
                mode: 1,
                bits: None
            }
        );

        assert!(steps(Target::Wav, &i).is_empty());
        assert!(steps(Target::Aiff, &i).is_empty());
    }

    #[test]
    fn every_step_is_a_valid_setting() {
        let i = song();
        for t in ALL {
            for s in steps(t, &i) {
                let ok = match (t, s.setting) {
                    (Target::Mp3, Setting::LameVbr { tenths }) => tenths <= 90,
                    (Target::Ogg, Setting::VorbisQ { tenths }) => (-10..=100).contains(&tenths),
                    (Target::Opus, Setting::Opus { kbps, speech }) => {
                        !speech && (6..=510).contains(&kbps)
                    }
                    (
                        Target::Aac | Target::M4a | Target::Wma | Target::Ac3,
                        Setting::Kbps { kbps },
                    ) => kbps > 0,
                    (Target::Flac, Setting::FlacLevel { level, bits }) => {
                        level <= 8 && bits.is_none()
                    }
                    (Target::WavPack, Setting::WavPack { mode, bits }) => {
                        mode <= 3 && bits.is_none()
                    }
                    _ => false,
                };
                assert!(ok, "{t:?} step {:?} is the wrong family", s.setting);
                assert_eq!(s.kbps.is_some(), !t.lossless(), "{t:?} kbps presence");
            }
        }
    }

    #[test]
    fn steps_run_small_to_best() {
        let i = song();
        for t in ALL {
            let ks: Vec<u32> = steps(t, &i).iter().filter_map(|s| s.kbps).collect();
            assert!(ks.windows(2).all(|w| w[0] <= w[1]), "{t:?} not ascending");
        }
    }

    #[test]
    fn vorbis_steps_and_labels() {
        let s = steps(Target::Ogg, &song());
        assert_eq!(s.len(), 111);
        assert_eq!(s[0].label, "q-1 · about 43 kbps");
        assert_eq!(s[1].label, "q-0.9 · about 45 kbps");
        assert_eq!(s[110].label, "q10 · about 418 kbps");
        assert_eq!(
            label_of(Target::Ogg, &Setting::VorbisQ { tenths: 53 }, &song()),
            "q5.3 · about 150 kbps"
        );
    }

    #[test]
    fn labels_for_settings_off_the_slider() {
        let i = song();
        assert_eq!(
            label_of(Target::Mp3, &Setting::LameCbr { kbps: 320 }, &i),
            "320 kbps CBR"
        );
        assert_eq!(
            label_of(Target::Mp3, &Setting::LameVbr { tenths: 25 }, &i),
            "V2.5 · about 164 kbps"
        );
        assert_eq!(
            label_of(
                Target::Opus,
                &Setting::Opus {
                    kbps: 96,
                    speech: false
                },
                &i
            ),
            "96 kbps"
        );
        assert_eq!(
            label_of(Target::Aac, &Setting::Kbps { kbps: 96 }, &i),
            "96 kbps"
        );
    }

    #[test]
    fn fit_under_picks_the_highest_that_fits() {
        let i = song();
        let s = fit_under(Target::Opus, 3_000_000, &i).unwrap();
        assert_eq!(
            s,
            Setting::Opus {
                kbps: 106,
                speech: false
            }
        );
        assert!(estimate_bytes(Target::Opus, &s, &i).unwrap() <= 3_000_000);

        let s = fit_under(Target::Mp3, 3_000_000, &i).unwrap();
        assert_eq!(s, Setting::LameAbr { kbps: 106 });
        let s = fit_under(Target::Ogg, 3_000_000, &i).unwrap();
        assert_eq!(s, Setting::VorbisAbr { kbps: 106 });
        let s = fit_under(Target::Aac, 3_000_000, &i).unwrap();
        assert_eq!(s, Setting::Kbps { kbps: 96 });
        let s = fit_under(Target::Wma, 3_000_000, &i).unwrap();
        assert_eq!(s, Setting::Kbps { kbps: 96 });
        // Plenty of room: the top of the range.
        let s = fit_under(Target::Aac, 100_000_000, &i).unwrap();
        assert_eq!(s, Setting::Kbps { kbps: 192 });
        let s = fit_under(Target::Mp3, 100_000_000, &i).unwrap();
        assert_eq!(s, Setting::LameAbr { kbps: 320 });
    }

    #[test]
    fn fit_under_below_the_floor_is_too_small() {
        let err = fit_under(Target::Mp3, 100_000, &song()).unwrap_err();
        match err {
            MediaError::TooSmall { format, min_bytes } => {
                assert_eq!(format, "MP3");
                assert!(
                    (min_bytes as i64 - 903_000).abs() < 5_000,
                    "min_bytes {min_bytes}"
                );
            }
            other => panic!("expected TooSmall, got {other:?}"),
        }
        // The floor itself fits.
        let at_floor = (32_000.0_f64 * 221.0 / 8.0 / 0.98).ceil() as u64;
        assert!(fit_under(Target::Mp3, at_floor, &song()).is_ok());
        assert!(fit_under(Target::Mp3, at_floor - 10, &song()).is_err());

        let name = |t| match fit_under(t, 1_000, &song()).unwrap_err() {
            MediaError::TooSmall { format, .. } => format,
            other => panic!("expected TooSmall, got {other:?}"),
        };
        assert_eq!(name(Target::Ogg), "Vorbis");
        assert_eq!(name(Target::Opus), "Opus");
        assert_eq!(name(Target::M4a), "M4A");
        assert_eq!(name(Target::Aac), "AAC");
        // AAC floor is 96 kbps.
        assert!(fit_under(Target::Aac, 2_000_000, &song()).is_err());
    }

    fn kbps_of(t: Target, i: &SourceInfo) -> Vec<u32> {
        steps(t, i).iter().filter_map(|s| s.kbps).collect()
    }

    #[test]
    fn wma_steps_are_what_windows_lists() {
        let i = song();
        let listed = crate::mf::wma::bitrates(44_100, 2);
        assert!(listed.contains(&128), "{listed:?}");
        assert_eq!(kbps_of(Target::Wma, &i), listed);
        // More than two channels are folded to stereo, so they get stereo's list.
        assert_eq!(kbps_of(Target::Wma, &info(221, 6)), listed);
        // A rate Windows doesn't list falls back to 44.1 kHz's.
        let odd = SourceInfo {
            rate: 37_800,
            ..song()
        };
        assert_eq!(kbps_of(Target::Wma, &odd), listed);
        // The floor is the lowest listed bitrate.
        let floor = listed[0] as f64;
        let at_floor = (floor * 1000.0 * 221.0 / 8.0 / 0.98).ceil() as u64;
        assert!(fit_under(Target::Wma, at_floor, &i).is_ok());
        assert!(matches!(
            fit_under(Target::Wma, at_floor - 1000, &i),
            Err(MediaError::TooSmall { .. })
        ));
    }

    #[test]
    fn surround_aac_steps_are_the_real_totals() {
        let i = info(221, 6);
        let listed = crate::mf::aac::bitrates(44_100, 6);
        assert_eq!(kbps_of(Target::M4a, &i), listed);
        for s in steps(Target::M4a, &i) {
            assert_eq!(s.label, format!("{} kbps", s.kbps.unwrap()));
        }
        // The presets stand for 96, 160 and 192 kbps per stereo pair.
        let near = |want: u32| {
            *listed
                .iter()
                .min_by_key(|&&k| (k.abs_diff(want), k))
                .unwrap()
        };
        for (p, per_pair) in [
            (Preset::Small, 96),
            (Preset::High, 160),
            (Preset::Best, 192),
        ] {
            assert_eq!(
                at(Target::M4a, p, &i).kbps,
                Some(near(per_pair * 3)),
                "{p:?}"
            );
        }
        // Fit under 10 MB picks a real total whose file fits.
        let s = fit_under(Target::M4a, 10_000_000, &i).unwrap();
        let Setting::Kbps { kbps } = s else {
            panic!("{s:?}")
        };
        assert!(listed.contains(&kbps), "{kbps}");
        assert!(estimate_bytes(Target::M4a, &s, &i).unwrap() <= 10_000_000);
        let next = listed.iter().find(|&&k| k > kbps).unwrap();
        assert!(
            estimate_bytes(Target::M4a, &Setting::Kbps { kbps: *next }, &i).unwrap() > 9_800_000
        );
        // Stereo is unchanged.
        assert_eq!(kbps_of(Target::M4a, &song()), [96, 128, 160, 192]);
    }

    #[test]
    fn ac3_steps_follow_the_channels() {
        assert_eq!(
            kbps_of(Target::Ac3, &info(221, 1)),
            [64, 80, 96, 112, 128, 160, 192, 224, 256]
        );
        assert_eq!(
            kbps_of(Target::Ac3, &song()),
            [128, 160, 192, 224, 256, 320, 384, 448]
        );
        assert_eq!(
            kbps_of(Target::Ac3, &info(221, 6)),
            [96, 112, 128, 160, 192, 224, 256, 320, 384, 448, 512, 576, 640]
        );
    }

    #[test]
    fn ac3_floors_are_64_mono_128_stereo_96_surround() {
        for (channels, floor) in [(1u16, 64.0), (2, 128.0), (6, 96.0)] {
            let i = info(221, channels);
            let at_floor = (floor * 1000.0 * 221.0 / 8.0 / 0.98_f64).ceil() as u64;
            let fits = fit_under(Target::Ac3, at_floor, &i).unwrap();
            assert_eq!(fits, Setting::Kbps { kbps: floor as u32 }, "{channels} ch");
            assert!(
                matches!(
                    fit_under(Target::Ac3, at_floor - 1000, &i),
                    Err(MediaError::TooSmall { .. })
                ),
                "{channels} ch"
            );
        }
    }

    #[test]
    fn unknown_length_has_no_estimate_and_fit_returns_high() {
        let i = SourceInfo {
            frames: None,
            ..song()
        };
        for s in [
            Setting::LameVbr { tenths: 20 },
            Setting::LameCbr { kbps: 128 },
            Setting::VorbisQ { tenths: 50 },
            Setting::Kbps { kbps: 128 },
            Setting::FlacLevel {
                level: 5,
                bits: None,
            },
            Setting::Pcm {
                bits: 16,
                float: false,
            },
        ] {
            assert_eq!(estimate_bytes(Target::Mp3, &s, &i), None, "{s:?}");
        }
        assert_eq!(
            fit_under(Target::Mp3, 3_000_000, &i).unwrap(),
            Setting::LameVbr { tenths: 20 }
        );
        assert_eq!(
            fit_under(Target::Opus, 10, &i).unwrap(),
            Setting::Opus {
                kbps: 96,
                speech: false
            }
        );
    }

    #[test]
    fn cbr_estimate_is_exact() {
        let i = info(60, 2);
        let n = estimate_bytes(Target::Aac, &Setting::Kbps { kbps: 160 }, &i).unwrap();
        assert!((n as i64 - 1_200_000).abs() < 12_000, "{n}");
        // CBR and ABR ignore the channel rule.
        let mono = info(60, 1);
        assert_eq!(
            estimate_bytes(Target::Mp3, &Setting::LameCbr { kbps: 160 }, &mono),
            Some(1_200_000)
        );
        assert_eq!(
            estimate_bytes(Target::Mp3, &Setting::LameAbr { kbps: 160 }, &info(60, 6)),
            Some(1_200_000)
        );
    }

    #[test]
    fn vbr_estimates_scale_with_channels() {
        let v2 = Setting::LameVbr { tenths: 20 };
        let stereo = estimate_bytes(Target::Mp3, &v2, &info(60, 2)).unwrap();
        assert_eq!(stereo, 1_297_500);
        assert_eq!(
            estimate_bytes(Target::Mp3, &v2, &info(60, 1)),
            Some(778_500)
        );
        assert_eq!(
            estimate_bytes(Target::Mp3, &v2, &info(60, 6)),
            Some(3_892_500)
        );
        let q5 = Setting::VorbisQ { tenths: 50 };
        assert_eq!(
            estimate_bytes(Target::Ogg, &q5, &info(60, 2)),
            Some(1_072_500)
        );
        assert_eq!(
            estimate_bytes(Target::Ogg, &q5, &info(60, 1)),
            Some(643_500)
        );
    }

    #[test]
    fn lossless_estimates() {
        let i = info(10, 2);
        let pcm = 10 * 44100 * 2 * 2 + 44;
        let pcm16 = Setting::Pcm {
            bits: 16,
            float: false,
        };
        let pcm24 = Setting::Pcm {
            bits: 24,
            float: false,
        };
        assert_eq!(estimate_bytes(Target::Wav, &pcm16, &i), Some(pcm));
        assert_eq!(
            estimate_bytes(Target::Wav, &pcm24, &i),
            Some(10 * 44100 * 2 * 3 + 44)
        );
        let flac = estimate_bytes(
            Target::Flac,
            &Setting::FlacLevel {
                level: 5,
                bits: None,
            },
            &i,
        )
        .unwrap();
        assert_eq!(flac, (pcm as f64 * 0.58).round() as u64);
        let wv = estimate_bytes(
            Target::WavPack,
            &Setting::WavPack {
                mode: 1,
                bits: Some(16),
            },
            &i,
        )
        .unwrap();
        assert_eq!(wv, (pcm as f64 * 0.56).round() as u64);
    }

    #[test]
    fn lossless_estimates_use_the_depth_written() {
        let flac5 = Setting::FlacLevel {
            level: 5,
            bits: None,
        };
        let at = |bits| (10 * 44100 * 2 * bits / 8 + 44) as f64 * 0.58;
        // A float WAV is written as 16-bit FLAC, so it is estimated so.
        let float = SourceInfo {
            codec: Codec::Pcm {
                bits: 32,
                float: true,
            },
            bits: Some(32),
            ..info(10, 2)
        };
        let est = estimate_bytes(Target::Flac, &flac5, &float).unwrap();
        assert_eq!(est, at(16).round() as u64);
        // 32-bit integer is written at 24.
        let int32 = SourceInfo {
            codec: Codec::Pcm {
                bits: 32,
                float: false,
            },
            bits: Some(32),
            ..info(10, 2)
        };
        let est = estimate_bytes(Target::Flac, &flac5, &int32).unwrap();
        assert_eq!(est, at(24).round() as u64);
        // The setting's depth comes first.
        let flac24 = Setting::FlacLevel {
            level: 5,
            bits: Some(24),
        };
        let est = estimate_bytes(Target::Flac, &flac24, &float).unwrap();
        assert_eq!(est, at(24).round() as u64);
    }

    #[test]
    fn nearest_step_snaps() {
        let i = song();
        let k = |t, kbps| steps(t, &i)[nearest_step(t, kbps, &i)].kbps;
        assert_eq!(k(Target::Aac, 150), Some(160));
        assert_eq!(k(Target::Aac, 1), Some(96));
        assert_eq!(k(Target::Aac, 5000), Some(192));
        assert_eq!(k(Target::Opus, 100), Some(96));
        assert_eq!(k(Target::Mp3, 190), Some(189));
        assert_eq!(k(Target::Mp3, 173), Some(173));
    }

    #[test]
    fn target_names_and_extensions() {
        for t in ALL {
            assert_eq!(Target::from_ext(t.ext()), Some(t));
            assert_eq!(Target::from_ext(&t.ext().to_uppercase()), Some(t));
        }
        assert_eq!(Target::from_ext("txt"), None);
        let codes: Vec<&str> = ALL.iter().map(|t| t.code()).collect();
        assert_eq!(
            codes,
            ["WAV", "AIFF", "FLAC", "WV", "MP3", "AAC", "M4A", "OGG", "OPUS", "WMA", "AC3"]
        );
        let names: Vec<&str> = ALL.iter().skip(4).map(|t| t.name()).collect();
        assert_eq!(names, ["MP3", "AAC", "M4A", "Vorbis", "Opus", "WMA", "AC3"]);
        assert!(Target::Flac.lossless() && Target::Wav.lossless());
        assert!(!Target::Mp3.lossless() && !Target::Ac3.lossless());
    }

    #[test]
    fn tune_defaults_follow_the_preset() {
        let t = Tune::default();
        assert_eq!(t.setting, None);
        assert_eq!(t.fit_bytes, None);
        assert_eq!(t.channels, ChannelChoice::Keep);
    }
    #[test]
    fn a_bitrate_picks_the_nearest_step_after_the_setting_and_fit_under() {
        let i = song();
        let with = |kbps| Tune {
            kbps: Some(kbps),
            ..Tune::default()
        };
        let high = chosen_setting(Target::Opus, Preset::High, &Tune::default(), &i).unwrap();
        assert_eq!(high, preset_setting(Target::Opus, Preset::High, &i));
        let near = chosen_setting(Target::Opus, Preset::High, &with(250), &i).unwrap();
        assert_eq!(
            near,
            steps(Target::Opus, &i)[nearest_step(Target::Opus, 250, &i)].setting
        );
        assert_ne!(near, high);
        // Lossless formats have no bitrate to pick.
        assert_eq!(
            chosen_setting(Target::Flac, Preset::High, &with(250), &i).unwrap(),
            preset_setting(Target::Flac, Preset::High, &i)
        );
        // Tune's own setting, then Fit under, come first.
        let mut t = with(250);
        t.setting = Some(Setting::Opus {
            kbps: 64,
            speech: false,
        });
        assert_eq!(
            chosen_setting(Target::Opus, Preset::High, &t, &i).unwrap(),
            t.setting.unwrap()
        );
        let mut t = with(250);
        t.fit_bytes = Some(2_000_000);
        assert_eq!(
            chosen_setting(Target::Opus, Preset::High, &t, &i).unwrap(),
            fit_under(Target::Opus, 2_000_000, &i).unwrap()
        );
    }
}
