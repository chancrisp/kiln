//! Encoding decoded frames: Media Foundation's video processor scales the
//! picture down, turns it upright and maps HDR to standard range, then an
//! H.264 or HEVC encoder writes it, on a graphics card or on the processor.
//!
//! What the probe (Task 1) found on this PC, and the rulings that follow:
//! - Microsoft's own DX12 encoders (vendor `VEN_1414`) bring the process
//!   down on an AMD device, so they are never used (R5). Every other hardware
//!   encoder is tried in Windows' merit order; one that won't start (AMD's
//!   here, `E_OUTOFMEMORY`) is skipped.
//! - Every encoder honours Quality mode, so the slider's steps use it (R6).
//!   A bitrate is tried as unconstrained VBR, then CBR. Microsoft's software
//!   H.264 encoder ignores the VBR bitrate but tracks a CBR one (within
//!   about 5% from 400 kbps up), so it gets CBR (R49).
//!   [`VideoEncoder::honours_bitrate`] says whether an encoder follows a
//!   bitrate (R33); every one Kiln uses now does.
//! - Each encoder that was measured gets its own key-frame spacing,
//!   B-frames, entropy coding and speed setting, read back as set
//!   ([`super::encoder_settings`], R85).
//! - HDR becomes standard range by the processor's tone map, on the card
//!   whose encoder works or in software, never on an AMD card's processor,
//!   whose curve is darker (R7). AMD's processor only scales, turns and
//!   crops a picture kept in 10 bits, with nothing tone-mapped (R87).
//! - The processor turns the picture by `MF_MT_VIDEO_ROTATION` on its input
//!   type, with its output at the turned size (R8).
//!
//! Decoded frames from a graphics card come from the decoder's own small
//! pool ([`MAX_HELD_FRAMES`](super::MAX_HELD_FRAMES)): each is let go as soon
//! as the processor has made its picture, before that is encoded.

// Off Windows only the stubs below use this module, so the helpers the
// Windows encoder calls go unused there.
#![cfg_attr(not(windows), allow(dead_code))]

use super::info::{Hdr, VideoInfo};
use super::tune::{
    self, written_fps, written_size, EncoderChoice, VCodec, VideoSetting, VideoTune,
};

/// How the picture is to be written: the codec, the size (upright, scaled,
/// even), the frame rate, the quality or bitrate, the dynamic range and bit
/// depth, and the turn that makes the source upright.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EncodePlan {
    pub codec: VCodec,
    pub width: u32,
    pub height: u32,
    /// The frame rate as written, for the encoder's rate control and its
    /// key-frame spacing; `None` when the file doesn't say.
    pub fps: Option<f64>,
    /// The limit frames are dropped to: a frame is kept when it crosses the
    /// next `1/max_fps` boundary. `None` keeps every frame with its own time,
    /// as for a variable-rate source with no limit.
    pub max_fps: Option<f64>,
    /// Never `Original` here: that is the top quality step when encoding.
    pub setting: VideoSetting,
    /// The dynamic range written: the source's for HEVC, standard range for
    /// H.264.
    pub hdr_out: Hdr,
    /// 8, or 10 for HEVC from a deeper or HDR source.
    pub bit_depth: u8,
    /// Degrees clockwise the source is turned to show it upright.
    pub rotate: u16,
    /// The pixel aspect ratio written, horizontal to vertical: the
    /// source's, turned with the picture (a quarter turn swaps it). The size
    /// is scaled by one factor on both sides, so it doesn't change (R37).
    pub sar: (u32, u32),
    /// The source's HDR10 static metadata (`mdcv` and `clli` bodies), kept
    /// when HDR is (R40).
    pub mastering: Option<[u8; 24]>,
    pub content_light: Option<[u8; 4]>,
    /// Standard range written as BT.601 (SMPTE 170M) rather than BT.709: a
    /// source that doesn't describe its colour and has under 720 lines, as
    /// players read it. Its samples are kept as they are (R68).
    pub bt601: bool,
}

/// The plan for encoding `info`'s picture as `tune` asks.
///
/// HDR is kept only for HEVC (in 10 bits), and only on a graphics card:
/// Windows' HEVC software encoder writes 8-bit Main only, so `Processor`,
/// and `Automatic` on a PC with no hardware HEVC encoder that starts, plan
/// 8-bit standard range (R35). H.264 is always 8-bit BT.709. Should
/// Automatic's hardware encoder fail before its first packet, the software
/// one takes over and [`VideoEncoder::stream`] says what is written; that
/// stream, not this plan, is what the muxer gets.
/// With no setting the encode uses the High step, and Original (which can't
/// copy when the picture is encoded) the top step. Fit under is the job's to
/// resolve into a setting first.
pub fn plan(info: &VideoInfo, tune: &VideoTune) -> EncodePlan {
    let (width, height) = written_size(info, tune);
    let setting = match tune.setting {
        None => VideoSetting::Quality(tune::HIGH),
        Some(VideoSetting::Original) => VideoSetting::Quality(tune::QUALITY_LAST),
        Some(s) => s,
    };
    let deep = info.bit_depth > 8 || info.hdr != Hdr::None;
    let (hdr_out, bit_depth) = match tune.codec {
        VCodec::H264 => (Hdr::None, 8),
        VCodec::Hevc if deep && hevc_ten_bit(tune.encoder) => (info.hdr, 10),
        VCodec::Hevc => (Hdr::None, 8),
    };
    let sar = if info.rotation % 180 == 90 {
        (info.sar.1, info.sar.0)
    } else {
        info.sar
    };
    let hdr = hdr_out != Hdr::None;
    // Frames are dropped only when the source can go over the limit: a
    // constant rate over it (within part 1's allowance, as Task 5 judges),
    // or any variable rate, whose fast stretches may.
    let max_fps = tune
        .max_fps
        .map(f64::from)
        .filter(|&m| info.variable_rate || info.fps.is_none_or(|f| f > m * tune::FPS_ALLOWANCE));
    EncodePlan {
        codec: tune.codec,
        width,
        height,
        fps: written_fps(info, tune),
        max_fps,
        setting,
        hdr_out,
        bit_depth,
        rotate: info.rotation % 360,
        sar,
        mastering: info.mastering.filter(|_| hdr),
        content_light: info.content_light.filter(|_| hdr),
        // An HDR source written as standard range is tone-mapped to BT.709,
        // whatever it says of its colour.
        bt601: info.hdr == Hdr::None && !info.colour_tagged && info.height < 720,
    }
}

/// True when HEVC can be written in 10 bits with `choice`: only by a
/// hardware encoder, and only where that card's own video processor can
/// make the frames (R87). `GraphicsCard` plans for one when there is no
/// encoder to ask (and fails without it).
fn hevc_ten_bit(choice: EncoderChoice) -> bool {
    match choice {
        EncoderChoice::Processor => false,
        EncoderChoice::GraphicsCard => {
            encoder_adapter(VCodec::Hevc, choice).is_none_or(card_processes)
        }
        EncoderChoice::Automatic => {
            encoder_adapter(VCodec::Hevc, choice).is_some_and(card_processes)
        }
    }
}

/// True when the card `luid` makes a D3D11 device with video support, so
/// its own video processor can make 10-bit frames (R87). Asked once per
/// card per process.
fn card_processes(luid: u64) -> bool {
    static KNOWN: std::sync::Mutex<Vec<(u64, bool)>> = std::sync::Mutex::new(Vec::new());
    let mut known = KNOWN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&(_, yes)) = known.iter().find(|(l, _)| *l == luid) {
        return yes;
    }
    let yes = crate::mf::d3d::d3d_for(luid).is_some();
    known.push((luid, yes));
    yes
}

/// `plan` written in 8-bit standard range, HDR tone-mapped to BT.709 as
/// for H.264 (R35, R87).
fn standard_range(plan: &EncodePlan) -> EncodePlan {
    EncodePlan {
        bit_depth: 8,
        hdr_out: Hdr::None,
        ..*plan
    }
}

/// True when frames must go through RGB on their way through Microsoft's
/// video processor in software (`software`) (Task 10c). From a 10-bit
/// picture (`ten_bit_in`) it writes only the luma of a YUV one, unless it
/// copies it unchanged. From an 8-bit one in another colour (`recoloured`:
/// its matrix, primaries or transfer aren't the output's) it writes only
/// the luma from BT.2020 or HDR, and from BT.601 copies the samples
/// unconverted, so they are read as BT.709. Only an 8-bit picture can be
/// made through RGB (`out_bits`). An 8-bit picture in the output's own
/// colour comes out whole (a change of range included), and the card's
/// processor always converts.
pub(crate) fn needs_detour(
    software: bool,
    out_bits: u8,
    ten_bit_in: bool,
    recoloured: bool,
) -> bool {
    software && out_bits == 8 && (ten_bit_in || recoloured)
}

/// True when the video processor may run on the card the frames are
/// decoded on, for `plan` as finally written. Any card but AMD's (`amd`).
/// AMD's only when the plan keeps 10 bits: then nothing is tone-mapped,
/// only scaled and turned (R87). It tone-maps HDR to standard range darker
/// (R7), so an 8-bit plan, the one a 10-bit plan becomes for a software
/// encoder included, runs in software there.
pub(crate) fn processor_on_card(amd: bool, plan: &EncodePlan) -> bool {
    !amd || plan.bit_depth > 8
}

/// True when a frame (`frame` in size, its picture `shown` as x, y, width,
/// height) comes out of the video processor as it went in: no turn, no
/// crop, written at the frame's size, in the same colour. Microsoft's
/// processor in software keeps a 10-bit picture whole only then (Task 10c).
pub(crate) fn unchanged(
    rotate: u16,
    frame: (u32, u32),
    shown: (u32, u32, u32, u32),
    written: (u32, u32),
    same_colour: bool,
) -> bool {
    rotate == 0 && shown == (0, 0, frame.0, frame.1) && written == frame && same_colour
}

impl VCodec {
    /// The codec's name, as the copy says it.
    pub(crate) fn name(self) -> &'static str {
        match self {
            VCodec::H264 => "H.264",
            VCodec::Hevc => "HEVC",
        }
    }
}

#[cfg(windows)]
pub use win::{encoder_adapter, encoder_adapter_known, VideoEncoder};

