//! AIFF and AIFF-C.
//!
//! Reads AIFF and AIFF-C with `NONE`, `twos`, `sowt` (little-endian integers),
//! `fl32` and `fl64`. Writes plain AIFF for integers and AIFF-C `fl32`/`fl64`
//! for float. Tags go in an `ID3 ` chunk. A file can't hold more than 4 GiB;
//! the sink reports an error rather than write a broken header.
//!
//! AIFF has no speaker mask, so mono and stereo get the usual one and wider
//! layouts report 0 (unknown).

use super::{put_chunk, Kind, Layout, PcmRun, PcmSource, PcmWriter};
use crate::error::{MediaError, Result};
use crate::pipeline::{Sink, SinkSpec, Source};
use crate::tags::id3;
use crate::types::{Block, Channels, Codec, SourceInfo};
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};

const CODE: &str = "AIFF";
const MAX_TAG_CHUNK: u64 = 64 * 1024 * 1024;
/// AIFF-C format version 1, required in an AIFF-C file.
const FVER_TIMESTAMP: u32 = 0xA280_5140;

fn damaged() -> MediaError {
    MediaError::Damaged { code: CODE.into() }
}

fn read_exact<R: Read>(r: &mut R, buf: &mut [u8]) -> Result<()> {
    r.read_exact(buf).map_err(|e| {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            damaged()
        } else {
            e.into()
        }
    })
}

fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

/// An IEEE 754 80-bit extended number (big-endian) as a sample rate.
fn decode_extended(b: &[u8]) -> u32 {
    let exp = (be16(b) & 0x7FFF) as i32;
    let mant = u64::from_be_bytes([b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9]]);
    if exp == 0 && mant == 0 {
        return 0;
    }
    let v = mant as f64 * 2f64.powi(exp - 16383 - 63);
    if v.is_finite() && v >= 0.0 && v < u32::MAX as f64 {
        v.round() as u32
    } else {
        0
    }
}

fn encode_extended(rate: u32) -> [u8; 10] {
    let mut out = [0u8; 10];
    if rate == 0 {
        return out;
    }
    let e = 31 - rate.leading_zeros();
    let exp = (16383 + e) as u16;
    let mant = (rate as u64) << (63 - e);
    out[0..2].copy_from_slice(&exp.to_be_bytes());
    out[2..10].copy_from_slice(&mant.to_be_bytes());
    out
}

struct Comm {
    layout: Layout,
    channels: u16,
    rate: u32,
    bits: u16,
    frames: u64,
}

fn parse_comm(b: &[u8], aifc: bool) -> Result<Comm> {
    if b.len() < 18 || (aifc && b.len() < 22) {
        return Err(damaged());
    }
    let channels = be16(b);
    let frames = be32(&b[2..]) as u64;
    let bits = be16(&b[6..]);
    let rate = decode_extended(&b[8..18]);
    if channels == 0 || rate == 0 || bits == 0 || bits > 64 {
        return Err(damaged());
    }
    let mut big_endian = true;
    let mut float = false;
    if aifc {
        match &b[18..22] {
            b"NONE" | b"twos" => {}
            b"sowt" => big_endian = false,
            b"fl32" | b"FL32" => {
                float = true;
            }
            b"fl64" | b"FL64" => {
                float = true;
            }
            other => {
                return Err(MediaError::Unsupported {
                    codec: String::from_utf8_lossy(other).trim().to_string(),
                })
            }
        }
    }
    let container = (bits as usize).div_ceil(8);
    let kind = match (float, container) {
        (false, 1) => Kind::I8,
        (false, 2) => Kind::I16,
        (false, 3) => Kind::I24,
        (false, 4) => Kind::I32,
        (true, 4) => Kind::F32,
        (true, 8) => Kind::F64,
        _ => {
            return Err(MediaError::Unsupported {
                codec: format!("{bits}-bit PCM"),
            })
        }
    };
    Ok(Comm {
        layout: Layout { kind, big_endian },
        channels,
        rate,
        bits,
        frames,
    })
}

