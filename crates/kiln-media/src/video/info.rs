//! What a video file says about its picture, read from the container's own
//! headers: an MP4 or MOV's `moov`, or a Matroska or WebM's `Info` and
//! `Tracks`. No decoder is involved, so none of it needs a codec installed.
//!
//! The codec's private data (`avcC`, `hvcC`, `av1C`, `vpcC`, or the same in
//! Matroska's `CodecPrivate`) gives the profile and bit depth. An HEVC
//! stream that carries its colour description only in the SPS's VUI (no
//! `colr` box, as FFmpeg writes HDR10 into MP4) is read from the SPS.

use crate::error::{MediaError, Result};
use crate::mkvread;
use crate::mp4read::{self, be16, be32, child};
use crate::types::SourceInfo;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

/// The video codecs Kiln names. Anything else is `Other`, in lower case
/// (`vp8`, `prores`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoCodec {
    H264,
    Hevc,
    Av1,
    Vp9,
    Mpeg4,
    Mjpeg,
    Other(String),
}

/// The dynamic range a video is mastered in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hdr {
    /// Standard dynamic range, or not stated.
    None,
    /// HDR10: the PQ (SMPTE ST 2084) transfer function.
    Pq,
    /// Hybrid log-gamma.
    Hlg,
}

/// What a video file holds.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoInfo {
    /// The coded picture size, before rotation.
    pub width: u32,
    pub height: u32,
    /// Degrees clockwise the picture is turned to show it upright: 0, 90, 180
    /// or 270.
    pub rotation: u16,
    /// Frames per second on average; `None` when the file doesn't say.
    pub fps: Option<f64>,
    /// True when the frames aren't all the same length.
    pub variable_rate: bool,
    pub codec: VideoCodec,
    pub profile: Option<String>,
    pub bit_depth: u8,
    pub hdr: Hdr,
    /// Seconds.
    pub duration: Option<f64>,
    /// The video's average bit rate, when the container lets it be worked out.
    pub video_kbps: Option<u32>,
    /// The sound's facts (part 1's); `None` when the file is silent, or when
    /// its sound can't be read (see `sound_unreadable`).
    pub sound: Option<SourceInfo>,
    /// The name of the sound's codec when the file has sound that part 1 can't
    /// read ("DTS"). The picture can still be converted, and the sound copied
    /// if the target holds that codec.
    pub sound_unreadable: Option<String>,
    /// The pixel aspect ratio of the coded picture, horizontal to vertical,
    /// in lowest terms: (1, 1) for square pixels. From an MP4's `pasp`, else
    /// a Matroska display size, else the stream's own (its SPS VUI), else
    /// square.
    pub sar: (u32, u32),
    /// HDR10 static metadata, when the file has it: the mastering display
    /// (an `mdcv` body) and the content light levels (a `clli` body), from
    /// the container's boxes or elements, else the `hvcC`'s SEI.
    pub mastering: Option<[u8; 24]>,
    pub content_light: Option<[u8; 4]>,
    /// When the file was made, in seconds since 1970-01-01 00:00:00 UTC:
    /// an MP4's `mvhd` creation time, or a Matroska's `Info/DateUTC`.
    /// `None` when the file doesn't say.
    pub created: Option<i64>,
    /// The title in a Matroska's `Info/Title`. (An MP4's title is a tag,
    /// which part 1's tag reader gives.)
    pub title: Option<String>,
    /// When an MP4's edit list hides frames before the start it presents
    /// (an excerpt cut by stream copy): how many seconds before that start
    /// lies the key frame a copy has to begin at, 0 when the start is on a
    /// key frame. `None` when no frame is hidden. Matroska has no edit list,
    /// so a copy into it can't hide a lead-in (R69).
    pub lead_in: Option<f64>,
    /// True when the file describes the picture's colour: an MP4's `colr`,
    /// a Matroska's `Colour`, or the stream's own VUI. An untagged picture
    /// is encoded as players read it: BT.601 under 720 lines, else BT.709
    /// (R68).
    pub colour_tagged: bool,
}

fn damaged() -> MediaError {
    MediaError::Damaged {
        code: "video".into(),
    }
}

/// The picture's facts from the file's headers, with `sound` left empty.
pub(crate) fn read(file: &mut File) -> Result<VideoInfo> {
    let mut magic = [0u8; 4];
    file.seek(SeekFrom::Start(0)).map_err(|_| damaged())?;
    let n = file.read(&mut magic).map_err(|_| damaged())?;
    if n == 4 && magic == [0x1A, 0x45, 0xDF, 0xA3] {
        from_matroska(file)
    } else {
        from_mp4(file)
    }
}

// ---------------------------------------------------------------------------
// MP4 and MOV

fn from_mp4(file: &mut File) -> Result<VideoInfo> {
    let moov = mp4read::read_moov_with(file, mp4read::may_start_a_movie).ok_or_else(damaged)?;
    let mut info = from_moov(&moov)?;
    info.created = child(&moov, b"mvhd").and_then(mvhd_created);
    Ok(info)
}

/// Seconds from 1904-01-01, the MP4 epoch, to 1970-01-01.
pub(crate) const MP4_EPOCH: i64 = 2_082_844_800;
/// Seconds from 1970-01-01 to 2001-01-01, Matroska's `DateUTC` epoch.
pub(crate) const MATROSKA_EPOCH: i64 = 978_307_200;

/// An `mvhd` body's creation time as Unix seconds; `None` when it is 0 (not
/// set) or the box is short.
fn mvhd_created(mvhd: &[u8]) -> Option<i64> {
    let secs = match *mvhd.first()? {
        1 => u64::from_be_bytes(mvhd.get(4..12)?.try_into().ok()?),
        _ => be32(mvhd, 4)? as u64,
    };
    (secs > 0)
        .then(|| i64::try_from(secs).ok())
        .flatten()
        .map(|s| s - MP4_EPOCH)
}

/// The picture's facts from the body of a `moov`.
fn from_moov(moov: &[u8]) -> Result<VideoInfo> {
    let v = match mp4read::video_track(moov) {
        Ok(Some(v)) => v,
        Ok(None) => return Err(MediaError::NoVideo),
        Err(_) => return Err(damaged()),
    };
    if v.width == 0 || v.height == 0 {
        return Err(damaged());
    }
    let (codec, cfg) = match &v.fourcc {
        b"avc1" | b"avc3" => (VideoCodec::H264, avc_config(child(v.entry, b"avcC"))),
        b"hvc1" | b"hev1" | b"dvh1" | b"dvhe" => {
            (VideoCodec::Hevc, hevc_config(child(v.entry, b"hvcC")))
        }
        b"av01" => (VideoCodec::Av1, av1_config(child(v.entry, b"av1C"))),
        b"vp09" => (VideoCodec::Vp9, vp9_config(child(v.entry, b"vpcC"))),
        b"mp4v" => (VideoCodec::Mpeg4, Config::default()),
        b"jpeg" | b"mjpa" | b"mjpb" => (VideoCodec::Mjpeg, Config::default()),
        other => (
            VideoCodec::Other(String::from_utf8_lossy(other).trim().to_ascii_lowercase()),
            Config::default(),
        ),
    };

    let seconds = |ticks: u64, scale: u32| {
        (ticks > 0 && scale > 0)
            .then(|| ticks as f64 / scale as f64)
            .filter(|s| s.is_finite() && *s < 1e9)
    };
    // How long the frames last, by their own durations.
    let sample_secs = seconds(v.sample_ticks, v.media_scale);
    // The shorter of the media and what the track's edit says to present: an
    // edit can trim the media, and some writers round it past the end.
    let duration = [
        v.movie_scale.and_then(|s| seconds(v.track_duration, s)),
        seconds(v.media_duration, v.media_scale),
    ]
    .into_iter()
    .flatten()
    .min_by(f64::total_cmp)
    .or(sample_secs)
    .or_else(|| v.movie_scale.and_then(|s| seconds(v.movie_duration, s)));
    let fps = sample_secs
        .filter(|_| v.samples > 0)
        .and_then(|s| sane_fps(v.samples as f64 / s));
    let video_kbps = v
        .bytes
        .zip(sample_secs.or(duration))
        .map(|(bytes, s)| (bytes as f64 * 8.0 / s / 1000.0).round() as u32);

    let pasp = child(v.entry, b"pasp").and_then(|b| Some((be32(b, 0)?, be32(b, 4)?)));
    let (sei_mastering, sei_light) = match codec {
        VideoCodec::Hevc => super::read::hdr_from_hvcc(child(v.entry, b"hvcC").unwrap_or(&[])),
        _ => (None, None),
    };
    Ok(VideoInfo {
        sar: pasp.and_then(lowest).or(cfg.sar).unwrap_or((1, 1)),
        mastering: child(v.entry, b"mdcv")
            .and_then(|b| b.get(..24)?.try_into().ok())
            .or(sei_mastering),
        content_light: child(v.entry, b"clli")
            .and_then(|b| b.get(..4)?.try_into().ok())
            .or(sei_light),
        width: v.width,
        height: v.height,
        rotation: rotation(&v.matrix),
        fps,
        variable_rate: v.varied_deltas,
        codec,
        profile: cfg.profile,
        bit_depth: cfg.bit_depth.unwrap_or(8),
        hdr: hdr_of(v.colr_transfer.or(cfg.transfer), v.hdr10_metadata),
        duration,
        video_kbps,
        sound: None,
        sound_unreadable: None,
        created: None,
        title: None,
        lead_in: lead_in(v.trak, v.media_scale),
        colour_tagged: v.colr.is_some_and(|c| colour_stated(c.map(u64::from))) || cfg.colour,
    })
}