/// What a look for a codec's graphics card encoder found
/// ([`encoder_adapter`]), and when, if ever, it is to be made again.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq)]
struct Found {
    card: Option<u64>,
    again: Option<std::time::Instant>,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl Found {
    /// True while the answer stands at `now`.
    fn stands(&self, now: std::time::Instant) -> bool {
        self.again.is_none_or(|t| now < t)
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use crate::error::{MediaError, Result};
    use crate::mf::com::{self, failed, get_blob, get_u32, sample_bytes, Mf};
    use crate::mf::d3d::{adapters, D3d};
    use crate::video::encoder_settings::{
        self, key_interval, settings_in_use, EncoderFamily, EncoderSettings, ReadBack,
    };
    use crate::video::packet::{Packet, StreamCodec, StreamSpec, VideoParams};
    use crate::video::read::{
        annex_b_with_colour, avcc_from_annex_b, hvcc_from_annex_b, Area, Frame, FrameFormat,
        NalCleaner, Pixel, Reorder, TIMESCALE,
    };
    use crate::video::reaper::{self, Reaper};
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};
    use windows::core::{Interface, GUID};
    use windows::Win32::Graphics::Direct3D11::D3D11_BIND_RENDER_TARGET;
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    use windows::Win32::System::Variant::VARIANT;

    /// AMD's PCI vendor ID: its video processor's tone map is darker (R7).
    const AMD: u32 = 0x1002;
    /// How long an asynchronous encoder may go without a word before it is
    /// taken to have stopped.
    const SILENCE: Duration = Duration::from_secs(30);
    /// How long a look for the graphics card's encoder that found none for a
    /// passing reason (a hardware encoder listed that wouldn't start, or
    /// Media Foundation that wouldn't) stands before it is made again.
    const LOOK_AGAIN: Duration = Duration::from_secs(60);

    /// True for a cancel, which is never an encoder's failure to fall back
    /// from.
    fn cancelled(e: &MediaError) -> bool {
        matches!(e, MediaError::Cancelled)
    }
    /// Media Foundation's DX12 encoders (vendor `VEN_1414`): R5.
    const MICROSOFT: u32 = 0x1414;
    /// Processed frames kept for the software encoder while a hardware one
    /// has given nothing yet (Automatic falls back with them). An encoder
    /// that takes more than this before its first packet can't fall back.
    const HOLD: usize = 16;
    /// The most processed frames on the card at once: those held above, the
    /// encoder's own queue and a few in hand.
    const POOL: u32 = HOLD as u32 + 24;

    /// True when, in a debug build, `KILN_HIDE_HW_ENCODERS` asks to act as if
    /// this PC had no hardware encoder.
    fn hardware_hidden() -> bool {
        #[cfg(debug_assertions)]
        if std::env::var_os("KILN_HIDE_HW_ENCODERS").is_some() {
            return true;
        }
        false
    }

    /// True when, in a debug build, `KILN_MUTE_HW_ENCODER` asks a hardware
    /// encoder to say nothing at all, as one that has stopped answering.
    fn hardware_mutes() -> bool {
        #[cfg(debug_assertions)]
        if std::env::var_os("KILN_MUTE_HW_ENCODER").is_some() {
            return true;
        }
        false
    }

    /// True when, in a debug build, `KILN_FAIL_HW_ENCODER` asks a hardware
    /// encoder to fail on its first frame.
    fn hardware_fails() -> bool {
        #[cfg(debug_assertions)]
        if std::env::var_os("KILN_FAIL_HW_ENCODER").is_some() {
            return true;
        }
        false
    }

    fn subtype(codec: VCodec) -> GUID {
        match codec {
            VCodec::H264 => MFVideoFormat_H264,
            VCodec::Hevc => MFVideoFormat_HEVC,
        }
    }

    /// A hardware encoder Windows lists, and the card it is on.
    struct Candidate {
        activate: IMFActivate,
        luid: Option<u64>,
    }

    /// The hardware encoders for `codec`, in Windows' merit order, without
    /// Microsoft's own DX12 encoders (R5). Each is matched to its card by
    /// the LUID it gives, else by its vendor: the NVIDIA encoders give their
    /// LUID only through `MFTEnum2`.
    fn hardware_encoders(codec: VCodec) -> Vec<Candidate> {
        if hardware_hidden() {
            return Vec::new();
        }
        let cards = adapters();
        com::enum_transforms(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_ASYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER,
            None,
            Some((MFMediaType_Video, subtype(codec))),
            None,
        )
        .into_iter()
        .filter_map(|activate| {
            let vendor = string(&activate, &MFT_ENUM_HARDWARE_VENDOR_ID_Attribute).and_then(|v| {
                u32::from_str_radix(v.to_ascii_uppercase().strip_prefix("VEN_")?, 16).ok()
            });
            // R5: never Microsoft's DX12 encoders, which bring the process
            // down on an AMD device. They list no CLSID, so one that names
            // no vendor either is left out too; never activated to find out.
            let name = string(&activate, &MFT_FRIENDLY_NAME_Attribute).unwrap_or_default();
            let clsid = activate_guid(&activate, &MFT_TRANSFORM_CLSID_Attribute);
            if vendor == Some(MICROSOFT)
                || name.contains("DX12")
                || name.starts_with("Microsoft")
                || (vendor.is_none() && clsid.is_none())
            {
                return None;
            }
            let luid = adapter_luid(&activate).or_else(|| {
                cards
                    .iter()
                    .find(|a| Some(a.vendor) == vendor)
                    .map(|a| a.luid)
            });
            Some(Candidate { activate, luid })
        })
        .collect()
    }

    fn string(a: &IMFAttributes, key: &GUID) -> Option<String> {
        // SAFETY: the buffer is sized from GetStringLength, with room for the
        // terminating NUL.
        unsafe {
            let n = a.GetStringLength(key).ok()? as usize;
            let mut v = vec![0u16; n + 1];
            a.GetString(key, &mut v, None).ok()?;
            Some(String::from_utf16_lossy(&v[..n]))
        }
    }

    /// The LUID an activation object names, as a number or as a blob.
    fn adapter_luid(a: &IMFAttributes) -> Option<u64> {
        // SAFETY: a plain attribute read.
        unsafe { a.GetUINT64(&MFT_ENUM_ADAPTER_LUID) }
            .ok()
            .or_else(|| {
                let b = get_blob(a, &MFT_ENUM_ADAPTER_LUID).filter(|b| b.len() >= 8)?;
                let lo = u32::from_le_bytes(b[..4].try_into().ok()?) as u64;
                let hi = u32::from_le_bytes(b[4..8].try_into().ok()?) as u64;
                Some((hi << 32) | lo)
            })
    }

    /// The LUID of the graphics card whose hardware encoder for `codec` will
    /// start, the first in merit order; `None` for `Processor`, or when no
    /// hardware encoder starts.
    ///
    /// The job opens its reader and the encoder on `d3d_for` this card, so
    /// decoded textures, the processor and the encoder share one device.
    ///
    /// Found once per codec for the life of the process (listing the
    /// encoders and starting one took 180 to 430 ms here), so Automatic's
    /// [`plan`](super::plan) and the job pay for it once between them. A
    /// look that found none only because a listed encoder or Media
    /// Foundation wouldn't start is made again after [`LOOK_AGAIN`]; one that
    /// found a card, or no hardware encoder at all, stands. A graphics card
    /// added, removed or changed while Kiln runs is otherwise seen after a
    /// restart. `KILN_HIDE_HW_ENCODERS` is read on every call, before the
    /// stored answer, so a test can set it at any time.
    pub fn encoder_adapter(codec: VCodec, choice: EncoderChoice) -> Option<u64> {
        if choice == EncoderChoice::Processor || hardware_hidden() {
            return None;
        }
        let (answer, looking) = found(codec);
        let standing = || {
            let a = answer.lock().unwrap_or_else(|p| p.into_inner());
            a.as_ref()
                .filter(|f| f.stands(Instant::now()))
                .map(|f| f.card)
        };
        if let Some(card) = standing() {
            return card;
        }
        // One look at a time; one made while this waited stands.
        let _one = looking.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(card) = standing() {
            return card;
        }
        let (card, passing) = first_encoder_card(codec);
        let look = Found {
            card,
            again: (card.is_none() && passing).then(|| Instant::now() + LOOK_AGAIN),
        };
        *answer.lock().unwrap_or_else(|p| p.into_inner()) = Some(look);
        card
    }

    /// [`encoder_adapter`]'s answer when it is already known, without
    /// starting anything: `None` until the graphics card has been looked
    /// for (R82). For `Processor`, and with the hardware hidden, the answer
    /// is known: no card.
    pub fn encoder_adapter_known(codec: VCodec, choice: EncoderChoice) -> Option<Option<u64>> {
        if choice == EncoderChoice::Processor || hardware_hidden() {
            return Some(None);
        }
        // The last answer, even one due to be looked at again: this never
        // starts anything, and never waits for a look under way.
        let (answer, _) = found(codec);
        let a = answer.try_lock().ok()?;
        a.as_ref().map(|f| f.card)
    }

    /// Where [`encoder_adapter`] keeps its answer for `codec`, and the lock
    /// one look at a time holds.
    fn found(codec: VCodec) -> (&'static Mutex<Option<Found>>, &'static Mutex<()>) {
        static ANSWER: [Mutex<Option<Found>>; 2] = [Mutex::new(None), Mutex::new(None)];
        static LOOKING: [Mutex<()>; 2] = [Mutex::new(()), Mutex::new(())];
        let i = match codec {
            VCodec::H264 => 0,
            VCodec::Hevc => 1,
        };
        (&ANSWER[i], &LOOKING[i])
    }

    /// The card of the first hardware encoder for `codec` that starts, and
    /// whether finding none was for a passing reason: Media Foundation
    /// wouldn't start, or an encoder was listed that wouldn't.
    fn first_encoder_card(codec: VCodec) -> (Option<u64>, bool) {
        let Ok(_mf) = Mf::start() else {
            return (None, true);
        };
        let mut refused = false;
        let card = hardware_encoders(codec).into_iter().find_map(|c| {
            let luid = c.luid?;
            // SAFETY: activates the transform the enumeration described, and
            // shuts it down again.
            unsafe {
                let Ok(t) = c.activate.ActivateObject::<IMFTransform>() else {
                    refused = true;
                    return None;
                };
                // As `Drop for Encoder`: shut down, then let go on the
                // reaper's thread, which waits for the shutdown to complete.
                if let Ok(s) = t.cast::<IMFShutdown>() {
                    let _ = s.Shutdown();
                }
                let _ = c.activate.ShutdownObject();
                reap(t, c.activate, None);
            }
            Some(luid)
        });
        (card, refused)
    }

    /// The last references to a hardware encoder that has been shut down,
    /// with Media Foundation kept started while they are held. Fields go in
    /// order: the encoder first, then Media Foundation.
    struct Reaped {
        /// Asked whether the shutdown has completed; `None` when the
        /// transform has no `IMFShutdown`, so there is nothing to wait for
        /// past the floor.
        shutdown: Option<IMFShutdown>,
        _t: IMFTransform,
        _activate: IMFActivate,
        _events: Option<IMFMediaEventGenerator>,
        _mf: Option<Mf>,
    }

    // SAFETY: Media Foundation's transforms, activation objects and event
    // queues are free-threaded; the reaper only asks the shutdown status and
    // releases them. `Mf` is a start-up count, balanced on any thread.
    unsafe impl Send for Reaped {}

    impl Reaped {
        /// True once the transform says its shutdown has completed, or it
        /// has no `IMFShutdown` to say. A failed question is not settled, so
        /// the cap decides. Many asynchronous transforms report
        /// `MFSHUTDOWN_COMPLETED` as soon as `Shutdown()` returns, with
        /// their own work still to run: the floor is the real protection.
        fn settled(&self) -> bool {
            match &self.shutdown {
                None => true,
                // SAFETY: a plain status read on a shut-down transform.
                Some(s) => matches!(unsafe { s.GetShutdownStatus() }, Ok(MFSHUTDOWN_COMPLETED)),
            }
        }
    }

    /// The one reaper for the process, started the first time it is needed;
    /// `None` if its thread couldn't be started.
    fn reaper() -> Option<&'static Reaper<Reaped>> {
        static REAPER: OnceLock<Option<Reaper<Reaped>>> = OnceLock::new();
        REAPER
            .get_or_init(|| {
                Reaper::start(
                    "kiln-encoder-reaper",
                    reaper::RULES,
                    reaper::TICK,
                    com::com,
                    Reaped::settled,
                )
            })
            .as_ref()
    }

    /// Lets go of a hardware encoder that has been shut down, later, on the
    /// reaper's thread (see [`super::reaper`]), so the caller does not wait.
    /// NVIDIA's transform runs a work item of its own after its shutdown,
    /// which enters a lock in the freed transform if the last reference has
    /// gone by then (seen in nvEncMFThevcx.dll on an RTWorkQ thread): R42.
    /// The reaper holds it until its shutdown has completed and
    /// [`reaper::RULES`]' floor has passed, or until the cap. Many
    /// transforms say their shutdown has completed as soon as `Shutdown()`
    /// returns, so the floor is what keeps the work item safe.
    ///
    /// Media Foundation is started here, on the caller's thread while the
    /// caller still has it started, and stays started until the encoder is
    /// let go. If the reaper can't be had, the references are kept for the
    /// life of the process: a little memory, never a crash.
    fn reap(t: IMFTransform, activate: IMFActivate, events: Option<IMFMediaEventGenerator>) {
        let held = Reaped {
            shutdown: t.cast::<IMFShutdown>().ok(),
            _t: t,
            _activate: activate,
            _events: events,
            _mf: Mf::start().ok(),
        };
        let kept = match reaper() {
            Some(r) => r.bury(held).err(),
            None => Some(held),
        };
        if let Some(held) = kept {
            std::mem::forget(held);
        }
    }