/// Opens an AIFF or AIFF-C file for reading.
pub fn open(file: File) -> Result<Box<dyn Source>> {
    let total = file.metadata()?.len();
    let mut r = BufReader::with_capacity(256 * 1024, file);
    let mut head = [0u8; 12];
    read_exact(&mut r, &mut head)?;
    if &head[0..4] != b"FORM" {
        return Err(damaged());
    }
    let aifc = match &head[8..12] {
        b"AIFF" => false,
        b"AIFC" => true,
        _ => return Err(damaged()),
    };

    let mut comm: Option<Comm> = None;
    // (start of the samples, bytes available from there)
    let mut ssnd: Option<(u64, u64)> = None;
    let mut tag_bytes: Option<Vec<u8>> = None;
    let mut pos = 12u64;
    while pos + 8 <= total {
        r.seek(SeekFrom::Start(pos))?;
        let mut h = [0u8; 8];
        read_exact(&mut r, &mut h)?;
        let id = [h[0], h[1], h[2], h[3]];
        let size = be32(&h[4..]) as u64;
        let body = pos + 8;
        match &id {
            b"COMM" => {
                if !(18..=4096).contains(&size) || body + size > total {
                    return Err(damaged());
                }
                let mut b = vec![0u8; size as usize];
                read_exact(&mut r, &mut b)?;
                comm = Some(parse_comm(&b, aifc)?);
            }
            b"SSND" if ssnd.is_none() => {
                if size < 8 || body + size > total {
                    return Err(damaged());
                }
                let mut b = [0u8; 8];
                read_exact(&mut r, &mut b)?;
                let offset = be32(&b) as u64;
                if 8 + offset > size {
                    return Err(damaged());
                }
                ssnd = Some((body + 8 + offset, size - 8 - offset));
            }
            b"ID3 " | b"id3 "
                if tag_bytes.is_none() && size <= MAX_TAG_CHUNK && body + size <= total =>
            {
                let mut b = vec![0u8; size as usize];
                read_exact(&mut r, &mut b)?;
                tag_bytes = Some(b);
            }
            _ => {}
        }
        pos = body + size + (size & 1);
    }

    let (comm, (data_start, avail)) = match (comm, ssnd) {
        (Some(c), Some(s)) => (c, s),
        _ => return Err(damaged()),
    };
    let block_align = comm.layout.kind.bytes() as u64 * comm.channels as u64;
    if comm.frames * block_align > avail {
        return Err(damaged());
    }
    let tags = tag_bytes.map(|b| id3::read(&b)).unwrap_or_default();
    let info = SourceInfo {
        codec: Codec::Pcm {
            bits: comm.bits,
            float: comm.layout.kind.is_float(),
        },
        rate: comm.rate,
        channels: Channels {
            count: comm.channels,
            mask: match comm.channels {
                1 => 0x4,
                2 => 0x3,
                _ => 0,
            },
        },
        bits: Some(comm.bits),
        frames: Some(comm.frames),
        kbps: Some((comm.rate as u64 * comm.channels as u64 * comm.bits as u64 / 1000) as u32),
        tags,
    };
    PcmSource::boxed(PcmRun {
        reader: r,
        info,
        layout: comm.layout,
        channels: comm.channels as usize,
        data_start,
        frames: comm.frames,
        partial_tail: false,
        total_bytes: total,
        code: CODE,
    })
}

