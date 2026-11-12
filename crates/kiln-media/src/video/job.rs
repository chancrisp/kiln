//! The video job: one file in, one MP4, MOV, M4V or MKV out.
//!
//! The picture is copied as the file holds it when the rules allow
//! ([`tune::can_copy_video`]), else decoded, processed and encoded
//! ([`VideoEncoder`]). The sound is copied when its codec fits the target
//! ([`tune::can_copy_sound`]), else run through part 1's pipeline and encoded
//! as AAC ([`AacPackets`], MP4, MOV and M4V) or Opus ([`OpusPackets`], MKV).
//! Sound part 1 can't read is copied when it fits and refused when it
//! doesn't; it is never dropped (R14, R15).
//!
//! Each source of packets (a compressed reader, a decoder feeding the
//! encoder, the sound pipeline) is a feed. The feed furthest behind in time
//! goes next, one packet, frame or block at a time, and the [`Interleaver`]
//! writes what they give in decode-time order. What waits is only what that
//! order holds back (an encoder's delay, say), so memory stays flat however
//! long the file.
//!
//! The file is written to the output's name plus `.part` and renamed when it
//! is complete; on any error or cancel the `.part` is removed.

use super::encode::{self, encoder_adapter, VideoEncoder};
use super::info::VideoInfo;
use super::packet::{muxer_at, Container, Mux, Packet, StreamCodec, StreamSpec};
use super::read::{ReadMode, Reader, TIMESCALE};
use super::tune::{
    self, can_copy_sound, can_copy_video, quality_under, SoundChoice, VideoSetting, VideoTarget,
    VideoTune,
};
use crate::error::{MediaError, Result};
use crate::mf::d3d::d3d_for;
use crate::mp4::AacPackets;
use crate::ogg::OpusPackets;
use crate::pipeline::{Reporter, Source, BLOCK_FRAMES};
use crate::process::Process;
use crate::tune::{Preset, Target};
use crate::types::{Block, Channels, Codec, SourceInfo, Tags};
use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

/// What a job did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobReport {
    /// The picture was copied as the source holds it.
    pub video_copied: bool,
    /// The sound was copied as the source holds it. False for a silent
    /// source.
    pub sound_copied: bool,
    /// A graphics card's encoder wrote the picture. False on a copy.
    pub hardware: bool,
    /// Automatic meant the picture for a graphics card, and its encoder
    /// refused it, so Windows' software encoder wrote it (R58). False on a
    /// copy, with Processor, and on a PC with no hardware encoder.
    pub fell_back: bool,
    /// The picture was decoded from Matroska whose decoded frames didn't
    /// line up one for one with its blocks, so from where they parted the
    /// frames kept Media Foundation's times rather than the blocks' (R78);
    /// or every frame lined up but the blocks outnumbered the frames. False
    /// on a copy and for any other source.
    pub decoder_times: bool,
}

/// Converts the video `input` to `target`, written to `output`, which must
/// not exist yet: the file is written to `output` with `.part` added and
/// renamed once complete, and on any error or cancel the `.part` is removed.
///
/// The picture's setting is, in order (R44): what Fit under finds for
/// `tune.fit_bytes`; `tune.setting`; else `preset`'s (High: Original where
/// the picture can be copied, else the High step; Small: quality 45; Best:
/// Original where it can be copied, else quality 80). The sound is copied
/// when its codec fits the target and `tune.sound` is Keep, else encoded at
/// `preset`'s bitrate (which replaces `tune.sound_preset`, R95) or the one
/// chosen, as the size model counts it (R90), starting where the source's
/// does.
///
/// Fit under never gives a file over its limit (R45): a pass that lands
/// over is measured against the size model, and the job picks again with
/// the model corrected, up to three passes, before it says `TooSmall`.
///
/// `progress` gets values from 0 to 1 that never go backwards: the end of
/// the last picture packet written over the length (or, when the length is
/// unknown, the share of the file read), the last 1.0 once the file is in
/// place. Under Fit under each pass has its own part of the bar. `cancel` is
/// asked before each packet, frame or block.
///
/// A file with no video is [`MediaError::NoVideo`], before anything is made.
/// Sound that part 1 can't read and the target can't hold is
/// [`MediaError::Unsupported`], also before anything is made.
pub fn convert(
    input: &Path,
    output: &Path,
    target: VideoTarget,
    preset: Preset,
    tune: &VideoTune,
    progress: &mut dyn FnMut(f32),
    cancel: &dyn Fn() -> bool,
) -> Result<JobReport> {
    let info = super::probe(input)?;
    if output.exists() {
        return Err(MediaError::Io(std::io::Error::from(
            std::io::ErrorKind::AlreadyExists,
        )));
    }
    // The sound the job encodes under Keep follows the job's preset, and
    // so does every count of it (R95).
    let tune = &VideoTune {
        sound_preset: preset,
        ..*tune
    };
    // Fit under's estimate reads only what is known of the card (R82), so
    // the job finds it out first; a job without Fit under asks only when it
    // encodes.
    if tune.fit_bytes.is_some() {
        let _ = encoder_adapter(tune.codec, tune.encoder);
    }
    let mut job = Job::plan(input, target, preset, tune, info)?;
    let part = part_path(output);
    let mut reporter = Reporter::new(progress);
    // Each pass removes its own `.part` on failure; a finished one is this
    // job's until it is renamed.
    let done = job.passes(&part, &mut reporter, cancel)?;
    if let Err(e) = rename_new(&part, output) {
        let _ = std::fs::remove_file(&part);
        return Err(MediaError::Io(e));
    }
    reporter.done();
    Ok(JobReport {
        video_copied: job.copy_video,
        sound_copied: job.sound == SoundPlan::Copy,
        hardware: done.hardware,
        fell_back: done.fell_back,
        decoder_times: done.decoder_times,
    })
}

/// The share of a pass's bar an MP4's finish takes, where it copies every
/// sample into place (R92): a good part of a copy's time, and little of an
/// encode's.
const FINISH_SHARE_COPY: f64 = 0.3;
const FINISH_SHARE_ENCODE: f64 = 0.02;

/// `output` with `.part` added to its name.
fn part_path(output: &Path) -> PathBuf {
    let mut name = OsString::from(output.as_os_str());
    name.push(".part");
    PathBuf::from(name)
}

/// Renames `from` to `to`, which must not exist: a file that has appeared
/// there since the job started is never replaced.
#[cfg(windows)]
fn rename_new(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_WRITE_THROUGH};
    let wide = |p: &Path| -> Vec<u16> { p.as_os_str().encode_wide().chain([0]).collect() };
    let (from, to) = (wide(from), wide(to));
    // SAFETY: both paths are NUL-terminated and live across the call.
    unsafe {
        MoveFileExW(
            PCWSTR(from.as_ptr()),
            PCWSTR(to.as_ptr()),
            MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(|e| std::io::Error::from_raw_os_error((e.code().0 as u32 & 0xFFFF) as i32))
}

#[cfg(not(windows))]
fn rename_new(from: &Path, to: &Path) -> std::io::Result<()> {
    if to.exists() {
        return Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists));
    }
    std::fs::rename(from, to)
}