    // -----------------------------------------------------------------------
    // Media types

    fn set_u32(t: &IMFMediaType, key: &GUID, v: u32) -> windows::core::Result<()> {
        // SAFETY: a plain attribute write.
        unsafe { t.SetUINT32(key, v) }
    }

    fn set_u64(t: &IMFMediaType, key: &GUID, v: u64) -> windows::core::Result<()> {
        // SAFETY: a plain attribute write.
        unsafe { t.SetUINT64(key, v) }
    }

    /// A frame rate as Media Foundation's ratio, numerator above denominator.
    fn rate(fps: f64) -> u64 {
        let fps = if fps.is_finite() && fps > 0.0 {
            fps
        } else {
            30.0
        };
        let (mut n, mut d) = ((fps * 1000.0).round().max(1.0) as u64, 1000u64);
        let (mut a, mut b) = (n, d);
        while b != 0 {
            (a, b) = (b, a % b);
        }
        n /= a;
        d /= a;
        (n << 32) | d
    }

    /// The plan's frame rate, or 30 when the file doesn't say.
    fn nominal_fps(plan: &EncodePlan) -> f64 {
        plan.fps
            .filter(|f| f.is_finite() && *f > 0.0)
            .unwrap_or(30.0)
    }

    /// A pixel aspect ratio as Media Foundation's ratio; square when unset.
    fn ratio((h, v): (u32, u32)) -> u64 {
        if h == 0 || v == 0 {
            (1 << 32) | 1
        } else {
            ((h as u64) << 32) | v as u64
        }
    }

    /// Sets the size, rate, pixel aspect and progressive scan on `t`.
    fn set_picture(t: &IMFMediaType, plan: &EncodePlan) -> windows::core::Result<()> {
        set_u64(
            t,
            &MF_MT_FRAME_SIZE,
            ((plan.width as u64) << 32) | plan.height as u64,
        )?;
        set_u64(t, &MF_MT_FRAME_RATE, rate(nominal_fps(plan)))?;
        set_u64(t, &MF_MT_PIXEL_ASPECT_RATIO, ratio(plan.sar))?;
        set_u32(
            t,
            &MF_MT_INTERLACE_MODE,
            MFVideoInterlace_Progressive.0 as u32,
        )
    }

    /// Sets the written picture's colour: BT.709 for standard range (BT.601
    /// when the plan says, R68), BT.2020 with PQ or HLG for HDR; limited
    /// range either way. Media Foundation has no BT.601 transfer; its curve
    /// is BT.709's.
    fn set_colour(t: &IMFMediaType, plan: &EncodePlan) -> windows::core::Result<()> {
        let (transfer, primaries, matrix) = match plan.hdr_out {
            Hdr::None if plan.bt601 => (
                MFVideoTransFunc_709.0,
                MFVideoPrimaries_SMPTE170M.0,
                MFVideoTransferMatrix_BT601.0,
            ),
            Hdr::None => (
                MFVideoTransFunc_709.0,
                MFVideoPrimaries_BT709.0,
                MFVideoTransferMatrix_BT709.0,
            ),
            Hdr::Pq => (
                MFVideoTransFunc_2084.0,
                MFVideoPrimaries_BT2020.0,
                MFVideoTransferMatrix_BT2020_10.0,
            ),
            Hdr::Hlg => (
                MFVideoTransFunc_HLG.0,
                MFVideoPrimaries_BT2020.0,
                MFVideoTransferMatrix_BT2020_10.0,
            ),
        };
        set_u32(t, &MF_MT_TRANSFER_FUNCTION, transfer as u32)?;
        set_u32(t, &MF_MT_VIDEO_PRIMARIES, primaries as u32)?;
        set_u32(t, &MF_MT_YUV_MATRIX, matrix as u32)?;
        set_u32(
            t,
            &MF_MT_VIDEO_NOMINAL_RANGE,
            MFNominalRange_16_235.0 as u32,
        )
    }

    /// True when `t` says its colour is BT.2020's or its transfer is HDR's.
    fn wide_colour(t: &IMFMediaType) -> bool {
        get_u32(t, &MF_MT_VIDEO_PRIMARIES) == Some(MFVideoPrimaries_BT2020.0 as u32)
            || hdr_transfer(t)
    }

    /// True when `t`'s transfer is PQ or HLG.
    fn hdr_transfer(t: &IMFMediaType) -> bool {
        get_u32(t, &MF_MT_TRANSFER_FUNCTION).is_some_and(|v| {
            v == MFVideoTransFunc_2084.0 as u32 || v == MFVideoTransFunc_HLG.0 as u32
        })
    }

    /// Fills in the colour a decoder leaves unstated, as players take it:
    /// limited range, BT.709 transfer and primaries, and the BT.601 matrix
    /// under 720 lines (BT.709 from there). Unstated, the processor on the
    /// graphics card stretches the levels (NVIDIA here: out = 1.2 x in - 24).
    /// When the plan writes an untagged source as BT.601 (R68), the
    /// primaries are BT.601's too, so the processor keeps the samples. A
    /// picture with BT.2020 primaries or an HDR transfer has BT.2020's
    /// matrix, whatever its size: Microsoft's HEVC decoder states the
    /// primaries and transfer of HDR10 but not its matrix.
    fn set_unstated_colour(
        t: &IMFMediaType,
        height: u32,
        plan: &EncodePlan,
    ) -> windows::core::Result<()> {
        let matrix = if wide_colour(t) {
            MFVideoTransferMatrix_BT2020_10.0
        } else if height < 720 {
            MFVideoTransferMatrix_BT601.0
        } else {
            MFVideoTransferMatrix_BT709.0
        };
        let primaries = if plan.bt601 {
            MFVideoPrimaries_SMPTE170M.0
        } else {
            MFVideoPrimaries_BT709.0
        };
        for (key, v) in [
            (&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0),
            (&MF_MT_YUV_MATRIX, matrix),
            (&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0),
            (&MF_MT_VIDEO_PRIMARIES, primaries),
        ] {
            if get_u32(t, key).is_none_or(|v| v == 0) {
                set_u32(t, key, v as u32)?;
            }
        }
        Ok(())
    }

    /// The uncompressed picture between the processor and the encoder.
    fn raw_subtype(plan: &EncodePlan) -> GUID {
        if plan.bit_depth > 8 {
            MFVideoFormat_P010
        } else {
            MFVideoFormat_NV12
        }
    }

    /// The processor's output and the encoder's input: the picture as
    /// written, uncompressed.
    fn raw_type(plan: &EncodePlan) -> windows::core::Result<IMFMediaType> {
        // SAFETY: plain calls making a new type.
        let t = unsafe { MFCreateMediaType() }?;
        // SAFETY: plain attribute writes.
        unsafe {
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            t.SetGUID(&MF_MT_SUBTYPE, &raw_subtype(plan))?;
        }
        set_picture(&t, plan)?;
        let bytes = if plan.bit_depth > 8 { 2 } else { 1 };
        set_u32(&t, &MF_MT_DEFAULT_STRIDE, plan.width * bytes)?;
        set_u32(&t, &MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
        set_colour(&t, plan)?;
        Ok(t)
    }

    /// The processor's picture in software when it can't make the plan's
    /// YUV picture itself (see [`Detour`]): 8-bit RGB at full range, in the
    /// written picture's size, primaries and transfer, rows top down.
    fn rgb_type(plan: &EncodePlan) -> windows::core::Result<IMFMediaType> {
        let t = raw_type(plan)?;
        // SAFETY: plain attribute calls on a type made here.
        unsafe {
            t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
            t.DeleteItem(&MF_MT_YUV_MATRIX)?;
        }
        set_u32(&t, &MF_MT_DEFAULT_STRIDE, plan.width * 4)?;
        set_u32(
            &t,
            &MF_MT_VIDEO_NOMINAL_RANGE,
            MFNominalRange_0_255.0 as u32,
        )?;
        Ok(t)
    }

    /// A copy of a media type, to change without touching the original.
    fn copy_type(t: &IMFMediaType) -> windows::core::Result<IMFMediaType> {
        // SAFETY: plain calls on a new media type.
        unsafe {
            let c = MFCreateMediaType()?;
            t.CopyAllItems(&c)?;
            Ok(c)
        }
    }

    // -----------------------------------------------------------------------
    // Samples

    /// A system-memory copy of a sample whose buffer may be a texture, with
    /// the picture's rows packed (no padding), and the same times.
    fn to_memory(s: &IMFSample) -> Result<IMFSample> {
        let oops = |e| failed("can't copy a frame off the graphics card", e);
        // SAFETY: the 2D buffer copies itself into the memory buffer, which
        // is locked while it is filled and sized to what the copy needs.
        unsafe {
            let b = s.GetBufferByIndex(0).map_err(oops)?;
            let b2: IMF2DBuffer = b.cast().map_err(oops)?;
            let len = b2.GetContiguousLength().map_err(oops)?;
            let m = MFCreateMemoryBuffer(len).map_err(oops)?;
            let mut p = std::ptr::null_mut();
            m.Lock(&mut p, None, None).map_err(oops)?;
            let copied = b2.ContiguousCopyTo(std::slice::from_raw_parts_mut(p, len as usize));
            let _ = m.Unlock();
            copied.map_err(oops)?;
            m.SetCurrentLength(len).map_err(oops)?;
            let out = MFCreateSample().map_err(oops)?;
            out.AddBuffer(&m).map_err(oops)?;
            if let Ok(t) = s.GetSampleTime() {
                let _ = out.SetSampleTime(t);
            }
            if let Ok(d) = s.GetSampleDuration() {
                let _ = out.SetSampleDuration(d);
            }
            Ok(out)
        }
    }

    /// Runs `ProcessOutput` for stream 0 with `sample` (`None` when the
    /// transform gives its own) and gives back what came out.
    fn process_output(
        t: &IMFTransform,
        sample: Option<IMFSample>,
    ) -> windows::core::Result<Option<IMFSample>> {
        // SAFETY: the output buffer array has one entry, for stream 0. The
        // sample and events it holds afterwards are taken back and dropped.
        unsafe {
            let mut db = [MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: std::mem::ManuallyDrop::new(sample),
                dwStatus: 0,
                pEvents: std::mem::ManuallyDrop::new(None),
            }];
            let mut status = 0u32;
            let r = t.ProcessOutput(0, &mut db, &mut status);
            let sample = std::mem::ManuallyDrop::take(&mut db[0].pSample);
            drop(std::mem::ManuallyDrop::take(&mut db[0].pEvents));
            r.map(|_| sample)
        }
    }

