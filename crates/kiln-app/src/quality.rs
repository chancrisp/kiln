//! Quality in the app: the defaults kept in settings.toml, the values each
//! control steps through, and how they read. Platform-independent, so the
//! choices can be tested anywhere.

use serde::{Deserialize, Serialize};

use kiln_core::quality::{self as q, MediaInfo};
use kiln_core::{Format, Kind, Knob, Limit, Preset, Quality};

use crate::model::human_size;

/// The quality each kind of result gets unless a conversion's options say
/// otherwise. Kept in settings.toml under `[quality]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct QualityDefaults {
    #[serde(with = "preset_text")]
    pub video: Preset,
    /// The short side of the picture, as in "1080p".
    #[serde(with = "limit_text")]
    pub resolution: Limit,
    #[serde(with = "limit_text")]
    pub fps: Limit,
    #[serde(with = "preset_text")]
    pub images: Preset,
    /// The long side of the picture, in pixels.
    #[serde(with = "limit_text")]
    pub image_size: Limit,
    /// Unset follows the preset.
    #[serde(with = "kbps_text", skip_serializing_if = "Option::is_none")]
    pub audio_kbps: Option<u32>,
    pub page_dpi: u32,
}

impl Default for QualityDefaults {
    fn default() -> Self {
        Self {
            video: Preset::High,
            resolution: Limit::Original,
            fps: Limit::Original,
            images: Preset::High,
            image_size: Limit::Original,
            audio_kbps: None,
            page_dpi: 150,
        }
    }
}

impl QualityDefaults {
    /// Whether the values are ones the controls offer; a hand-edited file
    /// could hold anything.
    pub fn is_valid(&self) -> bool {
        (36..=600).contains(&self.page_dpi)
            && self.audio_kbps.is_none_or(|k| (32..=640).contains(&k))
            && [self.resolution, self.fps, self.image_size]
                .iter()
                .all(|l| !matches!(l, Limit::Max(0)))
    }

    /// The quality for converting `from` to `to`.
    pub fn quality_for(&self, from: &Format, to: &Format) -> Quality {
        let mut quality = Quality {
            audio_kbps: self.audio_kbps,
            page_dpi: self.page_dpi,
            ..Quality::default()
        };
        match (from.kind, to.kind) {
            (_, Kind::Video) => {
                quality.preset = self.video;
                quality.size = self.resolution;
                quality.fps = self.fps;
            }
            // Animations: Kiln keeps them small unless asked otherwise.
            (Kind::Video, Kind::Image) => quality.preset = self.video,
            (Kind::Image, Kind::Image) => {
                quality.preset = self.images;
                quality.size = self.image_size;
            }
            (_, Kind::Image) => quality.preset = self.images,
            _ => {}
        }
        quality
    }

    /// The value of a setting on the Quality page.
    pub fn get(&self, s: DefaultSetting) -> Value {
        match s {
            DefaultSetting::Video => Value::Preset(self.video),
            DefaultSetting::Resolution => Value::Limit(self.resolution),
            DefaultSetting::Fps => Value::Limit(self.fps),
            DefaultSetting::Images => Value::Preset(self.images),
            DefaultSetting::ImageSize => Value::Limit(self.image_size),
            DefaultSetting::Audio => Value::Kbps(self.audio_kbps),
            DefaultSetting::PageDpi => Value::Dpi(self.page_dpi),
        }
    }

    pub fn set(&mut self, s: DefaultSetting, v: Value) {
        match (s, v) {
            (DefaultSetting::Video, Value::Preset(p)) => self.video = p,
            (DefaultSetting::Resolution, Value::Limit(l)) => self.resolution = l,
            (DefaultSetting::Fps, Value::Limit(l)) => self.fps = l,
            (DefaultSetting::Images, Value::Preset(p)) => self.images = p,
            (DefaultSetting::ImageSize, Value::Limit(l)) => self.image_size = l,
            (DefaultSetting::Audio, Value::Kbps(k)) => self.audio_kbps = k,
            (DefaultSetting::PageDpi, Value::Dpi(d)) => self.page_dpi = d,
            _ => {}
        }
    }

    /// Move a stepping setting one value along, wrapping at the ends.
    pub fn step(&mut self, s: DefaultSetting, delta: isize) {
        let list = s.values();
        let v = stepped(&list, self.get(s), delta, true);
        self.set(s, v);
    }
}