fn container(t: VideoTarget) -> Container {
    match t {
        VideoTarget::Mp4 => Container::Mp4,
        VideoTarget::Mov => Container::Mov,
        VideoTarget::M4v => Container::M4v,
        VideoTarget::Mkv => Container::Mkv,
    }
}

/// The picture's setting, by R44 ([`tune::chosen_setting`]), with Fit
/// under's limit too, whatever it answered (R45).
fn chosen_video(
    t: VideoTarget,
    preset: Preset,
    tune: &VideoTune,
    info: &VideoInfo,
) -> Result<(VideoSetting, Option<u64>)> {
    Ok((tune::chosen_setting(t, preset, tune, info)?, tune.fit_bytes))
}

/// The sound as part 1 would name it, from the stream a compressed reader
/// gives, for [`can_copy_sound`].
fn sound_of(spec: &StreamSpec) -> Option<SourceInfo> {
    let codec = match &spec.codec {
        StreamCodec::Aac { .. } => Codec::Aac,
        StreamCodec::Opus { .. } => Codec::Opus,
        StreamCodec::Vorbis { .. } => Codec::Vorbis,
        StreamCodec::Flac { .. } => Codec::Flac,
        StreamCodec::Ac3 => Codec::Ac3,
        _ => return None,
    };
    let a = spec.audio.as_ref()?;
    Some(SourceInfo {
        codec,
        rate: a.rate,
        channels: Channels {
            count: a.channels,
            mask: 0,
        },
        bits: None,
        frames: None,
        kbps: None,
        tags: Tags::default(),
    })
}

/// Packets read, at most, looking for the first sound packet.
const SOUND_SEARCH: usize = 20_000;

/// When the first sample part 1's reader gives of `input`'s sound plays, in
/// seconds: the first sound packet's time as the file presents it, plus what
/// that reader drops before its first sample (Opus's pre-skip; the priming
/// an MP4's edit list skips, which part 1 trims too). 0 when the file's
/// stored sound can't be read.
fn sound_start(input: &Path, sound: &SourceInfo) -> f64 {
    let Ok(mut reader) = open_reader(input, ReadMode::Compressed) else {
        return 0.0;
    };
    let Some(spec) = reader.streams().get(1).map(|s| s.spec.clone()) else {
        return 0.0;
    };
    let mut first = None;
    for _ in 0..SOUND_SEARCH {
        match reader.next_packet() {
            Ok(Some(p)) if p.stream == 1 => {
                first = Some(p.pts);
                break;
            }
            Ok(Some(_)) => {}
            _ => break,
        }
    }
    let Some(pts) = first else {
        return 0.0;
    };
    let dropped = match &spec.codec {
        StreamCodec::Opus { head } if sound.codec == Codec::Opus => head
            .get(10..12)
            .map_or(0.0, |b| u16::from_le_bytes([b[0], b[1]]) as f64 / 48_000.0),
        StreamCodec::Aac { .. } if sound.codec == Codec::Aac && sound.rate > 0 => File::open(input)
            .ok()
            .and_then(|mut f| crate::mp4read::aac_trim(&mut f, sound.rate))
            .map_or(0.0, |t| t.skip as f64 / sound.rate as f64),
        _ => 0.0,
    };
    pts as f64 / TIMESCALE as f64 + dropped
}

/// What happens to the sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SoundPlan {
    Silent,
    Copy,
    Encode,
}

/// A job, decided: what is copied, what is encoded and how.
struct Job<'a> {
    input: &'a Path,
    target: VideoTarget,
    info: VideoInfo,
    /// Tune, with the picture's setting resolved.
    vtune: VideoTune,
    copy_video: bool,
    sound: SoundPlan,
    /// Fit under's limit.
    fit: Option<u64>,
    /// The length progress is measured against, in seconds: the file's, or
    /// for a fragmented MP4 (which states none) its fragments' added up.
    length: Option<f64>,
    /// After a pass landed over the limit: its size over the model's
    /// estimate, by which the model is corrected (R45).
    ratio: Option<f64>,
    tags: Tags,
    /// What was opened to decide, kept for the first pass.
    packets: Option<Reader>,
}

/// What one pass came to.
struct Done {
    hardware: bool,
    fell_back: bool,
    /// The decoded frames kept the decoder's times (R78).
    decoder_times: bool,
    /// The picture's setting as written: an encoder that ignores a bitrate
    /// was given a quality step instead.
    setting: VideoSetting,
    /// Whether the picture's encoder honoured a bitrate, read once the pass
    /// was over (after any fallback); `None` when the picture was copied.
    honours: Option<bool>,
}

/// Each Fit under pass's part of the progress bar (R51): a job done in one
/// pass goes from 0.9 to 1.0.
const SPANS: [(f64, f64); 3] = [(0.0, 0.9), (0.9, 0.97), (0.97, 0.99)];
/// R45 aims the corrected model this far under the limit.
const AIM: f64 = 0.97;

impl<'a> Job<'a> {
    /// Decides the job, and refuses what can't be done before anything is
    /// written.
    fn plan(
        input: &'a Path,
        target: VideoTarget,
        preset: Preset,
        tune: &VideoTune,
        info: VideoInfo,
    ) -> Result<Job<'a>> {
        let (setting, fit) = chosen_video(target, preset, tune, &info)?;
        let setting = first_pass_over(setting, fit);
        let vtune = VideoTune {
            setting: Some(setting),
            ..*tune
        };
        let copy_video = can_copy_video(target, &info, &vtune);

        // A reader of the file as stored: for the picture when it is copied,
        // and to see what sound the file holds and whether it can be copied.
        let (packets, unopened) = match open_reader(input, ReadMode::Compressed) {
            Ok(r) => (Some(r), None),
            Err(e) if copy_video => return Err(e),
            Err(e) => (None, Some(e)),
        };
        let stored = packets
            .as_ref()
            .and_then(|r| r.streams().get(1))
            .map(|s| &s.spec);
        let fits = stored
            .and_then(sound_of)
            .is_some_and(|s| can_copy_sound(target, &s));
        // Sound the probe missed but the reader sees is sound all the same,
        // and part 1 couldn't read it (R14).
        let seen = packets.as_ref().and_then(|r| {
            r.sound_unavailable().map(str::to_string).or_else(|| {
                stored
                    .map(|s| sound_of(s).map_or(s.codec.name().into(), |i| i.codec.display_name()))
            })
        });
        let unreadable = match &info.sound {
            Some(_) => None,
            None => info.sound_unreadable.clone().or(seen),
        };

