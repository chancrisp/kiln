//! The codec table: which reader opens a file, which writer a target gets,
//! and what rate, channels and bit depth go between them. [`convert`] runs
//! one file through it; [`probe`] reads what a file holds the same way.
//!
//! Readers, best first:
//!
//! | Source | Reader |
//! |---|---|
//! | WAV, AIFF | Kiln's own ([`crate::pcm`]), then Symphonia |
//! | MP3, AAC-LC, ALAC, FLAC, Vorbis (and MP4, MKV, Ogg) | Symphonia ([`crate::sym`]) |
//! | Opus in Ogg (`.opus`, or an Ogg Symphonia names as Opus) | [`crate::ogg::opus_open`] |
//! | Opus in Matroska or WebM | Symphonia's demuxer, libopus ([`crate::sym`]) |
//! | WavPack (`.wv`) | [`crate::wavpack::open`] |
//! | WMA, AC3 (`.wma`, `.asf`, `.wmv`, `.ac3`) | Media Foundation, then Symphonia |
//! | HE-AAC, and any codec Symphonia names but can't decode | Media Foundation |
//! | any other extension | Symphonia, then Media Foundation |
//!
//! An AAC that Symphonia reads from an MP4 is trimmed to what the file's edit
//! list (or iTunSMPB) presents, see [`crate::mp4read`].
//!
//! Writers and what each takes:
//!
//! | Target | Writer | Rate | Channels |
//! |---|---|---|---|
//! | WAV, AIFF | [`crate::pcm`] | kept | kept |
//! | FLAC | [`crate::flac`] | kept | up to 8 |
//! | WavPack | [`crate::wavpack`] | kept | up to 8 |
//! | MP3 | [`crate::mp3`] (LAME) | a LAME rate, at most 48 kHz | 1 or 2 |
//! | Vorbis | [`crate::ogg::vorbis_sink`] | kept | up to 8 |
//! | Opus | [`crate::ogg::opus_sink`] | 48 kHz | up to 8 |
//! | AAC, M4A | Windows' AAC encoder in [`crate::mp4`] | 44.1 or 48 kHz | 1, 2 or 5.1 |
//! | WMA | [`crate::mf::wma`] | one Windows lists, else 44.1 kHz | 1 or 2 |
//! | AC3 | [`crate::mf::ac3`] (Windows for 1 or 2, Kiln's own for 5.1) | 48 kHz | 1, 2 or 5.1 |
//!
//! Tune's sample rate and channel choice apply first, then these limits.
//! Wider sources fold to stereo by ITU-R BS.775 for MP3 and WMA; AAC and AC3
//! get 5.1 with each speaker in place (3 to 5 channels leave the missing
//! speakers silent, 7.1 folds its side and back pairs).

use crate::error::{MediaError, Result};
use crate::mp4read::{aac_trim, Trimmed};
use crate::pipeline::{run, Sink, SinkSpec, Source};
use crate::process::Process;
use crate::tune::{chosen_setting, ChannelChoice, Preset, Setting, Target, Tune};
use crate::types::{default_mask, Channels, Codec, SourceInfo};
use crate::{flac, mf, mp3, mp4, ogg, pcm, sym, wavpack};
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::path::Path;

const MONO: Channels = Channels {
    count: 1,
    mask: 0x4,
};
const STEREO: Channels = Channels {
    count: 2,
    mask: 0x3,
};
const SURROUND_51: Channels = Channels {
    count: 6,
    mask: 0x3F,
};
/// The rates LAME writes (MPEG-1, 2 and 2.5).
const LAME_RATES: [u32; 9] = [
    8_000, 11_025, 12_000, 16_000, 22_050, 24_000, 32_000, 44_100, 48_000,
];
/// Extensions Symphonia is the first reader for; any other extension that
/// Symphonia can't read gets a try with Media Foundation.
const SYMPHONIA_EXTS: [&str; 15] = [
    "mp3", "m4a", "m4b", "mp4", "m4v", "mov", "aac", "flac", "ogg", "oga", "mkv", "mka", "webm",
    "alac", "3gp",
];