impl VideoInfo {
    /// Every frame's length in nanoseconds when the rate is constant, as
    /// Matroska's `DefaultDuration` gives it (R80); `None` for a variable or
    /// unknown rate.
    pub(crate) fn frame_ns(&self) -> Option<u64> {
        self.fps
            .filter(|f| !self.variable_rate && f.is_finite() && *f > 0.0)
            .map(|f| (1e9 / f).round() as u64)
            .filter(|&ns| ns > 0)
    }
}

/// [`VideoInfo::lead_in`] for an MP4 track: from its first media edit's
/// media time (the reader's own model of the list, R75), the sample tables'
/// presentation times (`stts` and `ctts`) and its key frames (`stss`, every
/// sample when there is none).
fn lead_in(trak: &[u8], media_scale: u32) -> Option<f64> {
    let elst = child(child(trak, b"edts")?, b"elst")?;
    let (_, start) = super::read::first_edit(elst)?;
    if start <= 0 {
        return None;
    }
    let stbl = child(child(child(trak, b"mdia")?, b"minf")?, b"stbl")?;
    let runs = |b: &[u8]| -> Option<Vec<(u32, i64)>> {
        let n = be32(b, 4)? as usize;
        let table = b.get(8..)?;
        let rows = table.chunks_exact(8).take(n).map(|e| {
            let count = u32::from_be_bytes([e[0], e[1], e[2], e[3]]);
            (count, i32::from_be_bytes([e[4], e[5], e[6], e[7]]) as i64)
        });
        Some(rows.collect())
    };
    let deltas = runs(child(stbl, b"stts")?)?;
    let offsets = child(stbl, b"ctts").and_then(runs).unwrap_or_default();
    let keys: Option<Vec<u32>> = child(stbl, b"stss").map(|b| {
        let n = be32(b, 4).unwrap_or(0) as usize;
        b.get(8..)
            .unwrap_or(&[])
            .chunks_exact(4)
            .take(n)
            .map(|k| u32::from_be_bytes([k[0], k[1], k[2], k[3]]))
            .collect()
    });
    let mut offsets = offsets
        .iter()
        .flat_map(|&(count, off)| std::iter::repeat_n(off, count as usize));
    let (mut dts, mut sample) = (0i64, 0u32);
    let (mut earliest, mut key) = (i64::MAX, None);
    'walk: for &(count, delta) in &deltas {
        for _ in 0..count {
            // A frame shown before the start, or the key frame a copy
            // starts at, is decoded no later than the start.
            if dts > start {
                break 'walk;
            }
            sample += 1;
            let shown = dts + offsets.next().unwrap_or(0);
            earliest = earliest.min(shown);
            let is_key = keys
                .as_ref()
                .is_none_or(|k| k.binary_search(&sample).is_ok());
            if is_key && shown <= start {
                key = Some(shown);
            }
            dts += delta.max(0);
        }
    }
    if earliest >= start {
        return None;
    }
    let from = key.unwrap_or(earliest);
    Some((start - from) as f64 / media_scale.max(1) as f64)
}

/// A frame rate worth believing: finite, above 0 and no more than 1000.
fn sane_fps(fps: f64) -> Option<f64> {
    (fps.is_finite() && fps > 0.0 && fps <= 1000.0).then_some(fps)
}

/// Degrees clockwise a `tkhd` matrix turns the picture, to the nearest
/// quarter turn. The matrix is a, b, u, c, d, v, x, y, w in 16.16 fixed
/// point (u, v and w in 2.30); 90 degrees clockwise is a=0, b=1, c=-1, d=0.
pub(crate) fn rotation(matrix: &[i32; 9]) -> u16 {
    let (a, b) = (matrix[0] as f64, matrix[1] as f64);
    if a == 0.0 && b == 0.0 {
        return 0;
    }
    let quarters = (b.atan2(a).to_degrees() / 90.0).round() as i64;
    (quarters.rem_euclid(4) * 90) as u16
}

// ---------------------------------------------------------------------------
// Matroska and WebM

fn from_matroska(file: &mut File) -> Result<VideoInfo> {
    let h = mkvread::read_headers(file).ok_or_else(damaged)?;
    let mut info = from_headers(&h)?;
    info.created = h
        .date_utc
        .map(|ns| ns.div_euclid(1_000_000_000) + MATROSKA_EPOCH);
    info.title = h.title.clone();
    Ok(info)
}

/// The picture's facts from a Matroska file's `Info` and `Tracks`.
fn from_headers(h: &mkvread::Headers) -> Result<VideoInfo> {
    let t = h
        .tracks
        .iter()
        .find(|t| t.kind == mkvread::TYPE_VIDEO)
        .ok_or(MediaError::NoVideo)?;
    let v = t.video.as_ref().ok_or_else(damaged)?;
    if v.width == 0 || v.height == 0 {
        return Err(damaged());
    }
    let private = (!t.codec_private.is_empty()).then_some(t.codec_private.as_slice());
    let (codec, cfg) = match t.codec_id.as_str() {
        "V_MPEG4/ISO/AVC" => (VideoCodec::H264, avc_config(private)),
        "V_MPEGH/ISO/HEVC" => (VideoCodec::Hevc, hevc_config(private)),
        "V_AV1" => (VideoCodec::Av1, av1_config(private)),
        "V_VP9" => (VideoCodec::Vp9, Config::default()),
        "V_MJPEG" => (VideoCodec::Mjpeg, Config::default()),
        id if id.starts_with("V_MPEG4/ISO/") => (VideoCodec::Mpeg4, Config::default()),
        id => (
            VideoCodec::Other(id.trim_start_matches("V_").to_ascii_lowercase()),
            Config::default(),
        ),
    };
    let bit_depth = v
        .bits
        .and_then(|b| u8::try_from(b).ok())
        .filter(|b| (8..=16).contains(b))
        .or(cfg.bit_depth)
        .unwrap_or(8);
    let transfer = v
        .transfer
        .and_then(|t| u16::try_from(t).ok())
        .or(cfg.transfer);
    let duration = h
        .duration
        .map(|d| d * h.timestamp_scale as f64 / 1e9)
        .filter(|s| s.is_finite() && *s > 0.0 && *s < 1e9);
    // The display size stretches the coded one by the pixel aspect. Its
    // unit (pixels, centimetres, inches, or a display aspect ratio, as
    // FFmpeg writes 16:9) doesn't matter: only the ratio of its sides does.
    let shown = match (v.display_width, v.display_height, v.display_unit) {
        (Some(dw), Some(dh), 0..=3) => u32::try_from(dw.saturating_mul(v.height as u64))
            .ok()
            .zip(u32::try_from(dh.saturating_mul(v.width as u64)).ok())
            .and_then(lowest),
        _ => None,
    };
    let (sei_mastering, sei_light) = match codec {
        VideoCodec::Hevc => super::read::hdr_from_hvcc(private.unwrap_or(&[])),
        _ => (None, None),
    };
    let content_light = v.max_cll.and_then(|cll| {
        let cll = u16::try_from(cll).ok()?;
        let fall = u16::try_from(v.max_fall.unwrap_or(0)).ok()?;
        let mut b = [0u8; 4];
        b[..2].copy_from_slice(&cll.to_be_bytes());
        b[2..].copy_from_slice(&fall.to_be_bytes());
        Some(b)
    });
    Ok(VideoInfo {
        sar: shown.or(cfg.sar).unwrap_or((1, 1)),
        mastering: v
            .mastering
            .map(super::read::mdcv_from_matroska)
            .or(sei_mastering),
        content_light: content_light.or(sei_light),
        width: v.width,
        height: v.height,
        rotation: 0,
        fps: t.default_duration.and_then(|d| sane_fps(1e9 / d as f64)),
        variable_rate: t.default_duration.is_none(),
        codec,
        profile: cfg.profile,
        bit_depth,
        hdr: hdr_of(transfer, v.hdr10_metadata),
        duration,
        video_kbps: None,
        sound: None,
        sound_unreadable: None,
        created: None,
        title: None,
        lead_in: None,
        colour_tagged: colour_stated([
            v.primaries.unwrap_or(2),
            v.transfer.unwrap_or(2),
            v.matrix.unwrap_or(2),
        ]) || cfg.colour,
    })
}

// ---------------------------------------------------------------------------
// HDR

/// True when a colour description (primaries, transfer and matrix, as H.273
/// numbers them) says anything: 2 is "unspecified", and 0 is reserved for
/// primaries and transfer.
pub(crate) fn colour_stated([primaries, transfer, matrix]: [u64; 3]) -> bool {
    !matches!(primaries, 0 | 2) || !matches!(transfer, 0 | 2) || matrix != 2
}

/// The dynamic range for a transfer characteristic (H.273: 16 is PQ, 18 is
/// HLG). With none stated, static HDR10 metadata (`mdcv`, `clli`) means PQ.
fn hdr_of(transfer: Option<u16>, hdr10_metadata: bool) -> Hdr {
    match transfer {
        Some(16) => Hdr::Pq,
        Some(18) => Hdr::Hlg,
        Some(0 | 2) | None if hdr10_metadata => Hdr::Pq,
        _ => Hdr::None,
    }
}

// ---------------------------------------------------------------------------
// Codec private data

/// What a codec's configuration record says. `None` for what it doesn't.
#[derive(Debug, Clone, Default)]
pub(crate) struct Config {
    pub profile: Option<String>,
    pub bit_depth: Option<u8>,
    /// The transfer characteristics of the stream's own colour description.
    pub transfer: Option<u16>,
    /// The sample aspect ratio its SPS VUI gives, in lowest terms.
    pub sar: Option<(u32, u32)>,
    /// True when its SPS VUI describes the colour (see [`colour_stated`]).
    pub colour: bool,
}

