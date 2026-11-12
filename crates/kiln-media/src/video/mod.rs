//! Video files: what they hold, read without a decoder; their packets and
//! frames, read through Media Foundation; writing them; and the job that
//! converts one ([`convert`]).

mod block_clock;
pub mod encode;
pub mod encoder_settings;
pub mod info;
pub mod job;
mod mux_mkv;
mod mux_mp4;
pub mod packet;
pub mod read;
#[cfg_attr(not(windows), allow(dead_code))]
mod reaper;
pub mod sample;
pub mod tune;

pub use crate::mf::d3d::{adapters, d3d, d3d_for, Adapter, D3d};
pub use encode::{encoder_adapter, encoder_adapter_known, EncodePlan, VideoEncoder};
pub use info::{Hdr, VideoCodec, VideoInfo};
#[doc(hidden)]
pub use job::Interleaver;
pub use job::{convert, JobReport};
pub use packet::{
    muxer, AudioParams, Container, Mux, Packet, StreamCodec, StreamSpec, VideoParams,
};
pub use read::{
    missing_extension, Area, Frame, FrameFormat, Luma, Pixel, ReadMode, ReadStream, Reader,
    MAX_HELD_FRAMES, TIMESCALE,
};
pub use sample::{sample, sampled};
pub use tune::{
    EncoderChoice, Measured, SoundChoice, VCodec, VStep, VideoSetting, VideoTarget, VideoTune,
};

use crate::error::{MediaError, Result};
use crate::types::SourceInfo;
use crate::{mkvread, mp4read};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// What a video file holds: the picture's facts from the container's headers,
/// and the sound's from part 1's [`crate::probe`].
///
/// A file with no video track is [`MediaError::NoVideo`]; one whose headers
/// can't be read, or are cut short, is `Damaged { code: "video" }`.
///
/// The sound never makes this fail. Its facts come from the same path: `None`
/// when the file is silent. When the file has sound that part 1 can't read
/// (a codec it doesn't know, one that needs Media Foundation Windows lacks,
/// or a track so damaged its reader errs or panics), `sound` is `None` and
/// `sound_unreadable` names the codec, so the caller can copy the sound or
/// refuse, but never drop it unseen.
pub fn probe(path: &Path) -> Result<VideoInfo> {
    let mut file = File::open(path).map_err(MediaError::Read)?;
    let mut video = info::read(&mut file)?;
    add_sound(&mut video, &mut file, || crate::table::probe(path));
    if video.video_kbps.is_none() {
        let bytes = file.metadata().map(|m| m.len()).unwrap_or(0);
        video.video_kbps = kbps_from_size(bytes, &video);
    }
    Ok(video)
}

/// How many sound tracks and subtitle tracks a video file holds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TrackCounts {
    pub sound: u16,
    pub subtitles: u16,
}

/// The sound tracks and subtitle tracks in the headers of an MP4, MOV, M4V,
/// MKV or WebM, counted without a decoder. A Matroska subtitle track is
/// type 17; an MP4's is a `sbtl` or `subt` track. A `text` track isn't one:
/// it can be a chapter list. `None` when the file is none of those, can't be
/// read, or is cut short in its headers.
pub fn track_counts(path: &Path) -> Option<TrackCounts> {
    let mut file = File::open(path).ok()?;
    let mut magic = [0u8; 4];
    file.seek(SeekFrom::Start(0)).ok()?;
    file.read_exact(&mut magic).ok()?;
    let mut counts = TrackCounts::default();
    let mut add = |sound: bool, subtitle: bool| {
        counts.sound = counts.sound.saturating_add(u16::from(sound));
        counts.subtitles = counts.subtitles.saturating_add(u16::from(subtitle));
    };
    if magic == [0x1A, 0x45, 0xDF, 0xA3] {
        for track in mkvread::read_headers(&mut file)?.tracks {
            add(
                track.kind == mkvread::TYPE_AUDIO,
                track.kind == mkvread::TYPE_SUBTITLE,
            );
        }
    } else {
        let moov = mp4read::read_moov_with(&mut file, mp4read::may_start_a_movie)?;
        for (kind, trak) in mp4read::boxes(&moov) {
            if &kind != b"trak" {
                continue;
            }
            let handler = mp4read::child(trak, b"mdia")
                .and_then(|m| mp4read::child(m, b"hdlr"))
                .and_then(|h| h.get(8..12));
            add(
                handler == Some(b"soun"),
                matches!(handler, Some(b"sbtl" | b"subt")),
            );
        }
    }
    Some(counts)
}

