//! The quality slider's model, for sound and video Kiln writes itself: its
//! steps, the Small, High and Best marks on it, the Advanced fields under
//! it, and how its value reads on screen and to Narrator. Platform-free;
//! the window draws it and moves it.
//!
//! [`TuneSheet`] is the sound's slider and [`VideoSheet`] the video's;
//! [`SheetKind`] is whichever one an options sheet has.

use kiln_core::quality::{
    codec_of, fit_under_offered, source_info, video_info, video_tune, MediaInfo,
};
use kiln_core::{Knob, Limit, Quality};
use kiln_media::tune::{self, ChannelChoice, Preset, Setting, Step, Target, Tune};
use kiln_media::video::tune as vtune;
use kiln_media::video::{
    EncoderChoice, SoundChoice, VCodec, VStep, VideoCodec, VideoInfo, VideoSetting, VideoTarget,
    VideoTune,
};
use kiln_media::{Channels, MediaError, SourceInfo};

use crate::model::human_size;

/// The video's last Quality radio: the picture kept as it is where it can
/// be, not encoded again.
pub const ORIGINAL_RADIO: &str = "Original (no re-encode)";

/// A field under Advanced. Which ones a format has, and in what order, is
/// [`TuneSheet::fields`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TuneField {
    /// Vorbis quality, q −1 to 10.
    Quality,
    /// A bitrate: any whole kbps for Opus, the listed ones for AAC, WMA
    /// and AC3.
    Kbps,
    /// Opus: tuned for music or for speech.
    Speech,
    /// FLAC's compression level, WavPack's mode.
    Level,
    /// The bit depth of lossless sound.
    Bits,
    /// The sample rate: kept, or lower.
    Rate,
    /// The channels: kept, or folded to stereo or mono.
    Channels,
    /// The largest file wanted. Only with a known length, and only for
    /// formats with a bitrate to trade (see the tune sheet).
    FitUnder,
    /// Video: the most lines the short side keeps, or Original. Only the
    /// steps below the source's: Kiln never enlarges.
    Resolution,
    /// Video: the most frames a second, or Original; only those below the
    /// source's.
    FrameRate,
    /// Video: H.264 or HEVC, for a picture that is encoded.
    Codec,
    /// Video: the sound kept (copied where the target holds it), or
    /// encoded at one of the target's bitrates.
    Sound,
    /// Every step of the quality, for the people who want one between
    /// Small, High and Best: a drop-down named Quality. The steps' own
    /// names ("q5", "V2") show here and nowhere else.
    Steps,
}

impl TuneField {
    /// The field's label, which is also its name for Narrator.
    pub fn name(self) -> &'static str {
        match self {
            TuneField::Kbps => "Bitrate",
            TuneField::Quality => "Quality level",
            TuneField::Steps => "Quality",
            TuneField::Speech => "Tuned for",
            TuneField::Level => "Compression",
            TuneField::Bits => "Bit depth",
            TuneField::Rate => "Sample rate",
            TuneField::Channels => "Channels",
            TuneField::FitUnder => "Max size",
            TuneField::Resolution => "Resolution",
            TuneField::FrameRate => "Frame rate",
            TuneField::Codec => "Codec",
            TuneField::Sound => "Sound",
        }
    }
}

/// The rates Advanced offers below the source's, highest first.
const RATES: [u32; 4] = [48_000, 44_100, 32_000, 22_050];
/// Windows' AAC encoder takes only these.
const AAC_RATES: [u32; 2] = [48_000, 44_100];
/// What Fit under steps through, in bytes (decimal MB, as the app counts).
const FIT_SIZES: [u64; 15] = [
    500_000,
    1_000_000,
    2_000_000,
    3_000_000,
    4_000_000,
    5_000_000,
    8_000_000,
    10_000_000,
    15_000_000,
    20_000_000,
    25_000_000,
    50_000_000,
    100_000_000,
    200_000_000,
    500_000_000,
];
/// Opus takes any whole kbps in this range.
const OPUS_MIN: u32 = 6;
const OPUS_MAX: u32 = 510;

/// The sound slider for one conversion and the Advanced fields under it.
#[derive(Debug, Clone, PartialEq)]
pub struct TuneSheet {
    pub target: Target,
    /// The slider's stops, smallest file first. Empty for WAV and AIFF,
    /// which have no slider.
    pub steps: Vec<Step>,
    /// Where the thumb is, an index into `steps`.
    pub at: usize,
    /// The Small, High and Best marks, as indexes into `steps`. Two can be
    /// the same step: FLAC's High and Best are both level 5.
    pub marks: [usize; 3],
    pub advanced_open: bool,
    /// The fields under Advanced, in Tab order.
    pub fields: Vec<TuneField>,
    preset: Preset,
    /// MP3 at a constant bitrate, which isn't on the slider.
    cbr: Option<u32>,
    /// Opus at a bitrate between the slider's stops.
    opus_kbps: Option<u32>,
    speech: bool,
    bits: Option<u16>,
    rate: Option<u32>,
    channels: ChannelChoice,
    fit_bytes: Option<u64>,
    /// Fit under asks for less than the format can reach (or can't be
    /// worked out): the engine isn't given it, and Convert waits.
    fit_failed: bool,
    /// Whether Fit under can be offered for this file and format.
    fit_offered: bool,
    /// The channels Keep writes (the source's, within what the format
    /// takes: MP3 and WMA fold 5.1 to stereo) and the source's rate, when
    /// known, so Advanced offers only what changes something.
    kept_channels: Option<u16>,
    source_rate: Option<u32>,
}

impl TuneSheet {
    /// The slider for writing `info`'s sound as `target`. It opens on
    /// `quality.audio`'s setting, else the step nearest the bitrate saved
    /// in Settings, else the preset's mark (High unless asked).
    pub fn new(target: Target, info: &MediaInfo, quality: &Quality) -> TuneSheet {
        let audio = quality.audio;
        let mut s = TuneSheet {
            target,
            steps: vec![],
            at: 0,
            marks: [0; 3],
            advanced_open: false,
            fields: vec![],
            preset: media_preset(quality.preset),
            cbr: None,
            opus_kbps: None,
            speech: false,
            bits: audio.bits,
            rate: audio.rate,
            channels: audio.channels,
            fit_bytes: None,
            fit_failed: false,
            fit_offered: false,
            kept_channels: None,
            source_rate: None,
        };
        s.restep(info);
        match audio.setting {
            Some(setting) => s.take(setting),
            None => {
                let kbps = audio.kbps.or(quality.audio_kbps);
                if let Some(k) = kbps.filter(|_| !target.lossless()) {
                    s.at = s.nearest(k);
                }
            }
        }
        if s.fit_offered {
            if let Some(bytes) = audio.fit_bytes {
                s.fit_bytes = Some(bytes);
                s.follow_fit(info);
            }
        }
        s.refields();
        s
    }

    /// Move one step or more; the ends hold.
    pub fn step(&mut self, delta: isize) {
        if self.steps.is_empty() {
            return;
        }
        let last = self.steps.len() as isize - 1;
        let to = (self.at as isize + delta).clamp(0, last) as usize;
        self.move_to(to);
    }

    /// Move to the next mark (`delta` > 0) or the previous one, as many
    /// times as `delta` says. Past the last mark, the end.
    pub fn page(&mut self, delta: isize) {
        if self.steps.is_empty() {
            return;
        }
        self.move_to(paged(self.at, &self.marks, self.steps.len(), delta));
    }

    /// The first step. Already there, nothing changes.
    pub fn home(&mut self) {
        self.set_at(0);
    }

    /// The last step. Already there, nothing changes.
    pub fn end(&mut self) {
        self.set_at(self.steps.len().saturating_sub(1));
    }

    /// Snap to mark `i`: 0 Small, 1 High, 2 Best.
    pub fn set_mark(&mut self, i: usize) {
        if let Some(&m) = self.marks.get(i) {
            self.set_at(m);
        }
    }

    /// Put the thumb on step `i`, as a click on the track does. The step
    /// it is already on changes nothing: MP3 at 320 kbps sits on V0, and
    /// End or Best there keeps constant, as a click on the mark the thumb
    /// is parked on keeps Fit under.
    pub fn set_at(&mut self, i: usize) {
        if self.steps.is_empty() {
            return;
        }
        self.move_to(i.min(self.steps.len() - 1));
    }

    /// How far Page Up goes from here, in steps: to the next mark, or to
    /// the end past the last one; at the end, back to the mark before.
    /// Never 0 while there are steps to move, so UI Automation's large
    /// change always moves the thumb. 0 only for WAV and AIFF.
    pub fn large_change(&self) -> usize {
        large_change(self.at, &self.marks, self.steps.len())
    }

    /// [`TuneSheet::new`], with `quality.audio` set to what the sheet
    /// carries, so a Convert pressed before the file is read carries only
    /// what the sheet offers: no saved Mono for WAV, no rate for Opus.
    pub fn opened(target: Target, info: &MediaInfo, quality: &mut Quality) -> TuneSheet {
        let s = TuneSheet::new(target, info, quality);
        quality.audio = s.to_tune();
        s
    }

    /// The sheet worked out again for `info`, once the file is read: from
    /// `opened_with`, the quality the sheet opened with, while nothing on
    /// it was `touched`; else from what it says now, so a move made before
    /// the file was read stays. Advanced stays open or closed.
    pub fn reread(&self, info: &MediaInfo, opened_with: &Quality, touched: bool) -> TuneSheet {
        let from = if touched {
            Quality {
                audio: self.to_tune(),
                ..*opened_with
            }
        } else {
            *opened_with
        };
        let mut s = TuneSheet::new(self.target, info, &from);
        s.advanced_open = self.advanced_open;
        s
    }

    pub fn toggle_advanced(&mut self) {
        self.advanced_open = !self.advanced_open;
    }

    /// What the slider says beside it: the setting, then the result's
    /// size where it can be told. "V2 · about 173 kbps · about 4.8 MB".
    pub fn value_text(&self, info: &MediaInfo) -> String {
        let w = self.written(info);
        let (s, _) = self.chosen(info);
        let mut text = tune::label_of(self.target, &s, &w);
        if let Some(bytes) = tune::estimate_bytes(self.target, &s, &w) {
            text.push_str(&format!(" · about {}", human_size(bytes)));
        }
        text
    }

    /// The value text as Narrator reads it, units in whole words: "V2,
    /// about 173 kilobits a second, about 4.8 megabytes".
    pub fn a11y_value(&self, info: &MediaInfo) -> String {
        spoken(&self.value_text(info))
    }

    /// The card's line: the result's size alone, "About 4.8 MB". `None`
    /// when no size can be told.
    pub fn readout(&self, info: &MediaInfo) -> Option<String> {
        let w = self.written(info);
        let (s, _) = self.chosen(info);
        tune::estimate_bytes(self.target, &s, &w).map(|b| format!("About {}", human_size(b)))
    }

    /// The sheet's line when Fit under asks for less than the format can
    /// reach: "MP3 can't go under 1.0 MB for this file." `None` otherwise.
    /// While it shows, the value text gives the slider's own setting,
    /// [`TuneSheet::can_convert`] is false and [`TuneSheet::to_tune`]
    /// leaves Fit under out.
    pub fn fit_error(&self, info: &MediaInfo) -> Option<String> {
        match self.chosen(info).1 {
            Some(e @ MediaError::TooSmall { .. }) => Some(e.to_string()),
            _ => None,
        }
    }

    /// Whether Convert can be pressed: not while Fit under asks for a size
    /// the format can't reach.
    pub fn can_convert(&self) -> bool {
        !(self.fit_bytes.is_some() && self.fit_failed)
    }

    /// The tuning a conversion gets. With Fit under the engine works the
    /// setting out from the exact length, so no setting is given. A Fit
    /// under that can't be met is never handed on: the slider's own
    /// setting goes instead (and Convert waits, [`TuneSheet::can_convert`]).
    pub fn to_tune(&self) -> Tune {
        let mut tune = self.asked();
        if tune.fit_bytes.is_some() && self.fit_failed {
            tune.fit_bytes = None;
            tune.setting = self.own_setting();
        }
        tune
    }

    /// Everything the sheet asks for, Fit under included even when it
    /// can't be met.
    fn asked(&self) -> Tune {
        let fit = self.fit_bytes.filter(|_| self.fit_offered);
        Tune {
            setting: if fit.is_some() {
                None
            } else {
                self.own_setting()
            },
            fit_bytes: fit,
            rate: self.rate,
            channels: self.channels,
            bits: self.bits,
            kbps: None,
        }
    }

    /// A field's value as its stepper shows it: "Variable", "192 kbps",
    /// "44.1 kHz", "Off".
    pub fn field_value(&self, f: TuneField) -> String {
        let step_part = || {
            self.steps
                .get(self.at)
                .and_then(|s| s.label.split(" · ").next())
                .unwrap_or_default()
                .to_owned()
        };
        match f {
            TuneField::Kbps => self
                .own_setting()
                .and_then(|s| kbps_of(&s))
                .map_or_else(String::new, |k| format!("{k} kbps")),
            TuneField::Quality | TuneField::Level | TuneField::Steps => step_part(),
            TuneField::Speech if self.speech => "Speech".to_owned(),
            TuneField::Speech => "Music".to_owned(),
            TuneField::Bits => match self.bits {
                None => "Keep".to_owned(),
                Some(32) => "32-bit float".to_owned(),
                Some(b) => format!("{b}-bit"),
            },
            TuneField::Rate => self
                .rate
                .map_or_else(|| "Keep".to_owned(), |r| format!("{} kHz", khz(r))),
            TuneField::Channels => match self.channels {
                ChannelChoice::Keep => "Keep",
                ChannelChoice::Stereo => "Stereo",
                ChannelChoice::Mono => "Mono",
            }
            .to_owned(),
            TuneField::FitUnder => self.fit_bytes.map_or_else(|| "Off".to_owned(), human_size),
            // The video's own fields aren't the sound's.
            TuneField::Resolution | TuneField::FrameRate | TuneField::Codec | TuneField::Sound => {
                String::new()
            }
        }
    }

