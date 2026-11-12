//! Quality settings for one conversion, and which of them a pair uses.
//!
//! A [`Quality`] travels in [`Options`](crate::Options). Each backend reads
//! only the parts that apply to it: FFmpeg retunes the encoder values in
//! formats.toml ([`tune`]), the image backend resizes, and the PDF backend
//! draws pages at [`Quality::page_dpi`]. [`knobs`] says which settings a
//! pair offers, so a UI shows nothing that would be ignored.
//!
//! Sound that Kiln's own engine writes (the "audio" and "audio-mf" routes)
//! is tuned by [`Quality::audio`] instead, read by `kiln-media`, and video
//! it writes (the "video" route: MP4, MOV, M4V and MKV among themselves) by
//! [`Quality::video`].

/// What happens to a video's sound ([`Quality::video`]'s `sound`).
pub use kiln_media::video::SoundChoice;
use kiln_media::video::{tune as video_tune_fns, Hdr, VideoCodec, VideoInfo, VideoTune};
use kiln_media::{tune as media_tune, Channels, Codec, SourceInfo, Target, Tune};

use crate::registry::{Format, Kind};

/// How hard to compress. Lossless targets ignore it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Preset {
    /// Noticeably smaller files; fine for sharing and previews.
    Small,
    /// Visually lossless for most material. The default.
    #[default]
    High,
    /// As close to the source as the format allows; large files.
    Best,
}

impl Preset {
    pub const ALL: [Preset; 3] = [Preset::Small, Preset::High, Preset::Best];

    pub fn label(self) -> &'static str {
        match self {
            Preset::Small => "Small",
            Preset::High => "High",
            Preset::Best => "Best",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Preset::Small => "small",
            Preset::High => "high",
            Preset::Best => "best",
        }
    }

    pub fn parse(s: &str) -> Option<Preset> {
        Preset::ALL
            .into_iter()
            .find(|p| p.id().eq_ignore_ascii_case(s.trim()))
    }

    /// The JPG quality this preset stands for.
    pub fn jpeg_quality(self) -> u8 {
        match self {
            Preset::Small => 75,
            Preset::High => 90,
            Preset::Best => 97,
        }
    }
}

/// A cap on picture size or frame rate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Limit {
    /// Kiln decides for the target: the original for most, smaller for
    /// animated GIFs, which grow very large otherwise.
    #[default]
    Auto,
    /// Keep the source's.
    Original,
    /// At most this much. Never scales up.
    Max(u32),
}