/// The settings on the Quality page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DefaultSetting {
    Video,
    Resolution,
    Fps,
    Images,
    ImageSize,
    Audio,
    PageDpi,
}

impl DefaultSetting {
    /// What the control steps through. Presets are a segmented control
    /// instead, but list theirs too.
    pub fn values(self) -> Vec<Value> {
        match self {
            DefaultSetting::Video | DefaultSetting::Images => {
                Preset::ALL.into_iter().map(Value::Preset).collect()
            }
            DefaultSetting::Resolution => limits(VIDEO_SIZES, false),
            DefaultSetting::Fps => limits(VIDEO_FPS, false),
            DefaultSetting::ImageSize => limits(IMAGE_SIZES, false),
            DefaultSetting::Audio => kbps_values(320),
            DefaultSetting::PageDpi => DPIS.iter().map(|d| Value::Dpi(*d)).collect(),
        }
    }

    /// How a value reads on the control.
    pub fn label(self, v: Value) -> String {
        match self {
            DefaultSetting::Resolution => label(Knob::Size, v, SizeUnit::Lines),
            DefaultSetting::ImageSize => label(Knob::Size, v, SizeUnit::Pixels),
            DefaultSetting::Fps => label(Knob::Fps, v, SizeUnit::Lines),
            DefaultSetting::Audio => label(Knob::AudioBitrate, v, SizeUnit::Lines),
            DefaultSetting::PageDpi => label(Knob::PageDpi, v, SizeUnit::Lines),
            DefaultSetting::Video | DefaultSetting::Images => {
                label(Knob::Preset, v, SizeUnit::Lines)
            }
        }
    }
}

/// One value a control can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value {
    Preset(Preset),
    Limit(Limit),
    /// `None` follows the preset.
    Kbps(Option<u32>),
    Dpi(u32),
}

const VIDEO_SIZES: &[u32] = &[2160, 1440, 1080, 720, 480, 360];
const VIDEO_FPS: &[u32] = &[60, 50, 30, 25, 24, 15];
const ANIMATION_SIZES: &[u32] = &[1280, 960, 720, 640, 480, 320];
const ANIMATION_FPS: &[u32] = &[30, 24, 20, 15, 12, 10];
const IMAGE_SIZES: &[u32] = &[4096, 2048, 1920, 1280, 1024, 512, 256];
const KBPS: &[u32] = &[64, 96, 128, 160, 192, 256, 320, 384, 448, 640];
const DPIS: &[u32] = &[72, 96, 150, 200, 300, 600];

/// Original (Auto first, when `auto`), then each cap from largest down.
fn limits(caps: &[u32], auto: bool) -> Vec<Value> {
    let mut v = vec![];
    if auto {
        v.push(Value::Limit(Limit::Auto));
    }
    v.push(Value::Limit(Limit::Original));
    v.extend(caps.iter().map(|c| Value::Limit(Limit::Max(*c))));
    v
}

fn kbps_values(cap: u32) -> Vec<Value> {
    let mut v = vec![Value::Kbps(None)];
    v.extend(
        KBPS.iter()
            .filter(|k| **k <= cap)
            .map(|k| Value::Kbps(Some(*k))),
    );
    v
}

/// What a size counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeUnit {
    /// Video lines: "1080p".
    Lines,
    /// The long side in pixels: "1280 px".
    Pixels,
}

/// How a value reads on its control: "High", "1080p", "30 fps".
pub fn label(knob: Knob, v: Value, unit: SizeUnit) -> String {
    match (knob, v) {
        (_, Value::Preset(p)) => p.label().to_owned(),
        (_, Value::Limit(Limit::Auto)) => "Auto".to_owned(),
        (_, Value::Limit(Limit::Original)) => "Original".to_owned(),
        (Knob::Fps, Value::Limit(Limit::Max(n))) => format!("{n} fps"),
        (_, Value::Limit(Limit::Max(n))) => match unit {
            SizeUnit::Lines => format!("{n}p"),
            SizeUnit::Pixels => format!("{n} px"),
        },
        (_, Value::Kbps(None)) => "Auto".to_owned(),
        (_, Value::Kbps(Some(k))) => format!("{k} kbps"),
        (_, Value::Dpi(d)) => format!("{d} dpi"),
    }
}

