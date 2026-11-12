//! What ffprobe says of a file: its picture's facts and colour tags, its
//! packets (with payload hashes when asked), and its decoded frames' times.
//! Packets and frames are separate calls: sizes need only the packets, and
//! decoding every frame is the slow part.

use super::ffmpeg::{run, tail, PROBE_LIMIT};
use kiln_media::video::Hdr;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

/// One packet or frame as ffprobe lists it.
#[derive(Clone, Debug, Default)]
pub struct Pkt {
    pub pts: Option<f64>,
    pub dur: Option<f64>,
    pub size: u64,
    /// The payload's MD5, when asked for.
    pub hash: String,
    /// A key frame.
    pub key: bool,
}

/// A picture's colour description, with ffprobe's names.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tags {
    pub primaries: String,
    pub transfer: String,
    pub space: String,
    pub range: String,
}

impl Tags {
    fn of(p: &str, t: &str, s: &str) -> Tags {
        Tags {
            primaries: p.into(),
            transfer: t.into(),
            space: s.into(),
            range: "tv".into(),
        }
    }

    /// BT.601 as R68 writes it: SMPTE 170M primaries, transfer and matrix.
    pub fn bt601() -> Tags {
        Tags::of("smpte170m", "smpte170m", "smpte170m")
    }

    pub fn bt709() -> Tags {
        Tags::of("bt709", "bt709", "bt709")
    }

    /// HDR10 (PQ) or HLG in BT.2020.
    pub fn hdr(hdr: Hdr) -> Tags {
        let transfer = if hdr == Hdr::Hlg {
            "arib-std-b67"
        } else {
            "smpte2084"
        };
        Tags::of("bt2020", transfer, "bt2020nc")
    }

    /// True when nothing is said of the primaries, transfer or matrix.
    pub fn untagged(&self) -> bool {
        [&self.primaries, &self.transfer, &self.space]
            .iter()
            .all(|v| matches!(v.as_str(), "" | "unknown" | "unspecified" | "reserved"))
    }

    /// With limited range when the range isn't said.
    pub fn or_tv(mut self) -> Tags {
        if matches!(self.range.as_str(), "" | "unknown" | "unspecified") {
            self.range = "tv".into();
        }
        self
    }

    /// The `setparams` filter that labels a picture with these tags.
    pub fn setparams(&self) -> String {
        format!(
            "setparams=range={}:color_primaries={}:color_trc={}:colorspace={}",
            self.range, self.primaries, self.transfer, self.space
        )
    }

    /// The matrix's name for `scale`'s `out_color_matrix`.
    pub fn matrix(&self) -> &'static str {
        match self.space.as_str() {
            "smpte170m" => "smpte170m",
            "bt470bg" => "bt470",
            "bt2020nc" | "bt2020c" => "bt2020",
            "fcc" => "fcc",
            "smpte240m" => "smpte240m",
            _ => "bt709",
        }
    }

    /// "bt709/bt709/bt709 tv": primaries, transfer and matrix, and range.
    pub fn label(&self) -> String {
        let or = |v: &str| {
            if v.is_empty() {
                "unknown".to_string()
            } else {
                v.to_string()
            }
        };
        format!(
            "{}/{}/{} {}",
            or(&self.primaries),
            or(&self.transfer),
            or(&self.space),
            or(&self.range)
        )
    }
}

/// A file's first video stream: its coded size, turn and colour.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    pub width: u32,
    pub height: u32,
    pub rotation: i32,
    pub tags: Tags,
}

impl Facts {
    /// The picture's size as shown: a turn of 90 or 270 degrees swaps sides.
    pub fn upright(&self) -> (u32, u32) {
        if self.rotation.rem_euclid(180) == 90 {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }
}

/// `key=value|key=value` lines (ffprobe's compact writer, `p=0`) as maps.
pub fn compact_lines(text: &str) -> Vec<HashMap<String, String>> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            l.trim()
                .split('|')
                .filter_map(|kv| kv.split_once('='))
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        })
        .collect()
}