        let mut sound = match (&info.sound, unreadable) {
            // Left out when asked: whatever it is, nothing is copied or
            // refused.
            _ if tune.sound == SoundChoice::Off => SoundPlan::Silent,
            (None, None) => SoundPlan::Silent,
            (Some(s), _)
                if tune.sound == SoundChoice::Keep && fits && can_copy_sound(target, s) =>
            {
                SoundPlan::Copy
            }
            (Some(_), _) => SoundPlan::Encode,
            // R14: sound part 1 can't read is copied, or refused; never dropped.
            (None, Some(_)) if fits => SoundPlan::Copy,
            (None, Some(codec)) => {
                // A file the reader can't open (damaged, say) is that, not
                // a sound problem; a picture it can't copy leaves the sound's.
                return Err(match unopened {
                    Some(e) if !matches!(e, MediaError::Encoder(_)) => e,
                    _ => MediaError::Unsupported { codec },
                });
            }
        };
        if sound == SoundPlan::Encode {
            if target != VideoTarget::Mkv {
                crate::table::can_run(Target::M4a)?;
            }
            // R50: sound part 1 names but can't decode after all (AC-3 in
            // Matroska) is copied when the target takes it, else refused
            // with part 1's sentence; never called damaged, never dropped.
            match first_block(input) {
                Ok(()) => {}
                Err(e @ (MediaError::Read(_) | MediaError::Cancelled)) => return Err(e),
                Err(_) if fits => sound = SoundPlan::Copy,
                Err(MediaError::NeedsMediaFoundation) => {
                    return Err(MediaError::NeedsMediaFoundation)
                }
                Err(_) => {
                    let codec = info.sound.as_ref().map(|s| s.codec.display_name());
                    return Err(MediaError::Unsupported {
                        codec: codec.unwrap_or_default(),
                    });
                }
            }
        }
        let packets = packets.filter(|_| copy_video || sound == SoundPlan::Copy);