fn ext_of(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Opens a file for reading through the codec table.
pub fn open_source(path: &Path) -> Result<Box<dyn Source>> {
    let ext = ext_of(path);
    match ext.as_str() {
        "wav" | "wave" | "aif" | "aiff" | "aifc" => {
            let own = if ext.starts_with('w') {
                pcm::wav::open(File::open(path)?)
            } else {
                pcm::aiff::open(File::open(path)?)
            };
            match own {
                Ok(s) => Ok(s),
                Err(own_error) => match symphonia(path, &ext, false) {
                    Ok(s) => Ok(s),
                    // Symphonia knows what it is and can't read it: say so.
                    Err(e @ (MediaError::Unsupported { .. } | MediaError::NoAudio)) => Err(e),
                    Err(_) => Err(own_error),
                },
            }
        }
        "wv" => wavpack::open(File::open(path)?),
        "opus" if is_ogg(path)? => match ogg::opus_open(File::open(path)?) {
            Ok(s) => Ok(s),
            Err(opus_error) => symphonia(path, &ext, false).map_err(|_| opus_error),
        },
        "wma" | "asf" | "wmv" | "ac3" => match mf::source::open(File::open(path)?) {
            Ok(s) => Ok(s),
            Err(MediaError::NeedsMediaFoundation) => Err(MediaError::NeedsMediaFoundation),
            Err(mf_error) => symphonia(path, &ext, false).map_err(|_| mf_error),
        },
        _ => symphonia(path, &ext, !SYMPHONIA_EXTS.contains(&ext.as_str())),
    }
}

/// True when the file starts with an Ogg page.
fn is_ogg(path: &Path) -> Result<bool> {
    let mut head = [0u8; 4];
    let n = File::open(path)?.read(&mut head)?;
    Ok(n == 4 && &head == b"OggS")
}

/// Symphonia, with what it can't decode passed on: Opus in Ogg to Kiln's
/// Opus reader, HE-AAC, AC3, WMA and the rest to Media Foundation, tried
/// once. With an `unknown` extension, a file Symphonia calls damaged gets
/// Media Foundation's try too. The more telling error wins: needing Media
/// Foundation, or an unsupported codec, over damaged. An AAC from an MP4 is
/// trimmed to what the file presents.
fn symphonia(path: &Path, ext: &str, unknown: bool) -> Result<Box<dyn Source>> {
    match sym::open(File::open(path)?, ext) {
        Ok(s) => trim_aac(s, path),
        Err(MediaError::Unsupported { codec }) => {
            if codec == Codec::Opus.display_name() && is_ogg(path)? {
                return ogg::opus_open(File::open(path)?);
            }
            let windows_reads = unknown
                || [Codec::HeAac, Codec::Ac3, Codec::Wma]
                    .iter()
                    .any(|c| c.display_name() == codec);
            match mf::source::open(File::open(path)?) {
                Ok(s) => Ok(s),
                Err(MediaError::NeedsMediaFoundation) if windows_reads => {
                    Err(MediaError::NeedsMediaFoundation)
                }
                Err(_) => Err(MediaError::Unsupported { codec }),
            }
        }
        Err(damaged @ MediaError::Damaged { .. }) if unknown => {
            match mf::source::open(File::open(path)?) {
                Ok(s) => Ok(s),
                Err(MediaError::NeedsMediaFoundation) => Err(MediaError::NeedsMediaFoundation),
                // A codec Windows names; "an unknown format" says less than
                // damaged does.
                Err(MediaError::Unsupported { codec }) if codec != unknown_format() => {
                    Err(MediaError::Unsupported { codec })
                }
                Err(_) => Err(damaged),
            }
        }
        Err(e) => Err(e),
    }
}

/// What Media Foundation calls a file it can't place at all.
fn unknown_format() -> String {
    Codec::Other("unknown".into()).display_name()
}

/// Drops the encoder priming and the end padding from an AAC read out of an
/// MP4, as its edit list (or iTunSMPB) says.
fn trim_aac(s: Box<dyn Source>, path: &Path) -> Result<Box<dyn Source>> {
    if s.info().codec != Codec::Aac {
        return Ok(s);
    }
    let rate = s.info().rate;
    match aac_trim(&mut File::open(path)?, rate) {
        Some(t) if t.skip > 0 || t.keep.is_some() => Ok(Box::new(Trimmed::new(s, t))),
        _ => Ok(s),
    }
}

/// Reads what a file holds, through the same readers [`convert`] uses.
pub fn probe(path: &Path) -> Result<SourceInfo> {
    Ok(open_source(path)?.info().clone())
}

/// True when converting from a file with extension `from_ext` to `to` needs
/// Media Foundation: WMA and AC3 sources, and AAC, M4A, WMA and AC3 targets.
pub fn needs_media_foundation(from_ext: &str, to: Target) -> bool {
    let ext = from_ext.trim_start_matches('.').to_ascii_lowercase();
    matches!(ext.as_str(), "wma" | "asf" | "wmv" | "ac3")
        || matches!(to, Target::Aac | Target::M4a | Target::Wma | Target::Ac3)
}

/// Whether this PC can write `to` at all: `NeedsMediaFoundation` when
/// Windows lacks the encoder, `LameMissing` when MP3's DLL can't be loaded.
pub fn can_run(to: Target) -> Result<()> {
    let c = mf::mf_codecs();
    let has = match to {
        Target::Aac | Target::M4a => c.aac_enc,
        Target::Wma => c.wma_enc,
        Target::Ac3 => c.ac3_enc,
        Target::Mp3 => return mp3::lame_version().map(|_| ()),
        _ => true,
    };
    if has {
        Ok(())
    } else {
        Err(MediaError::NeedsMediaFoundation)
    }
}

/// What the sink gets and how the audio is turned into it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Plan {
    setting: Setting,
    rate: u32,
    channels: Channels,
    /// The integer depth the output is rounded to, when dither is due.
    dither: Option<u16>,
    source_bits: Option<u16>,
}