    /// Step a field along its values, wrapping at the ends, as the
    /// sheet's steppers do. A field that names a setting moves the slider
    /// to the matching step, or the nearest where there is none.
    pub fn step_field(&mut self, f: TuneField, delta: isize, info: &MediaInfo) {
        match f {
            TuneField::Kbps if self.target == Target::Opus => {
                let now = self.own_setting().and_then(|s| kbps_of(&s)).unwrap_or(96);
                let span = (OPUS_MAX - OPUS_MIN + 1) as usize;
                let k =
                    OPUS_MIN + wrap((now.max(OPUS_MIN) - OPUS_MIN) as usize, delta, span) as u32;
                self.set_kbps(k);
            }
            TuneField::Quality | TuneField::Level | TuneField::Kbps | TuneField::Steps => {
                self.at = wrap(self.at, delta, self.steps.len());
                self.moved();
            }
            TuneField::Speech => self.speech = !self.speech,
            TuneField::Bits => {
                let list = self.bit_choices();
                let i = list.iter().position(|b| *b == self.bits).unwrap_or(0);
                self.bits = list[wrap(i, delta, list.len())];
            }
            TuneField::Rate => {
                let list = self.rates();
                let i = list.iter().position(|r| *r == self.rate).unwrap_or(0);
                self.rate = list[wrap(i, delta, list.len())];
                self.restep(info);
            }
            TuneField::Channels => {
                let list = self.channel_choices();
                let i = list.iter().position(|c| *c == self.channels).unwrap_or(0);
                self.channels = list[wrap(i, delta, list.len())];
                self.restep(info);
            }
            TuneField::FitUnder => {
                let now = || {
                    self.own_setting().and_then(|s| {
                        let w = self.written(info);
                        tune::estimate_bytes(self.target, &s, &w)
                    })
                };
                let next = next_fit(&FIT_SIZES, self.fit_bytes, now, delta);
                return self.set_fit_under(next, info);
            }
            TuneField::Resolution | TuneField::FrameRate | TuneField::Codec | TuneField::Sound => {}
        }
        if self.fit_bytes.is_some() {
            self.follow_fit(info);
        }
        self.refields();
    }

    /// How many values field `f` has to choose from, and which it has
    /// now, when it is a short list a drop-down shows. Opus's
    /// bitrate and Vorbis's quality level, which run through many values,
    /// have none: they keep their spin box.
    fn field_list(&self, f: TuneField) -> Option<(usize, usize)> {
        let at = |i: Option<usize>| i.unwrap_or(0);
        match f {
            TuneField::Kbps if self.target == Target::Opus => None,
            TuneField::Quality => None,
            TuneField::Kbps | TuneField::Level | TuneField::Steps => {
                Some((self.steps.len(), self.at))
            }
            TuneField::Speech => Some((2, usize::from(self.speech))),
            TuneField::Bits => {
                let list = self.bit_choices();
                Some((list.len(), at(list.iter().position(|b| *b == self.bits))))
            }
            TuneField::Rate => {
                let list = self.rates();
                Some((list.len(), at(list.iter().position(|r| *r == self.rate))))
            }
            TuneField::Channels => {
                let list = self.channel_choices();
                Some((
                    list.len(),
                    at(list.iter().position(|c| *c == self.channels)),
                ))
            }
            TuneField::FitUnder => {
                Some((FIT_SIZES.len() + 1, fit_index(&FIT_SIZES, self.fit_bytes)))
            }
            TuneField::Resolution | TuneField::FrameRate | TuneField::Codec | TuneField::Sound => {
                None
            }
        }
    }

    /// Choose value `i` of field `f`'s list ([`Self::field_list`]), as
    /// its drop-down does.
    fn pick_field(&mut self, f: TuneField, i: usize, info: &MediaInfo) {
        let Some((len, now)) = self.field_list(f) else {
            return;
        };
        if i >= len || i == now {
            return;
        }
        match f {
            TuneField::FitUnder => self.set_fit_under(i.checked_sub(1).map(|k| FIT_SIZES[k]), info),
            // It turns between its two, whatever the step.
            TuneField::Speech => self.step_field(f, 1, info),
            _ => self.step_field(f, i as isize - now as isize, info),
        }
    }

    /// Fit under, in bytes, or `None` for off. The slider follows the
    /// setting it picks.
    pub fn set_fit_under(&mut self, bytes: Option<u64>, info: &MediaInfo) {
        self.fit_bytes = bytes.filter(|&b| b > 0 && self.fit_offered);
        if self.fit_bytes.is_some() {
            self.cbr = None;
            self.opus_kbps = None;
            self.follow_fit(info);
        }
        self.refields();
    }

    /// A bitrate typed into the Bitrate field: any whole kbps Opus takes;
    /// for the other formats the nearest step.
    pub fn set_kbps(&mut self, kbps: u32) {
        let exact = self.steps.iter().position(|s| s.kbps == Some(kbps));
        self.at = exact.unwrap_or_else(|| self.nearest(kbps));
        self.cbr = None;
        self.fit_bytes = None;
        self.opus_kbps = match self.target {
            Target::Opus if exact.is_none() => Some(kbps.clamp(OPUS_MIN, OPUS_MAX)),
            _ => None,
        };
        self.refields();
    }

    /// Put the thumb on `to`. Only a real move gives up a constant bitrate,
    /// Opus's own bitrate and Fit under; staying put (a step of 0, or
    /// against an end) keeps them.
    fn move_to(&mut self, to: usize) {
        if to != self.at {
            self.at = to;
            self.moved();
        }
    }

    /// The thumb was moved: the slider's step is the setting again.
    fn moved(&mut self) {
        self.cbr = None;
        self.opus_kbps = None;
        self.fit_bytes = None;
        self.refields();
    }

    /// The setting the slider and the format's own fields say, before
    /// Fit under. `None` for WAV and AIFF, whose depth is [`Tune::bits`].
    fn own_setting(&self) -> Option<Setting> {
        if let Some(kbps) = self.cbr {
            return Some(Setting::LameCbr { kbps });
        }
        let depth = self.bits.map(|b| b.min(24));
        Some(match self.steps.get(self.at)?.setting {
            Setting::Opus { kbps, .. } => Setting::Opus {
                kbps: self.opus_kbps.unwrap_or(kbps),
                speech: self.speech,
            },
            Setting::FlacLevel { level, .. } => Setting::FlacLevel { level, bits: depth },
            Setting::WavPack { mode, .. } => Setting::WavPack { mode, bits: depth },
            s => s,
        })
    }

    /// The setting a conversion would write with, chosen as the engine
    /// chooses it, and Fit under's error if it can't be met (the slider's
    /// own setting then).
    fn chosen(&self, info: &MediaInfo) -> (Setting, Option<MediaError>) {
        let w = self.written(info);
        let tune = self.asked();
        match tune::chosen_setting(self.target, self.preset, &tune, &w) {
            Ok(s) => (s, None),
            Err(e) => {
                let plain = Tune {
                    setting: self.own_setting(),
                    fit_bytes: None,
                    ..tune
                };
                let s = tune::chosen_setting(self.target, self.preset, &plain, &w).unwrap_or(
                    Setting::Pcm {
                        bits: 16,
                        float: false,
                    },
                );
                (s, Some(e))
            }
        }
    }

    /// The thumb goes where Fit under's setting is, or nearest to it.
    fn follow_fit(&mut self, info: &MediaInfo) {
        let (s, err) = self.chosen(info);
        self.fit_failed = err.is_some();
        if err.is_none() {
            match self.steps.iter().position(|x| x.setting == s) {
                Some(i) => self.at = i,
                None => {
                    if let Some(k) = kbps_of(&s) {
                        self.at = self.nearest(k);
                    }
                }
            }
        }
    }

    /// Open on `setting`: its step, or the nearest, with what it says
    /// that isn't on the slider kept for its field.
    /// Only what belongs to this format's family is taken: a FLAC depth
    /// says nothing about MP3, nor Opus's speech about AAC.
    fn take(&mut self, setting: Setting) {
        let t = self.target;
        match setting {
            Setting::FlacLevel { bits, .. } | Setting::WavPack { bits, .. }
                if matches!(t, Target::Flac | Target::WavPack) =>
            {
                self.bits = bits.or(self.bits);
            }
            Setting::Opus { speech, .. } if t == Target::Opus => self.speech = speech,
            Setting::Pcm { bits, float } if matches!(t, Target::Wav | Target::Aiff) => {
                self.bits = Some(if float { 32 } else { bits });
            }
            _ => {}
        }
        self.keep_offered();
        let same = |a: &Setting| match (a, &setting) {
            (Setting::FlacLevel { level: x, .. }, Setting::FlacLevel { level: y, .. }) => x == y,
            (Setting::WavPack { mode: x, .. }, Setting::WavPack { mode: y, .. }) => x == y,
            (Setting::Opus { kbps: x, .. }, Setting::Opus { kbps: y, .. }) => x == y,
            _ => *a == setting,
        };
        if let Some(i) = self.steps.iter().position(|s| same(&s.setting)) {
            self.at = i;
            return;
        }
        match setting {
            Setting::LameCbr { kbps } if t == Target::Mp3 => self.cbr = Some(kbps),
            Setting::Opus { kbps, .. } if t == Target::Opus => {
                self.opus_kbps = Some(kbps.clamp(OPUS_MIN, OPUS_MAX));
            }
            _ => {}
        }
        if let Some(k) = kbps_of(&setting) {
            self.at = self.nearest(k);
        }
    }

    /// Work the steps and marks out again for the sound as written (after
    /// the rate and channels), keeping the thumb on its mark, else on its
    /// setting, else on the nearest bitrate.
    fn restep(&mut self, info: &MediaInfo) {
        self.kept_channels = info.channels.map(|_| {
            kiln_media::written(self.target, &Tune::default(), &source(info))
                .channels
                .count
        });
        self.source_rate = info.rate;
        self.keep_offered();
        let w = self.written(info);
        let was = self.steps.get(self.at).cloned();
        let on_mark = [1, 0, 2].into_iter().find(|&k| self.marks[k] == self.at);
        self.steps = tune::steps(self.target, &w);
        self.marks = [Preset::Small, Preset::High, Preset::Best]
            .map(|p| tune::preset_step(self.target, p, &w));
        self.at = match (was, on_mark) {
            (None, _) => self.marks[preset_index(self.preset)],
            (Some(_), Some(k)) => self.marks[k],
            (Some(step), None) => match self.steps.iter().position(|s| s.setting == step.setting) {
                Some(i) => i,
                None => step.kbps.map_or(self.at, |k| self.nearest(k)),
            },
        };
        self.at = self.at.min(self.steps.len().saturating_sub(1));
        self.fit_offered = fit_under_offered(info) && !self.target.lossless();
        if !self.fit_offered {
            self.fit_bytes = None;
        }
    }

    /// Drop a rate, channels or depth Advanced doesn't offer for this
    /// source and format, as a saved tune may hold: 32 kHz from a 22.05
    /// kHz source, any rate for Opus or AC3, a depth for a lossy format.
    fn keep_offered(&mut self) {
        if !self.rates().contains(&self.rate) {
            self.rate = None;
        }
        if !self.channel_choices().contains(&self.channels) {
            self.channels = ChannelChoice::Keep;
        }
        if !self.bit_choices().contains(&self.bits) {
            self.bits = None;
        }
    }

    /// The fields Advanced shows, leaving out any with nothing to choose.
    fn refields(&mut self) {
        use TuneField::*;
        let mut f = match self.target {
            Target::Wav | Target::Aiff => {
                self.fields = vec![Bits];
                return;
            }
            // Its bitrate mode is cut (spec 2.5): the slider's VBR steps,
            // or a constant bitrate a saved default holds.
            // The quality, bitrate and level fields these formats had were
            // the steps themselves: the Quality drop-down lists them.
            Target::Mp3 | Target::Ogg => vec![],
            Target::Aac | Target::M4a | Target::Wma | Target::Ac3 => vec![],
            // Opus takes any whole bitrate, which a step doesn't.
            Target::Opus => vec![Kbps, Speech],
            Target::Flac | Target::WavPack => vec![Bits],
        };
        // Every step, for a quality between Small, High and Best.
        if !self.steps.is_empty() {
            f.insert(0, Steps);
        }
        if self.rates().len() > 1 {
            f.push(Rate);
        }
        if self.channel_choices().len() > 1 {
            f.push(Channels);
        }
        if self.fit_offered {
            f.push(FitUnder);
        }
        self.fields = f;
    }

    /// Keep, then the rates below the source's the format takes. Opus is
    /// always 48 kHz and AC3 is written at 48 kHz, so they have none.
    fn rates(&self) -> Vec<Option<u32>> {
        let list: &[u32] = match self.target {
            Target::Opus | Target::Ac3 | Target::Wav | Target::Aiff => &[],
            Target::Aac | Target::M4a => &AAC_RATES,
            _ => &RATES,
        };
        let below = |r: &u32| self.source_rate.is_none_or(|s| *r < s);
        std::iter::once(None)
            .chain(list.iter().filter(|r| below(r)).copied().map(Some))
            .collect()
    }