/// A ratio in lowest terms; `None` when either side is 0.
fn lowest((h, v): (u32, u32)) -> Option<(u32, u32)> {
    (h > 0 && v > 0).then(|| {
        let g = crate::video::packet::gcd(h, v);
        (h / g, v / g)
    })
}

/// The sample aspect ratio an `aspect_ratio_idc` stands for (H.264 and
/// HEVC, table E-1); 255 is given in the stream itself.
fn sar_of_idc(idc: u32) -> Option<(u32, u32)> {
    const TABLE: [(u32, u32); 16] = [
        (1, 1),
        (12, 11),
        (10, 11),
        (16, 11),
        (40, 33),
        (24, 11),
        (20, 11),
        (32, 11),
        (80, 33),
        (18, 11),
        (15, 11),
        (64, 33),
        (160, 99),
        (4, 3),
        (3, 2),
        (2, 1),
    ];
    TABLE.get((idc as usize).checked_sub(1)?).copied()
}

/// What the start of a VUI (H.264 and HEVC alike) says: the sample aspect
/// ratio and the colour description.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Vui {
    pub sar: Option<(u32, u32)>,
    pub transfer: Option<u8>,
    /// The colour description: primaries, transfer and matrix, as H.273
    /// numbers them.
    pub colour: Option<[u8; 3]>,
}

/// Reads a VUI from its first flag up to its colour description; what was
/// read before a stream cut short is kept.
fn read_vui(r: &mut Bits, vui: &mut Vui) -> Option<()> {
    if r.flag()? {
        // aspect_ratio_info_present_flag
        let idc = r.u(8)?;
        vui.sar = if idc == 255 {
            lowest((r.u(16)?, r.u(16)?))
        } else {
            sar_of_idc(idc)
        };
    }
    if r.flag()? {
        r.skip(1)?; // overscan_appropriate_flag
    }
    if !r.flag()? {
        return None; // video_signal_type_present_flag
    }
    r.skip(4)?; // video_format, video_full_range_flag
    if !r.flag()? {
        return None; // colour_description_present_flag
    }
    let primaries = r.u(8)? as u8;
    let transfer = r.u(8)? as u8;
    vui.transfer = Some(transfer);
    vui.colour = Some([primaries, transfer, r.u(8)? as u8]);
    Some(())
}

/// A signed Exp-Golomb code, se(v).
fn se(r: &mut Bits) -> Option<i32> {
    let k = r.ue()? as i64;
    i32::try_from(if k % 2 == 1 { (k + 1) / 2 } else { -(k / 2) }).ok()
}

/// What the VUI of an H.264 SPS NAL unit (with its 1-byte header) says.
/// Walks the SPS up to the VUI, reading every field before it for its
/// length only.
pub(crate) fn h264_sps_vui(sps: &[u8]) -> Option<Vui> {
    let rbsp = unescape(sps.get(1..)?);
    let mut r = Bits::new(&rbsp);
    h264_sps_to_vui(&mut r)?;
    let mut vui = Vui::default();
    if r.flag()? {
        let _ = read_vui(&mut r, &mut vui);
    }
    Some(vui)
}

/// Reads an H.264 SPS's payload up to its `vui_parameters_present_flag`.
fn h264_sps_to_vui(r: &mut Bits) -> Option<()> {
    let profile_idc = r.u(8)? as u8;
    r.skip(16)?; // constraint flags, level
    r.ue()?; // seq_parameter_set_id
    if H264_HIGH.contains(&profile_idc) {
        let chroma = r.ue()?;
        if chroma == 3 {
            r.skip(1)?; // separate_colour_plane_flag
        }
        r.ue()?; // bit_depth_luma_minus8
        r.ue()?; // bit_depth_chroma_minus8
        r.skip(1)?; // qpprime_y_zero_transform_bypass_flag
        if r.flag()? {
            // seq_scaling_matrix_present_flag: each list present is read
            // through, delta by delta.
            for i in 0..if chroma == 3 { 12 } else { 8 } {
                if r.flag()? {
                    let (mut last, mut next) = (8i32, 8i32);
                    for _ in 0..if i < 6 { 16 } else { 64 } {
                        if next != 0 {
                            next = (last + se(r)? + 256).rem_euclid(256);
                        }
                        if next != 0 {
                            last = next;
                        }
                    }
                }
            }
        }
    }
    r.ue()?; // log2_max_frame_num_minus4
    match r.ue()? {
        0 => {
            r.ue()?; // log2_max_pic_order_cnt_lsb_minus4
        }
        1 => {
            r.skip(1)?; // delta_pic_order_always_zero_flag
            se(r)?; // offset_for_non_ref_pic
            se(r)?; // offset_for_top_to_bottom_field
            let n = r.ue()?;
            if n > 255 {
                return None;
            }
            for _ in 0..n {
                se(r)?; // offset_for_ref_frame
            }
        }
        _ => {}
    }
    r.ue()?; // max_num_ref_frames
    r.skip(1)?; // gaps_in_frame_num_value_allowed_flag
    r.ue()?; // pic_width_in_mbs_minus1
    r.ue()?; // pic_height_in_map_units_minus1
    if !r.flag()? {
        r.skip(1)?; // frame_mbs_only_flag unset: mb_adaptive_frame_field_flag
    }
    r.skip(1)?; // direct_8x8_inference_flag
    if r.flag()? {
        for _ in 0..4 {
            r.ue()?; // frame cropping offsets
        }
    }
    Some(())
}

/// An `avcC` record: profile from its header, bit depth from its extension or
/// else from the SPS.
pub(crate) fn avc_config(avcc: Option<&[u8]>) -> Config {
    let Some(a) = avcc.filter(|a| a.len() >= 7) else {
        return Config::default();
    };
    let profile_idc = a[1];
    let constraint_set1 = a[2] & 0x40 != 0;
    let profile = match profile_idc {
        66 if constraint_set1 => "Constrained Baseline",
        66 => "Baseline",
        77 => "Main",
        88 => "Extended",
        100 => "High",
        110 => "High 10",
        122 => "High 4:2:2",
        244 => "High 4:4:4 Predictive",
        44 => "CAVLC 4:4:4 Intra",
        83 => "Scalable Baseline",
        86 => "Scalable High",
        118 => "Multiview High",
        128 => "Stereo High",
        _ => "",
    };

    // The SPS list, then the PPS list, then (for the high profiles) the
    // extension with the chroma format and bit depths.
    let mut first_sps = None;
    let mut ext = None;
    let mut at = 6;
    let n_sps = (a[5] & 0x1F) as usize;
    let mut walk = || -> Option<()> {
        for i in 0..n_sps {
            let len = be16(a, at)? as usize;
            let sps = a.get(at + 2..at + 2 + len)?;
            if i == 0 {
                first_sps = Some(sps);
            }
            at += 2 + len;
        }
        let n_pps = *a.get(at)? as usize;
        at += 1;
        for _ in 0..n_pps {
            at += 2 + be16(a, at)? as usize;
        }
        if H264_HIGH.contains(&profile_idc) {
            ext = a.get(at..at + 4);
        }
        Some(())
    };
    let _ = walk();
    let bit_depth = ext
        .map(|e| (e[1] & 7) + 8)
        .or_else(|| first_sps.and_then(h264_sps_bit_depth));
    let vui = first_sps.and_then(h264_sps_vui).unwrap_or_default();
    Config {
        profile: (!profile.is_empty()).then(|| profile.to_string()),
        bit_depth,
        transfer: None,
        sar: vui.sar,
        colour: vui.colour.is_some_and(|c| colour_stated(c.map(u64::from))),
    }
}

/// The profiles whose SPS (and `avcC`) carry a chroma format and bit depths.
pub(crate) const H264_HIGH: [u8; 13] =
    [100, 110, 122, 244, 44, 83, 86, 118, 128, 138, 139, 134, 135];

fn h264_sps_bit_depth(sps: &[u8]) -> Option<u8> {
    u8::try_from(h264_sps_format(sps)?.1.checked_add(8)?)
        .ok()
        .filter(|d| *d <= 14)
}

/// An H.264 SPS's `chroma_format_idc`, `bit_depth_luma_minus8` and
/// `bit_depth_chroma_minus8`: 4:2:0 at 8 bits for the profiles that don't
/// carry them.
pub(crate) fn h264_sps_format(sps: &[u8]) -> Option<(u32, u32, u32)> {
    let rbsp = unescape(sps.get(1..)?);
    let mut r = Bits::new(&rbsp);
    let profile_idc = r.u(8)? as u8;
    r.skip(16)?; // constraint flags, level
    r.ue()?; // seq_parameter_set_id
    if !H264_HIGH.contains(&profile_idc) {
        return Some((1, 0, 0));
    }
    let chroma = r.ue()?;
    if chroma == 3 {
        r.skip(1)?; // separate_colour_plane_flag
    }
    let luma = r.ue()?;
    // A record cut short after the luma's depth has the same for chroma.
    Some((chroma, luma, r.ue().unwrap_or(luma)))
}

/// An `hvcC` record: profile and bit depth from its header, the transfer
/// characteristics from the VUI of the SPS it carries.
pub(crate) fn hevc_config(hvcc: Option<&[u8]>) -> Config {
    let Some(h) = hvcc.filter(|h| h.len() >= 23) else {
        return Config::default();
    };
    let profile = match h[1] & 0x1F {
        1 => Some("Main"),
        2 => Some("Main 10"),
        3 => Some("Main Still Picture"),
        4 => Some("Rext"),
        _ => None,
    };
    let vui = hevc_sps(h).and_then(hevc_sps_vui).unwrap_or_default();
    Config {
        profile: profile.map(String::from),
        bit_depth: Some((h[17] & 7) + 8),
        transfer: vui.transfer.map(u16::from),
        sar: vui.sar,
        colour: vui.colour.is_some_and(|c| colour_stated(c.map(u64::from))),
    }
}