/// The source's integer bit depth: `None` for float or unknown.
fn source_bits(info: &SourceInfo) -> Option<u16> {
    match info.codec {
        Codec::Pcm { float: true, .. } => None,
        _ => info.bits,
    }
}

/// The channels Tune asks for, before the target's limits.
fn chosen_channels(info: &SourceInfo, choice: ChannelChoice) -> Channels {
    match choice {
        ChannelChoice::Keep => info.channels,
        ChannelChoice::Stereo => STEREO,
        ChannelChoice::Mono => MONO,
    }
}

/// True for six channels that are 5.1 already: unlabelled, or with back or
/// side surrounds.
fn is_51(ch: Channels) -> bool {
    ch.count == 6 && matches!(ch.mask, 0 | 0x3F | 0x60F)
}

/// The channels `t` is written with. AAC and AC3 take 5.1 and nothing else
/// wider than stereo: other layouts (6.0 too) are mapped to it by speaker.
fn target_channels(t: Target, ch: Channels) -> Channels {
    match t {
        Target::Mp3 | Target::Wma if ch.count > 2 => STEREO,
        Target::Aac | Target::M4a | Target::Ac3 if ch.count > 2 && !is_51(ch) => SURROUND_51,
        Target::Flac | Target::WavPack | Target::Ogg | Target::Opus if ch.count > 8 => Channels {
            count: 8,
            mask: default_mask(8),
        },
        _ => ch,
    }
}

/// The rate `t` is written at, for `rate` asked and `channels` written.
fn target_rate(t: Target, rate: u32, channels: u16) -> u32 {
    match t {
        Target::Opus | Target::Ac3 => 48_000,
        Target::Aac | Target::M4a if rate == 44_100 => 44_100,
        Target::Aac | Target::M4a => 48_000,
        Target::Mp3 if rate > 48_000 => 48_000,
        // Up to the nearest rate LAME takes, so nothing is lost.
        Target::Mp3 => LAME_RATES
            .iter()
            .copied()
            .find(|&r| r >= rate)
            .unwrap_or(48_000),
        Target::Wma if !mf::wma::bitrates(rate, channels).is_empty() => rate,
        Target::Wma => 44_100,
        _ => rate,
    }
}