    /// Keep, then the folds that change something on what Keep writes: from
    /// 5.1 to MP3, Stereo is what Keep gives, so only Mono is offered. WAV
    /// and AIFF have no Channels field, so they keep the source's: a saved
    /// fold would be written with nothing on screen saying so.
    fn channel_choices(&self) -> Vec<ChannelChoice> {
        use ChannelChoice::*;
        if matches!(self.target, Target::Wav | Target::Aiff) {
            return vec![Keep];
        }
        match self.kept_channels {
            Some(0 | 1) => vec![Keep],
            Some(2) => vec![Keep, Mono],
            _ => vec![Keep, Stereo, Mono],
        }
    }

    fn bit_choices(&self) -> Vec<Option<u16>> {
        match self.target {
            Target::Flac | Target::WavPack => vec![None, Some(16), Some(24)],
            Target::Wav | Target::Aiff => vec![None, Some(16), Some(24), Some(32)],
            _ => vec![None],
        }
    }

    /// The sound as it will be written, as the engine works it out: after
    /// the sheet's rate and channels and the target's own (MP3 folds 5.1 to
    /// stereo, Opus is 48 kHz).
    fn written(&self, info: &MediaInfo) -> SourceInfo {
        kiln_media::written(self.target, &self.asked(), &source(info))
    }

    /// The step whose bitrate is nearest `kbps`; the lower wins a tie.
    fn nearest(&self, kbps: u32) -> usize {
        self.steps
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.kbps.map(|k| (i, k.abs_diff(kbps))))
            .min_by_key(|&(_, d)| d)
            .map_or(self.at, |(i, _)| i)
    }
}

/// The resolutions Advanced offers below the source's short side, largest
/// first.
const VIDEO_LINES: [u32; 6] = [2160, 1440, 1080, 720, 480, 360];
/// The frame rates Advanced offers below the source's, highest first.
const VIDEO_FPS: [u32; 3] = [60, 30, 24];
/// What Fit under steps through for video, in bytes (decimal, as the app
/// counts).
const VIDEO_FIT_SIZES: [u64; 17] = [
    1_000_000,
    2_000_000,
    3_000_000,
    4_000_000,
    5_000_000,
    8_000_000,
    10_000_000,
    15_000_000,
    20_000_000,
    25_000_000,
    50_000_000,
    100_000_000,
    200_000_000,
    500_000_000,
    1_000_000_000,
    2_000_000_000,
    4_000_000_000,
];
/// Opus bitrates the video's Sound field offers: music's, without
/// the speech rates below 64.
const VIDEO_OPUS_KBPS: [u32; 6] = [64, 96, 128, 160, 192, 256];
/// The least AAC bitrate the video's Sound field offers.
const VIDEO_AAC_FROM: u32 = 96;

/// kiln-media's video target for an extension: the four containers Kiln's
/// own engine writes.
pub fn video_target(ext: &str) -> Option<VideoTarget> {
    match ext {
        "mp4" => Some(VideoTarget::Mp4),
        "m4v" => Some(VideoTarget::M4v),
        "mov" => Some(VideoTarget::Mov),
        "mkv" => Some(VideoTarget::Mkv),
        _ => None,
    }
}

/// The video slider for one conversion and the Advanced fields under it:
/// quality 20 to 90 with Small, High and Best on 45, 65 and 80, and
/// Original last, which copies the picture where it can.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoSheet {
    pub target: VideoTarget,
    /// The slider's stops, smallest file first, Original last.
    pub steps: Vec<VStep>,
    /// Where the thumb is, an index into `steps`.
    pub at: usize,
    /// The Small, High and Best marks, as indexes into `steps`.
    pub marks: [usize; 3],
    pub advanced_open: bool,
    /// The fields under Advanced, in Tab order.
    pub fields: Vec<TuneField>,
    preset: Preset,
    /// The step the slider was moved to. `None` while it sits where the
    /// preset puts it, which the engine works out for each file:
    /// for High, Original where the picture can be copied, else the High
    /// step. So several files each get their own, and a limit chosen under
    /// Advanced takes the thumb off Original when the copy can't be.
    own: Option<VideoSetting>,
    max_lines: Option<u32>,
    max_fps: Option<u32>,
    codec: VCodec,
    encoder: EncoderChoice,
    sound: SoundChoice,
    fit_bytes: Option<u64>,
    /// Fit under asks for less than the target can reach: the engine
    /// isn't given it, and Convert waits.
    fit_failed: bool,
    fit_offered: bool,
    /// The picture's short side and frame rate, when the file is read, so
    /// Advanced offers only lower ones.
    short_side: Option<u32>,
    source_fps: Option<f64>,
    /// The sound bitrates the target's encoder takes (AAC, or Opus for
    /// MKV), Keep aside. Empty for a silent file.
    sound_kbps: Vec<u32>,
    /// Whether this PC has a graphics card encoder for H.264 and for HEVC,
    /// once looked for off the window's thread (the first look starts
    /// Media Foundation). `None` until then: nothing is said.
    cards: Option<[bool; 2]>,
}

impl VideoSheet {
    /// The slider for writing `info`'s video as `target`. It opens on
    /// `quality.video`'s setting, else where the preset puts the file
    /// (`default_setting` for High: Original where the picture can be
    /// copied, else High), with Settings' resolution and frame rate where
    /// the source is above them.
    pub fn new(target: VideoTarget, info: &MediaInfo, quality: &Quality) -> VideoSheet {
        let asked = video_tune(quality);
        let mut s = VideoSheet {
            target,
            steps: vec![],
            at: 0,
            marks: [0; 3],
            advanced_open: false,
            fields: vec![],
            preset: media_preset(quality.preset),
            own: None,
            max_lines: asked.max_lines,
            max_fps: asked.max_fps,
            codec: asked.codec,
            encoder: asked.encoder,
            sound: asked.sound,
            fit_bytes: None,
            fit_failed: false,
            fit_offered: false,
            short_side: None,
            source_fps: None,
            sound_kbps: vec![],
            cards: None,
        };
        s.restep(info);
        // Only a step on the slider: a bitrate is Fit under's, not the
        // slider's own.
        s.own = asked
            .setting
            .filter(|a| s.steps.iter().any(|x| x.setting == *a));
        s.place(info);
        if s.fit_offered {
            if let Some(bytes) = asked.fit_bytes {
                s.fit_bytes = Some(bytes);
                s.follow_fit(info);
            }
        }
        s.refields();
        s
    }

    /// [`VideoSheet::new`], with `quality` set to what the sheet carries
    /// ([`VideoSheet::carry`]), so a Convert pressed before the file is
    /// read carries only what the sheet offers.
    pub fn opened(target: VideoTarget, info: &MediaInfo, quality: &mut Quality) -> VideoSheet {
        let s = VideoSheet::new(target, info, quality);
        s.carry(quality);
        s
    }

    /// Put what the sheet says into `quality`: the video tuning, and the
    /// resolution and frame rate limits as the sheet's own, so a saved
    /// default it doesn't show is never applied behind it.
    pub fn carry(&self, quality: &mut Quality) {
        let t = self.to_tune();
        quality.video = t;
        quality.size = t.max_lines.map_or(Limit::Original, Limit::Max);
        quality.fps = t.max_fps.map_or(Limit::Original, Limit::Max);
    }

    /// The sheet worked out again for `info`, once the file is read: from
    /// `opened_with` while nothing on it was `touched`, else from what it
    /// says now. Advanced stays open or closed, and what was found of the
    /// graphics card's encoders stays.
    pub fn reread(&self, info: &MediaInfo, opened_with: &Quality, touched: bool) -> VideoSheet {
        let mut from = *opened_with;
        if touched {
            self.carry(&mut from);
        }
        let mut s = VideoSheet::new(self.target, info, &from);
        s.advanced_open = self.advanced_open;
        s.cards = self.cards;
        s
    }

    /// What was found of this PC's graphics card encoders: one for H.264,
    /// one for HEVC.
    pub fn set_cards(&mut self, cards: [bool; 2]) {
        self.cards = Some(cards);
    }

    /// Move one step or more; the ends hold.
    pub fn step(&mut self, delta: isize) {
        let last = self.steps.len().saturating_sub(1) as isize;
        self.set_at((self.at as isize + delta).clamp(0, last) as usize);
    }

    /// Move to the next mark (`delta` > 0) or the previous one. Past the
    /// last mark, Original.
    pub fn page(&mut self, delta: isize) {
        self.move_to(paged(self.at, &self.marks, self.steps.len(), delta));
    }

    pub fn home(&mut self) {
        self.set_at(0);
    }

    pub fn end(&mut self) {
        self.set_at(self.steps.len().saturating_sub(1));
    }

    /// Snap to mark `i`: 0 Small, 1 High, 2 Best.
    pub fn set_mark(&mut self, i: usize) {
        if let Some(&m) = self.marks.get(i) {
            self.set_at(m);
        }
    }

    /// Put the thumb on step `i`. The step it is already on changes
    /// nothing, so Fit under and the preset's own choice stay.
    pub fn set_at(&mut self, i: usize) {
        if !self.steps.is_empty() {
            self.move_to(i.min(self.steps.len() - 1));
        }
    }

    pub fn large_change(&self) -> usize {
        large_change(self.at, &self.marks, self.steps.len())
    }

    pub fn toggle_advanced(&mut self) {
        self.advanced_open = !self.advanced_open;
    }

    /// What the slider says beside it: the setting, its bitrate and the
    /// result's size, "High · about 6 Mbps · about 84 MB", or "Original ·
    /// copies the video". The size reads as the app writes sizes, the
    /// footer's among them. Several files, or one not read yet, have no
    /// one picture to size: only the step's name, "High".
    pub fn value_text(&self, info: &MediaInfo) -> String {
        let (s, _) = self.chosen(info);
        match video_info(info) {
            Some(v) => {
                let p = vtune::label_parts(self.target, &s, &v, &self.plain());
                std::iter::once(p.name)
                    .chain(p.what.map(str::to_owned))
                    .chain(p.rate)
                    .chain(p.size.map(|b| format!("about {}", human_size(b))))
                    .collect::<Vec<_>>()
                    .join(" · ")
            }
            None => self.steps[self.index_of(&s, info)]
                .label
                .split(" · ")
                .next()
                .unwrap_or_default()
                .to_owned(),
        }
    }

    /// The value text as Narrator reads it: "High, about 6 megabits a
    /// second, about 84 megabytes".
    pub fn a11y_value(&self, info: &MediaInfo) -> String {
        spoken(&self.value_text(info))
    }

    /// The card's line: the result's size alone, "About 84 MB". `None`
    /// when no size can be told, as for Original, which copies the video
    pub fn readout(&self, info: &MediaInfo) -> Option<String> {
        self.estimate_bytes(info)
            .map(|b| format!("About {}", human_size(b)))
    }

    /// The result's size, as the slider's value text gives it. `None` for
    /// several files and without a length.
    pub fn estimate_bytes(&self, info: &MediaInfo) -> Option<u64> {
        let v = video_info(info)?;
        let (s, _) = self.chosen(info);
        vtune::estimate_bytes(self.target, &s, &v, &self.plain())
    }

    /// The sheet's line when Fit under asks for less than the target can
    /// reach: "MP4 can't go under 1.5 MB for this file." While it shows,
    /// Convert waits and [`VideoSheet::to_tune`] leaves Fit under out.
    pub fn fit_error(&self, info: &MediaInfo) -> Option<String> {
        match self.chosen(info).1 {
            Some(e @ MediaError::TooSmall { .. }) => Some(e.to_string()),
            _ => None,
        }
    }

    /// The encoder a conversion gets: the one saved, except a saved
    /// Graphics card this PC has no encoder for the codec on, which is
    /// Automatic, as the Encoder field it could be changed in is cut
    /// (spec 2.5).
    fn encoder_now(&self) -> EncoderChoice {
        let has = |c: [bool; 2]| match self.codec {
            VCodec::H264 => c[0],
            VCodec::Hevc => c[1],
        };
        match (self.encoder, self.cards) {
            (EncoderChoice::GraphicsCard, Some(c)) if !has(c) => EncoderChoice::Automatic,
            (e, _) => e,
        }
    }

    /// Whether Convert can be pressed: not while Fit under asks for a size
    /// the target can't reach. A saved Graphics card with no encoder is
    /// Automatic (`encoder_now`), so it never waits.
    pub fn can_convert(&self, _info: &MediaInfo) -> bool {
        !(self.fit_bytes.is_some() && self.fit_failed)
    }

    /// The tuning a conversion gets. With Fit under the engine works the
    /// setting out from the exact length; one it can't meet is never
    /// handed on. While the slider sits where the preset put it, no setting
    /// is given, so the engine decides for each file.
    pub fn to_tune(&self) -> VideoTune {
        let mut t = self.asked();
        if t.fit_bytes.is_some() && self.fit_failed {
            t.fit_bytes = None;
            t.setting = self.own;
        }
        t
    }

