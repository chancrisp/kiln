//! Sound through Kiln's own engine, `kiln-media`.
//!
//! Two backends share one converter: "audio" for the formats every PC can
//! write, and "audio-mf" for those that need Windows' Media Foundation (AAC,
//! M4A, WMA and AC3, and reading WMA and AC3). formats.toml says which pair
//! takes which. FFmpeg is left the sound of the video types kiln-media
//! can't open.
//!
//! "audio-mf" counts as installed only when Media Foundation has every
//! codec those routes use: the AAC, WMA and AC3 encoders and the WMA and AC3
//! decoders. Windows has all of them or, in the N editions without the
//! Media Feature Pack, none, so one rule for the whole backend is enough.
//! Without it, AC3 is blocked as a whole even though Kiln's own encoder
//! writes 5.1 (ruling R28).

use std::path::Path;

use kiln_media::{MediaError, SourceInfo, Target};

use crate::converter::{Context, Converter, Progress, Request};
use crate::error::{Error, Result};
use crate::plain;
use crate::quality::{self, MediaInfo};
use crate::registry::{Format, Kind};

/// The video containers whose sound kiln-media reads itself.
pub(crate) const NATIVE_VIDEO: [&str; 5] = ["mp4", "m4v", "mov", "mkv", "webm"];

/// The two backend ids.
pub(crate) const AUDIO: &str = "audio";
pub(crate) const AUDIO_MF: &str = "audio-mf";

/// Converts sound with kiln-media. `mf` says which backend id it answers
/// to; both work the same way.
pub struct AudioConverter {
    mf: bool,
}

impl AudioConverter {
    /// The "audio" backend.
    pub const PLAIN: AudioConverter = AudioConverter { mf: false };
    /// The "audio-mf" backend.
    pub const MEDIA_FOUNDATION: AudioConverter = AudioConverter { mf: true };
}

impl Converter for AudioConverter {
    fn id(&self) -> &'static str {
        if self.mf {
            AUDIO_MF
        } else {
            AUDIO
        }
    }

    fn convert(&self, req: &Request<'_>, ctx: &mut Context<'_>) -> Result<()> {
        let target = Target::from_ext(&req.to.ext)
            .ok_or_else(|| Error::UnknownFormat(req.to.ext.clone()))?;
        let q = &req.options.quality;
        let tune = quality::media_tune(q);
        let cancel = ctx.cancel;
        let progress = &mut ctx.progress;
        kiln_media::convert(
            req.input,
            req.output,
            target,
            preset(q.preset),
            &tune,
            &mut |f| progress(Progress::Fraction(f)),
            &|| cancel.is_cancelled(),
        )
        .map_err(|e| from_media(e, req.input, req.output))
    }
}

/// kiln-media's preset for Kiln's.
pub(crate) fn preset(p: quality::Preset) -> kiln_media::Preset {
    match p {
        quality::Preset::Small => kiln_media::Preset::Small,
        quality::Preset::High => kiln_media::Preset::High,
        quality::Preset::Best => kiln_media::Preset::Best,
    }
}

/// What a kiln-media failure is to the rest of Kiln. Each plain sentence
/// is kiln-media's own (its Display), kept whole in [`Error::Media`]; an
/// encoder's words that aren't one become an engine failure whose words
/// wait in Show details. An I/O error names `input` when reading it failed,
/// else `output`.
pub(crate) fn from_media(e: MediaError, input: &Path, output: &Path) -> Error {
    match e {
        MediaError::Cancelled => Error::Cancelled,
        MediaError::NeedsMediaFoundation => Error::EngineMissing { engine: AUDIO_MF },
        MediaError::Read(source) => Error::io(input, source),
        MediaError::Io(source) => Error::io(output, source),
        MediaError::Encoder(words) => match plain::own_sentence(&words) {
            Some(sentence) if sentence == words => Error::Media(sentence),
            _ => Error::EngineFailed {
                engine: "Kiln",
                summary: words,
                details: String::new(),
            },
        },
        MediaError::NeedsExtension { product, .. }
        | MediaError::NeedsExtensionToWrite { product, .. } => Error::NeedsExtension {
            message: e.to_string(),
            product,
        },
        MediaError::Damaged { .. }
        | MediaError::Unsupported { .. }
        | MediaError::NoAudio
        | MediaError::NoVideo
        | MediaError::NoDecoder { .. }
        | MediaError::NoHardwareEncoder { .. }
        | MediaError::LameMissing
        | MediaError::TooSmall { .. } => Error::Media(e.to_string()),
    }
}

/// What a look at the sound of a file of format `f` found wrong, as the
/// row's sentence, and whether it is a video with no sound at all. Needing
/// Media Foundation isn't a problem with the file, so it gives `None`.
pub(crate) fn problem_of(
    f: &Format,
    sound: &kiln_media::Result<SourceInfo>,
) -> (Option<String>, bool) {
    match sound {
        Err(e @ MediaError::NoAudio) => (Some(e.to_string()), f.kind == Kind::Video),
        Err(e @ (MediaError::Damaged { .. } | MediaError::Unsupported { .. })) => {
            (Some(e.to_string()), false)
        }
        _ => (None, false),
    }
}