/// The sound as `t` writes it from `info` with `tune`: the channels Tune asks
/// for, within what the target takes (MP3 and WMA fold to stereo, AAC and
/// AC3 take 5.1), at the target's rate for the rate asked, with the length
/// counted at that rate. The slider's steps, the presets, Fit under and the
/// size estimate are worked out for this, as a conversion works them out.
pub fn written(t: Target, tune: &Tune, info: &SourceInfo) -> SourceInfo {
    let asked = chosen_channels(info, tune.channels);
    let channels = target_channels(t, asked);
    let asked_rate = tune.rate.filter(|&r| r > 0).unwrap_or(info.rate);
    let rate = target_rate(t, asked_rate, channels.count);
    let mut written = info.clone();
    written.channels = channels;
    written.rate = rate;
    written.frames = info.frames.map(|f| match info.rate {
        0 => f,
        from => (f as u128 * rate as u128 / from as u128) as u64,
    });
    written
}

fn plan(t: Target, preset: Preset, tune: &Tune, info: &SourceInfo) -> Result<Plan> {
    // The steps, presets and Fit under are worked out for what is written.
    let written = written(t, tune, info);
    let (rate, channels) = (written.rate, written.channels);

    let mut setting = chosen_setting(t, preset, tune, &written)?;
    let cap = |b: u16| b.min(24);
    match &mut setting {
        Setting::FlacLevel { bits, .. } | Setting::WavPack { bits, .. } if bits.is_none() => {
            *bits = tune.bits.map(cap);
        }
        _ => {}
    }

    let src_bits = source_bits(info);
    let out_bits = match setting {
        Setting::Pcm { bits, float: false } => Some(bits),
        Setting::FlacLevel { bits, .. } | Setting::WavPack { bits, .. } => {
            Some(pcm::lossless_bits(bits, src_bits))
        }
        _ => None,
    };
    let reshaped = rate != info.rate || channels.count != info.channels.count;
    let dither = out_bits.filter(|&b| b < 32 && (reshaped || src_bits.is_none_or(|s| s > b)));

    Ok(Plan {
        setting,
        rate,
        channels,
        dither,
        source_bits: src_bits,
    })
}

fn sink(t: Target, spec: SinkSpec) -> Result<Box<dyn Sink>> {
    match t {
        Target::Wav => pcm::wav::sink(spec),
        Target::Aiff => pcm::aiff::sink(spec),
        Target::Flac => flac::sink(spec),
        Target::WavPack => wavpack::sink(spec),
        Target::Mp3 => mp3::sink(spec),
        Target::Ogg => ogg::vorbis_sink(spec),
        Target::Opus => ogg::opus_sink(spec),
        Target::Aac | Target::M4a => {
            let Setting::Kbps { kbps } = *spec.setting else {
                return Err(MediaError::Encoder(
                    "this setting isn't an AAC bitrate".into(),
                ));
            };
            let enc = mf::aac::encoder(spec.rate, spec.channels.count, kbps)?;
            if t == Target::Aac {
                mp4::adts_sink(spec, enc)
            } else {
                mp4::m4a_sink(spec, enc)
            }
        }
        Target::Wma => mf::wma::sink(spec),
        Target::Ac3 => mf::ac3::sink(spec),
    }
}