    /// A field's value as its stepper shows it: "720p", "30 fps", "HEVC",
    /// "Graphics card", "160 kbps", "25 MB".
    pub fn field_value(&self, f: TuneField) -> String {
        let limit = |v: Option<u32>, unit: &str| {
            v.map_or_else(|| "Original".to_owned(), |n| format!("{n}{unit}"))
        };
        match f {
            TuneField::Resolution => limit(self.max_lines, "p"),
            TuneField::FrameRate => limit(self.max_fps, " fps"),
            TuneField::Codec => match self.codec {
                VCodec::H264 => "H.264",
                VCodec::Hevc => "HEVC",
            }
            .to_owned(),
            TuneField::Sound => match self.sound {
                SoundChoice::Keep => "Keep".to_owned(),
                SoundChoice::Kbps(k) => format!("{k} kbps"),
                SoundChoice::Off => "Off".to_owned(),
            },
            TuneField::FitUnder => self.fit_bytes.map_or_else(|| "Off".to_owned(), human_size),
            TuneField::Steps => self
                .steps
                .get(self.at)
                .and_then(|s| s.label.split(" · ").next())
                .unwrap_or_default()
                .to_owned(),
            _ => String::new(),
        }
    }

    /// Whether field `f` can be changed now. Codec and Encoder do nothing
    /// to a picture that is copied, so while the slider is on Original and
    /// the copy can be made they are disabled; they come back when
    /// the slider moves or the copy can't be made.
    pub fn field_enabled(&self, f: TuneField, info: &MediaInfo) -> bool {
        match f {
            TuneField::Codec => !self.copies(info),
            _ => true,
        }
    }

    /// Step a field along its values, wrapping at the ends. What changes
    /// the picture or the sound works the steps' sizes out again (HEVC's
    /// model, a smaller picture), and a slider still where the preset put
    /// it follows what the preset now gives (Original can't copy at 720p).
    /// A disabled field doesn't move.
    pub fn step_field(&mut self, f: TuneField, delta: isize, info: &MediaInfo) {
        if !self.field_enabled(f, info) {
            return;
        }
        match f {
            TuneField::Resolution => {
                self.max_lines = cycled(&self.resolutions(), self.max_lines, delta);
            }
            TuneField::FrameRate => {
                self.max_fps = cycled(&self.frame_rates(), self.max_fps, delta);
            }
            TuneField::Codec => {
                self.codec = cycled(&[VCodec::H264, VCodec::Hevc], self.codec, delta);
            }
            TuneField::Sound => self.sound = cycled(&self.sounds(), self.sound, delta),
            TuneField::Steps => {
                let to = wrap(self.at, delta, self.steps.len());
                return self.set_at(to);
            }
            TuneField::FitUnder => {
                let now = || self.estimate_bytes(info);
                let next = next_fit(&VIDEO_FIT_SIZES, self.fit_bytes, now, delta);
                return self.set_fit_under(next, info);
            }
            _ => return,
        }
        self.restep(info);
        self.place(info);
        if self.fit_bytes.is_some() {
            self.follow_fit(info);
        }
        self.refields();
    }

    /// How many values field `f` has to choose from, and which it has
    /// now: every video field is a short list a drop-down shows.
    fn field_list(&self, f: TuneField) -> Option<(usize, usize)> {
        fn place<T: PartialEq>(list: &[T], now: &T) -> (usize, usize) {
            (list.len(), list.iter().position(|v| v == now).unwrap_or(0))
        }
        match f {
            TuneField::Resolution => Some(place(&self.resolutions(), &self.max_lines)),
            TuneField::FrameRate => Some(place(&self.frame_rates(), &self.max_fps)),
            TuneField::Codec => Some(place(&[VCodec::H264, VCodec::Hevc], &self.codec)),
            TuneField::Sound => Some(place(&self.sounds(), &self.sound)),
            TuneField::Steps => Some((self.steps.len(), self.at)),
            TuneField::FitUnder => Some((
                VIDEO_FIT_SIZES.len() + 1,
                fit_index(&VIDEO_FIT_SIZES, self.fit_bytes),
            )),
            _ => None,
        }
    }

    /// Choose value `i` of field `f`'s list ([`Self::field_list`]), as
    /// its drop-down does. A disabled field doesn't change.
    fn pick_field(&mut self, f: TuneField, i: usize, info: &MediaInfo) {
        let Some((len, now)) = self.field_list(f) else {
            return;
        };
        if i >= len || i == now || !self.field_enabled(f, info) {
            return;
        }
        match f {
            TuneField::FitUnder => {
                self.set_fit_under(i.checked_sub(1).map(|k| VIDEO_FIT_SIZES[k]), info)
            }
            _ => self.step_field(f, i as isize - now as isize, info),
        }
    }

    /// Fit under, in bytes, or `None` for off. The slider follows the
    /// setting it picks.
    pub fn set_fit_under(&mut self, bytes: Option<u64>, info: &MediaInfo) {
        self.fit_bytes = bytes.filter(|&b| b > 0 && self.fit_offered);
        if self.fit_bytes.is_some() {
            self.follow_fit(info);
        } else {
            self.place(info);
        }
        self.refields();
    }

    /// Put the thumb on `to`. Only a real move makes its step the setting
    /// and gives up Fit under.
    fn move_to(&mut self, to: usize) {
        if to != self.at {
            self.at = to;
            self.own = self.steps.get(to).map(|s| s.setting);
            self.fit_bytes = None;
            self.refields();
        }
    }

    /// The tuning without a setting or Fit under: the limits, codec,
    /// encoder and sound.
    fn plain(&self) -> VideoTune {
        VideoTune {
            setting: None,
            fit_bytes: None,
            max_lines: self.max_lines,
            max_fps: self.max_fps,
            codec: self.codec,
            encoder: self.encoder_now(),
            sound: self.sound,
            sound_preset: self.preset,
        }
    }

    /// Everything the sheet asks for, Fit under included even when it
    /// can't be met.
    fn asked(&self) -> VideoTune {
        let fit = self.fit_bytes.filter(|_| self.fit_offered);
        VideoTune {
            setting: if fit.is_some() { None } else { self.own },
            fit_bytes: fit,
            ..self.plain()
        }
    }

    /// The setting a conversion would write with, chosen as the engine
    /// chooses it, and Fit under's error if it can't be met (the
    /// slider's own setting then).
    fn chosen(&self, info: &MediaInfo) -> (VideoSetting, Option<MediaError>) {
        let v = picture(info);
        let asked = self.asked();
        match vtune::chosen_setting(self.target, self.preset, &asked, &v) {
            Ok(s) => (s, None),
            Err(e) => {
                let plain = VideoTune {
                    setting: self.own,
                    ..self.plain()
                };
                let s = vtune::chosen_setting(self.target, self.preset, &plain, &v)
                    .unwrap_or(VideoSetting::Original);
                (s, Some(e))
            }
        }
    }

    /// Whether the picture would be copied as it is: never for several
    /// files, which have no one picture to judge.
    fn copies(&self, info: &MediaInfo) -> bool {
        let Some(v) = video_info(info) else {
            return false;
        };
        let t = VideoTune {
            setting: Some(self.chosen(info).0),
            ..self.plain()
        };
        vtune::can_copy_video(self.target, &v, &t)
    }

    /// The thumb to the slider's own step, or where the preset puts it.
    fn place(&mut self, info: &MediaInfo) {
        let s = match self.own {
            Some(s) => s,
            None => {
                let v = picture(info);
                vtune::chosen_setting(self.target, self.preset, &self.plain(), &v)
                    .unwrap_or(VideoSetting::Original)
            }
        };
        self.at = self.index_of(&s, info);
    }

    /// The thumb goes where Fit under's setting is: its step, or for a
    /// bitrate the quality step nearest its size.
    fn follow_fit(&mut self, info: &MediaInfo) {
        let (s, err) = self.chosen(info);
        self.fit_failed = err.is_some();
        if err.is_none() {
            self.at = self.index_of(&s, info);
        }
    }

    /// The step of setting `s`; for a bitrate, the quality step whose size
    /// is nearest it.
    fn index_of(&self, s: &VideoSetting, info: &MediaInfo) -> usize {
        if let Some(i) = self.steps.iter().position(|x| x.setting == *s) {
            return i;
        }
        let v = picture(info);
        let tune = self.plain();
        let size = |s: &VideoSetting| vtune::estimate_bytes(self.target, s, &v, &tune);
        let want = size(s).unwrap_or(0);
        self.steps
            .iter()
            .enumerate()
            .filter(|(_, x)| matches!(x.setting, VideoSetting::Quality(_)))
            .filter_map(|(i, x)| Some((i, size(&x.setting)?.abs_diff(want))))
            .min_by_key(|&(_, d)| d)
            .map_or(self.at, |(i, _)| i)
            .min(self.steps.len().saturating_sub(1))
    }

    /// Work the steps, marks and choices out again for the picture and
    /// sound as written.
    fn restep(&mut self, info: &MediaInfo) {
        let known = video_info(info);
        self.short_side = known.as_ref().map(|v| v.width.min(v.height));
        self.source_fps = known.as_ref().and_then(|v| v.fps);
        // A file not read yet, or several, may have sound: it is offered.
        // Opus for MKV at music's rates; AAC's listed steps from 96 up for
        // the others.
        self.sound_kbps = match self.target {
            _ if !(info.has_audio || known.is_none()) => vec![],
            VideoTarget::Mkv => VIDEO_OPUS_KBPS.to_vec(),
            _ => {
                let w = kiln_media::written(Target::Aac, &Tune::default(), &sound_source(info));
                tune::steps(Target::Aac, &w)
                    .iter()
                    .filter_map(|s| s.kbps)
                    .filter(|&k| k >= VIDEO_AAC_FROM)
                    .collect()
            }
        };
        self.keep_offered();
        let v = picture(info);
        let tune = self.plain();
        self.steps = vtune::steps(self.target, &v, &tune);
        self.marks =
            [Preset::Small, Preset::High, Preset::Best].map(|p| vtune::preset_step(p, &v, &tune));
        self.at = self.at.min(self.steps.len().saturating_sub(1));
        self.fit_offered = known.is_some() && fit_under_offered(info);
        if !self.fit_offered {
            self.fit_bytes = None;
        }
    }

    /// Drop a limit or bitrate Advanced doesn't offer for this source, as a
    /// saved default may hold: 1080p for a 720p clip is Original, which is
    /// what it does. A bitrate between the target's goes to the nearest.
    fn keep_offered(&mut self) {
        if !self.resolutions().contains(&self.max_lines) {
            self.max_lines = None;
        }
        if !self.frame_rates().contains(&self.max_fps) {
            self.max_fps = None;
        }
        if let SoundChoice::Kbps(k) = self.sound {
            self.sound = self
                .sound_kbps
                .iter()
                .copied()
                .min_by_key(|s| s.abs_diff(k))
                .map_or(SoundChoice::Keep, SoundChoice::Kbps);
        }
    }

    /// The fields Advanced shows, leaving out any with nothing to choose.
    fn refields(&mut self) {
        use TuneField::*;
        let mut f = vec![];
        if !self.steps.is_empty() {
            f.push(Steps);
        }
        if self.resolutions().len() > 1 {
            f.push(Resolution);
        }
        if self.frame_rates().len() > 1 {
            f.push(FrameRate);
        }
        // The encoder is cut (spec 2.5): Automatic, unless a saved default
        // says otherwise.
        f.push(Codec);
        if !self.sound_kbps.is_empty() {
            f.push(Sound);
        }
        if self.fit_offered {
            f.push(FitUnder);
        }
        self.fields = f;
    }

    /// Original, then the standard heights below the source's short side:
    /// Kiln never enlarges. All of them while the source isn't known.
    fn resolutions(&self) -> Vec<Option<u32>> {
        let below = |n: &u32| self.short_side.is_none_or(|s| *n < s);
        std::iter::once(None)
            .chain(VIDEO_LINES.iter().filter(|n| below(n)).copied().map(Some))
            .collect()
    }

    /// Original, then the frame rates below the source's (a rate within
    /// 0.5% of a step counts as at it).
    fn frame_rates(&self) -> Vec<Option<u32>> {
        let below = |n: &u32| {
            self.source_fps
                .is_none_or(|f| f > f64::from(*n) * vtune::FPS_ALLOWANCE)
        };
        std::iter::once(None)
            .chain(VIDEO_FPS.iter().filter(|n| below(n)).copied().map(Some))
            .collect()
    }

    /// Keep, then the target's sound bitrates.
    fn sounds(&self) -> Vec<SoundChoice> {
        std::iter::once(SoundChoice::Keep)
            .chain(self.sound_kbps.iter().copied().map(SoundChoice::Kbps))
            .collect()
    }
}

/// `info`'s picture as kiln-media's video tuning reads it. Several files,
/// or one not read yet, have none: a 1080p H.264 picture at 30 fps of
/// whatever length is known stands in, for the steps' names.
fn picture(info: &MediaInfo) -> VideoInfo {
    video_info(info).unwrap_or_else(|| {
        let stand_in = MediaInfo {
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            video_codec: Some(VideoCodec::H264),
            duration: info.duration,
            ..MediaInfo::default()
        };
        video_info(&stand_in).expect("the stand-in has a picture")
    })
}

/// A video's sound as part 1's tuning reads it, for the bitrates Advanced
/// offers. Before the file is read, a stereo 48 kHz source stands in.
fn sound_source(info: &MediaInfo) -> SourceInfo {
    source_info(info).unwrap_or(SourceInfo {
        codec: codec_of(info),
        rate: 48_000,
        channels: Channels {
            count: info.channels.unwrap_or(2),
            mask: 0,
        },
        bits: None,
        frames: None,
        kbps: info.kbps,
        tags: Default::default(),
    })
}