    /// True when a transform's output stream gives its own samples.
    fn provides(info: &MFT_OUTPUT_STREAM_INFO) -> bool {
        info.dwFlags
            & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32
                | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0 as u32)
            != 0
    }

    /// A sample with an empty memory buffer of `size` bytes.
    fn memory_sample(size: u32) -> windows::core::Result<IMFSample> {
        // SAFETY: plain calls making a sample and its buffer.
        unsafe {
            let s = MFCreateSample()?;
            s.AddBuffer(&MFCreateMemoryBuffer(size)?)?;
            Ok(s)
        }
    }

    // -----------------------------------------------------------------------
    // The video processor

    /// A second video processor in software, from [`rgb_type`] to the
    /// plan's YUV picture.
    ///
    /// Microsoft's video processor in software (no D3D) writes only the luma
    /// plane of a YUV picture (NV12, P010 or P016) it makes from a 10-bit,
    /// BT.2020 (even 8-bit standard range) or HDR one, unless it copies the
    /// picture unchanged (see [`needs_detour`]): the chroma plane is
    /// left as it was given, which made HDR to standard range on the
    /// processor green (Task 10c). From 8-bit BT.601 to BT.709 it copies
    /// the samples, unconverted. Its RGB pictures are whole, and RGB to
    /// NV12 is a plain 8-bit conversion it does fully. Through 8-bit RGB the
    /// tone-mapped picture's mean Y, U and V are within a level of the
    /// graphics card's on the harness's HDR clip.
    struct Detour {
        t: IMFTransform,
        out_size: u32,
        provides: bool,
        /// The RGB picture the first processor writes, one for every frame.
        rgb: Option<IMFSample>,
    }

    impl Detour {
        fn new(plan: &EncodePlan, out_type: &IMFMediaType) -> windows::core::Result<Detour> {
            // SAFETY: plain COM creation and transform calls with types
            // made here.
            unsafe {
                let t: IMFTransform =
                    CoCreateInstance(&CLSID_VideoProcessorMFT, None, CLSCTX_INPROC_SERVER)?;
                t.GetAttributes()?.SetUINT32(&MF_XVP_DISABLE_FRC, 1)?;
                t.SetInputType(0, &rgb_type(plan)?, 0)?;
                t.SetOutputType(0, out_type, 0)?;
                let info = t.GetOutputStreamInfo(0)?;
                t.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
                Ok(Detour {
                    t,
                    out_size: info.cbSize,
                    provides: provides(&info),
                    rgb: None,
                })
            }
        }

        /// The sample the first processor writes its RGB picture of
        /// `size` bytes into: the same one every frame, as the second has
        /// read the last before the next is written.
        fn rgb_sample(&mut self, size: u32) -> windows::core::Result<IMFSample> {
            if let Some(s) = &self.rgb {
                return Ok(s.clone());
            }
            let s = memory_sample(size)?;
            self.rgb = Some(s.clone());
            Ok(s)
        }

        /// The plan's YUV picture of the RGB picture `s`, at its time.
        fn process(&self, s: &IMFSample) -> Result<IMFSample> {
            let oops = |e| failed("the video processor failed", e);
            // SAFETY: plain transform calls on a sample we hold.
            unsafe { self.t.ProcessInput(0, s, 0) }.map_err(oops)?;
            let given = if self.provides {
                None
            } else {
                Some(memory_sample(self.out_size.max(1)).map_err(oops)?)
            };
            process_output(&self.t, given)
                .map_err(oops)?
                .ok_or_else(|| MediaError::Encoder("the video processor gave no picture".into()))
        }
    }

    impl Drop for Detour {
        fn drop(&mut self) {
            com::com();
            // SAFETY: a plain message to the transform.
            unsafe {
                let _ = self.t.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
            }
        }
    }

    /// `CLSID_VideoProcessorMFT`, one frame in and one out: crops a padded
    /// frame to its picture, turns it, scales it, and converts its colour.
    struct Processor {
        t: IMFTransform,
        /// The card it runs on; `None` in software.
        gpu: Option<D3d>,
        /// Textures for its output on the card, from a pool of [`POOL`].
        pool: Option<IMFVideoSampleAllocatorEx>,
        plan: EncodePlan,
        out_type: IMFMediaType,
        out_size: u32,
        provides: bool,
        /// The frames' type the input is set to; a frame of another type
        /// sets it again.
        input: Option<IMFMediaType>,
        /// True when frames are on a card the processor doesn't use: each is
        /// copied to memory first.
        copy_in: bool,
        /// The HDR10 light levels of the input, as a `clli` body.
        content_light: Option<[u8; 4]>,
        streaming: bool,
        /// In software, for frames it can't make YUV from itself: this
        /// processor makes RGB and the detour the plan's picture.
        detour: Option<Detour>,
    }

    impl Processor {
        fn new(plan: &EncodePlan, gpu: Option<D3d>, copy_in: bool) -> Result<Processor> {
            let oops = |e| failed("the video processor won't start", e);
            // SAFETY: plain COM creation and a message with a live manager.
            let t: IMFTransform = unsafe {
                let t: IMFTransform =
                    CoCreateInstance(&CLSID_VideoProcessorMFT, None, CLSCTX_INPROC_SERVER)
                        .map_err(oops)?;
                // One picture out for each in, at its own time: the
                // frame-rate limit is Kiln's (by dropping), never the
                // processor's own conversion. On the card, its pictures go
                // into Kiln's pool of textures.
                let attrs = t.GetAttributes().map_err(oops)?;
                attrs.SetUINT32(&MF_XVP_DISABLE_FRC, 1).map_err(oops)?;
                if let Some(d) = &gpu {
                    attrs
                        .SetUINT32(&MF_XVP_CALLER_ALLOCATES_OUTPUT, 1)
                        .map_err(oops)?;
                    t.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, d.manager().as_raw() as usize)
                        .map_err(oops)?;
                }
                t
            };
            Ok(Processor {
                t,
                gpu,
                pool: None,
                plan: *plan,
                out_type: raw_type(plan).map_err(oops)?,
                out_size: 0,
                provides: false,
                input: None,
                copy_in,
                content_light: None,
                streaming: false,
                detour: None,
            })
        }

        /// Sets the input to `f`'s type, turned as the plan says, and the
        /// output to the picture as written, from `f`'s picture area.
        fn configure(&mut self, f: &FrameFormat) -> Result<()> {
            let oops = |e| failed("the video processor refused the frames", e);
            let it = copy_type(&f.media_type).map_err(oops)?;
            set_u32(&it, &MF_MT_VIDEO_ROTATION, self.plan.rotate as u32).map_err(oops)?;
            set_unstated_colour(&it, f.shown.height, &self.plan).map_err(oops)?;
            // The source's pixel aspect, which the container may say and the
            // decoder not: the processor keeps the shape it shows, so with
            // the output's (the same, turned) nothing is letterboxed (R37).
            let sar = if self.plan.rotate % 180 == 90 {
                (self.plan.sar.1, self.plan.sar.0)
            } else {
                self.plan.sar
            };
            set_u64(&it, &MF_MT_PIXEL_ASPECT_RATIO, ratio(sar)).map_err(oops)?;
            if self.copy_in {
                let bytes = if f.pixel == Pixel::P010 { 2 } else { 1 };
                set_u32(&it, &MF_MT_DEFAULT_STRIDE, f.width * bytes).map_err(oops)?;
            }
            let level = |key: &GUID| get_u32(&it, key).filter(|&v| v > 0 && v <= 0xFFFF);
            self.content_light = level(&MF_MT_MAX_LUMINANCE_LEVEL).map(|cll| {
                let fall = level(&MF_MT_MAX_FRAME_AVERAGE_LUMINANCE_LEVEL).unwrap_or(0);
                let mut b = [0u8; 4];
                b[..2].copy_from_slice(&(cll as u16).to_be_bytes());
                b[2..].copy_from_slice(&(fall as u16).to_be_bytes());
                b
            });
            // The picture within a padded frame: the processor crops to the
            // input's display aperture (and stretches it to the output, so an
            // odd size's lost pixel isn't letterboxed). Setting rectangles on
            // it instead squeezes a turned picture.
            let a = f.shown;
            if (a.x, a.y, a.width, a.height) != (0, 0, f.width, f.height) {
                let mut area = [0u8; 16];
                area[2..4].copy_from_slice(&(a.x as i16).to_le_bytes());
                area[6..8].copy_from_slice(&(a.y as i16).to_le_bytes());
                area[8..12].copy_from_slice(&(a.width as i32).to_le_bytes());
                area[12..16].copy_from_slice(&(a.height as i32).to_le_bytes());
                // SAFETY: a plain attribute write from a local.
                unsafe { it.SetBlob(&MF_MT_MINIMUM_DISPLAY_APERTURE, &area) }.map_err(oops)?;
            }
            // In software, a 10-bit picture, or one in another colour, made
            // 8-bit goes through RGB (see `Detour`). A 10-bit picture kept
            // 10-bit has no such way: P010 to P010 comes out whole only
            // unchanged, and anything else would be green, so it is refused
            // (R87 keeps such plans on a card).
            let software = self.gpu.is_none();
            let ten_bit = f.pixel == Pixel::P010;
            let colour = |t: &IMFMediaType| {
                [
                    &MF_MT_TRANSFER_FUNCTION,
                    &MF_MT_VIDEO_PRIMARIES,
                    &MF_MT_YUV_MATRIX,
                ]
                .map(|k| get_u32(t, k))
            };
            let same_colour = colour(&it) == colour(&self.out_type);
            let detour = needs_detour(software, self.plan.bit_depth, ten_bit, !same_colour);
            if software && ten_bit && self.plan.bit_depth > 8 {
                let a = f.shown;
                if !unchanged(
                    self.plan.rotate,
                    (f.width, f.height),
                    (a.x, a.y, a.width, a.height),
                    (self.plan.width, self.plan.height),
                    same_colour,
                ) {
                    return Err(MediaError::Encoder(
                        "the video processor can't change a 10-bit picture without a graphics card"
                            .into(),
                    ));
                }
            }
            if !detour {
                self.detour = None;
            } else if self.detour.is_none() {
                self.detour = Some(Detour::new(&self.plan, &self.out_type).map_err(oops)?);
            }
            let out_type = if detour {
                rgb_type(&self.plan).map_err(oops)?
            } else {
                self.out_type.clone()
            };
            // SAFETY: plain transform calls with types made here.
            unsafe {
                if self.streaming {
                    let _ = self.t.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
                    self.streaming = false;
                }
                self.t.SetInputType(0, &it, 0).map_err(oops)?;
                self.t.SetOutputType(0, &out_type, 0).map_err(oops)?;
                let info = self.t.GetOutputStreamInfo(0).map_err(oops)?;
                self.provides = provides(&info);
                self.out_size = info.cbSize;
                if self.gpu.is_some() && !self.provides && self.pool.is_none() {
                    self.pool = Some(self.make_pool().map_err(oops)?);
                }
                self.t
                    .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                    .map_err(oops)?;
            }
            self.streaming = true;
            self.input = Some(f.media_type.clone());
            Ok(())
        }

        /// A pool of textures on the card for the processor's output.
        fn make_pool(&self) -> windows::core::Result<IMFVideoSampleAllocatorEx> {
            let d = self
                .gpu
                .as_ref()
                .ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?;
            // SAFETY: the allocator comes back as a raw pointer of the
            // interface asked for, owned from here on.
            unsafe {
                let mut p = std::ptr::null_mut();
                MFCreateVideoSampleAllocatorEx(&IMFVideoSampleAllocatorEx::IID, &mut p)?;
                let pool = IMFVideoSampleAllocatorEx::from_raw(p);
                let mut attrs = None;
                MFCreateAttributes(&mut attrs, 1)?;
                let attrs = attrs.ok_or_else(|| {
                    windows::core::Error::from(windows::Win32::Foundation::E_FAIL)
                })?;
                attrs.SetUINT32(&MF_SA_D3D11_BINDFLAGS, D3D11_BIND_RENDER_TARGET.0 as u32)?;
                pool.SetDirectXManager(d.manager())?;
                pool.InitializeSampleAllocatorEx(4, POOL, &attrs, &self.out_type)?;
                Ok(pool)
            }
        }

        /// The picture `f` makes, lasting `duration`. `f` is let go here,
        /// before the picture is encoded.
        fn process(&mut self, f: Frame, duration: i64) -> Result<IMFSample> {
            let changed = self
                .input
                .as_ref()
                .is_none_or(|t| t.as_raw() != f.format.media_type.as_raw());
            if changed {
                self.configure(&f.format)?;
            }
            let oops = |e| failed("the video processor failed", e);
            let input = if self.copy_in {
                to_memory(&f.sample)?
            } else {
                f.sample.clone()
            };
            // SAFETY: plain transform calls on a sample we hold.
            unsafe { self.t.ProcessInput(0, &input, 0) }.map_err(oops)?;
            let given = match (&self.pool, self.provides) {
                (_, true) => None,
                // SAFETY: a plain call on the pool.
                (Some(pool), false) => Some(unsafe { pool.AllocateSample() }.map_err(oops)?),
                (None, false) => Some(
                    match &mut self.detour {
                        Some(d) => d.rgb_sample(self.out_size.max(1)),
                        None => memory_sample(self.out_size.max(1)),
                    }
                    .map_err(oops)?,
                ),
            };
            let out = process_output(&self.t, given)
                .map_err(oops)?
                .ok_or_else(|| MediaError::Encoder("the video processor gave no picture".into()))?;
            let pts = f.pts;
            drop(input);
            drop(f);
            let stamp = |s: &IMFSample| {
                // SAFETY: plain calls on a sample that is ours.
                unsafe {
                    s.SetSampleTime(pts)
                        .and_then(|_| s.SetSampleDuration(duration))
                        .map_err(oops)
                }
            };
            stamp(&out)?;
            let Some(detour) = &self.detour else {
                return Ok(out);
            };
            let out = detour.process(&out)?;
            stamp(&out)?;
            Ok(out)
        }
    }

    impl Drop for Processor {
        fn drop(&mut self) {
            com::com();
            if self.streaming {
                // SAFETY: a plain message to the transform.
                unsafe {
                    let _ = self.t.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // The encoder

    /// An H.264 or HEVC encoder transform, synchronous (software) or
    /// asynchronous (hardware, driven by its events).
    struct Encoder {
        t: IMFTransform,
        activate: IMFActivate,
        /// An asynchronous transform's events.
        events: Option<IMFMediaEventGenerator>,
        /// Frames the asynchronous transform has asked for and not yet had.
        wanted: u32,
        drained: bool,
        provides: bool,
        out_size: u32,
        hardware: bool,
        honours_bitrate: bool,
        /// Microsoft's software H.264 encoder: a bitrate is CBR (R49).
        cbr_only: bool,
        fail: bool,
        /// `KILN_MUTE_HW_ENCODER`: its events are never read.
        mute: bool,
        streaming: bool,
        /// Which measured encoder this is, if any (R85).
        family: EncoderFamily,
        /// What this encoder is told besides its rate control (R85).
        settings: EncoderSettings,
        /// Each setting as the encoder read it back.
        read_back: Vec<ReadBack>,
        /// On a quality step with a QP set frame by frame: the quality and
        /// the key-frame interval the encoder read back.
        frame_qp: Option<(u8, u32)>,
        /// Frames given to the encoder so far.
        frames_in: u64,
    }

    /// What one `ProcessOutput` came to.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Output {
        /// A packet (or nothing to give, for a transform that gives its own).
        Given,
        /// The encoder wants more input first.
        NeedsInput,
        /// The output type changed and is set again; ask again.
        Changed,
    }

    /// One rate-control value for `ICodecAPI`.
    fn value(api: &ICodecAPI, key: &GUID, v: u32) -> bool {
        let v = VARIANT::from(v);
        // SAFETY: a plain ICodecAPI call with a VARIANT that outlives it.
        unsafe { api.SetValue(key, &v) }.is_ok()
    }

    impl Encoder {
        /// Starts the encoder `activate` describes for `plan`; with `gpu`,
        /// it takes textures on that card, else frames in memory. The
        /// transform is shut down again when anything fails.
        fn open(
            activate: IMFActivate,
            hardware: bool,
            plan: &EncodePlan,
            gpu: Option<&D3d>,
        ) -> Result<Encoder> {
            // SAFETY: activates the transform the enumeration described.
            let t: IMFTransform = unsafe { activate.ActivateObject() }
                .map_err(|e| failed("the encoder won't start", e))?;
            // R49: Microsoft's software H.264 encoder ignores an
            // unconstrained VBR bitrate but follows a CBR one.
            let ms_h264 = activate_guid(&activate, &MFT_TRANSFORM_CLSID_Attribute)
                == Some(CLSID_MSH264EncoderMFT);
            let cbr_only = !hardware && ms_h264;
            // R85: each measured encoder's own settings.
            let family = EncoderFamily::of(
                plan.codec,
                hardware,
                ms_h264,
                string(&activate, &MFT_FRIENDLY_NAME_Attribute).as_deref(),
                string(&activate, &MFT_ENUM_HARDWARE_VENDOR_ID_Attribute).as_deref(),
            );
            let mut enc = Encoder {
                t,
                activate,
                events: None,
                wanted: 0,
                drained: false,
                provides: false,
                out_size: 0,
                hardware,
                honours_bitrate: true,
                cbr_only,
                fail: hardware && hardware_fails(),
                mute: hardware && hardware_mutes(),
                streaming: false,
                family,
                settings: settings_in_use(family),
                read_back: Vec::new(),
                frame_qp: None,
                frames_in: 0,
            };
            enc.start(plan, gpu)?;
            Ok(enc)
        }

        fn start(&mut self, plan: &EncodePlan, gpu: Option<&D3d>) -> Result<()> {
            let oops = |e| failed("the encoder refused the picture", e);
            let t = &self.t;
            let cbr_only = self.cbr_only;
            // SAFETY: plain transform, attribute and ICodecAPI calls with
            // types made here and a live device manager.
            unsafe {
                if let Ok(attrs) = t.GetAttributes() {
                    if get_u32(&attrs, &MF_TRANSFORM_ASYNC) == Some(1) {
                        attrs
                            .SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)
                            .map_err(oops)?;
                        self.events = Some(t.cast().map_err(oops)?);
                    }
                }
                if let Some(d) = gpu {
                    t.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, d.manager().as_raw() as usize)
                        .map_err(oops)?;
                }
                let api = t.cast::<ICodecAPI>().ok();
                // The rate control before the types, and again after them
                // for what an encoder takes only then.
                let rc = |api: &ICodecAPI| -> bool {
                    match plan.setting {
                        VideoSetting::Kbps(k) => {
                            ((!cbr_only
                                && value(
                                    api,
                                    &CODECAPI_AVEncCommonRateControlMode,
                                    eAVEncCommonRateControlMode_UnconstrainedVBR.0 as u32,
                                ))
                                || value(
                                    api,
                                    &CODECAPI_AVEncCommonRateControlMode,
                                    eAVEncCommonRateControlMode_CBR.0 as u32,
                                ))
                                && value(
                                    api,
                                    &CODECAPI_AVEncCommonMeanBitRate,
                                    k.saturating_mul(1000),
                                )
                        }
                        VideoSetting::Quality(q) => {
                            value(
                                api,
                                &CODECAPI_AVEncCommonRateControlMode,
                                eAVEncCommonRateControlMode_Quality.0 as u32,
                            ) && value(api, &CODECAPI_AVEncCommonQuality, q.min(100) as u32)
                        }
                        VideoSetting::Original => true,
                    }
                };
                // The key-frame spacing and the encoder's own settings
                // (R85), before the types; a measured encoder gets them again
                // after the types, when they are read back. An unmeasured one
                // gets them once, as before R85.
                let settings = self.settings;
                let fps = nominal_fps(plan);
                let extras = |api: &ICodecAPI| encoder_settings::apply(api, &settings, fps);
                let mut first = Vec::new();
                let early = api.as_ref().is_some_and(|a| {
                    first = extras(a);
                    rc(a)
                });

                let ot = MFCreateMediaType().map_err(oops)?;
                ot.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                    .map_err(oops)?;
                ot.SetGUID(&MF_MT_SUBTYPE, &subtype(plan.codec))
                    .map_err(oops)?;
                set_picture(&ot, plan).map_err(oops)?;
                let profile = match plan.codec {
                    VCodec::H264 => eAVEncH264VProfile_High.0 as u32,
                    VCodec::Hevc if plan.bit_depth > 8 => eAVEncH265VProfile_Main_420_10.0 as u32,
                    VCodec::Hevc => eAVEncH265VProfile_Main_420_8.0 as u32,
                };
                set_u32(&ot, &MF_MT_MPEG2_PROFILE, profile).map_err(oops)?;
                if let VideoSetting::Kbps(k) = plan.setting {
                    set_u32(&ot, &MF_MT_AVG_BITRATE, k.saturating_mul(1000)).map_err(oops)?;
                }
                set_colour(&ot, plan).map_err(oops)?;
                t.SetOutputType(0, &ot, 0)
                    .map_err(|e| failed("the encoder refused the codec's settings", e))?;

                // The input type the encoder offers in the plan's layout,
                // sized, else one made here.
                let raw = raw_subtype(plan);
                let offered = (0..64)
                    .map_while(|i| t.GetInputAvailableType(0, i).ok())
                    .find(|a| a.GetGUID(&MF_MT_SUBTYPE).ok() == Some(raw));
                let it = match offered {
                    Some(a) => {
                        set_picture(&a, plan).map_err(oops)?;
                        set_colour(&a, plan).map_err(oops)?;
                        a
                    }
                    None => raw_type(plan).map_err(oops)?,
                };
                t.SetInputType(0, &it, 0)
                    .map_err(|e| failed("the encoder refused the frames", e))?;
                if let Some(a) = &api {
                    if !early && !rc(a) {
                        return Err(MediaError::Encoder(
                            "the encoder refused the quality or bitrate".into(),
                        ));
                    }
                    self.read_back = if self.family == EncoderFamily::Other {
                        first
                    } else {
                        extras(a)
                    };
                    // The finer key frames only where the encoder says it
                    // spaces them as asked; else the QP is left to it.
                    let interval = key_interval(&settings, fps);
                    let read = self
                        .read_back
                        .iter()
                        .find(|(name, _)| *name == encoder_settings::KEY_INTERVAL)
                        .and_then(|(_, v)| *v);
                    if let (VideoSetting::Quality(q), Some(_), Some(read)) =
                        (plan.setting, settings.key_qp_finer, read)
                    {
                        if read == interval {
                            self.frame_qp = Some((q, read));
                        }
                    }
                }

                let info = t.GetOutputStreamInfo(0).map_err(oops)?;
                self.provides = provides(&info);
                self.out_size = info.cbSize.max(1 << 20);
                t.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                    .and_then(|_| t.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0))
                    .map_err(|e| failed("the encoder won't start", e))?;
            }
            self.streaming = true;
            Ok(())
        }

        /// Hands the encoder one picture, collecting what it gives back.
        /// `cancel` is asked while an asynchronous encoder is waited on.
        fn input(
            &mut self,
            s: &IMFSample,
            out: &mut Vec<IMFSample>,
            cancel: &dyn Fn() -> bool,
        ) -> Result<()> {
            if self.fail {
                return Err(MediaError::Encoder(
                    "the graphics card's encoder failed (KILN_FAIL_HW_ENCODER)".into(),
                ));
            }
            let refused = |e| failed("the encoder refused a frame", e);
            // R85: a QP set frame by frame, finer on the key frames.
            if let Some((q, interval)) = self.frame_qp {
                if let (Some(qp), Ok(api)) = (
                    encoder_settings::frame_qp(&self.settings, q, interval, self.frames_in),
                    self.t.cast::<ICodecAPI>(),
                ) {
                    encoder_settings::set_frame_qp(&api, qp);
                }
            }
            self.frames_in += 1;
            if self.events.is_some() {
                self.pump(out, |e| e.wanted > 0, cancel)?;
                // SAFETY: a sample we hold, given to the transform.
                unsafe { self.t.ProcessInput(0, s, 0) }.map_err(refused)?;
                self.wanted -= 1;
                return self.pump(out, |_| true, cancel);
            }
            loop {
                // SAFETY: a sample we hold, given to the transform.
                match unsafe { self.t.ProcessInput(0, s, 0) } {
                    Ok(()) => break,
                    Err(e) if e.code() == MF_E_NOTACCEPTING => self.pull(out)?,
                    Err(e) => return Err(refused(e)),
                }
            }
            self.pull(out)
        }

        /// Tells the encoder the picture has ended and collects the rest,
        /// asking `cancel` while an asynchronous encoder is waited on.
        fn drain(&mut self, out: &mut Vec<IMFSample>, cancel: &dyn Fn() -> bool) -> Result<()> {
            // SAFETY: plain messages to a streaming transform.
            unsafe {
                self.t
                    .ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0)
                    .and_then(|_| self.t.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0))
                    .map_err(|e| failed("the encoder won't finish", e))?;
            }
            if self.events.is_some() {
                self.pump(out, |e| e.drained, cancel)?;
            } else {
                self.pull(out)?;
            }
            self.stop();
            Ok(())
        }

        /// Collects every packet a synchronous encoder has ready.
        fn pull(&mut self, out: &mut Vec<IMFSample>) -> Result<()> {
            while self.output_one(out)? != Output::NeedsInput {}
            Ok(())
        }

        /// One `ProcessOutput`.
        fn output_one(&mut self, out: &mut Vec<IMFSample>) -> Result<Output> {
            let oops = |e| failed("the encoder failed", e);
            let given = if self.provides {
                None
            } else {
                Some(memory_sample(self.out_size).map_err(oops)?)
            };
            match process_output(&self.t, given) {
                Ok(Some(s)) => {
                    // SAFETY: a plain call on a sample that is ours.
                    if unsafe { s.GetTotalLength() }.unwrap_or(0) > 0 {
                        out.push(s);
                    }
                    Ok(Output::Given)
                }
                Ok(None) => Ok(Output::Given),
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => Ok(Output::NeedsInput),
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    // SAFETY: plain calls on the transform with a type it offers.
                    unsafe {
                        let t = self.t.GetOutputAvailableType(0, 0).map_err(oops)?;
                        self.t.SetOutputType(0, &t, 0).map_err(oops)?;
                    }
                    Ok(Output::Changed)
                }
                Err(e) => Err(oops(e)),
            }
        }

        /// Handles an asynchronous encoder's events (asking for input,
        /// giving output, done draining) until `until` holds and none is
        /// waiting. Waits for more while it doesn't, for at most [`SILENCE`]
        /// between events, asking `cancel` all the while: a cancel ends the
        /// wait at once.
        fn pump(
            &mut self,
            out: &mut Vec<IMFSample>,
            until: impl Fn(&Self) -> bool,
            cancel: &dyn Fn() -> bool,
        ) -> Result<()> {
            let Some(events) = self.events.clone() else {
                return Ok(());
            };
            let mut heard = Instant::now();
            loop {
                let next = if self.mute {
                    Err(windows::core::Error::from(MF_E_NO_EVENTS_AVAILABLE))
                } else {
                    // SAFETY: plain calls on the transform's event queue.
                    unsafe { events.GetEvent(MF_EVENT_FLAG_NO_WAIT) }
                };
                match next {
                    Ok(ev) => {
                        heard = Instant::now();
                        // SAFETY: plain calls on an event we hold.
                        let kind = unsafe { ev.GetType() }.unwrap_or(0);
                        if kind == METransformNeedInput.0 as u32 {
                            self.wanted += 1;
                        } else if kind == METransformHaveOutput.0 as u32 {
                            while self.output_one(out)? == Output::Changed {}
                        } else if kind == METransformDrainComplete.0 as u32 {
                            self.drained = true;
                        } else if kind == MEError.0 as u32 {
                            // SAFETY: as above.
                            let status = unsafe { ev.GetStatus() }.unwrap_or_default();
                            return Err(failed(
                                "the graphics card's encoder failed",
                                windows::core::Error::from(status),
                            ));
                        }
                    }
                    Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => {
                        if until(self) {
                            return Ok(());
                        }
                        if cancel() {
                            return Err(MediaError::Cancelled);
                        }
                        if heard.elapsed() > SILENCE {
                            return Err(MediaError::Encoder(
                                "the graphics card's encoder stopped answering".into(),
                            ));
                        }
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(e) => return Err(failed("the graphics card's encoder failed", e)),
                }
            }
        }

        fn stop(&mut self) {
            if self.streaming {
                self.streaming = false;
                // SAFETY: a plain message to the transform.
                unsafe {
                    let _ = self.t.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
                }
            }
        }
    }

    impl Drop for Encoder {
        /// An asynchronous (hardware) encoder works on its own threads: let
        /// go at once, NVIDIA's brought the process down (an access
        /// violation) in 3 to 5 of every 6 runs of 20 drops, mid-stream or
        /// after draining. So it is told to drop what it holds, its waiting
        /// events are taken off its queue, it is shut down (`IMFShutdown`,
        /// which asynchronous transforms need) before its activation object
        /// is, and its last reference goes to [`reap`], whose one reaper thread
        /// lets go of it once its shutdown has completed and tens of seconds
        /// have passed.
        fn drop(&mut self) {
            com::com();
            // SAFETY: plain messages and calls on the transform this object
            // activated, and on its event queue, without waiting.
            unsafe {
                if self.events.is_some() && self.streaming {
                    let _ = self.t.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
                }
                self.stop();
                if let Some(events) = &self.events {
                    for _ in 0..4096 {
                        if events.GetEvent(MF_EVENT_FLAG_NO_WAIT).is_err() {
                            break;
                        }
                    }
                    if let Ok(s) = self.t.cast::<IMFShutdown>() {
                        let _ = s.Shutdown();
                    }
                }
                let _ = self.activate.ShutdownObject();
            }
            if self.events.is_some() {
                // This object's own references go when it does, after the
                // reaper has taken its own.
                reap(self.t.clone(), self.activate.clone(), self.events.clone());
            }
        }
    }

    fn activate_guid(a: &IMFActivate, key: &GUID) -> Option<GUID> {
        // SAFETY: a plain attribute read.
        unsafe { a.GetGUID(key) }.ok()
    }

    /// True when, in a debug build, `KILN_HIDE_HEVC` asks to act as if the
    /// HEVC Video Extensions weren't installed (as the reader does).
    fn hevc_hidden() -> bool {
        #[cfg(debug_assertions)]
        if std::env::var_os("KILN_HIDE_HEVC").is_some() {
            return true;
        }
        false
    }

    /// The first software encoder for the plan that starts and takes it.
    ///
    /// With none at all: HEVC's comes with Microsoft's HEVC Video
    /// Extensions (R39); H.264's with Windows' Media Feature Pack.
    fn software_encoder(plan: &EncodePlan) -> Result<Encoder> {
        let listed = if plan.codec == VCodec::Hevc && hevc_hidden() {
            Vec::new()
        } else {
            com::enum_transforms(
                MFT_CATEGORY_VIDEO_ENCODER,
                MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_LOCALMFT | MFT_ENUM_FLAG_SORTANDFILTER,
                None,
                Some((MFMediaType_Video, subtype(plan.codec))),
                None,
            )
        };
        if listed.is_empty() {
            return Err(match plan.codec {
                VCodec::Hevc => MediaError::NeedsExtensionToWrite {
                    codec: "HEVC",
                    product: "9N4WGH0Z6VHQ",
                },
                VCodec::H264 => MediaError::NeedsMediaFoundation,
            });
        }
        let mut last = None;
        for a in listed {
            match Encoder::open(a, false, plan, None) {
                Ok(e) => return Ok(e),
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap_or_else(|| {
            MediaError::Encoder(format!("Windows has no {} encoder", plan.codec.name()))
        }))
    }

    /// The software encoder for `plan`, and the plan it takes: Windows' HEVC
    /// software encoder writes 8-bit Main only (it offers no P010 and refuses
    /// Main 10), so a 10-bit plan it can't take becomes 8-bit standard
    /// range, the HDR tone-mapped as for H.264.
    fn software_for(plan: &EncodePlan) -> Result<(Encoder, EncodePlan)> {
        match software_encoder(plan) {
            Err(_) if plan.bit_depth > 8 => {
                let eight = standard_range(plan);
                Ok((software_encoder(&eight)?, eight))
            }
            r => r.map(|e| (e, *plan)),
        }
    }

    /// The first hardware encoder for the plan that starts and takes it:
    /// with `d3d`, only one on that card, taking textures when `gpu` (the
    /// processor's card) is that card.
    fn hardware_encoder(
        plan: &EncodePlan,
        d3d: Option<&D3d>,
        gpu: Option<&D3d>,
    ) -> Option<Encoder> {
        hardware_encoders(plan.codec)
            .into_iter()
            .filter(|c| d3d.is_none_or(|d| c.luid == Some(d.adapter().luid)))
            .find_map(|c| Encoder::open(c.activate, true, plan, gpu).ok())
    }

    // -----------------------------------------------------------------------
    // The whole

    /// The video processor and an encoder: decoded frames in, packets out.
    pub struct VideoEncoder {
        plan: EncodePlan,
        choice: EncoderChoice,
        /// Automatic's hardware encoder refused the picture (R58).
        fell_back: bool,
        // The encoder before the processor: it goes first, while the
        // processor's textures and device it may still hold are alive.
        enc: Encoder,
        proc: Processor,
        /// True when the encoder takes frames in memory but the processor
        /// makes them on the card (a software encoder): each is copied off.
        copy_out: bool,
        /// Processed frames given to a hardware encoder that has given
        /// nothing yet, with their times and lengths, for the software
        /// encoder should it fail.
        held: Vec<(IMFSample, i64, i64)>,
        holding: bool,
        /// Packets out of the encoder, waiting for the codec private data.
        encoded: Vec<IMFSample>,
        spec: Option<StreamSpec>,
        cleaner: Option<NalCleaner>,
        reorder: Reorder,
        /// The next frame-rate boundary, in 100 ns.
        next_keep: Option<i64>,
        /// Under a frame-rate limit: the last frame kept, and each kept
        /// frame's time to the next one, by its time, for its packet.
        last_kept: Option<i64>,
        gaps: std::collections::BTreeMap<i64, i64>,
        /// R38, the packets keep their frames' times. Each encoder is given
        /// its frames at even times, frame `n` at `n` steps of the plan's
        /// rate, and each packet takes back the time and length of the frame
        /// whose step it comes out at. The encoder sees a steady rate, and
        /// Microsoft's H.264 encoder, which gives every packet a frame late
        /// (a frame of whatever length the frames have) and rounds the times
        /// to its own 90 kHz clock, is late by the same for all of them.
        /// `frames` holds, by number, the frames whose packets haven't come
        /// out; `fed` counts those given to the current encoder; `shift` is
        /// how late its first packet (frame 0, a key frame) came.
        frames: std::collections::BTreeMap<i64, (i64, i64)>,
        fed: i64,
        step: i64,
        shift: Option<i64>,
        /// The card the frames are decoded on, to tell a lost device.
        device: Option<D3d>,
        // Last, so Media Foundation outlives everything above.
        _mf: Mf,
    }

    // SAFETY: the transforms, samples and pool belong to the encoder alone
    // and live in the multithreaded apartment; every call first joins the
    // calling thread to it.
    unsafe impl Send for VideoEncoder {}

    impl VideoEncoder {
        /// The processor and the encoder for `plan`.
        ///
        /// `d3d` is the card the frames are decoded on, or `None` for frames
        /// in memory; the job opens it on [`encoder_adapter`]'s card. The
        /// processor runs there, except on an AMD card for a plan that is
        /// tone-mapped or made 8-bit (R7, R87), and in software with no
        /// card; a hardware encoder on that card takes its textures.
        ///
        /// `Automatic` tries the hardware encoders first and falls back to
        /// software, also later should the hardware one fail before its
        /// first packet. `GraphicsCard` with no hardware encoder that starts
        /// is `NoHardwareEncoder`; `Processor` uses software only.
        pub fn new(
            plan: &EncodePlan,
            choice: EncoderChoice,
            d3d: Option<&D3d>,
        ) -> Result<VideoEncoder> {
            let mf = Mf::start()?;
            // R7, R87: AMD's processor only for a plan that keeps 10 bits.
            let amd = d3d.is_some_and(|d| d.adapter().vendor == AMD);
            let gpu = d3d.filter(|_| processor_on_card(amd, plan)).cloned();
            // R87: 10 bits are kept only by a card's own processor; with
            // none, the picture is 8-bit standard range (R35), and `stream`
            // says so.
            let plan = &if gpu.is_none() && plan.bit_depth > 8 {
                standard_range(plan)
            } else {
                *plan
            };
            let hardware = match choice {
                EncoderChoice::Processor => None,
                _ => hardware_encoder(plan, d3d, gpu.as_ref()),
            };
            let (enc, plan) = match hardware {
                Some(e) => (e, *plan),
                None if choice == EncoderChoice::GraphicsCard => {
                    return Err(MediaError::NoHardwareEncoder {
                        codec: plan.codec.name(),
                    })
                }
                None => software_for(plan)?,
            };
            // The software encoder may take only 8 bits, tone-mapped: then
            // not on AMD's processor either (R7).
            let gpu = gpu.filter(|_| processor_on_card(amd, &plan));
            let copy_out = gpu.is_some() && !enc.hardware;
            let copy_in = d3d.is_some() && gpu.is_none();
            let holding = enc.hardware && choice == EncoderChoice::Automatic;
            // R58: Automatic fell back when a card has an encoder for the
            // codec that didn't take this job; a PC with none didn't.
            let fell_back = !enc.hardware
                && choice == EncoderChoice::Automatic
                && encoder_adapter(plan.codec, choice).is_some();
            Ok(VideoEncoder {
                plan,
                choice,
                fell_back,
                proc: Processor::new(&plan, gpu, copy_in)?,
                enc,
                copy_out,
                held: Vec::new(),
                holding,
                encoded: Vec::new(),
                spec: None,
                cleaner: None,
                reorder: Reorder::default(),
                next_keep: None,
                last_kept: None,
                gaps: Default::default(),
                frames: Default::default(),
                fed: 0,
                step: (TIMESCALE as f64 / nominal_fps(&plan)).round().max(1.0) as i64,
                shift: None,
                device: d3d.cloned(),
                _mf: mf,
            })
        }

        /// True when a hardware encoder is writing the picture.
        pub fn hardware(&self) -> bool {
            self.enc.hardware
        }

        /// True when Automatic meant the picture for a graphics card whose
        /// encoder refused it, at the start or before its first packet, and
        /// Windows' software encoder is writing it instead (R58).
        pub fn fell_back(&self) -> bool {
            self.fell_back
        }

        /// False when the encoder ignores a bitrate: Fit under then steers
        /// it by quality instead (R33). Every encoder Kiln uses follows one
        /// (Microsoft's software H.264 encoder in CBR, R49).
        pub fn honours_bitrate(&self) -> bool {
            self.enc.honours_bitrate
        }

        /// The settings the encoder was given besides its rate control, each
        /// as it read it back (`None` where it refused it), in the order
        /// they were set (R85).
        pub fn settings_read_back(&self) -> &[ReadBack] {
            &self.enc.read_back
        }

        /// The settings chosen for the encoder writing the picture (R85).
        pub fn encoder_settings(&self) -> EncoderSettings {
            self.enc.settings
        }

        /// The stream the packets belong to. Its codec private data (the
        /// `avcC` or `hvcC`) is known from the encoder's first output, and
        /// no packet is given out before then: once `push` or `finish` has
        /// given one, this is complete. Before that the record is empty.
        pub fn stream(&self) -> StreamSpec {
            self.spec
                .clone()
                .unwrap_or_else(|| self.spec_with(Vec::new()))
        }

        fn spec_with(&self, record: Vec<u8>) -> StreamSpec {
            let p = &self.plan;
            StreamSpec {
                codec: match p.codec {
                    VCodec::H264 => StreamCodec::H264 { avcc: record },
                    VCodec::Hevc => StreamCodec::Hevc { hvcc: record },
                },
                timescale: TIMESCALE,
                video: Some(VideoParams {
                    width: p.width,
                    height: p.height,
                    rotation: 0,
                    hdr: p.hdr_out,
                    bit_depth: p.bit_depth,
                    mastering: p.mastering.filter(|_| p.hdr_out != Hdr::None),
                    content_light: p
                        .content_light
                        .or(self.proc.content_light)
                        .filter(|_| p.hdr_out != Hdr::None),
                    sar: p.sar,
                    tag_colour: true,
                    bt601: p.bt601 && p.hdr_out == Hdr::None,
                    // The job says the rate, which the plan doesn't know.
                    frame_ns: None,
                    stated_colour: None,
                }),
                audio: None,
            }
        }

        /// Processes and encodes one decoded frame, adding any packets ready
        /// to `out`. The frame is let go as soon as the processor is done
        /// with it. A frame under the frame-rate limit is dropped here.
        pub fn push(&mut self, f: Frame, out: &mut Vec<Packet>) -> Result<()> {
            self.push_watched(f, out, &|| false)
        }

        /// [`VideoEncoder::push`], asking `cancel` while a graphics card's
        /// encoder is waited on: a cancel is [`MediaError::Cancelled`] at
        /// once, not after the encoder's silence runs out.
        pub fn push_watched(
            &mut self,
            f: Frame,
            out: &mut Vec<Packet>,
            cancel: &dyn Fn() -> bool,
        ) -> Result<()> {
            com::com();
            let r = self.push_frame(f, out, cancel);
            r.map_err(|e| self.lost(e))
        }

        fn push_frame(
            &mut self,
            f: Frame,
            out: &mut Vec<Packet>,
            cancel: &dyn Fn() -> bool,
        ) -> Result<()> {
            let Some(duration) = self.keep(&f) else {
                return Ok(());
            };
            let pts = f.pts;
            let s = self.proc.process(f, duration)?;
            if self.holding {
                if self.held.len() < HOLD {
                    self.held.push((s.clone(), pts, duration));
                } else {
                    self.holding = false;
                    self.held.clear();
                }
            }
            let r = self.encode(&s, pts, duration, cancel);
            drop(s);
            self.recover(r)?;
            self.emit(out, false)
        }

        /// Encodes what the encoder still holds and gives out every packet
        /// left.
        pub fn finish(&mut self, out: &mut Vec<Packet>) -> Result<()> {
            self.finish_watched(out, &|| false)
        }

        /// [`VideoEncoder::finish`], asking `cancel` while a graphics card's
        /// encoder is drained, as [`VideoEncoder::push_watched`] does.
        pub fn finish_watched(
            &mut self,
            out: &mut Vec<Packet>,
            cancel: &dyn Fn() -> bool,
        ) -> Result<()> {
            com::com();
            let r = self.finish_stream(out, cancel);
            r.map_err(|e| self.lost(e))
        }

        fn finish_stream(
            &mut self,
            out: &mut Vec<Packet>,
            cancel: &dyn Fn() -> bool,
        ) -> Result<()> {
            let r = self.drain(cancel);
            if r.as_ref().is_err_and(|e| !cancelled(e)) && self.can_fall_back() {
                self.fall_back()?;
                self.drain(cancel)?;
            } else {
                r?;
            }
            if let (Some(last), Some(cap)) = (self.last_kept, self.plan.max_fps) {
                // The last frame kept lasts one step of the limit.
                let step = (TIMESCALE as f64 / cap).round().max(1.0) as i64;
                self.gaps.insert(last, step);
            }
            self.emit(out, true)
        }

        /// Drains the encoder. One that took frames and gave nothing for
        /// them has failed as surely as one that says so.
        fn drain(&mut self, cancel: &dyn Fn() -> bool) -> Result<()> {
            self.enc.drain(&mut self.encoded, cancel)?;
            if self.fed > 0 && self.encoded.is_empty() && self.spec.is_none() {
                return Err(MediaError::Encoder(format!(
                    "the encoder gave nothing for {} frames",
                    self.fed
                )));
            }
            Ok(())
        }

        /// `e`, or what the graphics card's loss says when the frames' card
        /// was lost (its removal reason), as the reader does.
        fn lost(&self, e: MediaError) -> MediaError {
            // SAFETY: a plain query on the device.
            let reason = self
                .device
                .as_ref()
                .and_then(|d| unsafe { d.device().GetDeviceRemovedReason() }.err());
            match reason {
                Some(r) => MediaError::Encoder(format!(
                    "the graphics card failed while encoding the video ({:#010X})",
                    r.code().0 as u32
                )),
                None => e,
            }
        }

        /// The length to give a frame that is kept, or `None` to drop it: a
        /// frame is kept when it reaches the next `1/max_fps` boundary.
        fn keep(&mut self, f: &Frame) -> Option<i64> {
            let Some(cap) = self.plan.max_fps.filter(|m| *m > 0.0) else {
                return Some(f.duration);
            };
            let step = (TIMESCALE as f64 / cap).round().max(1.0) as i64;
            // Times rounded to 100 ns land a tick either side of a boundary.
            let slack = step / 50;
            let keep = self.next_keep.is_none_or(|next| f.pts + slack >= next);
            if !keep {
                return None;
            }
            let mut next = self.next_keep.unwrap_or(f.pts) + step;
            if next <= f.pts {
                // After a gap, the boundaries start again from this frame.
                next = f.pts + step;
            }
            self.next_keep = Some(next);
            if let Some(last) = self.last_kept.replace(f.pts) {
                self.gaps.insert(last, f.pts - last);
            }
            Some(f.duration.max(step))
        }

        /// Encodes the picture of the frame at `pts` lasting `duration`.
        fn encode(
            &mut self,
            s: &IMFSample,
            pts: i64,
            duration: i64,
            cancel: &dyn Fn() -> bool,
        ) -> Result<()> {
            if self.copy_out {
                let m = to_memory(s)?;
                self.feed(&m, pts, duration, cancel)
            } else {
                self.feed(s, pts, duration, cancel)
            }
        }

        /// Gives the encoder one picture at its even time (see `frames`).
        fn feed(
            &mut self,
            s: &IMFSample,
            pts: i64,
            duration: i64,
            cancel: &dyn Fn() -> bool,
        ) -> Result<()> {
            let n = self.fed;
            self.fed += 1;
            self.frames.insert(n, (pts, duration.max(0)));
            // SAFETY: plain writes to a sample we hold.
            unsafe {
                s.SetSampleTime(n * self.step)
                    .and_then(|_| s.SetSampleDuration(self.step))
                    .map_err(|e| failed("can't time a frame", e))?;
            }
            self.enc.input(s, &mut self.encoded, cancel)
        }

        fn can_fall_back(&self) -> bool {
            self.choice == EncoderChoice::Automatic
                && self.enc.hardware
                && self.holding
                && self.spec.is_none()
        }

        /// After an encoder error: Automatic's hardware encoder that has
        /// given nothing yet is replaced by the software one, given every
        /// frame so far. Any other error stands, a cancel too.
        fn recover(&mut self, r: Result<()>) -> Result<()> {
            match r {
                Err(e) if !cancelled(&e) && self.can_fall_back() => self.fall_back(),
                r => r,
            }
        }

        fn fall_back(&mut self) -> Result<()> {
            let (enc, plan) = software_for(&self.plan)?;
            // The hardware encoder goes here, through `Drop for Encoder`.
            self.enc = enc;
            self.fell_back = true;
            self.holding = false;
            self.encoded.clear();
            self.shift = None;
            self.fed = 0;
            self.frames.clear();
            let held = std::mem::take(&mut self.held);
            if plan == self.plan {
                self.copy_out = self.proc.gpu.is_some();
                // The software encoder is never waited on.
                for (s, pts, duration) in held {
                    self.encode(&s, pts, duration, &|| false)?;
                }
                return Ok(());
            }
            // The software encoder takes less than the plan (10-bit HEVC):
            // a new processor makes the rest of the frames as it takes them,
            // and a second, beside it (on the card, so the frames made
            // already are tone-mapped as the rest are), brings those already
            // made over. On AMD's card neither runs there: the plan is now
            // tone-mapped (R7).
            let old = FrameFormat {
                pixel: if self.plan.bit_depth > 8 {
                    Pixel::P010
                } else {
                    Pixel::Nv12
                },
                width: self.plan.width,
                height: self.plan.height,
                shown: Area {
                    x: 0,
                    y: 0,
                    width: self.plan.width,
                    height: self.plan.height,
                },
                media_type: self.proc.out_type.clone(),
            };
            let on_card = self.proc.gpu.is_some();
            let amd = self
                .proc
                .gpu
                .as_ref()
                .is_some_and(|d| d.adapter().vendor == AMD);
            let gpu = self
                .proc
                .gpu
                .clone()
                .filter(|_| processor_on_card(amd, &plan));
            let copy_in = self.proc.copy_in || (on_card && gpu.is_none());
            self.proc = Processor::new(&plan, gpu.clone(), copy_in)?;
            self.copy_out = gpu.is_some();
            self.plan = plan;
            // The frames held were made on the card when the first
            // processor ran there: in software, they are copied off.
            let bridge_copy_in = on_card && gpu.is_none();
            let mut bridge =
                Processor::new(&EncodePlan { rotate: 0, ..plan }, gpu, bridge_copy_in)?;
            for (sample, pts, duration) in held {
                let f = Frame {
                    sample,
                    pts,
                    duration,
                    format: old.clone(),
                };
                let s = bridge.process(f, duration)?;
                self.encode(&s, pts, duration, &|| false)?;
            }
            Ok(())
        }

        /// Turns the encoder's output into packets, and gives out those
        /// whose decode time is settled (all of them at the end).
        fn emit(&mut self, out: &mut Vec<Packet>, ended: bool) -> Result<()> {
            for s in std::mem::take(&mut self.encoded) {
                let data = sample_bytes(&s)?;
                if self.spec.is_none() {
                    self.start_stream(&data)?;
                }
                let key = get_u32(&s, &MFSampleExtension_CleanPoint) == Some(1);
                // SAFETY: a plain read from a sample that is ours.
                let pts = unsafe { s.GetSampleTime() }
                    .map_err(|e| failed("the encoder gave a packet with no time", e))?;
                // R38: back to the frame's own time and length (see
                // `frames`).
                let late = *self.shift.get_or_insert(pts);
                let n = ((pts - late) as f64 / self.step as f64).round() as i64;
                let (pts, duration) = self.frames.remove(&n).ok_or_else(|| {
                    MediaError::Encoder("the encoder gave a packet for no frame".into())
                })?;
                let data = match &mut self.cleaner {
                    Some(c) => c.clean(data, key),
                    None => data,
                };
                self.reorder.push(Packet {
                    stream: 0,
                    pts,
                    dts: pts,
                    duration,
                    key,
                    data,
                });
            }
            while self.reorder.ready(ended).is_some() {
                if self.reorder.broken() {
                    return Err(MediaError::Encoder(
                        "the encoder gave its packets out of order".into(),
                    ));
                }
                if let Some(mut p) = self.reorder.pop() {
                    if self.plan.max_fps.is_some() {
                        // Under a limit a packet lasts until the next kept
                        // frame, whatever the encoder says.
                        if let Some(gap) = self.gaps.remove(&p.pts) {
                            p.duration = gap;
                        }
                    }
                    out.push(p);
                }
            }
            Ok(())
        }

        /// Works out the codec private data from the encoder's first
        /// packet: its output type's sequence header, else the parameter
        /// sets in the packet itself (the HEVC software encoder's way).
        fn start_stream(&mut self, first: &[u8]) -> Result<()> {
            // SAFETY: a plain call on the transform.
            let seq = unsafe { self.enc.t.GetOutputCurrentType(0) }
                .ok()
                .and_then(|t| get_blob(&t, &MF_MT_MPEG_SEQUENCE_HEADER));
            let hevc = self.plan.codec == VCodec::Hevc;
            // R84: the stream says its colour as the container does, in
            // every SPS: Microsoft's software encoders write none, and
            // NVIDIA's writes BT.709's transfer for BT.601.
            let colour = self
                .spec_with(Vec::new())
                .video
                .and_then(|v| v.described_colour());
            let build = |b: &[u8]| {
                let b = match colour {
                    Some(c) => annex_b_with_colour(b, hevc, c),
                    None => b.to_vec(),
                };
                match self.plan.codec {
                    VCodec::H264 => avcc_from_annex_b(&b),
                    VCodec::Hevc => hvcc_from_annex_b(&b),
                }
            };
            let record = seq
                .as_deref()
                .and_then(build)
                .or_else(|| build(first))
                .ok_or_else(|| MediaError::Encoder("the encoder gave no parameter sets".into()))?;
            self.cleaner = Some(
                NalCleaner::new(hevc, &record, &record, Vec::new(), false).with_colour(colour),
            );
            self.spec = Some(self.spec_with(record));
            self.holding = false;
            self.held.clear();
            Ok(())
        }
    }

    impl Drop for VideoEncoder {
        fn drop(&mut self) {
            com::com();
        }
    }
}