        // R48: part 1's tags, with the title the container itself keeps.
        let mut tags = match &info.sound {
            Some(s) => s.tags.clone(),
            None => File::open(input)
                .map(|f| {
                    let ext = input.extension().and_then(|e| e.to_str()).unwrap_or("");
                    crate::sym::container_tags(f, ext)
                })
                .unwrap_or_default(),
        };
        if tags.title.is_none() {
            tags.title = info.title.clone();
        }
        let length = info
            .duration
            .or_else(|| {
                let mut f = File::open(input).ok()?;
                super::read::fragmented_seconds(&mut f)
            })
            .filter(|d| d.is_finite() && *d > 0.0);
        Ok(Job {
            input,
            target,
            info,
            vtune,
            copy_video,
            sound,
            fit,
            length,
            ratio: None,
            tags,
            packets,
        })
    }

    /// Writes the file to `part`: once, or under Fit under as many passes
    /// as R45 allows. On success `part` is complete and under any limit; on
    /// error it has been removed.
    fn passes(
        &mut self,
        part: &Path,
        reporter: &mut Reporter<'_>,
        cancel: &dyn Fn() -> bool,
    ) -> Result<Done> {
        let Some(limit) = self.fit else {
            return self.run(part, reporter, cancel, (0.0, 1.0));
        };
        for (pass, span) in SPANS.into_iter().enumerate() {
            let done = self.run(part, reporter, cancel, span)?;
            let size = match std::fs::metadata(part) {
                Ok(m) => m.len(),
                Err(e) => {
                    let _ = std::fs::remove_file(part);
                    return Err(MediaError::Io(e));
                }
            };
            if size <= limit {
                return Ok(done);
            }
            let _ = std::fs::remove_file(part);
            let Some(estimate) = self.estimate(&done.setting).filter(|&e| e > 0) else {
                // No length, so no model to correct: what was written is the
                // least known.
                return Err(self.too_small(size));
            };
            let ratio = size as f64 / estimate as f64;
            self.ratio = Some(ratio);
            if pass + 1 == SPANS.len() {
                return Err(self.too_small(self.floor_bytes(limit, done.honours)));
            }
            let next = self.repick(limit, done.honours)?;
            self.vtune.setting = Some(next);
            self.copy_video = can_copy_video(self.target, &self.info, &self.vtune);
        }
        unreachable!("the last pass returns")
    }

    /// The size model's estimate for the picture at `s` with the sound; a
    /// copy whose bitrate the container doesn't give is judged as the top
    /// quality step, as Fit under judges it.
    fn estimate(&self, s: &VideoSetting) -> Option<u64> {
        let t = self.target;
        tune::estimate_bytes(t, s, &self.info, &self.vtune).or_else(|| {
            let top = VideoSetting::Quality(tune::QUALITY_LAST);
            tune::estimate_bytes(t, &top, &self.info, &self.vtune)
        })
    }

    /// The estimate the corrected model gives for `s`.
    fn corrected(&self, s: &VideoSetting) -> Option<f64> {
        Some(self.estimate(s)? as f64 * self.ratio.unwrap_or(1.0))
    }

    /// The highest quality step the corrected model puts at or under 97% of
    /// `limit`.
    fn corrected_quality(&self, limit: u64) -> Option<u8> {
        (tune::QUALITY_FIRST..=tune::QUALITY_LAST)
            .rev()
            .step_by(tune::QUALITY_STEP as usize)
            .find(|&q| {
                self.corrected(&VideoSetting::Quality(q))
                    .is_some_and(|b| b <= AIM * limit as f64)
            })
    }

    /// The highest bitrate the corrected model puts at or under 97% of
    /// `limit`; `None` below the least Fit under writes.
    fn corrected_kbps(&self, limit: u64) -> Option<u32> {
        // The estimate is the sound's bytes plus the picture's, which grow
        // with the bitrate in step.
        let none = self.corrected(&VideoSetting::Kbps(0))?;
        let per_kbps = (self.corrected(&VideoSetting::Kbps(1000))? - none) / 1000.0;
        if per_kbps <= 0.0 {
            return None;
        }
        let kbps = ((AIM * limit as f64 - none) / per_kbps).floor();
        (kbps >= tune::VIDEO_FLOOR_KBPS).then(|| kbps.min(u32::MAX as f64) as u32)
    }

    /// The setting for the next pass under `limit` (R45): a bitrate, or for
    /// an encoder that ignores one, a quality step.
    fn repick(&self, limit: u64, honours: Option<bool>) -> Result<VideoSetting> {
        let next = if honours == Some(false) {
            self.corrected_quality(limit).map(VideoSetting::Quality)
        } else {
            self.corrected_kbps(limit).map(VideoSetting::Kbps)
        };
        next.ok_or_else(|| self.too_small(self.floor_bytes(limit, honours)))
    }

    /// The least limit the corrected model says the job can meet: the
    /// lowest quality step for an encoder that ignores a bitrate, else the
    /// least bitrate Fit under writes, at 97%. Always over `limit`.
    fn floor_bytes(&self, limit: u64, honours: Option<bool>) -> u64 {
        let floor = if honours == Some(false) {
            VideoSetting::Quality(tune::QUALITY_FIRST)
        } else {
            VideoSetting::Kbps(tune::VIDEO_FLOOR_KBPS as u32)
        };
        let bytes = self
            .corrected(&floor)
            .map_or(0, |b| (b / AIM).ceil() as u64);
        bytes.max(limit + 1)
    }

    fn too_small(&self, min_bytes: u64) -> MediaError {
        MediaError::TooSmall {
            format: self.target.name().to_string(),
            min_bytes,
        }
    }

    /// Writes the whole file to `part`, which is removed again on any error.
    /// Progress runs over `span` of the bar.
    fn run(
        &mut self,
        part: &Path,
        reporter: &mut Reporter<'_>,
        cancel: &dyn Fn() -> bool,
        span: (f64, f64),
    ) -> Result<Done> {
        let mut feeds = Vec::new();
        let mut setting = self.vtune.setting.unwrap_or(VideoSetting::Original);
        let video = 0;
        let sound = (self.sound != SoundPlan::Silent).then_some(1);
        let streams = 1 + sound.is_some() as usize;

        // Everything is opened before the file is made, so whatever can't be
        // done is said before anything is written.
        let copy_sound = self.sound == SoundPlan::Copy;
        if self.copy_video || copy_sound {
            let reader = match self.packets.take() {
                Some(r) => r,
                None => open_reader(self.input, ReadMode::Compressed)?,
            };
            let lead_in = LeadIn::new(&self.info, &reader);
            feeds.push(Feed::packets(
                reader,
                self.copy_video.then_some(video),
                sound.filter(|_| copy_sound),
                lead_in,
            ));
        }
        if !self.copy_video {
            let (feed, used) = self.frames(video)?;
            setting = used;
            feeds.push(feed);
        }
        if let (Some(s), false) = (sound, copy_sound) {
            feeds.push(self.sound_feed(s)?);
        }

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(part)?;
        // Declared before the muxer, so it goes after it: the file is closed
        // by the time it is removed.
        let mut partial = Partial {
            path: part,
            keep: false,
        };
        // An MP4's samples wait beside the .part, on its volume (R92).
        let mut mux = muxer_at(container(self.target), file, part)?;
        mux.describe(self.info.created, self.tags.title.as_deref());
        let mut il = Interleaver::new(mux, streams);
        for f in &feeds {
            f.streams(&mut il);
        }

        // An MP4's finish copies every sample once more, so it has its own
        // share of the bar at the end, and cancel is heard during it (R92).
        let finish_share = match self.target {
            VideoTarget::Mkv => 0.0,
            _ if self.copy_video => FINISH_SHARE_COPY,
            _ => FINISH_SHARE_ENCODE,
        };
        let written = span.0 + (span.1 - span.0) * (1.0 - finish_share);
        let duration = self.length;
        loop {
            if cancel() {
                return Err(MediaError::Cancelled);
            }
            let Some(f) = next_feed(&mut feeds) else {
                break;
            };
            f.pull(&mut il, cancel)?;
            let fraction = match duration {
                Some(d) => il.written_until(video).unwrap_or(0.0) / d,
                None => feeds
                    .iter()
                    .find(|f| f.video.is_some())
                    .and_then(Feed::bytes_fraction)
                    .unwrap_or(0.0),
            };
            let fraction = if fraction.is_nan() {
                0.0
            } else {
                fraction.clamp(0.0, 1.0)
            };
            reporter.during(span.0 + (written - span.0) * fraction);
        }
        let hardware = feeds.iter().any(Feed::hardware);
        let fell_back = feeds.iter().any(Feed::fell_back);
        let decoder_times = feeds.iter().any(Feed::decoder_times);
        let honours = feeds.iter().find_map(Feed::honours);
        il.finish_watched(&self.tags, &mut |f| {
            reporter.during(written + (span.1 - written) * f);
            cancel()
        })?;
        drop(feeds);
        partial.keep = true;
        Ok(Done {
            hardware,
            fell_back,
            decoder_times,
            setting,
            honours,
        })
    }

    /// The decoder and the encoder for the picture, and the setting it is
    /// written at. With Fit under's bitrate and an encoder that ignores one,
    /// the encoder is made again for a quality step (R33): the highest whose
    /// estimate fits, or once a pass has corrected the model, the highest
    /// the corrected model fits (R45).
    fn frames(&self, stream: usize) -> Result<(Feed, VideoSetting)> {
        let mut vtune = self.vtune;
        // R34: decoded on the card whose encoder will start, or in memory.
        let d3d = encoder_adapter(vtune.codec, vtune.encoder).and_then(d3d_for);
        let reader = open_reader(self.input, ReadMode::Decoded { d3d })?;
        let mut enc = VideoEncoder::new(
            &encode::plan(&self.info, &vtune),
            vtune.encoder,
            reader.d3d(),
        )?;
        if let (Some(limit), Some(VideoSetting::Kbps(_))) = (self.fit, vtune.setting) {
            if !enc.honours_bitrate() {
                let q = match self.ratio {
                    None => quality_under(self.target, limit, &self.info, &vtune)
                        .unwrap_or(tune::QUALITY_FIRST),
                    Some(_) => self
                        .corrected_quality(limit)
                        .ok_or_else(|| self.too_small(self.floor_bytes(limit, Some(false))))?,
                };
                vtune.setting = Some(VideoSetting::Quality(q));
                drop(enc);
                enc = VideoEncoder::new(
                    &encode::plan(&self.info, &vtune),
                    vtune.encoder,
                    reader.d3d(),
                )?;
            }
        }
        let setting = vtune
            .setting
            .unwrap_or(VideoSetting::Quality(tune::QUALITY_LAST));
        // The frames keep their times (R38): the rate written is the
        // source's, constant unless frames are dropped to a limit.
        let frame_ns = self
            .info
            .frame_ns()
            .filter(|_| encode::plan(&self.info, &vtune).max_fps.is_none());
        Ok((
            Feed {
                kind: Kind::Frames {
                    reader,
                    enc: Box::new(enc),
                    stream,
                    given: false,
                    frame_ns,
                },
                video: Some(stream),
                at: f64::NEG_INFINITY,
                ended: false,
            },
            setting,
        ))
    }

    /// Part 1's pipeline and the target's sound encoder, its packets moved
    /// to where the source's sound starts.
    fn sound_feed(&mut self, stream: usize) -> Result<Feed> {
        let source = crate::table::open_source(self.input).map_err(MediaError::reading)?;
        let info = source.info().clone();
        // As the size model counts it (R90).
        let (audio, written, setting) = tune::sound_encoding(self.target, &info, &self.vtune)?;
        let process = Process::new(
            info.rate,
            info.channels,
            written.rate,
            written.channels,
            None,
        )?;
        let enc = match audio {
            Target::Opus => SoundEnc::Opus(OpusPackets::new(written.channels, &setting, stream)?),
            _ => {
                let crate::tune::Setting::Kbps { kbps } = setting else {
                    return Err(MediaError::Encoder(
                        "this setting isn't an AAC bitrate".into(),
                    ));
                };
                let enc = crate::mf::aac::encoder(written.rate, written.channels.count, kbps)?;
                SoundEnc::Aac(AacPackets::new(enc, stream))
            }
        };
        let start = sound_start(self.input, &info);
        let scale = enc.stream().timescale;
        Ok(Feed {
            kind: Kind::Sound {
                block: Block {
                    channels: info.channels.count,
                    rate: info.rate,
                    samples: Vec::with_capacity(BLOCK_FRAMES * info.channels.count as usize),
                },
                converted: Block {
                    channels: 0,
                    rate: 0,
                    samples: Vec::new(),
                },
                rate: info.rate.max(1),
                source,
                process,
                enc,
                stream,
                start,
                shift: (start * scale as f64).round() as i64,
            },
            video: None,
            at: f64::NEG_INFINITY,
            ended: false,
        })
    }
}