/// Whether Media Foundation has everything the "audio-mf" routes use.
pub(crate) fn media_foundation_available() -> bool {
    let c = kiln_media::mf_codecs();
    c.aac_enc && c.wma_enc && c.ac3_enc && c.wma_dec && c.ac3_dec
}

/// Whether `backend` is one of the two sound backends.
pub(crate) fn is_audio_backend(backend: &str) -> bool {
    backend == AUDIO || backend == AUDIO_MF
}

/// Whether kiln-media converts `from` to `to`: every audio target of an
/// audio file or of one of [`NATIVE_VIDEO`]. This is what formats.toml's
/// routes say; a test holds the two together.
pub(crate) fn native_pair(from: &Format, to: &Format) -> bool {
    to.kind == Kind::Audio
        && (from.kind == Kind::Audio || NATIVE_VIDEO.contains(&from.ext.as_str()))
}

/// Whether kiln-media reads the sound of files of this format.
pub(crate) fn reads_sound(f: &Format) -> bool {
    f.kind == Kind::Audio || NATIVE_VIDEO.contains(&f.ext.as_str())
}

/// Copies what kiln-media found into `info`: the codec, bitrate, channels,
/// rate and depth, the length when nothing gave one yet, and that there is
/// sound.
pub(crate) fn fill(info: &mut MediaInfo, src: &SourceInfo) {
    info.has_audio = true;
    info.codec = Some(src.codec.display_name());
    info.kbps = src.kbps;
    info.channels = Some(src.channels.count);
    info.rate = Some(src.rate);
    info.bits = src.bits;
    info.float = matches!(src.codec, kiln_media::Codec::Pcm { float: true, .. });
    if info.duration.is_none() {
        info.duration = src.duration_secs();
    }
}

/// What Kiln knows about a picture, sound or video file `f`, given what
/// kiln-media read of its `sound` (`None` for a file whose sound it doesn't
/// read, see [`crate::Kiln`]'s `hear`). An audio file's facts are
/// kiln-media's, or FFprobe's (`ffprobe`) when kiln-media can't read it. A
/// video Kiln reads the sound of gets its picture's facts from FFprobe and
/// its sound's from kiln-media, so with no FFprobe the sound's facts still
/// show; one with no sound says so. Other files are FFprobe's alone.
/// `None` when nothing could be read.
pub(crate) fn media_info(
    f: &Format,
    sound: Option<kiln_media::Result<SourceInfo>>,
    ffprobe: impl FnOnce() -> Option<MediaInfo>,
) -> Option<MediaInfo> {
    let known = |m: &MediaInfo| m.duration.is_some() || m.width.is_some();
    let Some(sound) = sound.filter(|_| reads_sound(f)) else {
        return ffprobe().filter(known);
    };
    if f.kind == Kind::Audio {
        return match sound {
            Ok(src) => {
                let mut info = MediaInfo::default();
                fill(&mut info, &src);
                Some(info)
            }
            Err(_) => ffprobe().filter(known),
        };
    }
    let mut info = ffprobe().filter(known);
    match sound {
        Ok(src) => fill(info.get_or_insert_with(MediaInfo::default), &src),
        Err(MediaError::NoAudio) => {
            if let Some(m) = info.as_mut() {
                m.has_audio = false;
            }
        }
        Err(_) => {}
    }
    info
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Registry;
    use kiln_media::Codec;

    #[test]
    fn float_sound_is_said_so() {
        let src = |codec| SourceInfo {
            codec,
            rate: 48_000,
            channels: kiln_media::Channels {
                count: 2,
                mask: 0x3,
            },
            bits: Some(32),
            frames: Some(48_000),
            kbps: None,
            tags: Default::default(),
        };
        let mut info = MediaInfo::default();
        fill(
            &mut info,
            &src(Codec::Pcm {
                bits: 32,
                float: true,
            }),
        );
        assert!(info.float);
        assert_eq!(
            crate::quality::codec_of(&info),
            Codec::Pcm {
                bits: 32,
                float: true
            }
        );
        fill(
            &mut info,
            &src(Codec::Pcm {
                bits: 32,
                float: false,
            }),
        );
        assert!(!info.float);
        fill(&mut info, &src(Codec::Flac));
        assert!(!info.float);
    }

    #[test]
    fn the_routes_and_native_pair_agree() {
        let r = Registry::builtin();
        for (from, to, steps) in r.pairs() {
            let audio = steps.iter().all(|s| is_audio_backend(&s.backend));
            assert_eq!(
                audio,
                native_pair(from, to),
                "{} -> {}: {steps:?}",
                from.ext,
                to.ext
            );
            if audio {
                assert_eq!(steps.len(), 1);
                let t = Target::from_ext(&to.ext).expect("a kiln-media target");
                // "audio-mf" exactly where Media Foundation is needed.
                assert_eq!(
                    steps[0].backend == AUDIO_MF,
                    kiln_media::needs_media_foundation(&from.ext, t),
                    "{} -> {}",
                    from.ext,
                    to.ext
                );
            }
        }
    }

    #[test]
    fn presets_carry_over() {
        assert_eq!(preset(quality::Preset::Small), kiln_media::Preset::Small);
        assert_eq!(preset(quality::Preset::High), kiln_media::Preset::High);
        assert_eq!(preset(quality::Preset::Best), kiln_media::Preset::Best);
    }
}