fn ffprobe_text(stream: &str, args: &[&str], path: &Path) -> Result<String, String> {
    let mut cmd = Command::new("ffprobe");
    cmd.args(["-v", "error", "-select_streams", stream])
        .args(args)
        .args(["-of", "compact=p=0"])
        .arg(path);
    let out = run(&mut cmd, PROBE_LIMIT).map_err(|e| format!("ffprobe: {e}"))?;
    if !out.status.success() {
        return Err(format!("ffprobe failed: {}", tail(&out.stderr)));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The first video stream's size, turn and colour tags.
pub fn facts(path: &Path) -> Result<Facts, String> {
    let text = ffprobe_text(
        "v:0",
        &[
            "-show_entries",
            "stream=width,height,color_range,color_space,color_transfer,color_primaries:\
             stream_side_data=rotation",
        ],
        path,
    )?;
    let first = compact_lines(&text).into_iter().next().unwrap_or_default();
    let get = |k: &str| first.get(k).cloned().unwrap_or_default();
    let num = |k: &str| {
        first
            .get(k)
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(0)
    };
    let rotation = first
        .iter()
        .find(|(k, _)| k.ends_with("rotation"))
        .and_then(|(_, v)| v.parse::<f64>().ok())
        .map_or(0, |r| r.round() as i32);
    if num("width") == 0 {
        return Err("ffprobe found no video stream".into());
    }
    Ok(Facts {
        width: num("width"),
        height: num("height"),
        rotation,
        tags: Tags {
            primaries: get("color_primaries"),
            transfer: get("color_transfer"),
            space: get("color_space"),
            range: get("color_range"),
        },
    })
}

/// The packets of `stream` ("v:0" or "a:0") in decode order, with each
/// payload's MD5 when `hashes`. A missing sound stream is no packets.
pub fn packets(path: &Path, stream: &str, hashes: bool) -> Result<Vec<Pkt>, String> {
    let mut entries = "packet=pts_time,duration_time,size,flags".to_string();
    if hashes {
        entries.push_str(",data_hash");
    }
    let mut args = vec!["-show_entries", entries.as_str()];
    if hashes {
        args.extend(["-show_data_hash", "MD5"]);
    }
    let list: Vec<Pkt> = compact_lines(&ffprobe_text(stream, &args, path)?)
        .into_iter()
        .map(|m| Pkt {
            pts: m.get("pts_time").and_then(|v| v.parse().ok()),
            dur: m.get("duration_time").and_then(|v| v.parse().ok()),
            size: m.get("size").and_then(|v| v.parse().ok()).unwrap_or(0),
            hash: m.get("data_hash").cloned().unwrap_or_default(),
            key: m.get("flags").is_some_and(|f| f.starts_with('K')),
        })
        .collect();
    if list.is_empty() && stream.starts_with('v') {
        return Err("ffprobe found no video packets".into());
    }
    Ok(list)
}

/// The decoded frames' times, in the order shown. A file without an edit
/// list may hold packets that are never shown, so lengths come from here.
pub fn frames(path: &Path) -> Result<Vec<Pkt>, String> {
    let list: Vec<Pkt> = compact_lines(&ffprobe_text(
        "v:0",
        &[
            "-show_entries",
            "frame=best_effort_timestamp_time,duration_time",
        ],
        path,
    )?)
    .into_iter()
    .map(|m| Pkt {
        pts: m
            .get("best_effort_timestamp_time")
            .and_then(|v| v.parse().ok()),
        dur: m.get("duration_time").and_then(|v| v.parse().ok()),
        ..Pkt::default()
    })
    .collect();
    if list.is_empty() {
        return Err("ffprobe decoded no frames".into());
    }
    Ok(list)
}

/// From the first start to the end of the last one: the greatest pts plus
/// its duration, less the least pts.
pub fn span(list: &[Pkt]) -> Option<f64> {
    let start = list
        .iter()
        .filter_map(|p| p.pts)
        .fold(f64::INFINITY, f64::min);
    let end = list
        .iter()
        .filter_map(|p| Some(p.pts? + p.dur.unwrap_or(0.0)))
        .fold(f64::NEG_INFINITY, f64::max);
    (start.is_finite() && end.is_finite()).then_some(end - start)
}

/// The packets' payload bytes.
pub fn bytes(list: &[Pkt]) -> u64 {
    list.iter().map(|p| p.size).sum()
}

/// The first shown frame's time.
pub fn first_shown(frames: &[Pkt]) -> Option<f64> {
    frames
        .iter()
        .filter_map(|f| f.pts)
        .min_by(|a, b| a.total_cmp(b))
}

/// How long after the key frame a copy would start at the first shown frame
/// comes: the hidden lead-in an edit list skips (R69). 0 when the first
/// shown frame is a key frame.
pub fn hidden_lead_in(packets: &[Pkt], frames: &[Pkt]) -> f64 {
    let Some(shown) = first_shown(frames) else {
        return 0.0;
    };
    packets
        .iter()
        .filter(|p| p.key)
        .filter_map(|p| p.pts)
        .filter(|&t| t <= shown + 1e-3)
        .max_by(|a, b| a.total_cmp(b))
        .map_or(0.0, |k| (shown - k).max(0.0))
}

/// How a copy's packets stand against the source's: `out` must be the
/// source's packets from some point on, every one of them, with the same
/// payloads. Packets dropped from the front must all start before `before`
/// (the hidden lead-in R69 may drop). (dropped, payloads differing or
/// missing, what is wrong).
pub fn compare_packets(src: &[Pkt], out: &[Pkt], before: f64) -> (usize, usize, Option<String>) {
    let start = out.first().and_then(|first| {
        src.iter()
            .position(|p| !p.hash.is_empty() && p.hash == first.hash)
    });
    let Some(k) = start else {
        let differ = src
            .iter()
            .zip(out)
            .filter(|(a, b)| a.hash != b.hash || a.hash.is_empty())
            .count()
            + src.len().abs_diff(out.len());
        return (
            0,
            differ,
            Some("its first packet is none of the source's".into()),
        );
    };
    let rest = &src[k..];
    let differ = rest
        .iter()
        .zip(out)
        .filter(|(a, b)| a.hash != b.hash)
        .count()
        + rest.len().abs_diff(out.len());
    let shown_dropped = src[..k]
        .iter()
        .filter(|p| p.pts.is_some_and(|t| t >= before - 1e-3))
        .count();
    let problem = if shown_dropped > 0 {
        Some(format!(
            "{shown_dropped} of the {k} packets dropped from the front are shown in the source"
        ))
    } else if differ > 0 {
        Some(format!(
            "{differ} packets differ from the source's or are missing ({} against {} after the \
             {k} dropped)",
            out.len(),
            rest.len()
        ))
    } else {
        None
    };
    (k, differ, problem)
}