impl Limit {
    /// The cap in effect, given what `Auto` means for this target.
    pub fn resolve(self, auto: Option<u32>) -> Option<u32> {
        match self {
            Limit::Auto => auto,
            Limit::Original => None,
            Limit::Max(n) => Some(n.max(1)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quality {
    pub preset: Preset,
    /// For video, the height of a landscape picture in lines, as in
    /// "1080p" (the short side, so portrait video works the same way).
    /// For images and animations, the long side in pixels.
    pub size: Limit,
    /// Frames per second for video and animations.
    pub fps: Limit,
    /// Bitrate for lossy audio, in kbit/s. `None` follows the preset.
    /// For sound Kiln writes itself it picks the nearest step of the
    /// format, unless `audio` already says what to write.
    pub audio_kbps: Option<u32>,
    /// How Kiln's own engine writes sound: the slider's setting, Fit
    /// under, sample rate, channels and bit depth. The default follows
    /// the preset. Set per job, never saved.
    pub audio: Tune,
    /// How Kiln's own engine writes video: the slider's setting, Fit under,
    /// resolution, frame rate, codec, encoder and sound. The default follows
    /// the preset, with [`Quality::size`] and [`Quality::fps`] standing in
    /// for the limits it leaves unset. The sound is re-encoded only when
    /// `video.sound` asks for it for this job (the sheet's Sound field, the
    /// command line's `--audio-kbps`); [`Quality::audio_kbps`], Settings'
    /// saved audio default, never applies to video (R55). Set per job, never
    /// saved.
    pub video: VideoTune,
    /// Pixels per inch when PDF pages and slides become images.
    pub page_dpi: u32,
}

impl Default for Quality {
    fn default() -> Self {
        Self {
            preset: Preset::High,
            size: Limit::Auto,
            fps: Limit::Auto,
            audio_kbps: None,
            audio: Tune::default(),
            video: VideoTune::default(),
            page_dpi: 150,
        }
    }
}

/// One setting a pair offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Knob {
    Preset,
    /// Resolution for video, maximum size for images.
    Size,
    Fps,
    AudioBitrate,
    PageDpi,
    /// The sound slider and its Advanced fields, for sound Kiln writes
    /// itself ([`Quality::audio`]).
    AudioTune,
    /// The video slider and its Advanced fields, for video Kiln writes
    /// itself ([`Quality::video`]).
    VideoTune,
}

/// Image types that throw detail away to save space.
pub fn lossy_image(ext: &str) -> bool {
    matches!(ext, "jpg" | "webp" | "avif" | "jxl")
}

/// Audio types with a bitrate to choose.
pub fn lossy_audio(ext: &str) -> bool {
    matches!(ext, "mp3" | "ogg" | "opus" | "m4a" | "aac" | "wma" | "ac3")
}

/// Types whose pages become one image each.
fn paged(f: &Format) -> bool {
    f.kind == Kind::Presentation
        || matches!(
            f.ext.as_str(),
            "pdf" | "docx" | "odt" | "doc" | "vsdx" | "pub"
        )
}

/// The settings that change the result of converting `from` to `to`, in
/// the order a UI should show them. Empty when there is nothing to choose,
/// as for documents and lossless audio.
pub fn knobs(from: &Format, to: &Format) -> Vec<Knob> {
    if crate::audio::native_pair(from, to) {
        return vec![Knob::AudioTune];
    }
    if crate::video::native_pair(from, to) {
        return vec![Knob::VideoTune];
    }
    match (from.kind, to.kind) {
        (Kind::Image, Kind::Video) => vec![Knob::Preset, Knob::Size, Knob::Fps],
        (Kind::Video, Kind::Video) => {
            vec![Knob::Preset, Knob::Size, Knob::Fps, Knob::AudioBitrate]
        }
        (Kind::Video | Kind::Audio, Kind::Audio) if lossy_audio(&to.ext) => {
            vec![Knob::AudioBitrate]
        }
        (Kind::Video, Kind::Image) => match to.ext.as_str() {
            "apng" => vec![Knob::Size, Knob::Fps],
            _ => vec![Knob::Preset, Knob::Size, Knob::Fps],
        },
        (_, Kind::Image) if paged(from) => {
            let mut k = vec![Knob::PageDpi];
            if lossy_image(&to.ext) {
                k.insert(0, Knob::Preset);
            }
            k
        }
        (Kind::Image, Kind::Image) => {
            let mut k = vec![Knob::Size];
            if lossy_image(&to.ext) {
                k.insert(0, Knob::Preset);
            }
            k
        }
        _ => vec![],
    }
}

/// What `Limit::Auto` means for picture size when writing `to` from a
/// video: animations are kept small, everything else keeps its size.
pub fn auto_size(to: &Format) -> Option<u32> {
    match to.ext.as_str() {
        "gif" => Some(480),
        "webp" | "apng" => Some(720),
        _ => None,
    }
}

/// What `Limit::Auto` means for frame rate when writing `to`.
pub fn auto_fps(to: &Format) -> Option<u32> {
    match to.ext.as_str() {
        "gif" => Some(15),
        "webp" | "apng" => Some(24),
        _ => None,
    }
}

fn round_kbps(k: u32) -> u32 {
    const STEPS: [u32; 12] = [48, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384];
    // Ties go to the higher step.
    *STEPS
        .iter()
        .rev()
        .min_by_key(|s| s.abs_diff(k))
        .expect("STEPS is not empty")
}

/// The highest audio bitrate worth asking an encoder for.
fn audio_cap(codec: &str) -> u32 {
    match codec {
        "libopus" => 256,
        "ac3" => 640,
        "mp2" => 384,
        _ => 320,
    }
}

/// The audio bitrate a preset gives, starting from formats.toml's value.
fn preset_kbps(base: u32, codec: &str, preset: Preset) -> u32 {
    match preset {
        Preset::Small => round_kbps(base * 2 / 3).max(48),
        Preset::High => base,
        Preset::Best => round_kbps(base * 3 / 2).min(audio_cap(codec)),
    }
}

/// The quality Windows' H.264 and HEVC encoders get through FFmpeg
/// (`h264_mf -rate_control quality -quality Q`, 1 to 100, higher is
/// better) for libx264's `crf`: 60 at formats.toml's CRF 20 (High), 41 at
/// Small's 26 and 70 at Best's 17.
pub fn mf_quality(crf: u32) -> u32 {
    (125.0 - 3.25 * f64::from(crf)).round().clamp(1.0, 100.0) as u32
}

/// formats.toml's FFmpeg arguments for `to`, with the encoder values
/// changed to suit `q`. The values in formats.toml are the High preset.
pub fn tune(args: &[String], q: &Quality) -> Vec<String> {
    let codec = |flag: &str| {
        args.windows(2)
            .find(|w| w[0] == flag)
            .map(|w| w[1].clone())
            .unwrap_or_default()
    };
    let video = codec("-c:v");
    let audio = codec("-c:a");
    let mut out: Vec<String> = Vec::with_capacity(args.len() + 2);
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        let Some(value) = args.get(i + 1) else {
            out.push(args[i].clone());
            break;
        };
        let replaced: Option<(String, String)> = match flag {
            "-crf" => {
                let base: i32 = value.parse().unwrap_or(23);
                let v = match (video.as_str(), q.preset) {
                    ("libaom-av1", Preset::Small) => 34,
                    ("libaom-av1", Preset::Best) => 16,
                    ("libvpx-vp9", Preset::Small) => base + 7,
                    ("libvpx-vp9", Preset::Best) => base - 7,
                    (_, Preset::Small) => base + 6,
                    (_, Preset::Best) => base - 3,
                    (_, Preset::High) => base,
                };
                Some((flag.to_owned(), v.clamp(0, 63).to_string()))
            }
            "-distance" => Some((
                flag.to_owned(),
                match q.preset {
                    Preset::Small => "2.5".to_owned(),
                    Preset::High => value.clone(),
                    Preset::Best => "0.4".to_owned(),
                },
            )),
            "-q:v" => {
                let base: i32 = value.parse().unwrap_or(3);
                let v = match (video.as_str(), q.preset) {
                    // Higher is better for these two.
                    ("libtheora", Preset::Small) => 5,
                    ("libtheora", Preset::Best) => 10,
                    ("libwebp_anim", Preset::Small) => 60,
                    ("libwebp_anim", Preset::Best) => 92,
                    // Lower is better for the MPEG family.
                    (_, Preset::Small) => base + 3,
                    (_, Preset::Best) => (base - 1).max(1),
                    (_, Preset::High) => base,
                };
                Some((flag.to_owned(), v.to_string()))
            }
            "-b:a" => {
                let base: u32 = value.trim_end_matches('k').parse().unwrap_or(160);
                let kbps = q
                    .audio_kbps
                    .map(|k| k.min(audio_cap(&audio)))
                    .unwrap_or_else(|| preset_kbps(base, &audio, q.preset));
                Some((flag.to_owned(), format!("{kbps}k")))
            }
            "-q:a" => match q.audio_kbps {
                // An explicit bitrate replaces variable-bitrate quality.
                Some(k) => Some(("-b:a".to_owned(), format!("{}k", k.min(audio_cap(&audio))))),
                None => {
                    let v = match (audio.as_str(), q.preset) {
                        ("libmp3lame", Preset::Small) => "5".to_owned(),
                        ("libmp3lame", Preset::Best) => "0".to_owned(),
                        ("libvorbis", Preset::Small) => "3".to_owned(),
                        ("libvorbis", Preset::Best) => "8".to_owned(),
                        _ => value.clone(),
                    };
                    Some((flag.to_owned(), v))
                }
            },
            _ => None,
        };
        match replaced {
            Some((f, v)) => {
                out.push(f);
                out.push(v);
                i += 2;
            }
            None => {
                out.push(args[i].clone());
                i += 1;
            }
        }
    }
    out
}

/// What Kiln knows about a media file, for estimates and for offering
/// only the choices that make sense for it. The picture's facts come from
/// Kiln's own engine for MP4, MOV, M4V and MKV, else from FFprobe; the
/// sound's from Kiln's own engine where it reads the file, else from
/// FFprobe too.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MediaInfo {
    /// Seconds. `None` when the file doesn't say, as a fragmented MP4
    /// doesn't (Fit under is not offered then).
    pub duration: Option<f64>,
    /// The coded picture size, before any rotation.
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    /// The picture's codec, when Kiln's own engine read the picture; `None`
    /// for FFprobe's facts. (`codec` is the sound's.)
    pub video_codec: Option<VideoCodec>,
    /// Degrees clockwise the picture is turned to show upright: 0, 90, 180
    /// or 270.
    pub rotation: u16,
    /// The picture's dynamic range and bit depth, when Kiln's own engine
    /// read the picture.
    pub hdr: Option<Hdr>,
    pub bit_depth: Option<u8>,
    /// The picture's average bitrate, when the container lets it be worked
    /// out.
    pub video_kbps: Option<u32>,
    /// The Microsoft Store extension Windows needs to read the picture and
    /// lacks, as (codec, Store product ID): ("HEVC", "9N4WGH0Z6VHQ"), for
    /// the row's line naming the formats that need it, with Get it (R56,
    /// R89; see [`decodes_picture`]). `None` when the picture can be read,
    /// or wasn't read by Kiln's own engine.
    pub needs_extension: Option<(&'static str, &'static str)>,
    /// How far before the start an MP4's edit list presents lies the key
    /// frame a copy begins at, in seconds, when the list hides frames (see
    /// kiln-media's `VideoInfo::lead_in`): MKV can't copy such a picture.
    pub lead_in: Option<f64>,
    pub has_audio: bool,
    /// How many sound tracks and subtitle tracks the file holds, when its
    /// headers or FFprobe say; 0 means none, or not known.
    pub sound_tracks: u16,
    pub subtitle_tracks: u16,
    /// Kiln's own sound and video engines are not known to read the first of
    /// a video's `sound_tracks`: they read the first whose codec Symphonia
    /// can name, a later one when the first is AC-3, E-AC-3, DTS or TrueHD,
    /// and it is also set when the file can't be told. Set where the tracks
    /// are counted, for a video with more than one; `false` for anything
    /// else. [`crate::Kiln::drops`] leaves out "Keeps the first of n sound
    /// tracks." for such a file on a route those engines run.
    pub later_sound: bool,
    /// The sound's codec as people know it: "MP3", "AAC", "PCM".
    pub codec: Option<String>,
    /// The sound's bitrate, when the file says.
    pub kbps: Option<u32>,
    pub channels: Option<u16>,
    /// Samples a second.
    pub rate: Option<u32>,
    /// Bit depth of integer sound (PCM, FLAC, WavPack, ALAC), or of float
    /// PCM when `float` says so.
    pub bits: Option<u16>,
    /// The sound is floating-point PCM (a float WAV, say), as Kiln's own
    /// engine read it.
    pub float: bool,
}

/// The sound in `info` as kiln-media's tuning reads it, for slider steps,
/// presets, estimates and Fit under. `None` unless Kiln's own engine read
/// the sound (its rate and channels are known).
pub fn source_info(info: &MediaInfo) -> Option<SourceInfo> {
    let (rate, count) = (info.rate?, info.channels?);
    Some(SourceInfo {
        codec: codec_of(info),
        rate,
        channels: Channels { count, mask: 0 },
        bits: info.bits,
        frames: info
            .duration
            .map(|secs| (secs * f64::from(rate)).round() as u64),
        kbps: info.kbps,
        tags: Default::default(),
    })
}

/// The sound's codec as far as tuning needs it: float PCM is said so, since
/// it changes the depth written; any other codec only by its name.
pub fn codec_of(info: &MediaInfo) -> Codec {
    match (info.float, info.bits) {
        (true, Some(bits)) => Codec::Pcm { bits, float: true },
        (true, None) => Codec::Pcm {
            bits: 32,
            float: true,
        },
        _ => Codec::Other(info.codec.clone().unwrap_or_default()),
    }
}

/// The sound's codec in `info` from the name kiln-media gave it
/// ([`Codec::display_name`]): "AAC" is [`Codec::Aac`]. A name it doesn't
/// give is [`codec_of`]'s.
fn sound_codec(info: &MediaInfo) -> Codec {
    match info.codec.as_deref() {
        Some("PCM") => Codec::Pcm {
            bits: info.bits.unwrap_or(16),
            float: info.float,
        },
        Some("MP3") => Codec::Mp3,
        Some("AAC") => Codec::Aac,
        Some("HE-AAC") => Codec::HeAac,
        Some("ALAC") => Codec::Alac,
        Some("FLAC") => Codec::Flac,
        Some("Vorbis") => Codec::Vorbis,
        Some("Opus") => Codec::Opus,
        Some("WMA") => Codec::Wma,
        Some("AC3") => Codec::Ac3,
        Some("WavPack") => Codec::WavPack,
        _ => codec_of(info),
    }
}

/// The video in `info` as kiln-media's video tuning reads it, for slider
/// steps, labels, estimates and Fit under. `None` unless Kiln's own engine
/// read the picture (see [`MediaInfo::video_codec`]).
pub fn video_info(info: &MediaInfo) -> Option<VideoInfo> {
    let codec = info.video_codec.clone()?;
    // The sound's codec by name, so the model knows when it is copied (R90).
    let sound = source_info(info).map(|s| SourceInfo {
        codec: sound_codec(info),
        ..s
    });
    // Sound Kiln can't read still counts toward the size.
    let sound_unreadable =
        (info.has_audio && sound.is_none()).then(|| info.codec.clone().unwrap_or_default());
    Some(VideoInfo {
        width: info.width?,
        height: info.height?,
        rotation: info.rotation,
        fps: info.fps,
        variable_rate: false,
        codec,
        profile: None,
        bit_depth: info.bit_depth.unwrap_or(8),
        hdr: info.hdr.unwrap_or(Hdr::None),
        duration: info.duration,
        video_kbps: info.video_kbps,
        sound,
        sound_unreadable,
        sar: (1, 1),
        mastering: None,
        content_light: None,
        created: None,
        title: None,
        lead_in: info.lead_in,
        // The colour changes no step or estimate.
        colour_tagged: false,
    })
}

/// Whether converting the picture in `info` to the format `ext` ("mp4")
/// decodes it: `ext` is one of Kiln's own video targets and, with the plain
/// settings (Original where it can copy, R44), the picture can't go into it
/// as it is. A copy needs no decoder, so a file whose picture waits on a
/// Store extension still copies; only these targets need the extension
/// (R89).
pub fn decodes_picture(info: &MediaInfo, ext: &str) -> bool {
    let t = crate::video::target_of(&ext.to_ascii_lowercase());
    match (t, video_info(info)) {
        (Some(t), Some(v)) => !video_tune_fns::can_copy_video(t, &v, &VideoTune::default()),
        _ => false,
    }
}

/// The video tuning Kiln's own engine gets for `q`: [`Quality::video`],
/// with the resolution and frame rate limits from [`Quality::size`] and
/// [`Quality::fps`] where it sets none, so a saved default or the command
/// line's `--size` and `--fps` apply. The sound is `video.sound`'s alone
/// (R55), and sound encoded under Keep takes [`Quality::preset`]'s bitrate
/// (R95).
pub fn video_tune(q: &Quality) -> VideoTune {
    let mut tune = q.video;
    tune.sound_preset = crate::audio::preset(q.preset);
    if tune.max_lines.is_none() {
        tune.max_lines = q.size.resolve(None);
    }
    if tune.max_fps.is_none() {
        tune.max_fps = q.fps.resolve(None);
    }
    tune
}

/// Whether Fit under can be offered: only with a known length, since no
/// honest bitrate can be worked out without one (ruling R12).
pub fn fit_under_offered(info: &MediaInfo) -> bool {
    info.duration.is_some_and(|d| d > 0.0)
}

/// The tuning Kiln's own engine gets for `q`: [`Quality::audio`], with
/// [`Quality::audio_kbps`] as its bitrate when it names none, so a saved
/// default or the command line's bitrate picks the nearest step.
pub fn media_tune(q: &Quality) -> Tune {
    let mut tune = q.audio;
    if tune.kbps.is_none() {
        tune.kbps = q.audio_kbps;
    }
    tune
}

/// What Kiln's own engine would write `t` with for `written`, the sound as
/// written ([`kiln_media::written`]), chosen the way a conversion chooses it.
fn tuned_setting(t: Target, q: &Quality, written: &SourceInfo) -> Option<media_tune::Setting> {
    media_tune::chosen_setting(t, crate::audio::preset(q.preset), &media_tune(q), written).ok()
}

/// The size after `size` is applied to a `w` x `h` picture: the short
/// side for video, the long side for everything else. Never larger than
/// the source; video sides are rounded to even numbers.
pub fn scaled(w: u32, h: u32, cap: Option<u32>, video: bool) -> (u32, u32) {
    let Some(cap) = cap else {
        return (w, h);
    };
    let side = if video { w.min(h) } else { w.max(h) };
    if side <= cap || side == 0 {
        return (w, h);
    }
    let f = cap as f64 / side as f64;
    let (sw, sh) = ((w as f64 * f).round() as u32, (h as f64 * f).round() as u32);
    if video {
        (sw.max(2) & !1, sh.max(2) & !1)
    } else {
        (sw.max(1), sh.max(1))
    }
}

/// A rough size for the result in bytes, when one can be given: for video
/// and lossy audio. Animated images and documents vary too much.
pub fn estimate(to: &Format, q: &Quality, info: &MediaInfo) -> Option<u64> {
    let secs = info.duration?;
    let audio_bits = |default_kbps: u32| -> f64 {
        let kbps = q.audio_kbps.unwrap_or(match q.preset {
            Preset::Small => default_kbps * 2 / 3,
            Preset::High => default_kbps,
            Preset::Best => (default_kbps * 3 / 2).min(320),
        });
        kbps as f64 * 1000.0 * secs
    };
    if let (Some(t), Some(v)) = (crate::video::target_of(&to.ext), video_info(info)) {
        // Video Kiln writes itself, estimated as its engine chooses the
        // setting (R44).
        let tune = video_tune(q);
        let s = video_tune_fns::chosen_setting(t, crate::audio::preset(q.preset), &tune, &v);
        return video_tune_fns::estimate_bytes(t, &s.ok()?, &v, &tune);
    }
    if let (Some(t), Some(src)) = (Target::from_ext(&to.ext), source_info(info)) {
        // Sound Kiln writes itself, estimated the way its engine does: for
        // the sound as written, after the target's and Tune's channels and
        // rate (5.1 to MP3 is stereo, 96 kHz asked down to 48 is 48).
        let w = kiln_media::written(t, &media_tune(q), &src);
        return media_tune::estimate_bytes(t, &tuned_setting(t, q, &w)?, &w);
    }
    let bits = match to.kind {
        Kind::Audio if lossy_audio(&to.ext) => {
            let base = to
                .ffmpeg
                .windows(2)
                .find(|w| w[0] == "-b:a")
                .and_then(|w| w[1].trim_end_matches('k').parse().ok())
                .unwrap_or(192);
            audio_bits(base)
        }
        Kind::Video => {
            let (w, h) = (info.width?, info.height?);
            let (w, h) = scaled(w, h, q.size.resolve(None), true);
            let src_fps = info.fps.unwrap_or(30.0);
            let fps = q
                .fps
                .resolve(None)
                .map_or(src_fps, |f| src_fps.min(f as f64));
            // Bits per pixel per frame for H.264 at each preset; other
            // encoders are scaled from it.
            let bpp: f64 = match q.preset {
                Preset::Small => 0.045,
                Preset::High => 0.09,
                Preset::Best => 0.15,
            };
            let codec = to
                .ffmpeg
                .windows(2)
                .find(|w| w[0] == "-c:v")
                .map(|w| w[1].as_str())
                .unwrap_or("");
            let factor: f64 = match codec {
                "libvpx-vp9" | "libaom-av1" => 0.7,
                "mpeg4" | "wmv2" | "mpeg2video" | "libtheora" => 1.8,
                _ => 1.0,
            };
            // Never more bits a pixel than the source spends at its own size
            // and frame rate (ENG-14): a screen recording's few are
            // re-encoded in about as few. A smaller picture gets the looser
            // cap Kiln's own engine gives it.
            let own = info
                .width
                .zip(info.height)
                .map(|(sw, sh)| sw as f64 * sh as f64 * src_fps);
            let source = info
                .video_kbps
                .filter(|&k| k > 0)
                .zip(own)
                .map(|(k, own)| (k as f64 * 1000.0 / own, own))
                .filter(|(b, _)| b.is_finite());
            let bpp = source.map_or(bpp * factor, |(s, own)| {
                let cap = video_tune_fns::source_cap(s, own, w as f64 * h as f64 * fps);
                (bpp * factor).min(cap)
            });
            let video = bpp * w as f64 * h as f64 * fps * secs;
            let audio = if info.has_audio { audio_bits(160) } else { 0.0 };
            video + audio
        }
        _ => return None,
    };
    Some((bits / 8.0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Registry;

    fn strings(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    fn with(preset: Preset) -> Quality {
        Quality {
            preset,
            ..Quality::default()
        }
    }

    #[test]
    fn high_leaves_the_registry_values_alone() {
        let args = strings(&[
            "-c:v", "libx264", "-crf", "20", "-c:a", "aac", "-b:a", "160k",
        ]);
        assert_eq!(tune(&args, &Quality::default()), args);
    }

    #[test]
    fn presets_move_crf_and_bitrate() {
        let args = strings(&[
            "-c:v", "libx264", "-crf", "20", "-c:a", "aac", "-b:a", "160k",
        ]);
        assert_eq!(
            tune(&args, &with(Preset::Small)),
            strings(&["-c:v", "libx264", "-crf", "26", "-c:a", "aac", "-b:a", "112k"])
        );
        assert_eq!(
            tune(&args, &with(Preset::Best)),
            strings(&["-c:v", "libx264", "-crf", "17", "-c:a", "aac", "-b:a", "256k"])
        );
    }

    #[test]
    fn windows_h264_quality_follows_the_preset() {
        // formats.toml's H.264 lines say CRF 20; the presets move it to 26,
        // 20 and 17, and h264_mf's quality (higher is better) follows.
        let args = strings(&["-c:v", "@h264", "-crf", "20"]);
        let q_for = |p| {
            let tuned = tune(&args, &with(p));
            let crf: u32 = tuned[3].parse().unwrap();
            mf_quality(crf)
        };
        assert_eq!(q_for(Preset::Small), 41);
        assert_eq!(q_for(Preset::High), 60);
        assert_eq!(q_for(Preset::Best), 70);
        // 3GP's CRF 23, and the ends of the quality steps' range.
        assert_eq!(mf_quality(23), 50);
        assert_eq!(mf_quality(16), 73);
        assert_eq!(mf_quality(35), 11);
        // Never outside h264_mf's 1..=100.
        assert_eq!(mf_quality(0), 100);
        assert_eq!(mf_quality(63), 1);
    }

    #[test]
    fn an_explicit_bitrate_replaces_vbr_quality() {
        let args = strings(&["-c:a", "libmp3lame", "-q:a", "2"]);
        let q = Quality {
            audio_kbps: Some(128),
            ..Quality::default()
        };
        assert_eq!(
            tune(&args, &q),
            strings(&["-c:a", "libmp3lame", "-b:a", "128k"])
        );
        let opus = strings(&["-c:a", "libopus", "-b:a", "128k"]);
        let q = Quality {
            audio_kbps: Some(320),
            ..Quality::default()
        };
        assert_eq!(
            tune(&opus, &q),
            strings(&["-c:a", "libopus", "-b:a", "256k"])
        );
    }

    #[test]
    fn mpeg_family_quality_goes_down_for_better() {
        let args = strings(&["-c:v", "mpeg4", "-q:v", "3"]);
        assert_eq!(tune(&args, &with(Preset::Best))[3], "2");
        assert_eq!(tune(&args, &with(Preset::Small))[3], "6");
        let theora = strings(&["-c:v", "libtheora", "-q:v", "8"]);
        assert_eq!(tune(&theora, &with(Preset::Best))[3], "10");
    }

    #[test]
    fn scaling_never_grows_and_keeps_video_even() {
        assert_eq!(scaled(1920, 1080, Some(720), true), (1280, 720));
        assert_eq!(scaled(1080, 1920, Some(720), true), (720, 1280));
        assert_eq!(scaled(640, 360, Some(720), true), (640, 360));
        assert_eq!(scaled(1001, 501, Some(250), true), (500, 250));
        assert_eq!(scaled(4000, 3000, Some(2000), false), (2000, 1500));
        assert_eq!(scaled(400, 300, None, false), (400, 300));
    }

    #[test]
    fn knobs_fit_the_pair() {
        let r = Registry::builtin();
        let k = |a: &str, b: &str| knobs(r.format(a).unwrap(), r.format(b).unwrap());
        // Video Kiln writes itself has the video slider.
        assert_eq!(k("mkv", "mp4"), [Knob::VideoTune]);
        assert_eq!(
            k("webm", "mp4"),
            [Knob::Preset, Knob::Size, Knob::Fps, Knob::AudioBitrate]
        );
        assert_eq!(
            k("mkv", "webm"),
            [Knob::Preset, Knob::Size, Knob::Fps, Knob::AudioBitrate]
        );
        // Sound Kiln writes itself has the slider, lossless too.
        assert_eq!(k("wav", "mp3"), [Knob::AudioTune]);
        assert_eq!(k("wav", "flac"), [Knob::AudioTune]);
        assert_eq!(k("mp4", "m4a"), [Knob::AudioTune]);
        // FFmpeg's sound keeps its bitrate row.
        assert_eq!(k("avi", "mp3"), [Knob::AudioBitrate]);
        assert!(k("avi", "flac").is_empty());
        assert_eq!(k("png", "jpg"), [Knob::Preset, Knob::Size]);
        assert_eq!(k("png", "webp"), [Knob::Preset, Knob::Size]);
        assert_eq!(k("png", "bmp"), [Knob::Size]);
        assert_eq!(k("pdf", "png"), [Knob::PageDpi]);
        assert_eq!(k("pptx", "jpg"), [Knob::Preset, Knob::PageDpi]);
        assert_eq!(k("mp4", "gif"), [Knob::Preset, Knob::Size, Knob::Fps]);
        assert!(k("md", "docx").is_empty());
    }

    #[test]
    fn estimates_cover_video_and_lossy_audio() {
        let r = Registry::builtin();
        let info = MediaInfo {
            duration: Some(60.0),
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            has_audio: true,
            ..MediaInfo::default()
        };
        let mp4 = estimate(r.format("mp4").unwrap(), &Quality::default(), &info).unwrap();
        assert!((20_000_000..80_000_000).contains(&mp4), "{mp4}");
        let small = estimate(r.format("mp4").unwrap(), &with(Preset::Small), &info).unwrap();
        assert!(small < mp4);
        let mp3 = estimate(r.format("opus").unwrap(), &Quality::default(), &info).unwrap();
        assert_eq!(mp3, 128_000 * 60 / 8);
        assert!(estimate(r.format("flac").unwrap(), &Quality::default(), &info).is_none());
    }

    #[test]
    fn ffmpeg_s_video_estimate_is_capped_by_the_source_s_own_rate() {
        // A screen recording, 1080p at 30 fps in 1000 kbps: whatever FFmpeg
        // writes it as, the estimate is never over the source's picture.
        let r = Registry::builtin();
        let screen = MediaInfo {
            duration: Some(60.0),
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            video_kbps: Some(1000),
            ..MediaInfo::default()
        };
        let source = 1000 * 1000 / 8 * 60;
        for ext in ["webm", "avi", "wmv"] {
            for preset in [Preset::Small, Preset::High, Preset::Best] {
                let got = estimate(r.format(ext).unwrap(), &with(preset), &screen).unwrap();
                assert!(got <= source, "{ext} {preset:?}: {got} > {source}");
            }
        }
        // A source in more bits a pixel than the preset spends keeps the
        // preset's figure.
        let rich = MediaInfo {
            video_kbps: Some(40_000),
            ..screen.clone()
        };
        let plain = MediaInfo {
            video_kbps: None,
            ..screen
        };
        let webm = r.format("webm").unwrap();
        assert_eq!(
            estimate(webm, &with(Preset::High), &rich),
            estimate(webm, &with(Preset::High), &plain)
        );
    }

    #[test]
    fn ffmpeg_s_cap_is_looser_for_a_smaller_picture() {
        // At the source's own size the cap is its own bits a pixel; at 720
        // lines, which need more bits a pixel, it is looser, though the
        // estimate stays under the source's.
        let r = Registry::builtin();
        let screen = MediaInfo {
            duration: Some(60.0),
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            video_kbps: Some(1000),
            ..MediaInfo::default()
        };
        let webm = r.format("webm").unwrap();
        let full = estimate(webm, &with(Preset::Small), &screen).unwrap() as f64;
        let q = Quality {
            size: Limit::Max(720),
            ..with(Preset::Small)
        };
        let less = estimate(webm, &q, &screen).unwrap() as f64;
        let pixels = (1280.0 * 720.0) / (1920.0 * 1080.0);
        assert!(less > full * pixels * 1.2, "{less} {full}");
        assert!(less < full, "{less} {full}");
    }

    #[test]
    fn sound_kiln_writes_is_estimated_as_written() {
        let r = Registry::builtin();
        let song = |channels| MediaInfo {
            duration: Some(221.0),
            has_audio: true,
            codec: Some("FLAC".into()),
            channels: Some(channels),
            rate: Some(48_000),
            bits: Some(16),
            ..MediaInfo::default()
        };
        let est = |ext: &str, q: &Quality, i: &MediaInfo| estimate(r.format(ext).unwrap(), q, i);
        // MP3 folds 5.1 to stereo, so V2 from 5.1 is V2 in stereo: about
        // 4.8 MB, not three times that.
        let q = Quality::default();
        let stereo = est("mp3", &q, &song(2)).unwrap();
        assert_eq!(est("mp3", &q, &song(6)), Some(stereo));
        assert_eq!(stereo, 4_779_125);
        // Tune's channels and rate count: mono MP3 is 0.6 of stereo, and
        // FLAC asked down to 24 kHz from 48 is half the size.
        let mono = Quality {
            audio: Tune {
                channels: kiln_media::ChannelChoice::Mono,
                ..Tune::default()
            },
            ..Quality::default()
        };
        assert_eq!(est("mp3", &mono, &song(2)), Some(2_867_475));
        let flac = est("flac", &q, &song(2)).unwrap();
        let half = Quality {
            audio: Tune {
                rate: Some(24_000),
                ..Tune::default()
            },
            ..Quality::default()
        };
        let halved = est("flac", &half, &song(2)).unwrap();
        assert!(halved.abs_diff(flac / 2) < 100, "{halved} against {flac}");
    }

    #[test]
    fn the_video_tune_carries_the_presets_sound() {
        // R95: sound Keep encodes follows the preset, and the tune says so.
        for (preset, want) in [
            (Preset::Small, kiln_media::Preset::Small),
            (Preset::High, kiln_media::Preset::High),
            (Preset::Best, kiln_media::Preset::Best),
        ] {
            let q = Quality {
                preset,
                ..Quality::default()
            };
            assert_eq!(video_tune(&q).sound_preset, want);
        }
    }

    #[test]
    fn the_video_model_knows_the_sound_by_name() {
        // R90: AAC is copied into MP4, so Keep counts its own 256 kbps;
        // FLAC isn't, so it counts the AAC encoder's 160.
        use kiln_media::video::{tune as vt, VideoTarget};
        let with = |codec: &str, kbps| MediaInfo {
            width: Some(1280),
            height: Some(720),
            fps: Some(30.0),
            duration: Some(10.0),
            video_codec: Some(VideoCodec::H264),
            has_audio: true,
            codec: Some(codec.to_owned()),
            kbps: Some(kbps),
            channels: Some(2),
            rate: Some(48_000),
            ..MediaInfo::default()
        };
        let tune = VideoTune::default();
        let aac = video_info(&with("AAC", 256)).unwrap();
        assert_eq!(aac.sound.as_ref().unwrap().codec, Codec::Aac);
        assert_eq!(vt::sound_kbps(VideoTarget::Mp4, &aac, &tune), 256.0);
        let flac = video_info(&with("FLAC", 900)).unwrap();
        assert_eq!(vt::sound_kbps(VideoTarget::Mp4, &flac, &tune), 160.0);
        assert_eq!(vt::sound_kbps(VideoTarget::Mkv, &flac, &tune), 900.0);
        let ac3 = video_info(&with("AC3", 448)).unwrap();
        assert_eq!(vt::sound_kbps(VideoTarget::Mkv, &ac3, &tune), 448.0);
        assert_eq!(vt::sound_kbps(VideoTarget::Mp4, &ac3, &tune), 160.0);
    }

    #[test]
    fn only_targets_that_encode_the_picture_decode_it() {
        // R89: an AV1 MP4 copies into MKV, and only MP4, MOV and M4V need
        // the picture decoded. Sound-only targets never do.
        let av1 = MediaInfo {
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            duration: Some(10.0),
            video_codec: Some(VideoCodec::Av1),
            ..MediaInfo::default()
        };
        assert!(!decodes_picture(&av1, "mkv"));
        assert!(!decodes_picture(&av1, "MKV"));
        for ext in ["mp4", "mov", "m4v"] {
            assert!(decodes_picture(&av1, ext), "{ext}");
        }
        assert!(!decodes_picture(&av1, "mp3"));
        // HEVC copies into all four.
        let hevc = MediaInfo {
            video_codec: Some(VideoCodec::Hevc),
            ..av1.clone()
        };
        for ext in ["mp4", "mov", "m4v", "mkv"] {
            assert!(!decodes_picture(&hevc, ext), "{ext}");
        }
        // A turned picture can't be copied into MKV.
        let turned = MediaInfo {
            rotation: 90,
            ..hevc.clone()
        };
        assert!(decodes_picture(&turned, "mkv"));
        // Facts FFprobe read have no picture codec: nothing to judge.
        let probed = MediaInfo {
            video_codec: None,
            ..hevc
        };
        assert!(!decodes_picture(&probed, "mp4"));
    }
}