/// The slider an options sheet has: the sound's, for sound Kiln writes
/// itself, or the video's, for MP4, MOV, M4V and MKV among themselves.
/// The window moves and draws either the same way.
#[derive(Debug, Clone, PartialEq)]
pub enum SheetKind {
    Audio(TuneSheet),
    Video(VideoSheet),
}

/// `$body` with `$s` bound to whichever sheet `$kind` holds.
macro_rules! either {
    ($kind:expr, $s:ident => $body:expr) => {
        match $kind {
            SheetKind::Audio($s) => $body,
            SheetKind::Video($s) => $body,
        }
    };
}

impl SheetKind {
    /// The slider for converting to `to`, when `knobs` hold one, with
    /// `quality` set to what it carries.
    pub fn opened(
        knobs: &[Knob],
        to: &str,
        info: &MediaInfo,
        quality: &mut Quality,
    ) -> Option<SheetKind> {
        if knobs.contains(&Knob::AudioTune) {
            let t = Target::from_ext(to)?;
            return Some(SheetKind::Audio(TuneSheet::opened(t, info, quality)));
        }
        if knobs.contains(&Knob::VideoTune) {
            let t = video_target(to)?;
            return Some(SheetKind::Video(VideoSheet::opened(t, info, quality)));
        }
        None
    }

    /// The slider's label, which is also its name for Narrator.
    pub fn name(&self) -> &'static str {
        match self {
            SheetKind::Audio(_) => "Sound quality",
            SheetKind::Video(_) => "Video quality",
        }
    }

    /// How many steps the quality has.
    pub fn steps(&self) -> usize {
        either!(self, s => s.steps.len())
    }

    /// The Quality radio buttons: each one's name and the step it puts the
    /// quality on. Small, High and Best, and for video Original (no
    /// re-encode), which copies the picture where it can. None for a
    /// format with no steps (WAV, AIFF). Two can be on one step: FLAC's
    /// High and Best are both level 5.
    pub fn radios(&self) -> Vec<(&'static str, usize)> {
        let marks = self.marks();
        let mut radios: Vec<(&'static str, usize)> = if self.steps() == 0 {
            vec![]
        } else {
            vec![("Small", marks[0]), ("High", marks[1]), ("Best", marks[2])]
        };
        if let (SheetKind::Video(v), false) = (self, radios.is_empty()) {
            radios.push((ORIGINAL_RADIO, v.steps.len() - 1));
        }
        radios
    }

    /// Which radio is on: `picked`, the one last chosen, while the quality
    /// is still on its step; else the first radio whose step it is on;
    /// none when it is between them (a step chosen under More options).
    pub fn radio_on(&self, picked: Option<usize>) -> Option<usize> {
        let radios = self.radios();
        let at = self.at();
        picked
            .filter(|&p| radios.get(p).is_some_and(|r| r.1 == at))
            .or_else(|| radios.iter().position(|r| r.1 == at))
    }

    /// Choose radio `i`: the quality goes to its step.
    pub fn choose_radio(&mut self, i: usize) {
        if let Some(&(_, step)) = self.radios().get(i) {
            either!(self, s => s.set_at(step))
        }
    }

    /// The step the thumb is on.
    pub fn at(&self) -> usize {
        either!(self, s => s.at)
    }

    pub fn marks(&self) -> [usize; 3] {
        either!(self, s => s.marks)
    }

    pub fn advanced_open(&self) -> bool {
        either!(self, s => s.advanced_open)
    }

    pub fn fields(&self) -> &[TuneField] {
        either!(self, s => &s.fields)
    }

    pub fn step(&mut self, delta: isize) {
        either!(self, s => s.step(delta))
    }

    pub fn page(&mut self, delta: isize) {
        either!(self, s => s.page(delta))
    }

    pub fn home(&mut self) {
        either!(self, s => s.home())
    }

    pub fn end(&mut self) {
        either!(self, s => s.end())
    }

    pub fn set_mark(&mut self, i: usize) {
        either!(self, s => s.set_mark(i))
    }

    pub fn set_at(&mut self, i: usize) {
        either!(self, s => s.set_at(i))
    }

    pub fn large_change(&self) -> usize {
        either!(self, s => s.large_change())
    }

    pub fn toggle_advanced(&mut self) {
        either!(self, s => s.toggle_advanced())
    }

    pub fn value_text(&self, info: &MediaInfo) -> String {
        either!(self, s => s.value_text(info))
    }

    pub fn a11y_value(&self, info: &MediaInfo) -> String {
        either!(self, s => s.a11y_value(info))
    }

    /// The card's line, "About 1.1 MB": only the size, and `None` without
    /// one.
    pub fn readout(&self, info: &MediaInfo) -> Option<String> {
        either!(self, s => s.readout(info))
    }

    /// Fit under's error, shown in place of the value text.
    pub fn fit_error(&self, info: &MediaInfo) -> Option<String> {
        either!(self, s => s.fit_error(info))
    }

    pub fn can_convert(&self, info: &MediaInfo) -> bool {
        match self {
            SheetKind::Audio(s) => s.can_convert(),
            SheetKind::Video(s) => s.can_convert(info),
        }
    }

    pub fn field_value(&self, f: TuneField) -> String {
        either!(self, s => s.field_value(f))
    }

    /// Whether field `f` can be changed now: the sound's always can; the
    /// video's Codec and Encoder can't while the picture is copied.
    pub fn field_enabled(&self, f: TuneField, info: &MediaInfo) -> bool {
        match self {
            SheetKind::Audio(_) => true,
            SheetKind::Video(s) => s.field_enabled(f, info),
        }
    }

    pub fn step_field(&mut self, f: TuneField, delta: isize, info: &MediaInfo) {
        either!(self, s => s.step_field(f, delta, info))
    }

    pub fn reread(&self, info: &MediaInfo, opened_with: &Quality, touched: bool) -> SheetKind {
        match self {
            SheetKind::Audio(s) => SheetKind::Audio(s.reread(info, opened_with, touched)),
            SheetKind::Video(s) => SheetKind::Video(s.reread(info, opened_with, touched)),
        }
    }

    /// Put what the slider says into `quality`, for the conversion.
    pub fn carry(&self, quality: &mut Quality) {
        match self {
            SheetKind::Audio(s) => quality.audio = s.to_tune(),
            SheetKind::Video(s) => s.carry(quality),
        }
    }

    /// What was found of this PC's graphics card encoders (H.264, HEVC);
    /// only the video's slider asks.
    pub fn set_cards(&mut self, cards: [bool; 2]) {
        if let SheetKind::Video(s) = self {
            s.set_cards(cards);
        }
    }

    /// Field `f`'s values as its drop-down lists them, and which it has
    /// now; `None` for a field that keeps its spin box (Opus's bitrate,
    /// Vorbis's quality level). Each value reads as the field shows it.
    pub fn field_choices(&self, f: TuneField, info: &MediaInfo) -> Option<(Vec<String>, usize)> {
        let (len, now) = either!(self, s => s.field_list(f))?;
        let names = (0..len)
            .map(|i| {
                let mut what = self.clone();
                what.pick_field(f, i, info);
                what.field_value(f)
            })
            .collect();
        Some((names, now))
    }

    /// Choose value `i` of field `f`'s drop-down.
    pub fn pick_field(&mut self, f: TuneField, i: usize, info: &MediaInfo) {
        either!(self, s => s.pick_field(f, i, info))
    }
}

/// Where Fit under's `now` is in a drop-down of Off and then `list`: 0
/// for Off, and a size not in the list as Off.
fn fit_index(list: &[u64], now: Option<u64>) -> usize {
    now.and_then(|b| list.iter().position(|&x| x == b))
        .map_or(0, |k| k + 1)
}

/// `info`'s sound as kiln-media's tuning reads it. Before the file is
/// read, or when only FFprobe could read it, a stereo 44.1 kHz source of
/// whatever length is known stands in.
fn source(info: &MediaInfo) -> SourceInfo {
    source_info(info).unwrap_or_else(|| {
        let rate = info.rate.filter(|&r| r > 0).unwrap_or(44_100);
        SourceInfo {
            codec: codec_of(info),
            rate,
            channels: Channels {
                count: info.channels.unwrap_or(2),
                mask: 0,
            },
            bits: info.bits,
            frames: info
                .duration
                .map(|secs| (secs * f64::from(rate)).round() as u64),
            kbps: info.kbps,
            tags: Default::default(),
        }
    })
}

fn media_preset(p: kiln_core::Preset) -> Preset {
    match p {
        kiln_core::Preset::Small => Preset::Small,
        kiln_core::Preset::High => Preset::High,
        kiln_core::Preset::Best => Preset::Best,
    }
}

fn preset_index(p: Preset) -> usize {
    match p {
        Preset::Small => 0,
        Preset::High => 1,
        Preset::Best => 2,
    }
}

/// A setting's bitrate, for the ones that name one.
fn kbps_of(s: &Setting) -> Option<u32> {
    match *s {
        Setting::LameCbr { kbps }
        | Setting::LameAbr { kbps }
        | Setting::VorbisAbr { kbps }
        | Setting::Opus { kbps, .. }
        | Setting::Kbps { kbps } => Some(kbps),
        _ => None,
    }
}

/// The index `delta` steps from `value` in the ascending `list`, unwrapped
/// (it may fall off either end). From a value not in the list, a step up
/// lands on the next larger entry and a step down on the next smaller.
fn off_list_index<T: Copy + Into<u64>>(list: &[T], value: u64, delta: isize) -> isize {
    match list.iter().position(|&x| x.into() == value) {
        Some(i) => i as isize + delta,
        None => {
            // The first entry above `value`.
            let above = list
                .iter()
                .position(|&x| x.into() > value)
                .unwrap_or(list.len()) as isize;
            if delta > 0 {
                above + delta - 1
            } else {
                above + delta
            }
        }
    }
}

fn wrap(i: usize, delta: isize, n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    (i as isize + delta).rem_euclid(n as isize) as usize
}

/// The entry of `list` `delta` steps from `now`'s place in it, wrapping: a
/// value not in the list starts from the first entry.
fn cycled<T: Copy + PartialEq>(list: &[T], now: T, delta: isize) -> T {
    let i = list.iter().position(|v| *v == now).unwrap_or(0);
    list.get(wrap(i, delta, list.len())).copied().unwrap_or(now)
}

/// Where Page Up (`delta` > 0) or Page Down takes the thumb from step `at`
/// of `len`, as many times as `delta` says: to the next mark, or past the
/// last one to the end.
fn paged(at: usize, marks: &[usize; 3], len: usize, delta: isize) -> usize {
    let mut to = at;
    for _ in 0..delta.unsigned_abs() {
        to = if delta > 0 {
            let next = marks.iter().copied().filter(|&m| m > to).min();
            next.unwrap_or(len.saturating_sub(1))
        } else {
            let next = marks.iter().copied().filter(|&m| m < to).max();
            next.unwrap_or(0)
        };
    }
    to
}

/// How far Page Up goes from step `at` of `len`, in steps: to the next mark,
/// or to the end past the last one; at the end, back to the mark before.
/// Never 0 while there are steps to move, so UI Automation's large change
/// always moves the thumb.
fn large_change(at: usize, marks: &[usize; 3], len: usize) -> usize {
    if len < 2 {
        return 0;
    }
    let up = marks
        .iter()
        .copied()
        .filter(|&m| m > at)
        .min()
        .unwrap_or(len - 1);
    if up > at {
        return up - at;
    }
    let down = marks.iter().copied().filter(|&m| m < at).max().unwrap_or(0);
    (at - down).max(1)
}

/// Fit under's next size from `at` (`None` for Off) in the ascending
/// `list`. From Off, a step up gives the largest size under what the file
/// comes to `now` (the smallest when that isn't known or nothing is under
/// it), a step down the largest; past either end of the list, Off.
fn next_fit(
    list: &[u64],
    at: Option<u64>,
    now: impl FnOnce() -> Option<u64>,
    delta: isize,
) -> Option<u64> {
    match at {
        None if delta > 0 => {
            let now = now().unwrap_or(u64::MAX);
            list.iter()
                .rev()
                .copied()
                .find(|&b| b < now)
                .or(list.first().copied())
        }
        None => list.last().copied(),
        Some(b) => {
            let j = off_list_index(list, b, delta);
            (0..list.len() as isize)
                .contains(&j)
                .then(|| list[j as usize])
        }
    }
}