/// Opens `input`'s sound through part 1 and decodes its first block: part 1
/// can name a sound (AC-3 in Matroska, through Media Foundation) whose
/// blocks it then can't read. Facts use it to flag such sound before any
/// conversion (R91).
pub fn first_block(input: &Path) -> Result<()> {
    let mut source = crate::table::open_source(input).map_err(MediaError::reading)?;
    let mut block = Block {
        channels: source.info().channels.count,
        rate: source.info().rate,
        samples: Vec::new(),
    };
    source.read(&mut block).map_err(MediaError::reading)?;
    Ok(())
}

/// A test hook, in debug builds only: `KILN_FIT_FIRST_PASS_OVER` set to a
/// factor multiplies Fit under's first bitrate by it, so the first pass
/// lands over the limit and the job's check after it is seen to work.
#[cfg(debug_assertions)]
fn first_pass_over(setting: VideoSetting, fit: Option<u64>) -> VideoSetting {
    let factor = std::env::var("KILN_FIT_FIRST_PASS_OVER")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|f| f.is_finite() && *f > 0.0);
    match (setting, fit, factor) {
        (VideoSetting::Kbps(k), Some(_), Some(f)) => {
            VideoSetting::Kbps((k as f64 * f).min(u32::MAX as f64) as u32)
        }
        _ => setting,
    }
}

#[cfg(not(debug_assertions))]
fn first_pass_over(setting: VideoSetting, _fit: Option<u64>) -> VideoSetting {
    setting
}

fn open_reader(input: &Path, mode: ReadMode) -> Result<Reader> {
    Reader::open(File::open(input).map_err(MediaError::Read)?, mode)
}

/// A `.part` file this job made, removed when this goes unless kept.
struct Partial<'a> {
    path: &'a Path,
    keep: bool,
}

impl Drop for Partial<'_> {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_file(self.path);
        }
    }
}

// ---------------------------------------------------------------------------
// The feeds

/// The sound encoder for the target.
enum SoundEnc {
    Aac(AacPackets),
    Opus(OpusPackets),
}

impl SoundEnc {
    fn stream(&self) -> StreamSpec {
        match self {
            SoundEnc::Aac(e) => e.stream(),
            SoundEnc::Opus(e) => e.stream(),
        }
    }
    fn push(&mut self, b: &Block, out: &mut Vec<Packet>) -> Result<()> {
        match self {
            SoundEnc::Aac(e) => e.push(b, out),
            SoundEnc::Opus(e) => e.push(b, out),
        }
    }
    fn finish(&mut self, out: &mut Vec<Packet>) -> Result<()> {
        match self {
            SoundEnc::Aac(e) => e.finish(out),
            SoundEnc::Opus(e) => e.finish(out),
        }
    }
}

/// What a copy leaves out before the start the source presents (R69): an
/// MP4's edit list can hide frames before it (an excerpt cut by stream
/// copy), and sound with them. Each stream's start is the reader's
/// ([`Reader::start`]): 0, or later by the edit list's empty edits.
///
/// The picture starts at the key frame at or before the start, found by
/// [`VideoInfo::lead_in`]: the packets before it in decode order go, and
/// so do any after it shown before it (the leading pictures of an open GOP,
/// which need the GOP that went). The MP4 writer's edit list then hides
/// what is still before the start (the job says where it is); in MKV, which
/// has none, a copy is made only when that is less than a frame
/// (`tune::copyable`).
///
/// Sound that ends at or before the start goes too, but for the last such
/// packet, which an AAC decoder primes on.
#[derive(Debug, Default)]
struct LeadIn {
    /// Until the key frame the picture starts at has come: the earliest
    /// presentation time it can have, in the reader's ticks.
    video_from: Option<i64>,
    /// That key frame's presentation time, once it has come.
    key: Option<i64>,
    /// Where the sound's presentation starts.
    sound_start: i64,
    /// The last sound packet before the start, until one after it comes.
    sound_before: Option<Packet>,
    sound_started: bool,
}

impl LeadIn {
    fn new(info: &VideoInfo, reader: &Reader) -> LeadIn {
        // The key frame is `lead_in` before the start, give or take the
        // ticks of the times' rounding; the one before it is at least a
        // frame earlier.
        let half_frame = 0.5 / info.fps.filter(|f| *f > 0.0).unwrap_or(30.0);
        let start = reader.start(0);
        LeadIn {
            video_from: info
                .lead_in
                .map(|l| start - ((l + half_frame) * TIMESCALE as f64).round() as i64),
            sound_start: reader.start(1),
            ..LeadIn::default()
        }
    }

    /// What of the reader's packet `p` the copy keeps, in decode order.
    fn keep(&mut self, p: Packet) -> Vec<Packet> {
        if p.stream == 0 {
            return if self.keep_picture(&p) {
                vec![p]
            } else {
                Vec::new()
            };
        }
        if self.sound_started {
            return vec![p];
        }
        if p.pts.saturating_add(p.duration.max(0)) <= self.sound_start {
            self.sound_before = Some(p);
            return Vec::new();
        }
        self.sound_started = true;
        self.sound_before.take().into_iter().chain([p]).collect()
    }

    fn keep_picture(&mut self, p: &Packet) -> bool {
        match self.video_from {
            Some(from) if p.key && p.pts >= from => {
                self.video_from = None;
                self.key = Some(p.pts);
                true
            }
            Some(_) => false,
            None => self.key.is_none_or(|k| p.pts >= k),
        }
    }

    /// At the end: sound that all ended before the start still gives its
    /// last packet, so the stream isn't empty.
    fn sound_end(&mut self) -> Option<Packet> {
        self.sound_before.take()
    }
}

/// One source of packets for the interleaver.
struct Feed {
    kind: Kind,
    /// The picture's stream, when this feed gives it.
    video: Option<usize>,
    /// How far this feed has got, in seconds: the feed furthest behind goes
    /// next.
    at: f64,
    ended: bool,
}

enum Kind {
    /// A reader of the file as stored: its picture (stream 0) and sound
    /// (stream 1) packets go to the streams given, or are passed over.
    Packets {
        reader: Reader,
        video: Option<usize>,
        sound: Option<usize>,
        lead_in: LeadIn,
    },
    /// Decoded frames into the encoder.
    Frames {
        reader: Reader,
        enc: Box<VideoEncoder>,
        stream: usize,
        /// True once the encoder's stream has been given to the interleaver.
        given: bool,
        /// Each frame's length when the rate written is constant (R80).
        frame_ns: Option<u64>,
    },
    /// Part 1's blocks into a sound encoder.
    Sound {
        source: Box<dyn Source>,
        process: Process,
        enc: SoundEnc,
        block: Block,
        converted: Block,
        rate: u32,
        stream: usize,
        /// Where the source's sound starts, in seconds and in the encoder's
        /// ticks: every packet is moved by it.
        start: f64,
        shift: i64,
    },
}