/// Converts `input` to `target`, written to `output`, which must not exist
/// yet: it is created here, and on any error or cancel it is removed again
/// (with any temporary file a writer made). `progress` gets values from 0 to
/// 1, the last 1.0 once the file is complete; `cancel` is asked before each
/// block.
///
/// The setting is `tune.setting`, else what Fit under finds for
/// `tune.fit_bytes`, else `preset`'s.
pub fn convert(
    input: &Path,
    output: &Path,
    target: Target,
    preset: Preset,
    tune: &Tune,
    progress: &mut dyn FnMut(f32),
    cancel: &dyn Fn() -> bool,
) -> Result<()> {
    // Whatever this PC can't write is refused before anything is made: AC3
    // in 5.1 is Kiln's own encoder, but AC3 needs Media Foundation (R28).
    can_run(target)?;
    let source = open_source(input).map_err(MediaError::reading)?;
    let info = source.info().clone();
    let plan = plan(target, preset, tune, &info)?;
    // Read as well as write: Media Foundation reads back what it writes.
    let out = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(output)?;
    let result = (|| {
        let process = Process::new(
            info.rate,
            info.channels,
            plan.rate,
            plan.channels,
            plan.dither,
        )?;
        let sink = sink(
            target,
            SinkSpec {
                rate: plan.rate,
                channels: plan.channels,
                tags: &info.tags,
                setting: &plan.setting,
                source_bits: plan.source_bits,
                out,
            },
        )?;
        run(source, process, sink, progress, cancel)
    })();
    if result.is_err() {
        // Everything holding the file has been dropped by now.
        let _ = std::fs::remove_file(output);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Tags;

    fn info(codec: Codec, rate: u32, count: u16, mask: u32, bits: Option<u16>) -> SourceInfo {
        SourceInfo {
            codec,
            rate,
            channels: Channels { count, mask },
            bits,
            frames: Some(rate as u64 * 60),
            kbps: None,
            tags: Tags::default(),
        }
    }

    fn cd() -> SourceInfo {
        info(Codec::Flac, 44_100, 2, 0x3, Some(16))
    }

    fn high(t: Target, i: &SourceInfo) -> Plan {
        plan(t, Preset::High, &Tune::default(), i).unwrap()
    }

    #[test]
    fn a_bitrate_the_file_lacks_snaps_to_its_nearest() {
        let kbps = |t: Target, k: u32, i: &SourceInfo| {
            let tune = Tune {
                setting: Some(Setting::Kbps { kbps: k }),
                ..Tune::default()
            };
            let p = plan(t, Preset::High, &tune, i).unwrap();
            let listed: Vec<u32> = crate::tune::steps(t, &written(t, &tune, i))
                .iter()
                .filter_map(|s| s.kbps)
                .collect();
            let Setting::Kbps { kbps } = p.setting else {
                panic!("{:?}", p.setting)
            };
            assert!(listed.contains(&kbps), "{t:?} {kbps} not in {listed:?}");
            kbps
        };
        // A WMA batch's 128 kbps, for a mono 48 kHz file; and a bitrate no
        // list has.
        let mono = info(Codec::Flac, 48_000, 1, 0x4, Some(16));
        kbps(Target::Wma, 128, &mono);
        let odd = kbps(Target::Wma, 129, &mono);
        assert_ne!(odd, 129);
        // An AAC batch's stereo 160 (High), for a 5.1 file: the 5.1 total
        // High gives it, per channel pair, not the nearest total (128).
        let film = info(Codec::Flac, 48_000, 6, 0x3F, Some(16));
        let surround = kbps(Target::Aac, 160, &film);
        assert_eq!(
            Setting::Kbps { kbps: surround },
            high(Target::Aac, &film).setting
        );
        // And a 5.1 total for a stereo file: per pair again.
        let stereo = info(Codec::Flac, 48_000, 2, 0x3, Some(16));
        assert_eq!(kbps(Target::Aac, surround, &stereo), 160);
        // A listed bitrate is kept as it is.
        assert_eq!(kbps(Target::Aac, 160, &cd()), 160);
        assert_eq!(kbps(Target::Ac3, 448, &film), 448);
    }

    #[test]
    fn the_sound_as_written_is_what_plan_writes() {
        let film = info(Codec::Flac, 96_000, 6, 0x3F, Some(24));
        // MP3 folds 5.1 to stereo at 48 kHz; the length is the same in time.
        let w = written(Target::Mp3, &Tune::default(), &film);
        assert_eq!((w.channels, w.rate), (STEREO, 48_000));
        assert_eq!(w.frames, Some(48_000 * 60));
        // FLAC keeps it all; AAC keeps 5.1, at 48 kHz.
        let w = written(Target::Flac, &Tune::default(), &film);
        assert_eq!((w.channels.count, w.rate), (6, 96_000));
        let w = written(Target::Aac, &Tune::default(), &film);
        assert_eq!((w.channels.count, w.rate), (6, 48_000));
        // Tune's choices first, then the target's limits.
        let tune = Tune {
            channels: ChannelChoice::Mono,
            rate: Some(22_050),
            ..Tune::default()
        };
        let w = written(Target::Opus, &tune, &film);
        assert_eq!((w.channels, w.rate), (MONO, 48_000));
        for t in [Target::Mp3, Target::Flac, Target::Aac, Target::Opus] {
            let p = plan(t, Preset::High, &tune, &film).unwrap();
            let w = written(t, &tune, &film);
            assert_eq!((p.channels, p.rate), (w.channels, w.rate), "{t:?}");
        }
    }

    #[test]
    fn rates_follow_each_target() {
        let hi = info(Codec::Flac, 96_000, 2, 0x3, Some(24));
        assert_eq!(high(Target::Mp3, &hi).rate, 48_000);
        assert_eq!(high(Target::Opus, &cd()).rate, 48_000);
        assert_eq!(high(Target::Ac3, &cd()).rate, 48_000);
        assert_eq!(high(Target::Aac, &cd()).rate, 44_100);
        assert_eq!(high(Target::M4a, &hi).rate, 48_000);
        assert_eq!(high(Target::Ogg, &hi).rate, 96_000);
        assert_eq!(high(Target::Flac, &hi).rate, 96_000);
        assert_eq!(high(Target::Wav, &hi).rate, 96_000);
        assert_eq!(high(Target::Mp3, &cd()).rate, 44_100);
        // A rate LAME doesn't take goes up to the next one it does.
        let odd = info(Codec::Flac, 37_800, 2, 0x3, Some(16));
        assert_eq!(high(Target::Mp3, &odd).rate, 44_100);
        // Tune's rate comes first, then the limits.
        let tune = Tune {
            rate: Some(22_050),
            ..Tune::default()
        };
        assert_eq!(
            plan(Target::Mp3, Preset::High, &tune, &cd()).unwrap().rate,
            22_050
        );
        assert_eq!(
            plan(Target::Opus, Preset::High, &tune, &cd()).unwrap().rate,
            48_000
        );
    }

    #[test]
    fn channels_follow_each_target() {
        let quad = info(Codec::Flac, 48_000, 4, 0x33, Some(16));
        let seven_one = info(Codec::Flac, 48_000, 8, 0x63F, Some(16));
        for t in [Target::Aac, Target::M4a, Target::Ac3] {
            assert_eq!(high(t, &quad).channels, SURROUND_51, "{t:?}");
            assert_eq!(high(t, &seven_one).channels, SURROUND_51, "{t:?}");
            assert_eq!(high(t, &cd()).channels, STEREO, "{t:?}");
        }
        for t in [Target::Mp3, Target::Wma] {
            assert_eq!(high(t, &seven_one).channels, STEREO, "{t:?}");
        }
        for t in [
            Target::Opus,
            Target::Ogg,
            Target::Flac,
            Target::WavPack,
            Target::Wav,
        ] {
            assert_eq!(high(t, &seven_one).channels, seven_one.channels, "{t:?}");
        }
        // Tune's choice comes first.
        let tune = Tune {
            channels: ChannelChoice::Mono,
            ..Tune::default()
        };
        let p = plan(Target::Ac3, Preset::High, &tune, &seven_one).unwrap();
        assert_eq!(p.channels, MONO);
        let tune = Tune {
            channels: ChannelChoice::Stereo,
            ..Tune::default()
        };
        let p = plan(Target::Aac, Preset::High, &tune, &seven_one).unwrap();
        assert_eq!(p.channels, STEREO);
    }

    #[test]
    fn surround_presets_use_the_written_layout() {
        // A quad source becomes 5.1 AAC, so the preset is a 5.1 total.
        let quad = info(Codec::Flac, 48_000, 4, 0x33, Some(16));
        let p = high(Target::Ac3, &quad);
        assert_eq!(p.setting, Setting::Kbps { kbps: 448 });
        // Folded to stereo by Tune, AC3 gets the stereo preset.
        let tune = Tune {
            channels: ChannelChoice::Stereo,
            ..Tune::default()
        };
        let p = plan(Target::Ac3, Preset::High, &tune, &quad).unwrap();
        assert_eq!(p.setting, Setting::Kbps { kbps: 256 });
    }

    #[test]
    fn lossless_keeps_the_depth_and_dithers_only_going_down() {
        let hi = info(Codec::Flac, 48_000, 2, 0x3, Some(24));
        let p = high(Target::Flac, &hi);
        assert_eq!(p.source_bits, Some(24));
        assert_eq!(p.dither, None);
        // 24 to 16 by Tune: dithered at 16.
        let tune = Tune {
            bits: Some(16),
            ..Tune::default()
        };
        for t in [Target::Flac, Target::WavPack, Target::Wav, Target::Aiff] {
            let p = plan(t, Preset::High, &tune, &hi).unwrap();
            assert_eq!(p.dither, Some(16), "{t:?}");
        }
        // 16 to 16: bit for bit, no dither.
        assert_eq!(high(Target::Wav, &cd()).dither, None);
        assert_eq!(
            high(Target::Wav, &cd()).setting,
            Setting::Pcm {
                bits: 16,
                float: false
            }
        );
        // A float source stays float in a WAV, and FLAC gets 16 dithered.
        let float = info(
            Codec::Pcm {
                bits: 32,
                float: true,
            },
            48_000,
            2,
            0x3,
            Some(32),
        );
        let p = high(Target::Wav, &float);
        assert_eq!(
            p.setting,
            Setting::Pcm {
                bits: 32,
                float: true
            }
        );
        assert_eq!(p.dither, None);
        assert_eq!(p.source_bits, None);
        assert_eq!(high(Target::Flac, &float).dither, Some(16));
        // Lossy targets are never dithered.
        assert_eq!(high(Target::Mp3, &hi).dither, None);
        // Resampled 16-bit audio is dithered back to 16.
        let tune = Tune {
            rate: Some(48_000),
            ..Tune::default()
        };
        let p = plan(Target::Flac, Preset::High, &tune, &cd()).unwrap();
        assert_eq!(p.dither, Some(16));
    }

    #[test]
    fn the_setting_is_tunes_then_fit_under_then_the_preset() {
        let given = Setting::LameCbr { kbps: 320 };
        let tune = Tune {
            setting: Some(given),
            fit_bytes: Some(1_000_000),
            ..Tune::default()
        };
        assert_eq!(
            plan(Target::Mp3, Preset::High, &tune, &cd())
                .unwrap()
                .setting,
            given
        );
        let tune = Tune {
            fit_bytes: Some(1_000_000),
            ..Tune::default()
        };
        assert!(matches!(
            plan(Target::Mp3, Preset::High, &tune, &cd())
                .unwrap()
                .setting,
            Setting::LameAbr { .. }
        ));
        let tune = Tune {
            fit_bytes: Some(1_000),
            ..Tune::default()
        };
        assert!(matches!(
            plan(Target::Mp3, Preset::High, &tune, &cd()),
            Err(MediaError::TooSmall { .. })
        ));
        assert_eq!(
            high(Target::Mp3, &cd()).setting,
            Setting::LameVbr { tenths: 20 }
        );
    }

    #[test]
    fn a_20_bit_source_keeps_its_depth_in_wav_and_aiff() {
        for (bits, want) in [(8, 16), (16, 16), (20, 24), (24, 24), (32, 32)] {
            let src = info(Codec::Flac, 48_000, 2, 0x3, Some(bits));
            for t in [Target::Wav, Target::Aiff] {
                let p = high(t, &src);
                assert_eq!(
                    p.setting,
                    Setting::Pcm {
                        bits: want,
                        float: false
                    },
                    "{bits}-bit to {t:?}"
                );
                assert_eq!(p.dither, None, "{bits}-bit to {t:?}");
            }
        }
    }

    #[test]
    fn six_channels_that_are_not_51_are_mapped_to_51() {
        // 6.0: FL FR FC BL BR BC.
        let six = info(Codec::Flac, 48_000, 6, 0x137, Some(16));
        for t in [Target::Aac, Target::M4a, Target::Ac3] {
            assert_eq!(high(t, &six).channels, SURROUND_51, "{t:?}");
        }
        // Lossless keeps the layout as it is.
        assert_eq!(high(Target::Flac, &six).channels, six.channels);
        // 5.1 with side speakers is 5.1 already.
        let side = info(Codec::Flac, 48_000, 6, 0x60F, Some(16));
        assert_eq!(high(Target::Ac3, &side).channels, side.channels);
    }

    #[test]
    fn media_foundation_is_needed_for_its_formats() {
        assert!(needs_media_foundation("wma", Target::Mp3));
        assert!(needs_media_foundation(".AC3", Target::Flac));
        assert!(needs_media_foundation("flac", Target::M4a));
        assert!(needs_media_foundation("mp3", Target::Wma));
        assert!(!needs_media_foundation("m4a", Target::Mp3));
        assert!(!needs_media_foundation("wav", Target::Opus));
    }
}