/// The value `delta` steps from `current` in `list`. A value not in the
/// list starts from the first entry.
pub fn stepped(list: &[Value], current: Value, delta: isize, wrap: bool) -> Value {
    if list.is_empty() {
        return current;
    }
    let n = list.len() as isize;
    let i = list.iter().position(|v| *v == current).unwrap_or(0) as isize;
    let j = if wrap {
        (i + delta).rem_euclid(n)
    } else {
        (i + delta).clamp(0, n - 1)
    };
    list[j as usize]
}

/// Whether converting `from` to `to` makes an animation from a video.
fn animation(from: &Format, to: &Format) -> bool {
    from.kind == Kind::Video && to.kind == Kind::Image
}

/// How sizes read for this pair.
pub fn size_unit(to: &Format) -> SizeUnit {
    if to.kind == Kind::Video {
        SizeUnit::Lines
    } else {
        SizeUnit::Pixels
    }
}

/// The values a knob offers for this pair. With `info` about the source,
/// caps the source already fits under are left out, since Kiln never
/// scales up.
pub fn values(knob: Knob, from: &Format, to: &Format, info: Option<&MediaInfo>) -> Vec<Value> {
    let side = info.and_then(|i| match (i.width, i.height) {
        (Some(w), Some(h)) if to.kind == Kind::Video => Some(w.min(h)),
        (Some(w), Some(h)) => Some(w.max(h)),
        _ => None,
    });
    let below = |v: &Value, top: Option<f64>| match (v, top) {
        (Value::Limit(Limit::Max(n)), Some(t)) => (*n as f64) < t - 0.5,
        _ => true,
    };
    match knob {
        Knob::Preset => Preset::ALL.into_iter().map(Value::Preset).collect(),
        Knob::Size => {
            let list = if to.kind == Kind::Video {
                limits(VIDEO_SIZES, false)
            } else if animation(from, to) {
                limits(ANIMATION_SIZES, q::auto_size(to).is_some())
            } else {
                limits(IMAGE_SIZES, false)
            };
            let top = side.map(f64::from);
            list.into_iter().filter(|v| below(v, top)).collect()
        }
        Knob::Fps => {
            let list = if to.kind == Kind::Video {
                limits(VIDEO_FPS, false)
            } else {
                limits(ANIMATION_FPS, q::auto_fps(to).is_some())
            };
            let top = info.and_then(|i| i.fps);
            list.into_iter().filter(|v| below(v, top)).collect()
        }
        Knob::AudioBitrate => {
            let cap = match to.ext.as_str() {
                "opus" => 256,
                "ac3" => 640,
                // Video containers carry AAC, MP3 or Vorbis at up to 320.
                _ => 320,
            };
            kbps_values(cap)
        }
        Knob::PageDpi => DPIS.iter().map(|d| Value::Dpi(*d)).collect(),
    }
}

/// A knob's value in `quality`.
pub fn get(quality: &Quality, knob: Knob) -> Value {
    match knob {
        Knob::Preset => Value::Preset(quality.preset),
        Knob::Size => Value::Limit(quality.size),
        Knob::Fps => Value::Limit(quality.fps),
        Knob::AudioBitrate => Value::Kbps(quality.audio_kbps),
        Knob::PageDpi => Value::Dpi(quality.page_dpi),
    }
}

pub fn set(quality: &mut Quality, knob: Knob, v: Value) {
    match (knob, v) {
        (Knob::Preset, Value::Preset(p)) => quality.preset = p,
        (Knob::Size, Value::Limit(l)) => quality.size = l,
        (Knob::Fps, Value::Limit(l)) => quality.fps = l,
        (Knob::AudioBitrate, Value::Kbps(k)) => quality.audio_kbps = k,
        (Knob::PageDpi, Value::Dpi(d)) => quality.page_dpi = d,
        _ => {}
    }
}

/// A knob's value as one of the values offered. A default the source
/// already fits under, such as 1080p for a 720p clip, shows as Original,
/// which is what it does.
pub fn shown(quality: &Quality, knob: Knob, list: &[Value]) -> Value {
    let v = get(quality, knob);
    if list.contains(&v) {
        return v;
    }
    match v {
        Value::Limit(Limit::Max(_)) if list.contains(&Value::Limit(Limit::Original)) => {
            Value::Limit(Limit::Original)
        }
        _ => list.first().copied().unwrap_or(v),
    }
}

/// The knob's name in the options sheet.
pub fn knob_name(knob: Knob, to: &Format) -> &'static str {
    match knob {
        Knob::Preset => "Quality",
        Knob::Size if to.kind == Kind::Video => "Resolution",
        Knob::Size => "Size",
        Knob::Fps => "Frame rate",
        Knob::AudioBitrate => "Audio",
        Knob::PageDpi => "Page detail",
    }
}