impl Feed {
    fn packets(
        reader: Reader,
        video: Option<usize>,
        sound: Option<usize>,
        lead_in: LeadIn,
    ) -> Feed {
        Feed {
            kind: Kind::Packets {
                reader,
                video,
                sound,
                lead_in,
            },
            video,
            at: f64::NEG_INFINITY,
            ended: false,
        }
    }

    /// Gives the interleaver the streams known before the first packet.
    fn streams(&self, il: &mut Interleaver) {
        match &self.kind {
            Kind::Packets {
                reader,
                video,
                sound,
                ..
            } => {
                for (from, to) in [(0, video), (1, sound)] {
                    if let (Some(to), Some(s)) = (to, reader.streams().get(from)) {
                        il.set_stream(*to, s.spec.clone());
                        il.present_from(*to, reader.start(from));
                    }
                }
            }
            // The encoder's stream is complete with its first packet; it
            // keeps the frames' times (R38), so its start is the source's.
            Kind::Frames { reader, stream, .. } => il.present_from(*stream, reader.start(0)),
            // Encoded sound starts where the source's does, its encoder's
            // priming before that hidden by the MP4 writer's edit list.
            Kind::Sound {
                enc, stream, shift, ..
            } => {
                il.set_stream(*stream, enc.stream());
                il.present_from(*stream, *shift);
            }
        }
    }

    /// Reads one packet, frame or block, and gives the interleaver what
    /// comes of it. `cancel` is asked while a graphics card's encoder is
    /// waited on.
    fn step(&mut self, il: &mut Interleaver, cancel: &dyn Fn() -> bool) -> Result<()> {
        let mut out = Vec::new();
        match &mut self.kind {
            Kind::Packets {
                reader,
                video,
                sound,
                lead_in,
            } => match reader.next_packet().map_err(MediaError::reading)? {
                None => {
                    self.ended = true;
                    if let (Some(to), Some(mut p)) = (*sound, lead_in.sound_end()) {
                        p.stream = to;
                        il.push(p)?;
                    }
                    for s in [*video, *sound].into_iter().flatten() {
                        il.end(s)?;
                    }
                }
                Some(p) => {
                    let t = p.dts as f64 / TIMESCALE as f64;
                    self.at = self.at.max(t);
                    let to = if p.stream == 0 { *video } else { *sound };
                    if let Some(to) = to {
                        for mut p in lead_in.keep(p) {
                            p.stream = to;
                            il.push(p)?;
                        }
                    }
                    // The reader gives its streams in decode order together.
                    for s in [*video, *sound].into_iter().flatten() {
                        il.reached(s, t);
                    }
                }
            },
            Kind::Frames {
                reader,
                enc,
                stream,
                given,
                frame_ns,
            } => {
                match reader.next_frame().map_err(MediaError::reading)? {
                    Some(f) => {
                        self.at = self.at.max(f.pts as f64 / TIMESCALE as f64);
                        enc.push_watched(f, &mut out, cancel)?;
                    }
                    None => {
                        enc.finish_watched(&mut out, cancel)?;
                        self.ended = true;
                    }
                }
                if !out.is_empty() && !*given {
                    let mut spec = enc.stream();
                    if let Some(v) = &mut spec.video {
                        v.frame_ns = *frame_ns;
                    }
                    il.set_stream(*stream, spec);
                    *given = true;
                }
                for mut p in out.drain(..) {
                    p.stream = *stream;
                    il.push(p)?;
                }
                if self.ended {
                    il.end(*stream)?;
                }
            }
            Kind::Sound {
                source,
                process,
                enc,
                block,
                converted,
                rate,
                stream,
                start,
                shift,
            } => {
                if source.read(block).map_err(MediaError::reading)? {
                    if block.frames() > 0 {
                        if process.is_identity() {
                            enc.push(block, &mut out)?;
                        } else {
                            process.run(block, converted);
                            if converted.frames() > 0 {
                                enc.push(converted, &mut out)?;
                            }
                        }
                    }
                    let read = source.position().frames as f64 / *rate as f64;
                    self.at = self.at.max(*start + read);
                } else {
                    process.flush(converted);
                    if converted.frames() > 0 {
                        enc.push(converted, &mut out)?;
                    }
                    enc.finish(&mut out)?;
                    self.ended = true;
                }
                for mut p in out.drain(..) {
                    p.pts = p.pts.saturating_add(*shift);
                    p.dts = p.dts.saturating_add(*shift);
                    il.push(p)?;
                }
                if self.ended {
                    il.end(*stream)?;
                }
            }
        }
        Ok(())
    }

    /// The share of the file the reader has read, for a file of unknown
    /// length (R43).
    fn bytes_fraction(&self) -> Option<f64> {
        match &self.kind {
            Kind::Packets { reader, .. } | Kind::Frames { reader, .. } => reader.bytes_fraction(),
            Kind::Sound { .. } => None,
        }
    }

    /// True when a graphics card's encoder wrote the picture.
    fn hardware(&self) -> bool {
        match &self.kind {
            Kind::Frames { enc, .. } => enc.hardware(),
            _ => false,
        }
    }

    /// True when the decoded frames kept the decoder's times (R78).
    fn decoder_times(&self) -> bool {
        match &self.kind {
            Kind::Frames { reader, .. } => reader.decoder_times(),
            _ => false,
        }
    }

    /// True when Automatic's hardware encoder refused the picture and the
    /// software one wrote it.
    fn fell_back(&self) -> bool {
        match &self.kind {
            Kind::Frames { enc, .. } => enc.fell_back(),
            _ => false,
        }
    }

    /// Whether the picture's encoder honours a bitrate, as of now (after
    /// any fallback); `None` for a feed with no encoder.
    fn honours(&self) -> Option<bool> {
        match &self.kind {
            Kind::Frames { enc, .. } => Some(enc.honours_bitrate()),
            _ => None,
        }
    }
}

/// What the job's loop needs of a feed.
trait Pull {
    /// How far the feed has got, in seconds.
    fn at(&self) -> f64;
    fn ended(&self) -> bool;
    /// Reads one packet, frame or block, and gives the interleaver what
    /// comes of it, asking `cancel` while an encoder is waited on.
    fn pull(&mut self, il: &mut Interleaver, cancel: &dyn Fn() -> bool) -> Result<()>;
}

impl Pull for Feed {
    fn at(&self) -> f64 {
        self.at
    }
    fn ended(&self) -> bool {
        self.ended
    }
    fn pull(&mut self, il: &mut Interleaver, cancel: &dyn Fn() -> bool) -> Result<()> {
        self.step(il, cancel)
    }
}

/// The feed that goes next: the one furthest behind in time, so no feed
/// runs ahead and what the interleaver holds stays small. `None` once every
/// feed has ended.
fn next_feed<T: Pull>(feeds: &mut [T]) -> Option<&mut T> {
    feeds
        .iter_mut()
        .filter(|f| !f.ended())
        .min_by(|a, b| a.at().total_cmp(&b.at()))
}

// ---------------------------------------------------------------------------
// The interleaver