/// Off Windows there is no Media Foundation: no encoder is ever made.
#[cfg(not(windows))]
pub use other::{encoder_adapter, encoder_adapter_known, VideoEncoder};

#[cfg(not(windows))]
mod other {
    use super::*;
    use crate::error::{MediaError, Result};
    use crate::mf::d3d::D3d;
    use crate::video::encoder_settings::{EncoderSettings, ReadBack};
    use crate::video::packet::{Packet, StreamSpec};
    use crate::video::read::Frame;

    /// No graphics card encoder is used off Windows.
    pub fn encoder_adapter(_codec: VCodec, _choice: EncoderChoice) -> Option<u64> {
        None
    }

    /// Known at once off Windows: no card.
    pub fn encoder_adapter_known(_codec: VCodec, _choice: EncoderChoice) -> Option<Option<u64>> {
        Some(None)
    }

    /// An encoder; there is never one off Windows.
    pub enum VideoEncoder {}

    impl VideoEncoder {
        pub fn new(
            _plan: &EncodePlan,
            _choice: EncoderChoice,
            _d3d: Option<&D3d>,
        ) -> Result<VideoEncoder> {
            Err(MediaError::NeedsMediaFoundation)
        }

        pub fn hardware(&self) -> bool {
            match *self {}
        }