/// The first SPS NAL unit (type 33) in an `hvcC`'s arrays.
fn hevc_sps(h: &[u8]) -> Option<&[u8]> {
    let mut at = 23;
    for _ in 0..*h.get(22)? {
        let kind = h.get(at)? & 0x3F;
        let count = be16(h, at + 1)?;
        at += 3;
        for _ in 0..count {
            let len = be16(h, at)? as usize;
            let nal = h.get(at + 2..at + 2 + len)?;
            if kind == 33 {
                return Some(nal);
            }
            at += 2 + len;
        }
    }
    None
}

/// The start of an HEVC SPS, up to its bit depths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HevcSpsHead {
    pub max_sub_layers_minus1: usize,
    pub temporal_id_nesting: bool,
    /// The general part of `profile_tier_level`: profile space, tier and
    /// profile; the 32 compatibility flags; 48 bits of constraint flags; the
    /// level. As an `hvcC` holds them.
    pub general: [u8; 12],
    pub chroma_format_idc: u32,
    pub bit_depth_luma_minus8: u32,
    pub bit_depth_chroma_minus8: u32,
}

/// The start of an HEVC SPS NAL unit (with its 2-byte header).
pub(crate) fn hevc_sps_format(nal: &[u8]) -> Option<HevcSpsHead> {
    let rbsp = unescape(nal.get(2..)?);
    hevc_sps_head(&mut Bits::new(&rbsp))
}

/// Reads an SPS's payload up to and including its bit depths.
fn hevc_sps_head(r: &mut Bits) -> Option<HevcSpsHead> {
    r.skip(4)?; // sps_video_parameter_set_id
    let max_sub_layers_minus1 = r.u(3)? as usize;
    let temporal_id_nesting = r.flag()?;

    // profile_tier_level: 88 bits of general profile, 8 of level.
    let mut general = [0u8; 12];
    for b in general.iter_mut() {
        *b = r.u(8)? as u8;
    }
    let mut present = [(false, false); 8];
    for sub_layer in present.iter_mut().take(max_sub_layers_minus1) {
        *sub_layer = (r.flag()?, r.flag()?);
    }
    if max_sub_layers_minus1 > 0 {
        r.skip(2 * (8 - max_sub_layers_minus1))?;
    }
    for &(profile, level) in present.iter().take(max_sub_layers_minus1) {
        if profile {
            r.skip(88)?;
        }
        if level {
            r.skip(8)?;
        }
    }

    r.ue()?; // sps_seq_parameter_set_id
    let chroma_format_idc = r.ue()?;
    if chroma_format_idc == 3 {
        r.skip(1)?; // separate_colour_plane_flag
    }
    r.ue()?; // pic_width_in_luma_samples
    r.ue()?; // pic_height_in_luma_samples
    if r.flag()? {
        for _ in 0..4 {
            r.ue()?; // conformance window offsets
        }
    }
    Some(HevcSpsHead {
        max_sub_layers_minus1,
        temporal_id_nesting,
        general,
        chroma_format_idc,
        bit_depth_luma_minus8: r.ue()?,
        bit_depth_chroma_minus8: r.ue()?,
    })
}

/// `transfer_characteristics` from the VUI of an HEVC SPS, when it has a
/// colour description.
#[cfg(test)]
fn hevc_sps_transfer(nal: &[u8]) -> Option<u8> {
    hevc_sps_vui(nal)?.transfer
}

/// What the VUI of an HEVC SPS says. Walks the SPS up to the VUI, so every
/// field before it is read for its length only.
fn hevc_sps_vui(nal: &[u8]) -> Option<Vui> {
    let rbsp = unescape(nal.get(2..)?);
    let mut r = Bits::new(&rbsp);
    hevc_sps_to_vui(&mut r)?;
    let mut vui = Vui::default();
    if r.flag()? {
        // vui_parameters_present_flag
        let _ = read_vui(&mut r, &mut vui);
    }
    Some(vui)
}

/// Reads an HEVC SPS's payload up to its `vui_parameters_present_flag`.
fn hevc_sps_to_vui(r: &mut Bits) -> Option<()> {
    let max_sub_layers_minus1 = hevc_sps_head(r)?.max_sub_layers_minus1;
    let poc_bits = r.ue()?.checked_add(4).filter(|b| *b <= 16)?;
    let from = if r.flag()? { 0 } else { max_sub_layers_minus1 };
    for _ in from..=max_sub_layers_minus1 {
        r.ue()?;
        r.ue()?;
        r.ue()?; // max_dec_pic_buffering, max_num_reorder, max_latency_increase
    }
    for _ in 0..6 {
        r.ue()?; // coding block and transform block sizes and depths
    }
    if r.flag()? && r.flag()? {
        // scaling_list_enabled_flag, sps_scaling_list_data_present_flag
        for size_id in 0..4 {
            let step = if size_id == 3 { 3 } else { 1 };
            for _matrix_id in (0..6).step_by(step) {
                if r.flag()? {
                    if size_id > 1 {
                        r.ue()?; // scaling_list_dc_coef_minus8
                    }
                    for _ in 0..(1usize << (4 + (size_id << 1))).min(64) {
                        r.ue()?; // scaling_list_delta_coef
                    }
                } else {
                    r.ue()?; // scaling_list_pred_matrix_id_delta
                }
            }
        }
    }
    r.skip(2)?; // amp_enabled_flag, sample_adaptive_offset_enabled_flag
    if r.flag()? {
        r.skip(8)?; // pcm sample bit depths
        r.ue()?;
        r.ue()?;
        r.skip(1)?;
    }
    let sets = r.ue()? as usize;
    if sets > 64 {
        return None;
    }
    let mut delta_pocs: Vec<u32> = Vec::with_capacity(sets);
    for i in 0..sets {
        let inter = i != 0 && r.flag()?;
        if inter {
            r.skip(1)?; // delta_rps_sign
            r.ue()?; // abs_delta_rps_minus1
            let mut kept = 0;
            for _ in 0..=*delta_pocs.get(i - 1)? {
                let used = r.flag()?;
                if used || r.flag()? {
                    kept += 1;
                }
            }
            delta_pocs.push(kept);
        } else {
            let negative = r.ue()?;
            let positive = r.ue()?;
            if negative > 16 || positive > 16 {
                return None;
            }
            for _ in 0..negative + positive {
                r.ue()?; // delta_poc_s0_minus1 or delta_poc_s1_minus1
                r.skip(1)?; // used_by_curr_pic_s0_flag or s1
            }
            delta_pocs.push(negative + positive);
        }
    }
    if r.flag()? {
        let n = r.ue()?;
        if n > 32 {
            return None;
        }
        for _ in 0..n {
            r.skip(poc_bits as usize + 1)?; // lt_ref_pic_poc_lsb_sps, used flag
        }
    }
    r.skip(2)?; // sps_temporal_mvp_enabled_flag, strong_intra_smoothing_enabled_flag
    Some(())
}

/// An H.264 SPS NAL unit (`hevc` false, a 1-byte header) or an HEVC one
/// (a 2-byte header) whose VUI says its colour is `colour` (primaries,
/// transfer and matrix, as H.273 numbers them) in limited range, the rest
/// of it unchanged. A VUI is added to an SPS that has none, saying only
/// that. `None` when the SPS can't be read up to its VUI.
///
/// Microsoft's software encoders ignore the colour on their media types
/// and write no colour description (R84), so Kiln writes it into each
/// encoded stream itself, as the container says it.
pub(crate) fn sps_with_colour(nal: &[u8], hevc: bool, colour: [u8; 3]) -> Option<Vec<u8>> {
    let header = if hevc { 2 } else { 1 };
    let rbsp = unescape(nal.get(header..)?);
    // The rbsp_stop_one_bit: the last bit set.
    let last = rbsp.iter().rposition(|&b| b != 0)?;
    let stop = last * 8 + 7 - rbsp[last].trailing_zeros() as usize;
    let mut r = Bits::new(&rbsp);
    if hevc {
        hevc_sps_to_vui(&mut r)?;
    } else {
        h264_sps_to_vui(&mut r)?;
    }
    let mut w = BitWriter::default();
    w.copy(&rbsp, 0, r.position());
    let had_vui = r.flag()?;
    w.put(1, 1); // vui_parameters_present_flag
    let mut video_format = 5; // unspecified
    if had_vui {
        let from = r.position();
        if r.flag()? && r.u(8)? == 255 {
            // aspect_ratio_info_present_flag, aspect_ratio_idc: Extended_SAR
            r.skip(32)?;
        }
        if r.flag()? {
            r.skip(1)?; // overscan_appropriate_flag
        }
        w.copy(&rbsp, from, r.position());
        if r.flag()? {
            // video_signal_type_present_flag
            video_format = r.u(3)?;
            r.skip(1)?; // video_full_range_flag
            if r.flag()? {
                r.skip(24)?; // the colour description
            }
        }
    } else {
        w.put(0, 2); // aspect_ratio_info_present_flag, overscan_info_present_flag
    }
    w.put(1, 1); // video_signal_type_present_flag
    w.put(video_format, 3);
    w.put(0, 1); // video_full_range_flag: limited
    w.put(1, 1); // colour_description_present_flag
    for c in colour {
        w.put(c as u32, 8);
    }
    if !had_vui {
        // H.264: chroma_loc_info_present_flag, timing_info_present_flag,
        // nal_ and vcl_hrd_parameters_present_flag, pic_struct_present_flag,
        // bitstream_restriction_flag. HEVC: chroma_loc_info_present_flag,
        // neutral_chroma_indication_flag, field_seq_flag,
        // frame_field_info_present_flag, default_display_window_flag,
        // vui_timing_info_present_flag, bitstream_restriction_flag.
        w.put(0, if hevc { 7 } else { 6 });
    }
    if r.position() > stop {
        return None;
    }
    w.copy(&rbsp, r.position(), stop);
    w.put(1, 1); // rbsp_stop_one_bit
    let mut out = nal[..header].to_vec();
    out.extend(escape(&w.finish()));
    Some(out)
}