/// The picture size a conversion writes, when the source's is known.
pub fn result_size(
    from: &Format,
    to: &Format,
    quality: &Quality,
    info: &MediaInfo,
) -> Option<(u32, u32)> {
    let (w, h) = (info.width?, info.height?);
    if to.kind == Kind::Video {
        return Some(q::scaled(w, h, quality.size.resolve(None), true));
    }
    if to.kind != Kind::Image || !matches!(from.kind, Kind::Image | Kind::Video) {
        return None;
    }
    let auto = if animation(from, to) {
        q::auto_size(to)
    } else {
        None
    };
    Some(q::scaled(w, h, quality.size.resolve(auto), false))
}

/// The sheet's line under its title: what the source is. "clip.mov  ·
/// 1920 × 1080  ·  30 fps  ·  2:14", or "3 files  ·  214 MB".
pub fn source_line(names: &[String], bytes: u64, info: Option<&MediaInfo>) -> String {
    let mut parts = vec![];
    match names {
        [one] => parts.push(one.clone()),
        many => parts.push(format!("{} files", many.len())),
    }
    let info = info.filter(|_| names.len() == 1);
    if let Some((w, h)) = info.and_then(|i| Some((i.width?, i.height?))) {
        parts.push(format!("{w} × {h}"));
    }
    if let Some(f) = info
        .and_then(|i| i.fps)
        .filter(|_| info.is_some_and(|i| i.duration.is_some()))
    {
        parts.push(format!("{} fps", trim_rate(f)));
    }
    if let Some(d) = info.and_then(|i| i.duration) {
        parts.push(clock(d));
    }
    if info.is_none_or(|i| i.width.is_none() && i.duration.is_none()) {
        parts.push(human_size(bytes));
    }
    parts.join("  ·  ")
}

/// The sheet's footer: the result's size and a rough file size, where
/// they can be told. "1280 × 720  ·  About 48 MB".
pub fn result_line(from: &Format, to: &Format, quality: &Quality, infos: &[MediaInfo]) -> String {
    let mut parts = vec![];
    if let [one] = infos {
        if let Some((w, h)) = result_size(from, to, quality, one) {
            parts.push(format!("{w} × {h}"));
        }
    }
    let each: Vec<Option<u64>> = infos.iter().map(|i| q::estimate(to, quality, i)).collect();
    if !each.is_empty() && each.iter().all(Option::is_some) {
        let total: u64 = each.into_iter().flatten().sum();
        parts.push(format!("About {}", human_size(total.max(1000))));
    }
    parts.join("  ·  ")
}

