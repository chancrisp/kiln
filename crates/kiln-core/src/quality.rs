//! Quality settings for one conversion, and which of them a pair uses.
//!
//! A [`Quality`] travels in [`Options`](crate::Options). Each backend reads
//! only the parts that apply to it: FFmpeg retunes the encoder values in
//! formats.toml ([`tune`]), the image backend resizes, and the PDF backend
//! draws pages at [`Quality::page_dpi`]. [`knobs`] says which settings a
//! pair offers, so a UI shows nothing that would be ignored.

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
    pub audio_kbps: Option<u32>,
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
}

/// Image types that throw detail away to save space.
pub fn lossy_image(ext: &str) -> bool {
    matches!(ext, "jpg" | "avif" | "jxl")
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

/// What ffprobe says about a media file, for estimates and for offering
/// only the choices that make sense for it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MediaInfo {
    pub duration: Option<f64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    pub has_audio: bool,
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
            let bpp = match q.preset {
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
            let factor = match codec {
                "libvpx-vp9" | "libaom-av1" => 0.7,
                "mpeg4" | "wmv2" | "mpeg2video" | "libtheora" => 1.8,
                _ => 1.0,
            };
            let video = bpp * factor * w as f64 * h as f64 * fps * secs;
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
        assert_eq!(
            k("mkv", "mp4"),
            [Knob::Preset, Knob::Size, Knob::Fps, Knob::AudioBitrate]
        );
        assert_eq!(k("wav", "mp3"), [Knob::AudioBitrate]);
        assert!(k("wav", "flac").is_empty());
        assert_eq!(k("png", "jpg"), [Knob::Preset, Knob::Size]);
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
        };
        let mp4 = estimate(r.format("mp4").unwrap(), &Quality::default(), &info).unwrap();
        assert!((20_000_000..80_000_000).contains(&mp4), "{mp4}");
        let small = estimate(r.format("mp4").unwrap(), &with(Preset::Small), &info).unwrap();
        assert!(small < mp4);
        let mp3 = estimate(r.format("opus").unwrap(), &Quality::default(), &info).unwrap();
        assert_eq!(mp3, 128_000 * 60 / 8);
        assert!(estimate(r.format("flac").unwrap(), &Quality::default(), &info).is_none());
    }
}