/// Makes a sink that writes an AIFF (or AIFF-C, for float) file.
pub fn sink(spec: SinkSpec) -> Result<Box<dyn Sink>> {
    let kind = Kind::for_sink(spec.setting)?;
    let count = spec.channels.count;
    if count == 0 {
        return Err(MediaError::Encoder(
            "an AIFF needs at least one channel".into(),
        ));
    }
    let layout = Layout {
        kind,
        big_endian: true,
    };
    let aifc = kind.is_float();
    let mut w = PcmWriter::new(spec.out, layout, count as usize);
    let bits = (kind.bytes() * 8) as u16;

    let mut comm = Vec::with_capacity(40);
    comm.extend_from_slice(&count.to_be_bytes());
    comm.extend_from_slice(&0u32.to_be_bytes()); // frames, patched in finish
    comm.extend_from_slice(&bits.to_be_bytes());
    comm.extend_from_slice(&encode_extended(spec.rate));
    if aifc {
        comm.extend_from_slice(if kind == Kind::F32 { b"fl32" } else { b"fl64" });
        // "IEEE 32-bit float" as a Pascal string: 18 bytes, already even.
        comm.push(17);
        comm.extend_from_slice(if kind == Kind::F32 {
            b"IEEE 32-bit float"
        } else {
            b"IEEE 64-bit float"
        });
    }

    let id3_bytes = id3::write_v24(spec.tags);
    let mut header = Vec::new();
    header.extend_from_slice(b"FORM\0\0\0\0");
    header.extend_from_slice(if aifc { b"AIFC" } else { b"AIFF" });
    if aifc {
        put_chunk(&mut header, b"FVER", &FVER_TIMESTAMP.to_be_bytes(), true)?;
    }
    let comm_body_pos = header.len() as u64 + 8;
    put_chunk(&mut header, b"COMM", &comm, true)?;
    if !id3_bytes.is_empty() {
        put_chunk(&mut header, b"ID3 ", &id3_bytes, true)?;
    }
    let ssnd_size_pos = header.len() as u64 + 4;
    header.extend_from_slice(b"SSND\0\0\0\0");
    header.extend_from_slice(&[0u8; 8]); // offset and block size
    w.out.write_all(&header)?;
    Ok(Box::new(AiffSink {
        w,
        header_len: header.len() as u64,
        frames_pos: comm_body_pos + 2,
        ssnd_size_pos,
    }))
}

struct AiffSink {
    w: PcmWriter,
    header_len: u64,
    frames_pos: u64,
    ssnd_size_pos: u64,
}

impl AiffSink {
    /// The most sample bytes the 32-bit chunk sizes can describe.
    fn limit(&self) -> u64 {
        0xFFFF_FFFFu64 - self.header_len - 16
    }
}

impl Sink for AiffSink {
    fn write(&mut self, b: &Block) -> Result<()> {
        let limit = self.limit();
        self.w.put(b, limit, "AIFF")
    }

    fn finish(self: Box<Self>) -> Result<()> {
        let AiffSink {
            mut w,
            header_len,
            frames_pos,
            ssnd_size_pos,
        } = *self;
        let data_bytes = w.data_bytes;
        let pad = data_bytes & 1;
        if pad == 1 {
            w.out.write_all(&[0])?;
        }
        let form_size = header_len - 8 + data_bytes + pad;
        let mut file = w.out.into_inner().map_err(|e| e.into_error())?;
        file.seek(SeekFrom::Start(4))?;
        file.write_all(&(form_size as u32).to_be_bytes())?;
        file.seek(SeekFrom::Start(frames_pos))?;
        file.write_all(&(w.frames as u32).to_be_bytes())?;
        file.seek(SeekFrom::Start(ssnd_size_pos))?;
        file.write_all(&((data_bytes + 8) as u32).to_be_bytes())?;
        file.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extended_float_round_trips_common_rates() {
        for rate in [8000, 11025, 22050, 44100, 48000, 88200, 96000, 192000, 1, 3] {
            assert_eq!(decode_extended(&encode_extended(rate)), rate);
        }
        // 44100 as written by every AIFF tool.
        assert_eq!(
            encode_extended(44100),
            [0x40, 0x0E, 0xAC, 0x44, 0, 0, 0, 0, 0, 0]
        );
    }
}