/// Writes bits, most significant first.
#[derive(Default)]
struct BitWriter {
    bytes: Vec<u8>,
    bits: usize,
}

impl BitWriter {
    /// The low `n` bits of `v`.
    fn put(&mut self, v: u32, n: usize) {
        for i in (0..n).rev() {
            if self.bits % 8 == 0 {
                self.bytes.push(0);
            }
            if (v >> i) & 1 == 1 {
                let at = self.bytes.len() - 1;
                self.bytes[at] |= 0x80 >> (self.bits % 8);
            }
            self.bits += 1;
        }
    }

    /// Bits `from..to` of `data`.
    fn copy(&mut self, data: &[u8], from: usize, to: usize) {
        for at in from..to {
            self.put(((data[at / 8] >> (7 - at % 8)) & 1) as u32, 1);
        }
    }

    /// The bytes, the last one padded with zeros.
    fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

/// An RBSP with emulation prevention bytes put in: a 3 after two zero
/// bytes when the next is 0 to 3, the inverse of [`unescape`].
fn escape(rbsp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rbsp.len() + rbsp.len() / 64);
    let mut zeros = 0;
    for &b in rbsp {
        if zeros >= 2 && b <= 3 {
            out.push(3);
            zeros = 0;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

/// An `av1C` record: the profile and bit depth in its first bytes.
pub(crate) fn av1_config(av1c: Option<&[u8]>) -> Config {
    let Some(a) = av1c.filter(|a| a.len() >= 4 && a[0] == 0x81) else {
        return Config::default();
    };
    let profile = match a[1] >> 5 {
        0 => "Main",
        1 => "High",
        2 => "Professional",
        _ => "",
    };
    let high = a[2] & 0x40 != 0;
    let twelve = a[2] & 0x20 != 0;
    Config {
        profile: (!profile.is_empty()).then(|| profile.to_string()),
        bit_depth: Some(match (high, twelve) {
            (true, true) => 12,
            (true, false) => 10,
            _ => 8,
        }),
        transfer: None,
        sar: None,
        colour: false,
    }
}

/// A `vpcC` record (a full box): profile, bit depth and transfer function.
pub(crate) fn vp9_config(vpcc: Option<&[u8]>) -> Config {
    let Some(v) = vpcc.filter(|v| v.len() >= 10) else {
        return Config::default();
    };
    let depth = v[6] >> 4;
    Config {
        profile: Some(format!("Profile {}", v[4])),
        bit_depth: (8..=12).contains(&depth).then_some(depth),
        transfer: Some(v[8] as u16),
        sar: None,
        colour: colour_stated([v[7], v[8], v[9]].map(u64::from)),
    }
}

// ---------------------------------------------------------------------------
// Bits

/// A NAL unit's payload with the emulation prevention bytes (the 3 in
/// 00 00 03) taken out.
pub(crate) fn unescape(nal: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nal.len());
    let mut zeros = 0;
    for &b in nal {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

/// Reads bits, most significant first, and Exp-Golomb codes.
pub(crate) struct Bits<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Bits<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Bits<'a> {
        Bits { data, at: 0 }
    }

    /// How many bits have been read.
    pub(crate) fn position(&self) -> usize {
        self.at
    }

    fn bit(&mut self) -> Option<u32> {
        let byte = *self.data.get(self.at / 8)?;
        let bit = (byte >> (7 - self.at % 8)) & 1;
        self.at += 1;
        Some(bit as u32)
    }

    pub(crate) fn flag(&mut self) -> Option<bool> {
        self.bit().map(|b| b == 1)
    }

    /// `n` bits, at most 32.
    pub(crate) fn u(&mut self, n: usize) -> Option<u32> {
        let mut v = 0u64;
        for _ in 0..n {
            v = (v << 1) | self.bit()? as u64;
        }
        u32::try_from(v).ok()
    }

    pub(crate) fn skip(&mut self, n: usize) -> Option<()> {
        let to = self.at.checked_add(n)?;
        (to <= self.data.len() * 8).then(|| self.at = to)
    }

    /// An unsigned Exp-Golomb code, ue(v).
    pub(crate) fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while !self.flag()? {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        let rest = self.u(zeros)? as u64;
        u32::try_from((1u64 << zeros) - 1 + rest).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_matrix_turns_to_the_nearest_quarter_clockwise() {
        let one = 0x10000;
        let m = |a: i32, b: i32, c: i32, d: i32| [a, b, 0, c, d, 0, 0, 0, 0x4000_0000];
        assert_eq!(rotation(&m(one, 0, 0, one)), 0);
        assert_eq!(rotation(&m(0, one, -one, 0)), 90);
        assert_eq!(rotation(&m(-one, 0, 0, -one)), 180);
        assert_eq!(rotation(&m(0, -one, one, 0)), 270);
        // A mirrored or skewed matrix still lands on a quarter.
        assert_eq!(rotation(&m(one, 0, 0, -one)), 0);
        assert_eq!(rotation(&m(one - 1000, 1000, -1000, one - 1000)), 0);
        assert_eq!(rotation(&[0; 9]), 0);
    }

    #[test]
    fn exp_golomb_codes_read_back() {
        // 1 | 010 | 011 | 00100 | 00111 -> 0, 1, 2, 3, 6 (ue), padded.
        let bytes = [0b1010_0110, 0b0100_0011, 0b1000_0000];
        let mut r = Bits::new(&bytes);
        assert_eq!(r.ue(), Some(0));
        assert_eq!(r.ue(), Some(1));
        assert_eq!(r.ue(), Some(2));
        assert_eq!(r.ue(), Some(3));
        assert_eq!(r.ue(), Some(6));
        // 32 zeros: not a code.
        assert_eq!(Bits::new(&[0; 8]).ue(), None);
        // Reading past the end is None, not a panic.
        assert_eq!(Bits::new(&[]).u(1), None);
        assert_eq!(Bits::new(&[0xFF]).skip(9), None);
    }

    #[test]
    fn emulation_prevention_bytes_are_taken_out() {
        assert_eq!(unescape(&[0, 0, 3, 1, 0, 0, 3]), vec![0, 0, 1, 0, 0]);
        assert_eq!(unescape(&[3, 0, 3, 0, 0, 0, 3]), vec![3, 0, 3, 0, 0, 0]);
    }

    #[test]
    fn hdr_follows_the_transfer_then_the_static_metadata() {
        assert_eq!(hdr_of(Some(16), false), Hdr::Pq);
        assert_eq!(hdr_of(Some(18), false), Hdr::Hlg);
        assert_eq!(hdr_of(Some(1), true), Hdr::None);
        assert_eq!(hdr_of(Some(2), true), Hdr::Pq);
        assert_eq!(hdr_of(None, true), Hdr::Pq);
        assert_eq!(hdr_of(None, false), Hdr::None);
    }

    #[test]
    fn an_h264_sps_gives_its_bit_depth() {
        // High profile, 8 bits: profile 100, flags, level 13, id 0, chroma 1, luma 0.
        // ue bits: id 0 -> 1, chroma 1 -> 010, luma 0 -> 1, chroma 0 -> 1
        let sps = [0x67, 100, 0, 13, 0b1010_1100];
        assert_eq!(h264_sps_bit_depth(&sps), Some(8));
        // High 10: luma_minus8 = 2 -> 011.
        let sps = [0x67, 110, 0, 13, 0b1010_0110];
        assert_eq!(h264_sps_bit_depth(&sps), Some(10));
        // Main has no field for it.
        assert_eq!(h264_sps_bit_depth(&[0x67, 77, 0, 30, 0x80]), Some(8));
        assert_eq!(h264_sps_bit_depth(&[0x67, 100]), None);
    }
}

/// Synthetic files and bit strings, for what no fixture has: `colr`, `mdcv`,
/// `clli`, HLG, Matroska `Colour`, and the rarer branches of the SPS walker.
#[cfg(test)]
mod synthetic {
    use super::*;
    use crate::mkvread::{Headers, Track, Video, TYPE_VIDEO};

    fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(body);
        v
    }

    fn colr(kind: &[u8; 4], transfer: u16) -> Vec<u8> {
        let mut body = kind.to_vec();
        body.extend(1u16.to_be_bytes()); // primaries
        body.extend(transfer.to_be_bytes());
        body.extend(1u16.to_be_bytes()); // matrix
        body.push(0);
        bx(b"colr", &body)
    }

    /// A `moov` with one video track of 1,000-byte samples whose `stts` is as
    /// given (the time scale is 1,000), with these boxes in its `hvc1` sample
    /// entry.
    fn moov(entry_boxes: &[u8], stts: &[(u32, u32)]) -> Vec<u8> {
        let mut mvhd = vec![0u8; 100];
        mvhd[12..16].copy_from_slice(&1000u32.to_be_bytes());
        let mut tkhd = vec![0u8; 84];
        for (i, v) in [0x10000u32, 0, 0, 0, 0x10000, 0, 0, 0, 0x4000_0000]
            .iter()
            .enumerate()
        {
            tkhd[40 + i * 4..44 + i * 4].copy_from_slice(&v.to_be_bytes());
        }
        let mut mdhd = vec![0u8; 24];
        mdhd[12..16].copy_from_slice(&1000u32.to_be_bytes());
        let mut hdlr = vec![0u8; 8];
        hdlr.extend(b"vide");
        hdlr.extend([0u8; 12]);
        let mut sample = vec![0u8; 78];
        sample[24..26].copy_from_slice(&64u16.to_be_bytes());
        sample[26..28].copy_from_slice(&48u16.to_be_bytes());
        sample.extend(entry_boxes);
        let mut stsd = vec![0, 0, 0, 0, 0, 0, 0, 1];
        stsd.extend(bx(b"hvc1", &sample));
        let mut table = vec![0u8; 4];
        table.extend((stts.len() as u32).to_be_bytes());
        let mut samples = 0u32;
        for &(count, delta) in stts {
            table.extend(count.to_be_bytes());
            table.extend(delta.to_be_bytes());
            samples += count;
        }
        let mut stsz = vec![0u8; 4];
        stsz.extend(1000u32.to_be_bytes());
        stsz.extend(samples.to_be_bytes());
        let stbl = [bx(b"stsd", &stsd), bx(b"stts", &table), bx(b"stsz", &stsz)].concat();
        let mdia = [
            bx(b"mdhd", &mdhd),
            bx(b"hdlr", &hdlr),
            bx(b"minf", &bx(b"stbl", &stbl)),
        ]
        .concat();
        let trak = [bx(b"tkhd", &tkhd), bx(b"mdia", &mdia)].concat();
        [bx(b"mvhd", &mvhd), bx(b"trak", &trak)].concat()
    }

    fn hdr_of_boxes(entry_boxes: &[u8]) -> Hdr {
        from_moov(&moov(entry_boxes, &[(30, 33)])).unwrap().hdr
    }

    #[test]
    fn a_colr_box_gives_the_dynamic_range() {
        assert_eq!(hdr_of_boxes(&colr(b"nclx", 16)), Hdr::Pq);
        assert_eq!(hdr_of_boxes(&colr(b"nclx", 18)), Hdr::Hlg);
        assert_eq!(hdr_of_boxes(&colr(b"nclx", 1)), Hdr::None);
        // QuickTime's nclc says the same with the same code points.
        assert_eq!(hdr_of_boxes(&colr(b"nclc", 16)), Hdr::Pq);
        // An ICC profile says nothing about the transfer function.
        assert_eq!(
            hdr_of_boxes(&bx(b"colr", b"prof\0\0\0\0\0\0\0\0\0")),
            Hdr::None
        );
        // The first colour description wins over a later one.
        let both = [colr(b"nclx", 18), colr(b"nclx", 16)].concat();
        assert_eq!(hdr_of_boxes(&both), Hdr::Hlg);
    }

    #[test]
    fn static_hdr10_metadata_alone_means_pq() {
        let mdcv = bx(b"mdcv", &[0u8; 24]);
        let clli = bx(b"clli", &[0u8; 4]);
        assert_eq!(hdr_of_boxes(&mdcv), Hdr::Pq);
        assert_eq!(hdr_of_boxes(&clli), Hdr::Pq);
        // A stated SDR transfer outweighs it; a stated HDR one stands.
        let sdr = [colr(b"nclx", 1), mdcv].concat();
        assert_eq!(hdr_of_boxes(&sdr), Hdr::None);
        let hlg = [colr(b"nclx", 18), clli.clone()].concat();
        assert_eq!(hdr_of_boxes(&hlg), Hdr::Hlg);
        // "Unspecified" (2) states nothing.
        let unspecified = [colr(b"nclx", 2), clli].concat();
        assert_eq!(hdr_of_boxes(&unspecified), Hdr::Pq);
        // Boxes too short to be the metadata don't count.
        assert_eq!(hdr_of_boxes(&bx(b"mdcv", &[0u8; 23])), Hdr::None);
        assert_eq!(hdr_of_boxes(&bx(b"clli", &[0u8; 3])), Hdr::None);
    }

    fn matroska(transfer: Option<u64>, hdr10_metadata: bool) -> Hdr {
        let h = Headers {
            timestamp_scale: 1_000_000,
            duration: Some(1000.0),
            date_utc: None,
            title: None,
            tracks: vec![Track {
                kind: TYPE_VIDEO,
                codec_id: "V_MPEGH/ISO/HEVC".into(),
                video: Some(Video {
                    width: 64,
                    height: 48,
                    transfer,
                    hdr10_metadata,
                    ..Video::default()
                }),
                ..Track::default()
            }],
        };
        from_headers(&h).unwrap().hdr
    }

    #[test]
    fn matroska_colour_gives_the_dynamic_range() {
        assert_eq!(matroska(Some(16), false), Hdr::Pq);
        assert_eq!(matroska(Some(18), false), Hdr::Hlg);
        assert_eq!(matroska(Some(1), false), Hdr::None);
        assert_eq!(matroska(None, false), Hdr::None);
        assert_eq!(matroska(None, true), Hdr::Pq);
        assert_eq!(matroska(Some(1), true), Hdr::None);
    }

    fn rate_of(stts: &[(u32, u32)]) -> (Option<f64>, bool) {
        let v = from_moov(&moov(&[], stts)).unwrap();
        (v.fps, v.variable_rate)
    }

    #[test]
    fn a_last_frame_of_its_own_length_is_not_a_variable_rate() {
        assert!(!rate_of(&[(59, 1000), (1, 1001)]).1);
        assert!(!rate_of(&[(60, 1000)]).1);
        // Real changes still count, with or without such a last frame.
        assert!(rate_of(&[(30, 1000), (30, 500)]).1);
        assert!(rate_of(&[(30, 1000), (29, 500), (1, 700)]).1);
        // Two or more frames in the last entry are not "a last frame".
        assert!(rate_of(&[(30, 1000), (2, 500)]).1);
        // Empty entries are nothing.
        assert!(!rate_of(&[(30, 1000), (0, 7), (1, 9)]).1);
    }

    #[test]
    fn an_absurd_frame_rate_is_none() {
        // 1,000 ticks a second: 1,000 samples of 1 tick are 1,000 fps.
        assert_eq!(rate_of(&[(1000, 1)]).0, Some(1000.0));
        assert_eq!(sane_fps(1000.5), None);
        assert_eq!(sane_fps(0.0), None);
        assert_eq!(rate_of(&[(30, 0)]).0, None);
        assert_eq!(sane_fps(f64::INFINITY), None);
        assert_eq!(sane_fps(f64::NAN), None);
        assert_eq!(sane_fps(29.97), Some(29.97));
        // Matroska: 100,000 ns a frame is 10,000 fps.
        let mut h = Headers {
            timestamp_scale: 1_000_000,
            duration: None,
            date_utc: None,
            title: None,
            tracks: vec![Track {
                kind: TYPE_VIDEO,
                codec_id: "V_AV1".into(),
                default_duration: Some(100_000),
                video: Some(Video {
                    width: 64,
                    height: 48,
                    ..Video::default()
                }),
                ..Track::default()
            }],
        };
        assert_eq!(from_headers(&h).unwrap().fps, None);
        h.tracks[0].default_duration = Some(33_333_333);
        assert!(from_headers(&h).unwrap().fps.is_some());
    }

    // ---- HEVC SPS bit strings

    #[derive(Default)]
    struct W(Vec<u8>);

    impl W {
        fn u(&mut self, n: usize, v: u64) -> &mut W {
            for i in (0..n).rev() {
                self.0.push(((v >> i) & 1) as u8);
            }
            self
        }
        fn zeros(&mut self, n: usize) -> &mut W {
            self.0.extend(std::iter::repeat_n(0, n));
            self
        }
        fn ue(&mut self, v: u32) -> &mut W {
            let x = v as u64 + 1;
            let len = 64 - x.leading_zeros() as usize;
            self.zeros(len - 1).u(len, x)
        }
        /// The bytes of a NAL unit: a header, then the payload with its
        /// trailing bit and emulation prevention.
        fn nal(&self) -> Vec<u8> {
            let mut bits = self.0.clone();
            bits.push(1);
            while bits.len() % 8 != 0 {
                bits.push(0);
            }
            let mut out = vec![0x42, 0x01];
            let mut zeros = 0;
            for byte in bits
                .chunks(8)
                .map(|c| c.iter().fold(0u8, |a, &b| (a << 1) | b))
            {
                if zeros >= 2 && byte <= 3 {
                    out.push(3);
                    zeros = 0;
                }
                zeros = if byte == 0 { zeros + 1 } else { 0 };
                out.push(byte);
            }
            out
        }
    }

    struct Sps {
        /// One per sub-layer above the first: whether it carries a profile and a level.
        sub_layers: Vec<(bool, bool)>,
        ordering_info: bool,
        /// 0: no scaling lists; 1: enabled with none sent; 2: sent.
        scaling: u8,
        pcm: bool,
        rps: bool,
        long_term: bool,
        vui: bool,
        aspect: Option<u8>,
        overscan: bool,
        signal: bool,
        colour: bool,
        transfer: u8,
    }

    impl Sps {
        fn plain(transfer: u8) -> Sps {
            Sps {
                sub_layers: vec![],
                ordering_info: false,
                scaling: 0,
                pcm: false,
                rps: false,
                long_term: false,
                vui: true,
                aspect: None,
                overscan: false,
                signal: true,
                colour: true,
                transfer,
            }
        }

        fn everything(transfer: u8) -> Sps {
            Sps {
                sub_layers: vec![(true, true), (false, true)],
                ordering_info: true,
                scaling: 2,
                pcm: true,
                rps: true,
                long_term: true,
                aspect: Some(255),
                overscan: true,
                ..Sps::plain(transfer)
            }
        }

        fn build(&self) -> Vec<u8> {
            let max = self.sub_layers.len();
            let mut w = W::default();
            w.u(4, 0).u(3, max as u64).u(1, 1);
            // profile_tier_level: the general part, then the sub-layers'.
            w.u(2, 0).u(1, 0).u(5, 2).u(32, 0x2000_0000).u(4, 0b1001);
            w.zeros(43).u(1, 0).u(8, 93);
            for &(profile, level) in &self.sub_layers {
                w.u(1, profile as u64).u(1, level as u64);
            }
            if max > 0 {
                w.zeros(2 * (8 - max));
            }
            for &(profile, level) in &self.sub_layers {
                if profile {
                    w.zeros(88);
                }
                if level {
                    w.u(8, 90);
                }
            }
            w.ue(0).ue(1).ue(1920).ue(1088);
            w.u(1, 1).ue(0).ue(0).ue(0).ue(4); // conformance window
            w.ue(2).ue(2).ue(4); // 10-bit; log2_max_poc_lsb_minus4 = 4
            w.u(1, self.ordering_info as u64);
            let from = if self.ordering_info { 0 } else { max };
            for _ in from..=max {
                w.ue(4).ue(2).ue(0);
            }
            w.ue(0).ue(3).ue(0).ue(3).ue(1).ue(1);
            match self.scaling {
                0 => {
                    w.u(1, 0);
                }
                1 => {
                    w.u(1, 1).u(1, 0);
                }
                _ => {
                    w.u(1, 1).u(1, 1);
                    for size_id in 0..4usize {
                        let step = if size_id == 3 { 3 } else { 1 };
                        for matrix_id in (0..6).step_by(step) {
                            if (size_id + matrix_id) % 2 == 0 {
                                w.u(1, 0).ue(0);
                            } else {
                                w.u(1, 1);
                                if size_id > 1 {
                                    w.ue(3);
                                }
                                for _ in 0..(1usize << (4 + 2 * size_id)).min(64) {
                                    w.ue(1);
                                }
                            }
                        }
                    }
                }
            }
            w.u(1, 1).u(1, 0); // amp, sao
            w.u(1, self.pcm as u64);
            if self.pcm {
                w.u(4, 7).u(4, 7).ue(0).ue(2).u(1, 1);
            }
            if self.rps {
                w.ue(3);
                // Set 0 codes itself: 2 before, 1 after.
                w.ue(2).ue(1);
                for _ in 0..3 {
                    w.ue(0).u(1, 1);
                }
                // Set 1 predicts from set 0 (3 pictures): four flags, of which
                // used, use_delta, neither, used keep three.
                w.u(1, 1).u(1, 0).ue(0);
                w.u(1, 1).u(1, 0).u(1, 1).u(1, 0).u(1, 0).u(1, 1);
                // Set 2 predicts from set 1 (3 pictures): four used flags.
                w.u(1, 1).u(1, 1).ue(1);
                w.u(1, 1).u(1, 1).u(1, 1).u(1, 1);
            } else {
                w.ue(0);
            }
            w.u(1, self.long_term as u64);
            if self.long_term {
                w.ue(2);
                for _ in 0..2 {
                    w.u(8, 0xAB).u(1, 1);
                }
            }
            w.u(1, 1).u(1, 1); // temporal mvp, strong intra smoothing
            w.u(1, self.vui as u64);
            if self.vui {
                w.u(1, self.aspect.is_some() as u64);
                if let Some(idc) = self.aspect {
                    w.u(8, idc as u64);
                    if idc == 255 {
                        w.u(32, 0x0001_0001);
                    }
                }
                w.u(1, self.overscan as u64);
                if self.overscan {
                    w.u(1, 1);
                }
                w.u(1, self.signal as u64);
                if self.signal {
                    w.u(3, 5).u(1, 0).u(1, self.colour as u64);
                    if self.colour {
                        w.u(8, 9).u(8, self.transfer as u64).u(8, 9);
                    }
                }
            }
            w.nal()
        }
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// R84: whatever an HEVC SPS's VUI said of the video signal, it now
    /// says the colour, and not a bit else changes.
    #[test]
    fn an_hevc_sps_is_made_to_say_its_colour() {
        let shapes: [fn() -> Sps; 2] = [|| Sps::plain(1), || Sps::everything(1)];
        for shape in shapes {
            let want = Sps {
                transfer: 16,
                ..shape()
            }
            .build();
            for (signal, colour) in [(false, false), (true, false), (true, true)] {
                let sps = Sps {
                    signal,
                    colour,
                    ..shape()
                }
                .build();
                let got = sps_with_colour(&sps, true, [9, 16, 9]).unwrap();
                assert_eq!(got, want, "signal {signal}, colour {colour}");
            }
            // With no VUI, one is added that says only the colour.
            let bare = Sps {
                vui: false,
                ..shape()
            }
            .build();
            let got = sps_with_colour(&bare, true, [6, 6, 6]).unwrap();
            let vui = hevc_sps_vui(&got).unwrap();
            assert_eq!((vui.colour, vui.sar), (Some([6, 6, 6]), None));
            assert_eq!(hevc_sps_format(&got), hevc_sps_format(&bare));
        }
    }

    /// An H.264 High SPS up to its VUI flag (a scaling list and picture
    /// order count type 1 to walk through), then `vui` (`None`: no VUI).
    fn h264_sps(vui: Option<&dyn Fn(&mut W)>) -> Vec<u8> {
        let mut w = W(Vec::new());
        w.u(8, 100).u(8, 0).u(8, 40).ue(0); // profile, flags, level, id
        w.ue(1).ue(0).ue(0).u(1, 0); // 4:2:0, 8-bit, no bypass
        w.u(1, 1).u(1, 1); // a scaling matrix, its first list
        for _ in 0..16 {
            w.ue(1); // delta_scale +1
        }
        for _ in 1..8 {
            w.u(1, 0);
        }
        w.ue(0).ue(1).u(1, 0).ue(0).ue(0).ue(2).ue(1).ue(2); // poc type 1
        w.ue(3).u(1, 0).ue(119).ue(67); // refs, gaps, 1920x1088
        w.u(1, 1).u(1, 1).u(1, 0); // frame_mbs_only, direct_8x8, no crop
        match vui {
            Some(vui) => {
                w.u(1, 1);
                vui(&mut w);
            }
            None => {
                w.u(1, 0);
            }
        }
        let mut sps = vec![0x67];
        sps.extend_from_slice(&w.nal()[2..]);
        sps
    }

    /// R84: an H.264 SPS with no VUI gains one that says only the colour
    /// (then its six flags after the video signal type, all off); one with
    /// an Extended_SAR aspect, timing and restrictions keeps every bit of
    /// them, byte for byte.
    #[test]
    fn an_h264_sps_is_made_to_say_its_colour() {
        let signal = |w: &mut W| {
            w.u(1, 1).u(3, 5).u(1, 0).u(1, 1); // video signal type, colour
            w.u(8, 6).u(8, 6).u(8, 6);
        };
        let bare = h264_sps(None);
        let want = h264_sps(Some(&|w: &mut W| {
            w.u(1, 0).u(1, 0); // no aspect, no overscan
            signal(w);
            w.zeros(6); // chroma location, timing, two HRDs, pic_struct, restrictions
        }));
        assert_eq!(sps_with_colour(&bare, false, [6, 6, 6]), Some(want.clone()));
        let vui = h264_sps_vui(&want).unwrap();
        assert_eq!((vui.colour, vui.sar), (Some([6, 6, 6]), None));
        assert_eq!(h264_sps_format(&want), h264_sps_format(&bare));

        // Extended_SAR 64:48, overscan, no signal type; then a chroma
        // location, timing (1001/60000, fixed) and bitstream restrictions.
        let rest = |w: &mut W| {
            w.u(1, 1).ue(0).ue(0); // chroma_loc_info
            w.u(1, 1).u(32, 1001).u(32, 60000).u(1, 1); // timing
            w.u(1, 0).u(1, 0).u(1, 0); // no HRDs, no pic_struct
            w.u(1, 1).u(1, 1).ue(2).ue(1).ue(16).ue(16).ue(2).ue(4); // restrictions
        };
        let head = |w: &mut W| {
            w.u(1, 1).u(8, 255).u(16, 64).u(16, 48); // Extended_SAR
            w.u(1, 1).u(1, 0); // overscan_info, not appropriate
        };
        let without = h264_sps(Some(&|w: &mut W| {
            head(w);
            w.u(1, 0);
            rest(w);
        }));
        let with = h264_sps(Some(&|w: &mut W| {
            head(w);
            signal(w);
            rest(w);
        }));
        assert_eq!(
            sps_with_colour(&without, false, [6, 6, 6]),
            Some(with.clone())
        );
        let vui = h264_sps_vui(&with).unwrap();
        assert_eq!((vui.colour, vui.sar), (Some([6, 6, 6]), Some((4, 3))));
        // Said again, nothing changes.
        assert_eq!(sps_with_colour(&with, false, [6, 6, 6]), Some(with));
    }

    /// R84, on the parameter sets the encoders on this PC wrote: each says
    /// the colour asked for, the rest unchanged, and one that already said
    /// it comes back byte for byte.
    #[test]
    fn encoders_sps_are_made_to_say_their_colour() {
        // Microsoft's software H.264 encoder: a VUI with timing, no signal.
        let ms_h264 = hex("6764001fac2cac05005bb0110000030001000003003c0da0884538");
        // Microsoft's software HEVC encoder: the video signal type, but no
        // colour description.
        let ms_hevc =
            hex("420101014000000300b0000003000003005da00280802d172a4edcb9808080000032000005dc04");
        // NVIDIA's, asked for BT.601: SMPTE 170M primaries, BT.709's
        // transfer, the matrix as BT.470 BG; then timing and restrictions.
        let nv_h264 = hex("6764000dac2b40a0fd80b5060105400000fa00003a9803c708a8");
        // NVIDIA's HDR10.
        let nv_hevc = hex(
            "42010102200000030090000003000003003ca00a080407c4b65ad2908464b0c050848804820000030002000003003c10",
        );
        assert_eq!(h264_sps_vui(&nv_h264).unwrap().colour, Some([6, 1, 5]));
        assert_eq!(hevc_sps_vui(&nv_hevc).unwrap().colour, Some([9, 16, 9]));
        assert_eq!(h264_sps_vui(&ms_h264).unwrap().colour, None);
        assert_eq!(hevc_sps_vui(&ms_hevc).unwrap().colour, None);
        assert_eq!(
            sps_with_colour(&nv_h264, false, [6, 1, 5]),
            Some(nv_h264.clone())
        );
        assert_eq!(
            sps_with_colour(&nv_hevc, true, [9, 16, 9]),
            Some(nv_hevc.clone())
        );
        for colour in [[1, 1, 1], [6, 6, 6], [9, 16, 9]] {
            for (sps, hevc) in [
                (&ms_h264, false),
                (&nv_h264, false),
                (&ms_hevc, true),
                (&nv_hevc, true),
            ] {
                let got = sps_with_colour(sps, hevc, colour).unwrap();
                let (old, new) = if hevc {
                    assert_eq!(hevc_sps_format(&got), hevc_sps_format(sps));
                    (hevc_sps_vui(sps).unwrap(), hevc_sps_vui(&got).unwrap())
                } else {
                    assert_eq!(h264_sps_format(&got), h264_sps_format(sps));
                    (h264_sps_vui(sps).unwrap(), h264_sps_vui(&got).unwrap())
                };
                assert_eq!((new.colour, new.sar), (Some(colour), old.sar));
                // Said again, nothing changes; and back as NVIDIA had it, its
                // own bytes.
                assert_eq!(sps_with_colour(&got, hevc, colour), Some(got.clone()));
                if sps == &nv_h264 {
                    assert_eq!(
                        sps_with_colour(&got, false, [6, 1, 5]),
                        Some(nv_h264.clone())
                    );
                }
                if sps == &nv_hevc {
                    assert_eq!(
                        sps_with_colour(&got, true, [9, 16, 9]),
                        Some(nv_hevc.clone())
                    );
                }
            }
        }
        // A unit that isn't an SPS, or is cut short, isn't one to change.
        assert_eq!(sps_with_colour(&nv_h264[..4], false, [1, 1, 1]), None);
        assert_eq!(sps_with_colour(&[], true, [1, 1, 1]), None);
    }

    /// An emulation prevention byte goes in where the payload needs one and
    /// comes out again.
    #[test]
    fn escaping_undoes_unescaping() {
        for rbsp in [
            vec![0, 0, 0, 1],
            vec![0, 0, 3, 0, 0, 2, 9],
            vec![1, 0, 0, 4, 0, 0],
            vec![],
        ] {
            assert_eq!(unescape(&escape(&rbsp)), rbsp, "{rbsp:?}");
        }
        assert_eq!(escape(&[0, 0, 1]), [0, 0, 3, 1]);
        assert_eq!(escape(&[0, 0, 4]), [0, 0, 4]);
    }

    #[test]
    fn the_vui_gives_the_sample_aspect_ratio() {
        // HEVC: a table entry, a ratio given in the stream, or none.
        let hevc = |idc| {
            let mut s = Sps::plain(1);
            s.aspect = Some(idc);
            hevc_sps_vui(&s.build()).unwrap().sar
        };
        assert_eq!(hevc(14), Some((4, 3)));
        assert_eq!(hevc(1), Some((1, 1)));
        assert_eq!(hevc(255), Some((1, 1)));
        assert_eq!(hevc(0), None);

        // H.264 High: a scaling list and picture order count type 1 to read
        // through first, then the VUI's aspect.
        let h264 = |aspect: &dyn Fn(&mut W)| {
            let mut w = W(Vec::new());
            w.u(8, 100).u(8, 0).u(8, 40).ue(0); // profile, flags, level, id
            w.ue(1).ue(0).ue(0).u(1, 0); // 4:2:0, 8-bit, no bypass
            w.u(1, 1); // seq_scaling_matrix_present_flag
            w.u(1, 1);
            for _ in 0..16 {
                w.ue(1); // delta_scale +1
            }
            for _ in 1..8 {
                w.u(1, 0);
            }
            w.ue(0).ue(1).u(1, 0).ue(0).ue(0).ue(2).ue(1).ue(2); // poc type 1
            w.ue(3).u(1, 0).ue(119).ue(67); // refs, gaps, 1920x1088
            w.u(1, 1).u(1, 1).u(1, 0); // frame_mbs_only, direct_8x8, no crop
            w.u(1, 1); // vui_parameters_present_flag
            aspect(&mut w);
            w.u(1, 0).u(1, 0); // no overscan, no video signal type
            let mut sps = vec![0x67];
            sps.extend_from_slice(&w.nal()[2..]);
            h264_sps_vui(&sps).unwrap().sar
        };
        assert_eq!(
            h264(&|w| {
                w.u(1, 1).u(8, 14);
            }),
            Some((4, 3))
        );
        assert_eq!(
            h264(&|w| {
                w.u(1, 1).u(8, 255).u(16, 64).u(16, 48);
            }),
            Some((4, 3))
        );
        assert_eq!(
            h264(&|w| {
                w.u(1, 0);
            }),
            None
        );
    }

    #[test]
    fn the_sps_walker_finds_the_transfer_through_every_branch() {
        assert_eq!(hevc_sps_transfer(&Sps::plain(16).build()), Some(16));
        assert_eq!(hevc_sps_transfer(&Sps::everything(18).build()), Some(18));
        // One branch at a time.
        let with = |f: &dyn Fn(&mut Sps)| {
            let mut s = Sps::plain(14);
            f(&mut s);
            hevc_sps_transfer(&s.build())
        };
        assert_eq!(with(&|s| s.sub_layers = vec![(true, false)]), Some(14));
        assert_eq!(with(&|s| s.sub_layers = vec![(false, false); 3]), Some(14));
        assert_eq!(
            with(&|s| {
                s.sub_layers = vec![(false, true); 2];
                s.ordering_info = true;
            }),
            Some(14)
        );
        assert_eq!(with(&|s| s.scaling = 1), Some(14));
        assert_eq!(with(&|s| s.scaling = 2), Some(14));
        assert_eq!(with(&|s| s.pcm = true), Some(14));
        assert_eq!(with(&|s| s.rps = true), Some(14));
        assert_eq!(with(&|s| s.long_term = true), Some(14));
        assert_eq!(with(&|s| s.aspect = Some(1)), Some(14));
        assert_eq!(with(&|s| s.aspect = Some(255)), Some(14));
        assert_eq!(with(&|s| s.overscan = true), Some(14));
        // No VUI, no signal type, or no colour description: no transfer.
        assert_eq!(with(&|s| s.vui = false), None);
        assert_eq!(with(&|s| s.signal = false), None);
        assert_eq!(with(&|s| s.colour = false), None);
        // Cut short anywhere, it is None and not a panic.
        let full = Sps::everything(18).build();
        for cut in 0..full.len() {
            let got = hevc_sps_transfer(&full[..cut]);
            // The transfer is followed by the matrix and the trailing bits.
            if cut + 3 <= full.len() {
                assert_eq!(got, None, "cut at {cut}");
            }
        }
    }

    #[test]
    fn an_hvcc_with_an_sps_gives_profile_depth_and_transfer() {
        let nal = Sps::everything(18).build();
        let mut hvcc = vec![0u8; 22];
        hvcc[0] = 1;
        hvcc[1] = 2; // Main 10
        hvcc[17] = 0xF8 | 2; // 10-bit
        hvcc.push(1); // one array
        hvcc.push(0xA1); // SPS, complete
        hvcc.extend(1u16.to_be_bytes());
        hvcc.extend((nal.len() as u16).to_be_bytes());
        hvcc.extend(&nal);
        let c = hevc_config(Some(&hvcc));
        assert_eq!(c.profile.as_deref(), Some("Main 10"));
        assert_eq!(c.bit_depth, Some(10));
        assert_eq!(c.transfer, Some(18));
        // And through the whole entry: no colr, so the SPS decides.
        let v = from_moov(&moov(&bx(b"hvcC", &hvcc), &[(30, 33)])).unwrap();
        assert_eq!((v.hdr, v.bit_depth), (Hdr::Hlg, 10));
        assert_eq!(v.codec, VideoCodec::Hevc);
        // A pasp box gives the pixel aspect, in lowest terms; without one it
        // is square here (the SPS has no VUI aspect).
        let pasp = bx(b"pasp", &[0, 0, 0, 8, 0, 0, 0, 6]);
        assert_eq!(from_moov(&moov(&pasp, &[(30, 33)])).unwrap().sar, (4, 3));
        assert_eq!(from_moov(&moov(&[], &[(30, 33)])).unwrap().sar, (1, 1));
        // A colr box takes precedence over the SPS.
        let both = [bx(b"hvcC", &hvcc), colr(b"nclx", 16)].concat();
        assert_eq!(from_moov(&moov(&both, &[(30, 33)])).unwrap().hdr, Hdr::Pq);
    }
}