/// A rate in kHz as people write it, the same on rows and in Advanced: "48", "44.1", "22.05".
pub(crate) fn khz(rate: u32) -> String {
    let s = format!("{:.3}", f64::from(rate) / 1000.0);
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// Text the slider and its fields show, as Narrator should read it: the
/// parts between " · " become a list, and units are whole words. "128
/// kbps CBR · about 3.5 MB" reads "128 kilobits a second, constant, about
/// 3.5 megabytes".
pub fn spoken(text: &str) -> String {
    text.split(" · ")
        .map(|part| {
            let part = match part.strip_prefix('q') {
                Some(rest) if rest.starts_with(|c: char| c.is_ascii_digit() || c == '-') => {
                    format!("quality {rest}")
                }
                _ => part.to_owned(),
            };
            part.replace(" kbps CBR", " kilobits a second, constant")
                .replace(" kbps ABR", " kilobits a second, average")
                .replace(" kbps", " kilobits a second")
                .replace(" Mbps", " megabits a second")
                .replace(" fps", " frames a second")
                .replace(" kHz", " kilohertz")
                .replace(" KB", " kilobytes")
                .replace(" MB", " megabytes")
                .replace(" GB", " gigabytes")
                .replace(" TB", " terabytes")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiln_core::quality::MediaInfo;
    use kiln_core::Quality;
    use kiln_media::tune::{Setting, Target};

    /// A 3:41 stereo song at 44.1 kHz, as Kiln's own engine reads it.
    fn song() -> MediaInfo {
        MediaInfo {
            duration: Some(221.0),
            has_audio: true,
            codec: Some("FLAC".into()),
            channels: Some(2),
            rate: Some(44_100),
            bits: Some(16),
            ..MediaInfo::default()
        }
    }

    fn index_of(s: &TuneSheet, setting: Setting) -> usize {
        s.steps.iter().position(|x| x.setting == setting).unwrap()
    }

    fn vbr(tenths: u8) -> Setting {
        Setting::LameVbr { tenths }
    }

    #[test]
    fn opens_on_the_high_preset() {
        let s = TuneSheet::new(Target::Mp3, &song(), &Quality::default());
        assert_eq!(s.at, index_of(&s, vbr(20)));
        assert_eq!(
            s.marks,
            [
                index_of(&s, vbr(50)),
                index_of(&s, vbr(20)),
                index_of(&s, vbr(0))
            ]
        );
        assert!(!s.advanced_open);
    }

    #[test]
    fn settings_bitrate_seeds_the_slider() {
        let q = Quality {
            audio_kbps: Some(150),
            ..Quality::default()
        };
        let s = TuneSheet::new(Target::Aac, &song(), &q);
        assert_eq!(s.at, index_of(&s, Setting::Kbps { kbps: 160 }));
        // The slider's own setting wins over the saved bitrate.
        let q = Quality {
            audio_kbps: Some(150),
            audio: kiln_media::Tune {
                setting: Some(Setting::Kbps { kbps: 96 }),
                ..Default::default()
            },
            ..Quality::default()
        };
        let s = TuneSheet::new(Target::Aac, &song(), &q);
        assert_eq!(s.at, index_of(&s, Setting::Kbps { kbps: 96 }));
    }

    #[test]
    fn page_moves_between_marks() {
        let mut s = TuneSheet::new(Target::Mp3, &song(), &Quality::default());
        s.page(1);
        assert_eq!(s.at, index_of(&s, vbr(0)));
        s.page(-1);
        s.page(-1);
        assert_eq!(s.at, index_of(&s, vbr(50)));
        // Past the last mark, the end.
        s.page(-1);
        assert_eq!(s.at, 0);
        s.end();
        assert_eq!(s.at, s.steps.len() - 1);
        s.step(1);
        assert_eq!(s.at, s.steps.len() - 1, "the slider doesn't wrap");
        s.home();
        assert_eq!(s.at, 0);
        s.set_mark(1);
        assert_eq!(s.at, index_of(&s, vbr(20)));
        s.step(-2);
        assert_eq!(s.to_tune().setting, Some(vbr(30)));
    }

    #[test]
    fn value_text_reads_plainly() {
        let info = song();
        let s = TuneSheet::new(Target::Mp3, &info, &Quality::default());
        // 173 kbps for 221 s is 4,779,125 bytes: 4.8 MB as the app rounds.
        assert_eq!(s.value_text(&info), "V2 · about 173 kbps · about 4.8 MB");
        assert_eq!(
            s.a11y_value(&info),
            "V2, about 173 kilobits a second, about 4.8 megabytes"
        );
        // Without a length there is no size to give.
        let unknown = MediaInfo {
            duration: None,
            ..song()
        };
        let s = TuneSheet::new(Target::Mp3, &unknown, &Quality::default());
        assert_eq!(s.value_text(&unknown), "V2 · about 173 kbps");
        let s = TuneSheet::new(Target::Flac, &info, &Quality::default());
        assert_eq!(s.value_text(&info), "Level 5 · about 23 MB");
    }

    #[test]
    fn fit_under_too_small_says_so() {
        let info = song();
        let mut s = TuneSheet::new(Target::Mp3, &info, &Quality::default());
        assert!(s.fields.contains(&TuneField::FitUnder));
        s.set_fit_under(Some(100_000), &info);
        // The floor is 32 kbps: 902,041 bytes, which rounds up to 1.0 MB.
        assert_eq!(
            s.fit_error(&info).as_deref(),
            Some("MP3 can't go under 1.0 MB for this file.")
        );
        assert_eq!(s.field_value(TuneField::FitUnder), "100 KB");
        // A size that fits picks MP3's average bitrate, and the slider
        // moves to the step nearest it.
        s.set_fit_under(Some(4_000_000), &info);
        assert_eq!(s.fit_error(&info), None);
        assert_eq!(s.to_tune().fit_bytes, Some(4_000_000));
        assert_eq!(s.to_tune().setting, None);
        assert!(s.value_text(&info).starts_with("141 kbps ABR · about "));
        assert_eq!(s.steps[s.at].kbps, Some(144));
        // Moving the slider gives Fit under up.
        s.step(1);
        assert_eq!(s.to_tune().fit_bytes, None);
        assert_eq!(s.field_value(TuneField::FitUnder), "Off");
    }

    #[test]
    fn fit_under_is_hidden_where_it_cant_work() {
        let unknown = MediaInfo {
            duration: None,
            ..song()
        };
        let s = TuneSheet::new(Target::Mp3, &unknown, &Quality::default());
        assert!(!s.fields.contains(&TuneField::FitUnder));
        for t in [Target::Flac, Target::WavPack, Target::Wav, Target::Aiff] {
            let s = TuneSheet::new(t, &song(), &Quality::default());
            assert!(!s.fields.contains(&TuneField::FitUnder), "{t:?}");
            assert_eq!(s.to_tune().fit_bytes, None);
        }
    }

    #[test]
    fn each_format_has_its_own_fields() {
        let info = MediaInfo {
            channels: Some(6),
            rate: Some(48_000),
            ..song()
        };
        let fields = |t| TuneSheet::new(t, &info, &kiln_core::Quality::default()).fields;
        use TuneField::*;
        // MP3's bitrate mode is cut (spec 2.5).
        // Every format with steps has them as the Quality drop-down first.
        assert_eq!(fields(Target::Mp3), [Steps, Rate, Channels, FitUnder]);
        assert_eq!(fields(Target::Ogg), [Steps, Rate, Channels, FitUnder]);
        assert_eq!(
            fields(Target::Opus),
            [Steps, Kbps, Speech, Channels, FitUnder]
        );
        assert_eq!(fields(Target::Aac), [Steps, Rate, Channels, FitUnder]);
        assert_eq!(fields(Target::Ac3), [Steps, Channels, FitUnder]);
        assert_eq!(fields(Target::Flac), [Steps, Bits, Rate, Channels]);
        // WAV and AIFF have no slider, only their depth.
        let wav = TuneSheet::new(Target::Wav, &info, &kiln_core::Quality::default());
        assert!(wav.steps.is_empty());
        assert_eq!(wav.fields, [Bits]);
        // A mono source has no channels to fold, and nothing below 22.05
        // kHz to go down to.
        let mono = MediaInfo {
            channels: Some(1),
            rate: Some(22_050),
            ..song()
        };
        let s = TuneSheet::new(Target::Mp3, &mono, &kiln_core::Quality::default());
        assert_eq!(s.fields, [Steps, FitUnder]);
    }

    #[test]
    fn fields_move_the_slider() {
        let info = song();
        let mut o = TuneSheet::new(Target::Opus, &info, &Quality::default());
        o.step_field(TuneField::Kbps, 1, &info);
        assert_eq!(
            o.to_tune().setting,
            Some(Setting::Opus {
                kbps: 97,
                speech: false
            })
        );
        o.step_field(TuneField::Speech, 1, &info);
        assert_eq!(o.field_value(TuneField::Speech), "Speech");
        assert_eq!(
            o.to_tune().setting,
            Some(Setting::Opus {
                kbps: 97,
                speech: true
            })
        );

        let mut f = TuneSheet::new(Target::Flac, &info, &Quality::default());
        f.step_field(TuneField::Bits, 2, &info);
        assert_eq!(f.field_value(TuneField::Bits), "24-bit");
        assert_eq!(
            f.to_tune().setting,
            Some(Setting::FlacLevel {
                level: 5,
                bits: Some(24)
            })
        );
        assert_eq!(f.to_tune().bits, Some(24));
        f.step_field(TuneField::Level, 1, &info);
        assert_eq!(f.field_value(TuneField::Level), "Level 4");

        let mut w = TuneSheet::new(Target::Wav, &info, &Quality::default());
        assert_eq!(w.to_tune().setting, None);
        w.step_field(TuneField::Bits, 3, &info);
        assert_eq!(w.field_value(TuneField::Bits), "32-bit float");
        assert_eq!(w.to_tune().bits, Some(32));
        assert_eq!(w.value_text(&info), "32-bit float · about 78 MB");

        let mut c = TuneSheet::new(Target::Ogg, &info, &Quality::default());
        c.step_field(TuneField::Channels, 1, &info);
        assert_eq!(c.field_value(TuneField::Channels), "Mono");
        assert_eq!(c.to_tune().channels, kiln_media::ChannelChoice::Mono);
        c.step_field(TuneField::Rate, 1, &info);
        assert_eq!(c.field_value(TuneField::Rate), "32 kHz");
        assert_eq!(c.to_tune().rate, Some(32_000));
    }

    #[test]
    fn a_saved_tune_opens_where_it_was() {
        let info = song();
        let mut s = TuneSheet::new(Target::Mp3, &info, &Quality::default());
        s.step(1);
        s.step_field(TuneField::Channels, 1, &info);
        let q = Quality {
            audio: s.to_tune(),
            ..Quality::default()
        };
        let again = TuneSheet::new(Target::Mp3, &info, &q);
        assert_eq!(again.to_tune(), s.to_tune());
        assert_eq!(again.at, s.at);
        assert_eq!(again.fields, s.fields);
    }

    #[test]
    fn float_sound_keeps_its_float() {
        // A float WAV, as Kiln's engine reads it.
        let float = MediaInfo {
            codec: Some("PCM".into()),
            bits: Some(32),
            float: true,
            ..song()
        };
        // To WAV on Keep it stays float.
        let s = TuneSheet::new(Target::Wav, &float, &Quality::default());
        assert!(
            s.value_text(&float).starts_with("32-bit float · "),
            "{}",
            s.value_text(&float)
        );
        // To FLAC it is written at 16 bits, and sized so: 23 MB, as from CD.
        let s = TuneSheet::new(Target::Flac, &float, &Quality::default());
        assert_eq!(s.value_text(&float), "Level 5 · about 23 MB");
        // A 32-bit integer WAV stays integer.
        let int = MediaInfo {
            float: false,
            ..float
        };
        let s = TuneSheet::new(Target::Wav, &int, &Quality::default());
        assert!(s.value_text(&int).starts_with("32-bit · "));
    }

    #[test]
    fn surround_to_mp3_is_sized_in_stereo() {
        // MP3 folds 5.1 to stereo, so the slider's size is the stereo one,
        // and Channels offers only Mono: Stereo would write what Keep does.
        let film = MediaInfo {
            channels: Some(6),
            rate: Some(48_000),
            ..song()
        };
        let mut s = TuneSheet::new(Target::Mp3, &film, &Quality::default());
        assert_eq!(s.value_text(&film), "V2 · about 173 kbps · about 4.8 MB");
        assert_eq!(
            s.channel_choices(),
            [ChannelChoice::Keep, ChannelChoice::Mono]
        );
        s.step_field(TuneField::Channels, 1, &film);
        assert_eq!(s.field_value(TuneField::Channels), "Mono");
        assert_eq!(s.value_text(&film), "V2 · about 173 kbps · about 2.9 MB");
        // A saved Stereo is the same as Keep here, so the sheet keeps.
        let q = with(kiln_media::Tune {
            channels: ChannelChoice::Stereo,
            ..Default::default()
        });
        let s = TuneSheet::new(Target::Mp3, &film, &q);
        assert_eq!(s.to_tune().channels, ChannelChoice::Keep);
        // Formats that write 5.1 still offer the fold to stereo.
        let s = TuneSheet::new(Target::Ogg, &film, &Quality::default());
        assert_eq!(s.channel_choices().len(), 3);
    }

    #[test]
    fn surround_aac_steps_follow_the_channels() {
        let info = MediaInfo {
            channels: Some(6),
            ..song()
        };
        let mut s = TuneSheet::new(Target::Aac, &info, &Quality::default());
        let surround = s.steps.clone();
        s.step_field(TuneField::Channels, 1, &info);
        assert_eq!(s.field_value(TuneField::Channels), "Stereo");
        assert_ne!(s.steps, surround);
        assert_eq!(s.steps[s.at].setting, Setting::Kbps { kbps: 160 });
    }

    fn with(audio: kiln_media::Tune) -> Quality {
        Quality {
            audio,
            ..Quality::default()
        }
    }

    #[test]
    fn saved_rate_and_channels_must_be_offered() {
        use kiln_media::{ChannelChoice, Tune};
        // 32 kHz saved, on a 22.05 kHz source: not offered, so not written.
        let low = MediaInfo {
            rate: Some(22_050),
            ..song()
        };
        let q = with(Tune {
            rate: Some(32_000),
            ..Tune::default()
        });
        let s = TuneSheet::new(Target::Mp3, &low, &q);
        assert!(!s.fields.contains(&TuneField::Rate));
        assert_eq!(s.to_tune().rate, None);
        assert_eq!(s.field_value(TuneField::Rate), "Keep");
        // Opus and AC3 never offer a rate.
        let q = with(Tune {
            rate: Some(22_050),
            ..Tune::default()
        });
        for t in [Target::Opus, Target::Ac3] {
            assert_eq!(TuneSheet::new(t, &song(), &q).to_tune().rate, None, "{t:?}");
        }
        // Stereo asked of a mono source.
        let mono = MediaInfo {
            channels: Some(1),
            ..song()
        };
        let q = with(Tune {
            channels: ChannelChoice::Stereo,
            ..Tune::default()
        });
        let s = TuneSheet::new(Target::Mp3, &mono, &q);
        assert_eq!(s.to_tune().channels, ChannelChoice::Keep);
        // What is offered stays.
        let q = with(Tune {
            rate: Some(32_000),
            channels: ChannelChoice::Mono,
            ..Tune::default()
        });
        let t = TuneSheet::new(Target::Mp3, &song(), &q).to_tune();
        assert_eq!((t.rate, t.channels), (Some(32_000), ChannelChoice::Mono));
    }

    #[test]
    fn wav_and_aiff_drop_saved_channels_they_dont_show() {
        use kiln_media::{ChannelChoice, Tune};
        // WAV and AIFF show only Bit depth, so a saved Mono would be
        // written with no field saying so.
        let q = with(Tune {
            channels: ChannelChoice::Mono,
            ..Tune::default()
        });
        for t in [Target::Wav, Target::Aiff] {
            let s = TuneSheet::new(t, &song(), &q);
            assert!(!s.fields.contains(&TuneField::Channels), "{t:?}");
            assert_eq!(s.to_tune().channels, ChannelChoice::Keep, "{t:?}");
        }
        // Where Channels shows, the saved choice stays.
        let s = TuneSheet::new(Target::Mp3, &song(), &q);
        assert_eq!(s.to_tune().channels, ChannelChoice::Mono);
    }

    #[test]
    fn a_large_change_reaches_the_next_mark() {
        let mut s = TuneSheet::new(Target::Mp3, &song(), &Quality::default());
        // V2 to V0: four half-steps.
        assert_eq!(s.large_change(), s.marks[2] - s.at);
        assert_eq!(s.large_change(), 4);
        // From V5, the way to V2.
        s.set_mark(0);
        assert_eq!(s.large_change(), s.marks[1] - s.marks[0]);
        // Below Small, the way to Small.
        s.home();
        assert_eq!(s.large_change(), s.marks[0]);
        // At the end, past the last mark, the way back to the one before.
        s.end();
        assert_eq!(s.large_change(), s.at - s.marks[1]);
        // FLAC's High and Best share a step: never a change of nothing.
        let f = TuneSheet::new(Target::Flac, &song(), &Quality::default());
        assert!(f.large_change() >= 1);
    }

    #[test]
    fn convert_waits_while_fit_under_cant_be_met() {
        let info = song();
        let mut s = TuneSheet::new(Target::Mp3, &info, &Quality::default());
        assert!(s.can_convert());
        s.set_fit_under(Some(100_000), &info);
        assert!(s.fit_error(&info).is_some());
        assert!(!s.can_convert());
        // The engine never gets a size it would refuse.
        assert_eq!(s.to_tune().fit_bytes, None);
        assert_eq!(s.to_tune().setting, Some(vbr(20)));
        s.set_fit_under(Some(4_000_000), &info);
        assert_eq!(s.fit_error(&info), None);
        assert!(s.can_convert());
        assert_eq!(s.to_tune().fit_bytes, Some(4_000_000));
        // A saved size it can't meet opens the same way.
        let q = with(kiln_media::Tune {
            fit_bytes: Some(100_000),
            ..Default::default()
        });
        let s = TuneSheet::new(Target::Mp3, &info, &q);
        assert!(s.fit_error(&info).is_some());
        assert!(!s.can_convert());
        assert_eq!(s.to_tune().fit_bytes, None);
    }

    #[test]
    fn steppers_snap_from_values_off_their_lists() {
        let info = song();
        // Fit under at 100 KB: up is 0.5 MB, down is Off. At 4.5 MB: up
        // 5 MB, down 4 MB.
        for (from, delta, to) in [
            (100_000, 1, "500 KB"),
            (100_000, -1, "Off"),
            (4_500_000, 1, "5.0 MB"),
            (4_500_000, -1, "4.0 MB"),
            (4_000_000, 1, "5.0 MB"),
        ] {
            let mut s = TuneSheet::new(Target::Mp3, &info, &Quality::default());
            s.set_fit_under(Some(from), &info);
            s.step_field(TuneField::FitUnder, delta, &info);
            assert_eq!(s.field_value(TuneField::FitUnder), to, "{from} {delta}");
        }
    }

    #[test]
    fn only_a_real_move_gives_up_the_fields() {
        let info = song();
        // A saved constant 256 kbps: its Bitrate mode field is cut, but
        // what a saved default holds is still written.
        let q = with(kiln_media::Tune {
            setting: Some(Setting::LameCbr { kbps: 256 }),
            ..Default::default()
        });
        let mut s = TuneSheet::new(Target::Mp3, &info, &q);
        s.step(0);
        assert_eq!(s.to_tune().setting, Some(Setting::LameCbr { kbps: 256 }));
        // 256 kbps sits nearest V0, the last step: right and Page Down
        // don't move it, so constant stays.
        assert_eq!(s.at, s.steps.len() - 1);
        s.step(1);
        s.page(1);
        assert_eq!(s.to_tune().setting, Some(Setting::LameCbr { kbps: 256 }));
        // End, Best's mark and a click on the step it is on are where it
        // already is: constant stays.
        s.end();
        s.set_mark(2);
        let at = s.at;
        s.set_at(at);
        assert_eq!(s.to_tune().setting, Some(Setting::LameCbr { kbps: 256 }));
        // A real move gives it up.
        s.step(-1);
        assert!(!matches!(
            s.to_tune().setting,
            Some(Setting::LameCbr { .. })
        ));
        // Fit under likewise.
        s.set_fit_under(Some(4_000_000), &info);
        s.step(0);
        let at = s.at;
        s.set_at(at);
        if let Some(m) = s.marks.iter().position(|&m| m == at) {
            s.set_mark(m);
        }
        assert_eq!(s.to_tune().fit_bytes, Some(4_000_000));
        let mut o = TuneSheet::new(Target::Opus, &info, &Quality::default());
        o.set_kbps(97);
        o.step(0);
        assert_eq!(o.field_value(TuneField::Kbps), "97 kbps");
        // 7 kbps sits on the first step: Home is where it is.
        o.set_kbps(7);
        assert_eq!(o.at, 0);
        o.home();
        assert_eq!(o.field_value(TuneField::Kbps), "7 kbps");
    }

    #[test]
    fn a_wav_sheet_carries_what_it_offers_before_the_file_is_read() {
        use kiln_media::{ChannelChoice, Tune};
        // A saved Mono, and the file not read yet: what Convert would
        // carry at once is already the sheet's own.
        let mut q = with(Tune {
            channels: ChannelChoice::Mono,
            ..Tune::default()
        });
        let s = TuneSheet::opened(Target::Wav, &MediaInfo::default(), &mut q);
        assert_eq!(q.audio.channels, ChannelChoice::Keep);
        assert_eq!(q.audio, s.to_tune());
    }

    #[test]
    fn a_move_made_before_the_file_is_read_survives_it() {
        let opened_with = Quality::default();
        let mut s = TuneSheet::new(Target::Mp3, &MediaInfo::default(), &opened_with);
        // Moved to Best before the probe came back.
        s.set_mark(2);
        let read = s.reread(&song(), &opened_with, true);
        assert_eq!(read.at, read.marks[2]);
        assert_eq!(read.to_tune().setting, Some(vbr(0)));
        // Untouched, it opens where the saved quality says: High.
        let fresh = TuneSheet::new(Target::Mp3, &MediaInfo::default(), &opened_with);
        let read = fresh.reread(&song(), &opened_with, false);
        assert_eq!(read.at, read.marks[1]);
        // Advanced stays open or closed as it was.
        s.toggle_advanced();
        assert!(s.reread(&song(), &opened_with, true).advanced_open);
    }

    #[test]
    fn settings_of_another_format_bring_nothing_foreign() {
        let info = song();
        // A FLAC depth asked of MP3: MP3 writes no depth.
        let q = with(kiln_media::Tune {
            setting: Some(Setting::FlacLevel {
                level: 5,
                bits: Some(24),
            }),
            bits: Some(24),
            ..Default::default()
        });
        assert_eq!(TuneSheet::new(Target::Mp3, &info, &q).to_tune().bits, None);
        // Opus at 97 kbps for speech, asked of AAC: the nearest AAC step,
        // and nothing about speech.
        let q = with(kiln_media::Tune {
            setting: Some(Setting::Opus {
                kbps: 97,
                speech: true,
            }),
            ..Default::default()
        });
        let s = TuneSheet::new(Target::Aac, &info, &q);
        assert_eq!(s.to_tune().setting, Some(Setting::Kbps { kbps: 96 }));
        // Asked of Opus itself, both are kept.
        let o = TuneSheet::new(Target::Opus, &info, &q);
        assert_eq!(
            o.to_tune().setting,
            Some(Setting::Opus {
                kbps: 97,
                speech: true
            })
        );
        // A PCM depth is WAV's and AIFF's, not FLAC's...
        let q = with(kiln_media::Tune {
            setting: Some(Setting::Pcm {
                bits: 24,
                float: false,
            }),
            ..Default::default()
        });
        assert_eq!(TuneSheet::new(Target::Flac, &info, &q).to_tune().bits, None);
        // ...and WAV keeps it.
        assert_eq!(
            TuneSheet::new(Target::Wav, &info, &q).to_tune().bits,
            Some(24)
        );
    }

    #[test]
    fn spoken_units_are_whole_words() {
        assert_eq!(
            spoken("128 kbps CBR · about 3.5 MB"),
            "128 kilobits a second, constant, about 3.5 megabytes"
        );
        assert_eq!(
            spoken("q5 · about 160 kbps"),
            "quality 5, about 160 kilobits a second"
        );
        assert_eq!(spoken("44.1 kHz"), "44.1 kilohertz");
        assert_eq!(spoken("about 410 KB"), "about 410 kilobytes");
        assert_eq!(
            spoken("High · about 6 Mbps · about 84 MB"),
            "High, about 6 megabits a second, about 84 megabytes"
        );
        assert_eq!(spoken("30 fps"), "30 frames a second");
    }

    /// A 0:42 1080p H.264 clip at 30 fps with stereo AAC, as Kiln's own
    /// engine reads it.
    fn clip() -> MediaInfo {
        use kiln_media::video::Hdr;
        MediaInfo {
            duration: Some(42.0),
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            video_codec: Some(VideoCodec::H264),
            hdr: Some(Hdr::None),
            bit_depth: Some(8),
            video_kbps: Some(6000),
            has_audio: true,
            codec: Some("AAC".into()),
            kbps: Some(128),
            channels: Some(2),
            rate: Some(48_000),
            ..MediaInfo::default()
        }
    }

    fn video(t: VideoTarget, info: &MediaInfo) -> VideoSheet {
        VideoSheet::new(t, info, &Quality::default())
    }

    /// Each value `f` steps through from where it is, back to the start.
    fn values_of(s: &mut VideoSheet, f: TuneField, info: &MediaInfo) -> Vec<String> {
        let first = s.field_value(f);
        let mut seen = vec![first.clone()];
        loop {
            s.step_field(f, 1, info);
            let v = s.field_value(f);
            if v == first {
                return seen;
            }
            seen.push(v);
        }
    }

    #[test]
    fn a_copyable_video_opens_on_original() {
        let info = clip();
        let s = video(VideoTarget::Mp4, &info);
        // Fifteen quality steps, Original last; Small, High and Best on
        // quality 45, 65 and 80.
        assert_eq!(s.steps.len(), 16);
        assert_eq!(s.steps[15].setting, VideoSetting::Original);
        assert_eq!(
            s.marks.map(|m| s.steps[m].setting),
            [45, 65, 80].map(VideoSetting::Quality)
        );
        assert_eq!(s.at, 15);
        assert_eq!(s.value_text(&info), "Original · copies the video");
        assert_eq!(s.a11y_value(&info), "Original, copies the video");
        // Untouched, the preset decides for the file as the engine does.
        assert_eq!(s.to_tune().setting, None);
        // MKV can't carry a turned picture, so it is encoded, at High.
        let turned = MediaInfo {
            rotation: 90,
            ..clip()
        };
        let s = video(VideoTarget::Mkv, &turned);
        assert_eq!(s.at, s.marks[1]);
        assert!(
            s.value_text(&turned).starts_with("High · about "),
            "{}",
            s.value_text(&turned)
        );
        // Settings' video default is a preset: Small opens on Small.
        let small = Quality {
            preset: kiln_core::Preset::Small,
            ..Quality::default()
        };
        let s = VideoSheet::new(VideoTarget::Mp4, &info, &small);
        assert_eq!(s.at, s.marks[0]);
    }

    #[test]
    fn an_hevc_to_h264_choice_moves_the_estimate_up() {
        let info = MediaInfo {
            video_codec: Some(VideoCodec::Hevc),
            ..clip()
        };
        let mut s = video(VideoTarget::Mp4, &info);
        assert_eq!(s.field_value(TuneField::Codec), "H.264");
        s.set_mark(1);
        s.step_field(TuneField::Codec, 1, &info);
        assert_eq!(s.field_value(TuneField::Codec), "HEVC");
        let hevc = s.estimate_bytes(&info).unwrap();
        let hevc_text = s.value_text(&info);
        s.step_field(TuneField::Codec, 1, &info);
        assert_eq!(s.field_value(TuneField::Codec), "H.264");
        let h264 = s.estimate_bytes(&info).unwrap();
        assert!(h264 > hevc, "{h264} {hevc}");
        // The thumb stays on High; the steps follow the codec's model.
        assert_eq!(s.at, s.marks[1]);
        assert_ne!(s.value_text(&info), hevc_text);
        assert!(s.value_text(&info).starts_with("High · about "));
        assert_eq!(s.to_tune().codec, VCodec::H264);
    }

    #[test]
    fn resolution_offers_only_lower_steps() {
        let info = MediaInfo {
            width: Some(1572),
            height: Some(884),
            ..clip()
        };
        let mut s = video(VideoTarget::Mp4, &info);
        assert_eq!(
            values_of(&mut s, TuneField::Resolution, &info),
            ["Original", "720p", "480p", "360p"]
        );
        // A lower resolution can't be copied: the thumb leaves Original for
        // High, and the size follows the smaller picture.
        s.step_field(TuneField::Resolution, 1, &info);
        assert_eq!(s.to_tune().max_lines, Some(720));
        assert_eq!(s.at, s.marks[1]);
        let at_720 = s.estimate_bytes(&info).unwrap();
        s.step_field(TuneField::Resolution, 1, &info);
        assert!(s.estimate_bytes(&info).unwrap() < at_720);
        // Frame rate likewise: 30.004 fps counts as 30, so only 24
        // is below it.
        let info = MediaInfo {
            fps: Some(30.004),
            ..clip()
        };
        let mut s = video(VideoTarget::Mp4, &info);
        assert_eq!(
            values_of(&mut s, TuneField::FrameRate, &info),
            ["Original", "24 fps"]
        );
        // A 360-line source has nothing below it: no Resolution field.
        let small = MediaInfo {
            width: Some(640),
            height: Some(360),
            ..clip()
        };
        assert!(!video(VideoTarget::Mp4, &small)
            .fields
            .contains(&TuneField::Resolution));
        // A saved default the source fits under shows as Original.
        let q = Quality {
            size: kiln_core::Limit::Max(1080),
            ..Quality::default()
        };
        let s = VideoSheet::new(VideoTarget::Mp4, &small, &q);
        assert_eq!(s.to_tune().max_lines, None);
    }

    #[test]
    fn fit_under_too_small_disables_convert() {
        let info = clip();
        let mut s = video(VideoTarget::Mp4, &info);
        assert!(s.fields.contains(&TuneField::FitUnder));
        assert!(s.can_convert(&info));
        s.set_fit_under(Some(1_000_000), &info);
        // The floor is 150 kbps of picture (aimed 3% low) and 128 of sound
        // for 42 s: 1,483,856 bytes.
        assert_eq!(
            s.fit_error(&info).as_deref(),
            Some("MP4 can't go under 1.5 MB for this file.")
        );
        assert!(!s.can_convert(&info));
        // The engine never gets a size it would refuse.
        assert_eq!(s.to_tune().fit_bytes, None);
        // 25 MB holds less than the copy (about 32 MB): a bitrate under it.
        s.set_fit_under(Some(25_000_000), &info);
        assert_eq!(s.fit_error(&info), None);
        assert!(s.can_convert(&info));
        assert_eq!(s.to_tune().fit_bytes, Some(25_000_000));
        assert_eq!(s.field_value(TuneField::FitUnder), "25 MB");
        // (25 MB over 42 s, less the sound's 128 kbps) aimed 3% low.
        assert_eq!(s.value_text(&info), "4494 kbps · about 24 MB");
        assert!(s.estimate_bytes(&info).unwrap() <= 25_000_000);
        // Moving the slider gives Fit under up.
        s.step(-1);
        assert_eq!(s.to_tune().fit_bytes, None);
        assert_eq!(s.field_value(TuneField::FitUnder), "Off");
        // From Off, the first size is the largest under what it comes to.
        s.end();
        s.step_field(TuneField::FitUnder, 1, &info);
        assert_eq!(s.field_value(TuneField::FitUnder), "25 MB");
        // Without a length (a fragmented MP4) there's no Fit under.
        let unknown = MediaInfo {
            duration: None,
            ..clip()
        };
        let s = video(VideoTarget::Mp4, &unknown);
        assert!(!s.fields.contains(&TuneField::FitUnder));
        assert_eq!(s.value_text(&unknown), "Original · copies the video");
    }

    #[test]
    fn several_files_show_no_size() {
        // Several files have no one file to size: only the step's name.
        let none = MediaInfo::default();
        let mut s = video(VideoTarget::Mp4, &none);
        assert_eq!(s.value_text(&none), "Original");
        assert_eq!(s.estimate_bytes(&none), None);
        s.set_mark(1);
        assert_eq!(s.value_text(&none), "High");
        s.step(-1);
        assert_eq!(s.value_text(&none), "Step 9");
        assert!(!s.fields.contains(&TuneField::FitUnder));
        // Every resolution is offered, none of them known to be too large.
        assert_eq!(
            values_of(&mut s, TuneField::Resolution, &none),
            ["Original", "2160p", "1440p", "1080p", "720p", "480p", "360p"]
        );
    }

    #[test]
    fn a_saved_graphics_card_with_no_encoder_converts_on_automatic() {
        // Spec 2.5 cut the Encoder field: nothing on screen could change
        // it, so a saved Graphics card this PC can't use never waits.
        let info = clip();
        let mut q = Quality::default();
        q.video.encoder = EncoderChoice::GraphicsCard;
        let mut s = VideoSheet::new(VideoTarget::Mp4, &info, &q);
        s.set_mark(1);
        s.set_cards([false, false]);
        assert!(s.can_convert(&info));
        assert_eq!(s.to_tune().encoder, EncoderChoice::Automatic);
        // With an encoder for the codec, it is kept.
        s.set_cards([true, false]);
        assert_eq!(s.to_tune().encoder, EncoderChoice::GraphicsCard);
    }

    #[test]
    fn codec_waits_while_the_picture_is_copied() {
        // on Original with a copy to make, they change nothing.
        let info = clip();
        let mut s = video(VideoTarget::Mp4, &info);
        assert_eq!(s.at, s.steps.len() - 1);
        assert!(!s.field_enabled(TuneField::Codec, &info));
        s.step_field(TuneField::Codec, 1, &info);
        assert_eq!(
            (s.to_tune().codec, s.to_tune().encoder),
            (VCodec::H264, EncoderChoice::Automatic)
        );
        assert!(s.field_enabled(TuneField::Sound, &info));
        // Off Original they come back...
        s.set_mark(1);
        assert!(s.field_enabled(TuneField::Codec, &info));
        // ...and on Original where the copy can't be made (MKV can't carry
        // a turned picture).
        let turned = MediaInfo {
            rotation: 90,
            ..clip()
        };
        let mut k = video(VideoTarget::Mkv, &turned);
        k.end();
        assert!(k.field_enabled(TuneField::Codec, &turned));
        // Several files have no one picture to judge: they stay enabled.
        let none = MediaInfo::default();
        assert!(video(VideoTarget::Mp4, &none).field_enabled(TuneField::Codec, &none));
    }

    #[test]
    fn sound_keeps_or_takes_the_targets_bitrates() {
        let info = clip();
        let mut s = video(VideoTarget::Mp4, &info);
        assert_eq!(
            values_of(&mut s, TuneField::Sound, &info),
            ["Keep", "96 kbps", "128 kbps", "160 kbps", "192 kbps"]
        );
        s.step_field(TuneField::Sound, 3, &info);
        assert_eq!(s.to_tune().sound, SoundChoice::Kbps(160));
        // MKV's sound is Opus, at music's rates.
        let mut m = video(VideoTarget::Mkv, &info);
        assert_eq!(
            values_of(&mut m, TuneField::Sound, &info),
            ["Keep", "64 kbps", "96 kbps", "128 kbps", "160 kbps", "192 kbps", "256 kbps"]
        );
        // A silent clip has no Sound field.
        let silent = MediaInfo {
            has_audio: false,
            codec: None,
            kbps: None,
            channels: None,
            rate: None,
            ..clip()
        };
        assert!(!video(VideoTarget::Mp4, &silent)
            .fields
            .contains(&TuneField::Sound));
        use TuneField::*;
        assert_eq!(
            video(VideoTarget::Mp4, &info).fields,
            [Steps, Resolution, FrameRate, Codec, Sound, FitUnder]
        );
    }

    #[test]
    fn a_video_move_made_before_the_file_is_read_survives_it() {
        let opened_with = Quality::default();
        let none = MediaInfo::default();
        let mut s = SheetKind::Video(VideoSheet::new(VideoTarget::Mp4, &none, &opened_with));
        s.set_mark(2);
        let read = s.reread(&clip(), &opened_with, true);
        assert_eq!(read.at(), read.marks()[2]);
        // Untouched, it opens where the preset puts the file: Original.
        let fresh = SheetKind::Video(VideoSheet::new(VideoTarget::Mp4, &none, &opened_with));
        let read = fresh.reread(&clip(), &opened_with, false);
        assert_eq!(read.at(), read.steps() - 1);
        // What the sheet carries is the job's: its limits too.
        let mut q = Quality {
            size: kiln_core::Limit::Max(720),
            ..Quality::default()
        };
        let mut s = SheetKind::Video(VideoSheet::opened(VideoTarget::Mp4, &clip(), &mut q));
        assert_eq!(q.video.max_lines, Some(720));
        s.step_field(TuneField::Resolution, 1, &clip());
        s.carry(&mut q);
        assert_eq!(q.video.max_lines, Some(480));
        assert_eq!(q.size, kiln_core::Limit::Max(480));
        // Original is Original: the saved 720 doesn't come back behind it.
        s.step_field(TuneField::Resolution, -2, &clip());
        s.carry(&mut q);
        assert_eq!(q.video.max_lines, None);
        assert_eq!(kiln_core::quality::video_tune(&q).max_lines, None);
        assert_eq!(s.name(), "Video quality");
    }

    #[test]
    fn video_fields_are_drop_downs_that_pick_any_value() {
        // each More options line of the video is a list to pick from.
        let info = clip();
        let mut s = SheetKind::Video(video(VideoTarget::Mp4, &info));
        let (names, now) = s.field_choices(TuneField::Resolution, &info).unwrap();
        assert_eq!(names[0], "Original");
        assert!(names.contains(&"720p".to_owned()), "{names:?}");
        assert_eq!(names[now], s.field_value(TuneField::Resolution));
        let k = names.iter().position(|n| n == "720p").unwrap();
        s.pick_field(TuneField::Resolution, k, &info);
        assert_eq!(s.field_value(TuneField::Resolution), "720p");
        // Back to Original, in one pick, from any value.
        s.pick_field(TuneField::Resolution, 0, &info);
        assert_eq!(s.field_value(TuneField::Resolution), "Original");
        // The codec is H.264 or HEVC, once the picture is encoded: while
        // it is copied the line is disabled.
        s.set_mark(1);
        assert!(s.field_enabled(TuneField::Codec, &info));
        let (codecs, _) = s.field_choices(TuneField::Codec, &info).unwrap();
        assert_eq!(codecs, ["H.264", "HEVC"]);
        // Max size starts at Off, then its sizes, smallest first.
        if let Some((sizes, at)) = s.field_choices(TuneField::FitUnder, &info) {
            assert_eq!((sizes[0].as_str(), at), ("Off", 0));
            s.pick_field(TuneField::FitUnder, 3, &info);
            assert_eq!(s.field_value(TuneField::FitUnder), sizes[3]);
        }
        // A pick past the end does nothing.
        let before = s.clone();
        s.pick_field(TuneField::Resolution, 99, &info);
        assert_eq!(s, before);
    }

    #[test]
    fn sound_fields_are_drop_downs_but_opus_bitrate_and_vorbis_quality_spin() {
        let info = song();
        let quality = Quality::default();
        let mut aac = SheetKind::Audio(TuneSheet::new(Target::Aac, &info, &quality));
        let (rates, _) = aac.field_choices(TuneField::Kbps, &info).unwrap();
        assert!(rates.iter().all(|r| r.ends_with(" kbps")), "{rates:?}");
        let last = rates.len() - 1;
        aac.pick_field(TuneField::Kbps, last, &info);
        assert_eq!(aac.field_value(TuneField::Kbps), rates[last]);
        let opus = SheetKind::Audio(TuneSheet::new(Target::Opus, &info, &quality));
        assert_eq!(opus.field_choices(TuneField::Kbps, &info), None);
        let (tuned, _) = opus.field_choices(TuneField::Speech, &info).unwrap();
        assert_eq!(tuned, ["Music", "Speech"]);
        let ogg = SheetKind::Audio(TuneSheet::new(Target::Ogg, &info, &quality));
        assert_eq!(ogg.field_choices(TuneField::Quality, &info), None);
        let mut flac = SheetKind::Audio(TuneSheet::new(Target::Flac, &info, &quality));
        let (bits, _) = flac.field_choices(TuneField::Bits, &info).unwrap();
        assert_eq!(bits, ["Keep", "16-bit", "24-bit"]);
        flac.pick_field(TuneField::Bits, 2, &info);
        assert_eq!(flac.field_value(TuneField::Bits), "24-bit");
    }
}