/// Puts the packets of several streams in the order a muxer wants them and
/// writes them.
///
/// Packets are held per stream and written in decode-time order, so no
/// stream is written ahead of another (the muxers want them within half a
/// second). One is written once no other stream can still bring a packet
/// before it: every other stream has a packet waiting, has ended, or has got
/// (by its own packets, or as [`Interleaver::reached`] says) at least as far.
/// Nothing is written until every stream's [`StreamSpec`] is known, as a
/// stream an encoder writes is complete only with its first packet.
///
/// Each stream's packets must come in decode order; the streams may come in
/// any order, and what is held is only what that order makes wait.
///
/// Public only so the memory test can drive it; not a stable API.
#[doc(hidden)]
pub struct Interleaver {
    mux: Box<dyn Mux>,
    lanes: Vec<Lane>,
    started: bool,
}

#[derive(Default)]
struct Lane {
    spec: Option<StreamSpec>,
    queue: VecDeque<Packet>,
    /// The decode time of the last packet pushed, in the stream's ticks.
    pushed: Option<i64>,
    /// A time, in seconds, before which no more packets will come.
    reached: Option<f64>,
    ended: bool,
    /// Where the furthest packet written ends, in seconds.
    written_end: Option<f64>,
    /// Where the stream is presented from, for the muxer (`present_from`).
    from: Option<i64>,
}

impl Lane {
    fn seconds(&self, ticks: i64) -> f64 {
        let scale = self.spec.as_ref().map_or(TIMESCALE, |s| s.timescale).max(1);
        ticks as f64 / scale as f64
    }

    /// The time no more packets of this stream will come before.
    fn floor(&self) -> Option<f64> {
        let pushed = self.pushed.map(|t| self.seconds(t));
        match (pushed, self.reached) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        }
    }
}

impl Interleaver {
    /// An interleaver for `streams` streams, numbered from 0, into `mux`.
    pub fn new(mux: Box<dyn Mux>, streams: usize) -> Interleaver {
        Interleaver {
            mux,
            lanes: (0..streams).map(|_| Lane::default()).collect(),
            started: false,
        }
    }

    /// Says what stream `i` is. Its packets wait until every stream's is
    /// known; then the streams are added to the muxer in their order.
    pub fn set_stream(&mut self, i: usize, spec: StreamSpec) {
        if let Some(l) = self.lanes.get_mut(i) {
            l.spec = Some(spec);
        }
    }

    /// Takes a packet of stream `p.stream`, and writes what can be written.
    pub fn push(&mut self, p: Packet) -> Result<()> {
        let lane = self.lanes.get_mut(p.stream).ok_or_else(|| {
            MediaError::Encoder("a packet came for a stream that isn't there".into())
        })?;
        lane.pushed = Some(lane.pushed.map_or(p.dts, |t| t.max(p.dts)));
        lane.queue.push_back(p);
        self.pump()
    }

    /// Says that stream `i` is presented from `ticks` on (see
    /// [`Mux::present_from`]).
    pub fn present_from(&mut self, i: usize, ticks: i64) {
        if let Some(l) = self.lanes.get_mut(i) {
            l.from = Some(ticks);
        }
        if self.started {
            self.mux.present_from(i, ticks);
        }
    }

    /// Says that stream `i` will bring no packet before `seconds`.
    pub fn reached(&mut self, i: usize, seconds: f64) {
        if let Some(l) = self.lanes.get_mut(i) {
            l.reached = Some(l.reached.map_or(seconds, |r| r.max(seconds)));
        }
    }

    /// Says that stream `i` has ended, and writes what can be written.
    pub fn end(&mut self, i: usize) -> Result<()> {
        if let Some(l) = self.lanes.get_mut(i) {
            l.ended = true;
        }
        self.pump()
    }

    /// Where the furthest packet of stream `i` written ends, in seconds.
    pub fn written_until(&self, i: usize) -> Option<f64> {
        self.lanes.get(i).and_then(|l| l.written_end)
    }

    /// The packets held, waiting to be written.
    pub fn waiting(&self) -> usize {
        self.lanes.iter().map(|l| l.queue.len()).sum()
    }

    /// Writes everything held and finishes the file with `tags`.
    pub fn finish(self, tags: &Tags) -> Result<()> {
        self.finish_watched(tags, &mut |_| false)
    }

    /// [`Interleaver::finish`], with `watch` told how far the muxer's
    /// finishing has got and asked whether to stop ([`Mux::finish_watched`]).
    pub fn finish_watched(mut self, tags: &Tags, watch: &mut dyn FnMut(f64) -> bool) -> Result<()> {
        for l in &mut self.lanes {
            l.ended = true;
        }
        self.pump()?;
        if !self.started {
            return Err(MediaError::Encoder("a stream got no packets".into()));
        }
        self.mux.finish_watched(tags, watch)
    }

    /// Writes every packet that can be written now.
    fn pump(&mut self) -> Result<()> {
        if !self.started {
            if self.lanes.iter().any(|l| l.spec.is_none()) {
                return Ok(());
            }
            for (i, l) in self.lanes.iter().enumerate() {
                let added = self
                    .mux
                    .add_stream(l.spec.clone().expect("checked above"))?;
                if added != i {
                    return Err(MediaError::Encoder(
                        "the muxer numbered the streams out of order".into(),
                    ));
                }
                if let Some(t) = l.from {
                    self.mux.present_from(i, t);
                }
            }
            self.started = true;
        }
        loop {
            let mut first: Option<(usize, f64)> = None;
            for (i, l) in self.lanes.iter().enumerate() {
                if let Some(p) = l.queue.front() {
                    let t = l.seconds(p.dts);
                    if first.is_none_or(|(_, f)| t < f) {
                        first = Some((i, t));
                    }
                }
            }
            let Some((i, t)) = first else {
                return Ok(());
            };
            let free = self.lanes.iter().enumerate().all(|(j, l)| {
                j == i || l.ended || !l.queue.is_empty() || l.floor().is_some_and(|f| f >= t)
            });
            if !free {
                return Ok(());
            }
            let p = self.lanes[i].queue.pop_front().expect("found above");
            self.write(p)?;
        }
    }

    fn write(&mut self, p: Packet) -> Result<()> {
        failing_write()?;
        let l = &mut self.lanes[p.stream];
        let end = l.seconds(p.pts.saturating_add(p.duration.max(0)));
        l.written_end = Some(l.written_end.map_or(end, |e| e.max(end)));
        self.mux.write(p)
    }
}