        pub fn fell_back(&self) -> bool {
            match *self {}
        }

        pub fn honours_bitrate(&self) -> bool {
            match *self {}
        }

        pub fn settings_read_back(&self) -> &[ReadBack] {
            match *self {}
        }

        pub fn encoder_settings(&self) -> EncoderSettings {
            match *self {}
        }

        pub fn stream(&self) -> StreamSpec {
            match *self {}
        }

        pub fn push(&mut self, _f: Frame, _out: &mut Vec<Packet>) -> Result<()> {
            match *self {}
        }

        pub fn push_watched(
            &mut self,
            _f: Frame,
            _out: &mut Vec<Packet>,
            _cancel: &dyn Fn() -> bool,
        ) -> Result<()> {
            match *self {}
        }

        pub fn finish(&mut self, _out: &mut Vec<Packet>) -> Result<()> {
            match *self {}
        }

        pub fn finish_watched(
            &mut self,
            _out: &mut Vec<Packet>,
            _cancel: &dyn Fn() -> bool,
        ) -> Result<()> {
            match *self {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_look_for_the_card_that_failed_for_a_passing_reason_is_made_again() {
        // Final review item 7: a card found, or no encoder at all, stands
        // for good; none found because one wouldn't start stands until its
        // time to look again.
        let now = std::time::Instant::now();
        let later = now + std::time::Duration::from_secs(61);
        let card = Found {
            card: Some(7),
            again: None,
        };
        assert!(card.stands(now) && card.stands(later));
        let passing = Found {
            card: None,
            again: Some(now + std::time::Duration::from_secs(60)),
        };
        assert!(passing.stands(now));
        assert!(!passing.stands(later));
        let none = Found {
            card: None,
            again: None,
        };
        assert!(none.stands(later));
    }

    /// Task 10c: only a 10-bit picture, or one in another colour (BT.601,
    /// BT.2020 or HDR made BT.709), written in 8 bits by the processor in
    /// software goes through RGB. An 8-bit picture in the output's own
    /// colour comes out of it whole, as everything does on a card, and a
    /// picture kept 10-bit can't go that way.
    #[test]
    fn only_deep_or_recoloured_pictures_made_8_bit_in_software_go_through_rgb() {
        // (software, bits written, 10-bit in, in another colour)
        assert!(
            !needs_detour(true, 8, false, false),
            "8-bit in the output's colour"
        );
        assert!(needs_detour(true, 8, true, false), "10-bit standard range");
        assert!(
            needs_detour(true, 8, false, true),
            "8-bit BT.601, BT.2020 or HDR"
        );
        assert!(needs_detour(true, 8, true, true), "10-bit HDR");
        for (ten, other) in [(false, false), (true, false), (false, true), (true, true)] {
            assert!(!needs_detour(false, 8, ten, other), "on a card");
            assert!(!needs_detour(true, 10, ten, other), "kept 10-bit");
            assert!(
                !needs_detour(false, 10, ten, other),
                "kept 10-bit on a card"
            );
        }
    }

    /// R7, R87: AMD's processor runs only a plan that keeps 10 bits (HDR or
    /// not), never one tone-mapped to 8 bits, nor one a 10-bit plan was cut
    /// to for a software encoder. Other cards run every plan.
    #[test]
    fn amds_processor_runs_only_plans_that_keep_ten_bits() {
        let hdr = test_plan(Hdr::Pq, 10);
        let sdr_10 = test_plan(Hdr::None, 10);
        let eight = test_plan(Hdr::None, 8);
        assert!(processor_on_card(true, &hdr), "HDR kept");
        assert!(
            processor_on_card(true, &sdr_10),
            "10-bit standard range kept"
        );
        assert!(!processor_on_card(true, &eight), "8-bit");
        assert!(
            !processor_on_card(true, &standard_range(&hdr)),
            "HDR cut to standard range"
        );
        assert!(
            !processor_on_card(true, &standard_range(&sdr_10)),
            "10 bits cut to 8"
        );
        for p in [hdr, sdr_10, eight, standard_range(&hdr)] {
            assert!(processor_on_card(false, &p), "another card");
        }
    }

    /// An HEVC 4K plan with `hdr_out` and `bit_depth`.
    fn test_plan(hdr_out: Hdr, bit_depth: u8) -> EncodePlan {
        EncodePlan {
            codec: VCodec::Hevc,
            width: 3840,
            height: 2160,
            fps: Some(24.0),
            max_fps: None,
            setting: VideoSetting::Quality(65),
            hdr_out,
            bit_depth,
            rotate: 90,
            sar: (1, 1),
            mastering: None,
            content_light: None,
            bt601: false,
        }
    }

    /// A picture comes out of the processor unchanged only with no turn,
    /// no crop, no new size and no new colour.
    #[test]
    fn a_picture_is_unchanged_only_with_nothing_to_do() {
        let whole = (0, 0, 1920, 1080);
        assert!(unchanged(0, (1920, 1080), whole, (1920, 1080), true));
        assert!(!unchanged(90, (1920, 1080), whole, (1920, 1080), true));
        assert!(!unchanged(0, (1920, 1088), whole, (1920, 1080), true));
        assert!(!unchanged(0, (1920, 1080), whole, (1280, 720), true));
        assert!(!unchanged(0, (1920, 1080), whole, (1920, 1080), false));
    }

    /// R35, R87: standard range is 8-bit, with no HDR, the rest as planned.
    #[test]
    fn standard_range_is_8_bit_without_hdr() {
        let p = test_plan(Hdr::Pq, 10);
        let s = standard_range(&p);
        assert_eq!((s.bit_depth, s.hdr_out), (8, Hdr::None));
        assert_eq!(
            EncodePlan {
                bit_depth: 10,
                hdr_out: Hdr::Pq,
                ..s
            },
            p
        );
    }
}