/// "29.97" or "30".
fn trim_rate(f: f64) -> String {
    let s = format!("{f:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// "2:14" or "1:02:09".
fn clock(secs: f64) -> String {
    let s = secs.round() as u64;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Presets as "small", "high" or "best".
mod preset_text {
    use kiln_core::Preset;
    use serde::{de::Error, Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(p: &Preset, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(p.id())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Preset, D::Error> {
        let s = String::deserialize(d)?;
        Preset::parse(&s)
            .ok_or_else(|| D::Error::custom(format!("'{s}' isn't small, high or best")))
    }
}

/// Limits as "auto", "original" or a number.
mod limit_text {
    use kiln_core::Limit;
    use serde::{de::Error, Deserialize, Deserializer, Serializer};

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        Word(String),
        Number(u32),
    }

    pub fn serialize<S: Serializer>(l: &Limit, s: S) -> Result<S::Ok, S::Error> {
        match l {
            Limit::Auto => s.serialize_str("auto"),
            Limit::Original => s.serialize_str("original"),
            Limit::Max(n) => s.serialize_u32(*n),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Limit, D::Error> {
        match Repr::deserialize(d)? {
            Repr::Number(n) if n > 0 => Ok(Limit::Max(n)),
            Repr::Word(w) if w.eq_ignore_ascii_case("auto") => Ok(Limit::Auto),
            Repr::Word(w) if w.eq_ignore_ascii_case("original") => Ok(Limit::Original),
            Repr::Number(n) => Err(D::Error::custom(format!("{n} isn't a size"))),
            Repr::Word(w) => Err(D::Error::custom(format!(
                "'{w}' isn't auto, original or a number"
            ))),
        }
    }
}

/// A bitrate as a number, or "auto".
mod kbps_text {
    use serde::{de::Error, Deserialize, Deserializer, Serializer};

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        Word(String),
        Number(u32),
    }

    pub fn serialize<S: Serializer>(k: &Option<u32>, s: S) -> Result<S::Ok, S::Error> {
        match k {
            Some(k) => s.serialize_u32(*k),
            None => s.serialize_str("auto"),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u32>, D::Error> {
        match Repr::deserialize(d)? {
            Repr::Number(n) => Ok(Some(n)),
            Repr::Word(w) if w.eq_ignore_ascii_case("auto") => Ok(None),
            Repr::Word(w) => Err(D::Error::custom(format!("'{w}' isn't auto or a number"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiln_core::Registry;

    fn f(ext: &str) -> Format {
        Registry::builtin().format(ext).unwrap().clone()
    }

    fn info(w: u32, h: u32, fps: f64, secs: f64) -> MediaInfo {
        MediaInfo {
            duration: Some(secs),
            width: Some(w),
            height: Some(h),
            fps: Some(fps),
            has_audio: true,
        }
    }

    #[test]
    fn defaults_follow_the_kind_of_result() {
        let d = QualityDefaults {
            video: Preset::Best,
            resolution: Limit::Max(720),
            images: Preset::Small,
            image_size: Limit::Max(1024),
            audio_kbps: Some(256),
            page_dpi: 300,
            ..QualityDefaults::default()
        };
        let v = d.quality_for(&f("mov"), &f("mp4"));
        assert_eq!(
            (v.preset, v.size, v.audio_kbps),
            (Preset::Best, Limit::Max(720), Some(256))
        );
        let i = d.quality_for(&f("png"), &f("jpg"));
        assert_eq!((i.preset, i.size), (Preset::Small, Limit::Max(1024)));
        // Animations keep Kiln's own small size.
        let g = d.quality_for(&f("mp4"), &f("gif"));
        assert_eq!(
            (g.preset, g.size, g.fps),
            (Preset::Best, Limit::Auto, Limit::Auto)
        );
        let p = d.quality_for(&f("pdf"), &f("png"));
        assert_eq!(p.page_dpi, 300);
        assert_eq!(p.size, Limit::Auto);
        let a = d.quality_for(&f("wav"), &f("mp3"));
        assert_eq!((a.preset, a.audio_kbps), (Preset::High, Some(256)));
        assert_eq!(
            QualityDefaults::default()
                .quality_for(&f("png"), &f("jpg"))
                .preset,
            Preset::High
        );
    }

    #[test]
    fn defaults_round_trip_through_toml() {
        let d = QualityDefaults {
            video: Preset::Small,
            resolution: Limit::Max(1080),
            fps: Limit::Max(30),
            audio_kbps: Some(192),
            ..QualityDefaults::default()
        };
        let text = toml::to_string(&d).unwrap();
        assert!(text.contains("resolution = 1080"), "{text}");
        assert!(text.contains("video = \"small\""), "{text}");
        assert_eq!(toml::from_str::<QualityDefaults>(&text).unwrap(), d);
        let back: QualityDefaults =
            toml::from_str("fps = 'original'\naudio_kbps = 'auto'").unwrap();
        assert_eq!((back.fps, back.audio_kbps), (Limit::Original, None));
        assert!(toml::from_str::<QualityDefaults>("video = 'huge'").is_err());
        assert!(toml::from_str::<QualityDefaults>("resolution = 0").is_err());
        assert!(QualityDefaults::default().is_valid());
        let silly = QualityDefaults {
            page_dpi: 5000,
            ..QualityDefaults::default()
        };
        assert!(!silly.is_valid());
    }

    #[test]
    fn stepping_wraps_on_the_settings_page() {
        let mut d = QualityDefaults::default();
        d.step(DefaultSetting::Resolution, 1);
        assert_eq!(d.resolution, Limit::Max(2160));
        d.step(DefaultSetting::Resolution, -2);
        assert_eq!(d.resolution, Limit::Max(360));
        d.step(DefaultSetting::Audio, 1);
        assert_eq!(d.audio_kbps, Some(64));
        assert_eq!(
            DefaultSetting::Audio.label(d.get(DefaultSetting::Audio)),
            "64 kbps"
        );
        d.step(DefaultSetting::PageDpi, 1);
        assert_eq!(d.page_dpi, 200);
        assert_eq!(
            DefaultSetting::Resolution.label(Value::Limit(Limit::Max(720))),
            "720p"
        );
        assert_eq!(
            DefaultSetting::ImageSize.label(Value::Limit(Limit::Max(512))),
            "512 px"
        );
    }

    #[test]
    fn sizes_never_offer_to_scale_up() {
        let hd = info(1280, 720, 29.97, 60.0);
        let v = values(Knob::Size, &f("mov"), &f("mp4"), Some(&hd));
        let labels: Vec<String> = v
            .iter()
            .map(|v| label(Knob::Size, *v, SizeUnit::Lines))
            .collect();
        assert_eq!(labels, ["Original", "480p", "360p"]);
        let fps = values(Knob::Fps, &f("mov"), &f("mp4"), Some(&hd));
        let labels: Vec<String> = fps
            .iter()
            .map(|v| label(Knob::Fps, *v, SizeUnit::Lines))
            .collect();
        assert_eq!(labels, ["Original", "25 fps", "24 fps", "15 fps"]);
        // Without a probe everything is offered.
        assert_eq!(values(Knob::Size, &f("mov"), &f("mp4"), None).len(), 7);
    }

    #[test]
    fn animations_start_from_auto() {
        let v = values(Knob::Size, &f("mp4"), &f("gif"), None);
        assert_eq!(v[0], Value::Limit(Limit::Auto));
        let r = result_size(
            &f("mp4"),
            &f("gif"),
            &Quality::default(),
            &info(1920, 1080, 30.0, 5.0),
        );
        assert_eq!(r, Some((480, 270)));
        assert_eq!(size_unit(&f("gif")), SizeUnit::Pixels);
        // Images scale by their long side.
        let q = Quality {
            size: Limit::Max(1000),
            ..Quality::default()
        };
        let r = result_size(&f("png"), &f("jpg"), &q, &info(4000, 3000, 0.0, 0.0));
        assert_eq!(r, Some((1000, 750)));
    }

    #[test]
    fn a_default_the_source_fits_under_shows_as_original() {
        let hd = info(1280, 720, 30.0, 60.0);
        let list = values(Knob::Size, &f("mov"), &f("mp4"), Some(&hd));
        let q = Quality {
            size: Limit::Max(1080),
            ..Quality::default()
        };
        assert_eq!(shown(&q, Knob::Size, &list), Value::Limit(Limit::Original));
        assert_eq!(
            stepped(&list, Value::Limit(Limit::Original), 1, false),
            list[1]
        );
        assert_eq!(stepped(&list, list[2], 1, false), list[2]);
    }

    #[test]
    fn bitrates_stop_at_what_the_encoder_takes() {
        let opus = values(Knob::AudioBitrate, &f("wav"), &f("opus"), None);
        assert_eq!(opus.last(), Some(&Value::Kbps(Some(256))));
        let ac3 = values(Knob::AudioBitrate, &f("wav"), &f("ac3"), None);
        assert_eq!(ac3.last(), Some(&Value::Kbps(Some(640))));
        assert_eq!(opus[0], Value::Kbps(None));
    }

    #[test]
    fn lines_give_context() {
        let names = vec!["clip.mov".to_owned()];
        let i = info(1920, 1080, 29.97, 134.0);
        assert_eq!(
            source_line(&names, 5_000_000, Some(&i)),
            "clip.mov  ·  1920 × 1080  ·  29.97 fps  ·  2:14"
        );
        let two = vec!["a.mov".to_owned(), "b.mov".to_owned()];
        assert_eq!(source_line(&two, 214_000_000, None), "2 files  ·  214 MB");
        assert_eq!(source_line(&names, 1_500, None), "clip.mov  ·  1.5 KB");
        let still = MediaInfo {
            width: Some(800),
            height: Some(600),
            ..MediaInfo::default()
        };
        assert_eq!(
            source_line(&["a.png".to_owned()], 9, Some(&still)),
            "a.png  ·  800 × 600"
        );

        let q = Quality {
            size: Limit::Max(720),
            ..Quality::default()
        };
        let line = result_line(&f("mov"), &f("mp4"), &q, &[i]);
        assert!(line.starts_with("1280 × 720  ·  About "), "{line}");
        assert!(line.ends_with(" MB"), "{line}");
        // No estimate for a document; nothing at all to say.
        assert_eq!(result_line(&f("docx"), &f("pdf"), &q, &[]), "");
        assert_eq!(clock(3729.0), "1:02:09");
    }
}