/// In a debug build, `KILN_FAIL_WRITE` set to an HRESULT (such as
/// `0x80070070`, the disk is full) makes every write fail with it, as Media
/// Foundation would report it, so a test can see what the job makes of it.
fn failing_write() -> Result<()> {
    #[cfg(all(windows, debug_assertions))]
    if let Some(v) = std::env::var_os("KILN_FAIL_WRITE") {
        let v = v.to_string_lossy();
        let code =
            u32::from_str_radix(v.trim().trim_start_matches("0x"), 16).unwrap_or(0x8000_4005);
        let e: windows::core::Error = windows::core::HRESULT(code as i32).into();
        return Err(crate::mf::com::failed("writing the video", e));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// A muxer that notes what it is given.
    #[derive(Clone, Default)]
    struct Notes(Arc<Mutex<Vec<(usize, i64)>>>);

    struct NoteMux(Notes, usize);

    impl Mux for NoteMux {
        fn describe(&mut self, _created: Option<i64>, _title: Option<&str>) {}
        fn add_stream(&mut self, _s: StreamSpec) -> Result<usize> {
            self.1 += 1;
            Ok(self.1 - 1)
        }
        fn write(&mut self, p: Packet) -> Result<()> {
            self.0 .0.lock().unwrap().push((p.stream, p.dts));
            Ok(())
        }
        fn finish(self: Box<Self>, _tags: &Tags) -> Result<()> {
            Ok(())
        }
    }

    fn spec(timescale: u32) -> StreamSpec {
        StreamSpec {
            codec: StreamCodec::Ac3,
            timescale,
            video: None,
            audio: Some(super::super::packet::AudioParams {
                rate: 48_000,
                channels: 2,
            }),
        }
    }

    fn packet(stream: usize, dts: i64) -> Packet {
        Packet {
            stream,
            pts: dts,
            dts,
            duration: 1,
            key: true,
            data: vec![0],
        }
    }

    #[test]
    fn packets_are_written_in_time_order_across_time_scales() {
        let notes = Notes::default();
        let mut il = Interleaver::new(Box::new(NoteMux(notes.clone(), 0)), 2);
        il.set_stream(0, spec(1000));
        il.set_stream(1, spec(10));
        // Stream 0 in ms, stream 1 in tenths.
        for t in [0, 100, 200, 300] {
            il.push(packet(0, t)).unwrap();
        }
        for t in [0, 1, 2, 3] {
            il.push(packet(1, t)).unwrap();
        }
        il.finish(&Tags::default()).unwrap();
        let got = notes.0.lock().unwrap().clone();
        let times: Vec<f64> = got
            .iter()
            .map(|&(s, t)| {
                if s == 0 {
                    t as f64 / 1000.0
                } else {
                    t as f64 / 10.0
                }
            })
            .collect();
        assert!(times.windows(2).all(|w| w[0] <= w[1]), "{got:?}");
        assert_eq!(got.len(), 8);
    }

    #[test]
    fn nothing_is_written_until_every_stream_is_known() {
        let notes = Notes::default();
        let mut il = Interleaver::new(Box::new(NoteMux(notes.clone(), 0)), 2);
        il.set_stream(1, spec(1000));
        for t in 0..10 {
            il.push(packet(1, t * 100)).unwrap();
        }
        assert!(notes.0.lock().unwrap().is_empty());
        assert_eq!(il.waiting(), 10);
        il.set_stream(0, spec(1000));
        il.push(packet(0, 0)).unwrap();
        // Stream 0 is at 0 s: stream 1 may go as far, and no further.
        let written = notes.0.lock().unwrap().clone();
        assert_eq!(written, vec![(0, 0), (1, 0)]);
    }

    #[test]
    fn no_stream_runs_ahead_of_another() {
        let notes = Notes::default();
        let mut il = Interleaver::new(Box::new(NoteMux(notes.clone(), 0)), 2);
        il.set_stream(0, spec(1000));
        il.set_stream(1, spec(1000));
        il.push(packet(0, 0)).unwrap();
        for t in 0..30 {
            il.push(packet(1, t * 100)).unwrap();
        }
        // Stream 0 got to 0 s: stream 1 is written that far and held after.
        let got = notes.0.lock().unwrap().clone();
        assert_eq!(got, vec![(0, 0), (1, 0)]);
        assert_eq!(il.waiting(), 29);
        // A feed that says stream 0 has got to 2 s lets stream 1 go as far.
        il.reached(0, 2.0);
        il.push(packet(1, 3000)).unwrap();
        let got = notes.0.lock().unwrap().clone();
        assert_eq!(got.last(), Some(&(1, 2000)));
        // An ended stream holds nothing back.
        il.end(0).unwrap();
        assert_eq!(il.waiting(), 0);
    }

    /// A picture encoder that gives each packet `delay` frames after it
    /// takes the frame, and its stream only with its first packet.
    struct SlowEncoder {
        taken: i64,
        frames: i64,
        delay: i64,
        ended: bool,
    }

    /// Sound in packets of 1024 samples at 48 kHz.
    struct Sound {
        made: i64,
        packets: i64,
        ended: bool,
    }

    enum Fake {
        Picture(SlowEncoder),
        Sound(Sound),
    }

    impl SlowEncoder {
        fn give(il: &mut Interleaver, n: i64) -> Result<()> {
            if n == 0 {
                il.set_stream(0, spec(30));
            }
            il.push(packet(0, n))
        }
    }

    impl Pull for Fake {
        fn at(&self) -> f64 {
            match self {
                Fake::Picture(e) => e.taken as f64 / 30.0,
                Fake::Sound(s) => (s.made * 1024) as f64 / 48_000.0,
            }
        }
        fn ended(&self) -> bool {
            match self {
                Fake::Picture(e) => e.ended,
                Fake::Sound(s) => s.ended,
            }
        }
        fn pull(&mut self, il: &mut Interleaver, _cancel: &dyn Fn() -> bool) -> Result<()> {
            match self {
                Fake::Picture(e) if e.taken < e.frames => {
                    let out = e.taken - e.delay;
                    e.taken += 1;
                    if out >= 0 {
                        SlowEncoder::give(il, out)?;
                    }
                    Ok(())
                }
                Fake::Picture(e) => {
                    for n in (e.frames - e.delay).max(0)..e.frames {
                        SlowEncoder::give(il, n)?;
                    }
                    e.ended = true;
                    il.end(0)
                }
                Fake::Sound(s) if s.made < s.packets => {
                    il.push(packet(1, s.made * 1024))?;
                    s.made += 1;
                    Ok(())
                }
                Fake::Sound(s) => {
                    s.ended = true;
                    il.end(1)
                }
            }
        }
    }

    #[test]
    fn sound_held_behind_an_encoder_stays_bounded() {
        // Thirty seconds of picture from an encoder a second behind its
        // input, whose stream is known only with its first packet, and the
        // sound alongside: the sound waits for the picture, but only about
        // as long as the encoder's delay, however long the file.
        let notes = Notes::default();
        let mut il = Interleaver::new(Box::new(NoteMux(notes.clone(), 0)), 2);
        il.set_stream(1, spec(48_000));
        let sound_packets = 30 * 48_000 / 1024;
        let mut feeds = [
            Fake::Picture(SlowEncoder {
                taken: 0,
                frames: 900,
                delay: 30,
                ended: false,
            }),
            Fake::Sound(Sound {
                made: 0,
                packets: sound_packets,
                ended: false,
            }),
        ];
        let mut most = 0;
        while let Some(f) = next_feed(&mut feeds) {
            f.pull(&mut il, &|| false).unwrap();
            most = most.max(il.waiting());
        }
        il.finish(&Tags::default()).unwrap();
        // A second of sound is 47 packets; the picture's own are a few.
        assert!(most <= 60, "{most} packets waited");
        assert!(most >= 40, "the sound never waited: {most}");
        assert_eq!(notes.0.lock().unwrap().len(), 900 + sound_packets as usize);
    }
}