/// The sound's bitrate assumed when the file doesn't say (Opus in MKV).
const UNKNOWN_SOUND_KBPS: f64 = 128.0;

/// The picture's bitrate worked out from the whole file, for a container
/// that states none (R54): the file's bits over its length, less the
/// sound's bitrate (128 kbps when the sound's is unknown, none when the
/// file is silent), never below 0. `None` when the length is unknown.
fn kbps_from_size(bytes: u64, video: &VideoInfo) -> Option<u32> {
    let secs = video.duration.filter(|d| d.is_finite() && *d > 0.0)?;
    let sound = match (&video.sound, &video.sound_unreadable) {
        (Some(s), _) => s.kbps.map_or(UNKNOWN_SOUND_KBPS, f64::from),
        (None, Some(_)) => UNKNOWN_SOUND_KBPS,
        (None, None) => 0.0,
    };
    let kbps = bytes as f64 * 8.0 / secs / 1000.0 - sound;
    Some(kbps.max(0.0).round() as u32)
}

/// Puts what `probe` says of the sound of `file` into `video`. The sound
/// reader can panic on a damaged file (a Matroska cut short, say): that is
/// damaged sound, like any other error from it.
fn add_sound(
    video: &mut VideoInfo,
    file: &mut File,
    probe: impl FnOnce() -> Result<SourceInfo> + std::panic::UnwindSafe,
) {
    match std::panic::catch_unwind(probe) {
        Ok(Ok(sound)) => video.sound = Some(sound),
        Ok(Err(MediaError::NoAudio)) => {}
        Ok(Err(MediaError::Unsupported { codec })) => video.sound_unreadable = Some(codec),
        // A sound track the headers don't show is no sound at all.
        Ok(Err(_)) | Err(_) => video.sound_unreadable = crate::sym::sound_codec_name(file),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_container_with_no_bitrate_gets_one_from_its_size() {
        // Matroska states no bitrate: the file's, less the sound's.
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/video/h264.mkv");
        let v = probe(&path).unwrap();
        let bytes = std::fs::metadata(&path).unwrap().len() as f64;
        let secs = v.duration.unwrap();
        // Its Opus sound states no bitrate, so 128 kbps is taken off.
        assert_eq!(v.sound.as_ref().and_then(|s| s.kbps), None);
        let want = (bytes * 8.0 / secs / 1000.0 - 128.0).max(0.0).round() as u32;
        assert_eq!(v.video_kbps, Some(want));
        // Never below 0, and nothing without a length.
        let mut silent = v.clone();
        silent.sound = None;
        silent.sound_unreadable = None;
        assert_eq!(kbps_from_size(1000, &silent), Some(4));
        let mut loud = silent.clone();
        loud.sound_unreadable = Some("DTS".into());
        assert_eq!(kbps_from_size(1000, &loud), Some(0));
        loud.duration = None;
        assert_eq!(kbps_from_size(1000, &loud), None);
    }

    #[test]
    fn a_panic_in_the_sound_reader_is_damaged_sound() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/video/h264.mkv");
        let mut file = File::open(&path).unwrap();
        let mut video = info::read(&mut file).unwrap();
        add_sound(&mut video, &mut file, || panic!("a damaged track"));
        assert!(video.sound.is_none());
        assert!(video.sound_unreadable.is_some());
    }

    #[test]
    fn a_failed_sound_probe_with_no_sound_track_is_silence() {
        // hevc.mp4 has no sound track: a sound reader that fails on it has
        // found nothing to drop or copy.
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/video/hevc.mp4");
        let mut file = File::open(&path).unwrap();
        let mut video = info::read(&mut file).unwrap();
        add_sound(&mut video, &mut file, || {
            Err(MediaError::Damaged { code: "MP4".into() })
        });
        assert!(video.sound.is_none());
        assert_eq!(video.sound_unreadable, None);
        add_sound(&mut video, &mut file, || panic!("a damaged track"));
        assert_eq!(video.sound_unreadable, None);
    }
}
