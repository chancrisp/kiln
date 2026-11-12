//! The size estimate's sample (spec 3.3): two seconds from the middle of a
//! file, encoded as the job would encode them, so the estimate can be scaled
//! by what the encoder really spends on this picture.
//!
//! A sample is slow (it starts the encoder and decodes a few seconds), so it
//! runs on a worker, never on the window's thread. Its answer is kept for the
//! life of the process by the file's path, size and modified time, and the
//! codec and encoder it was made with: one sample per file and codec.

use super::encode::{self, encoder_adapter, VideoEncoder};
use super::read::{ReadMode, Reader, TIMESCALE};
use super::tune::{self, Measured, VideoSetting, VideoTune};
use super::{EncoderChoice, VCodec};
use crate::error::{MediaError, Result};
use crate::mf::d3d::d3d_for;
use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

/// How long a sample is, in seconds.
pub const SAMPLE_SECONDS: f64 = 2.0;

/// The most samples kept; past it the oldest are forgotten all at once.
const KEPT: usize = 256;

/// What a sample is kept by.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
struct Key {
    path: PathBuf,
    len: u64,
    modified: Option<SystemTime>,
    codec: VCodec,
    encoder: EncoderChoice,
}

impl Key {
    fn of(input: &Path, tune: &VideoTune) -> Option<Key> {
        let meta = std::fs::metadata(input).ok()?;
        Some(Key {
            path: std::path::absolute(input).unwrap_or_else(|_| input.to_path_buf()),
            len: meta.len(),
            modified: meta.modified().ok(),
            codec: tune.codec,
            encoder: tune.encoder,
        })
    }
}

static KEPT_SAMPLES: Mutex<Option<HashMap<Key, Measured>>> = Mutex::new(None);

/// The sample already made for `input` with `tune`'s codec and encoder, if
/// one is kept and the file hasn't changed since. Quick: it reads the file's
/// size and time only, so the window's thread may ask.
pub fn sampled(input: &Path, tune: &VideoTune) -> Option<Measured> {
    let key = Key::of(input, tune)?;
    let kept = KEPT_SAMPLES.lock().unwrap_or_else(|p| p.into_inner());
    kept.as_ref()?.get(&key).copied()
}

/// What the picture of `input` costs when encoded with `tune` (its codec,
/// encoder, picture size and frame rate) at its quality step (the High step
/// when `tune.setting` is not a quality): [`SAMPLE_SECONDS`] from the middle
/// of the file (all of a shorter one), encoded on the graphics card the job
/// would use, or on the processor when there is none.
///
/// Kept for later by the file and codec ([`sampled`]); a sample already kept
/// is given at once. Slow, so never call this on the window's thread.
///
/// `Ok(None)` when there is no frame to measure where the sample is taken
/// (the file's picture ends early, or Windows moved past it): no sample,
/// which the estimate does without. That is not an error to show.
pub fn sample(
    input: &Path,
    tune: &VideoTune,
    cancel: &dyn Fn() -> bool,
) -> Result<Option<Measured>> {
    if let Some(m) = sampled(input, tune) {
        return Ok(Some(m));
    }
    let Some(measured) = measure(input, tune, cancel)? else {
        return Ok(None);
    };
    if let Some(key) = Key::of(input, tune) {
        let mut kept = KEPT_SAMPLES.lock().unwrap_or_else(|p| p.into_inner());
        let map = kept.get_or_insert_with(HashMap::new);
        if map.len() >= KEPT {
            map.clear();
        }
        map.insert(key, measured);
    }
    Ok(Some(measured))
}

/// The quality step a sample is made at for `tune`.
fn quality_of(tune: &VideoTune) -> u8 {
    match tune.setting {
        Some(VideoSetting::Quality(q)) => q,
        _ => tune::HIGH,
    }
}

/// Makes the sample, keeping nothing. `None` when no frame falls in it.
fn measure(input: &Path, tune: &VideoTune, cancel: &dyn Fn() -> bool) -> Result<Option<Measured>> {
    let info = super::probe(input)?;
    let quality = quality_of(tune);
    let vtune = VideoTune {
        setting: Some(VideoSetting::Quality(quality)),
        ..*tune
    };
    // From the middle: the opening and closing seconds (titles, a fade) are
    // the least like the rest.
    let length = info.duration.filter(|d| d.is_finite() && *d > 0.0);
    let from = length.map_or(0.0, |d| ((d - SAMPLE_SECONDS) / 2.0).max(0.0));
    let start = (from * TIMESCALE as f64) as i64;
    let end = start + (SAMPLE_SECONDS * TIMESCALE as f64) as i64;

    // Decoded on the card whose encoder will start, as the job does (R34).
    let d3d = encoder_adapter(vtune.codec, vtune.encoder).and_then(d3d_for);
    let file = File::open(input).map_err(MediaError::Read)?;
    let mut reader = Reader::open(file, ReadMode::Decoded { d3d })?;
    if from > 0.0 {
        reader.seek(from)?;
    }
    let mut enc = VideoEncoder::new(&encode::plan(&info, &vtune), vtune.encoder, reader.d3d())?;
    let mut packets = Vec::new();
    let mut bytes = 0u64;
    let (mut first, mut last) = (None::<i64>, None::<i64>);
    while let Some(frame) = reader.next_frame()? {
        if cancel() {
            return Err(MediaError::Cancelled);
        }
        if frame.pts < start {
            continue;
        }
        if frame.pts >= end {
            break;
        }
        first.get_or_insert(frame.pts);
        last = Some(frame.pts.saturating_add(frame.duration.max(0)));
        enc.push_watched(frame, &mut packets, cancel)?;
        bytes += packets.drain(..).map(|p| p.data.len() as u64).sum::<u64>();
    }
    enc.finish_watched(&mut packets, cancel)?;
    bytes += packets.drain(..).map(|p| p.data.len() as u64).sum::<u64>();
    let seconds = match (first, last) {
        (Some(a), Some(b)) if b > a => (b - a) as f64 / TIMESCALE as f64,
        _ => return Ok(None),
    };
    let kbps = bytes as f64 * 8.0 / seconds / 1000.0;
    let table = tune::table_kbps(quality, &info, &vtune);
    Ok(Some(Measured {
        quality,
        kbps,
        scale: if table > 0.0 { kbps / table } else { 1.0 },
    }))
}
