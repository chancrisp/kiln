//! WAV: RIFF, and RF64 for files past 4 GiB.
//!
//! Reads PCM at 8, 16, 24 and 32 bits and float at 32 and 64, plain or in
//! `WAVE_FORMAT_EXTENSIBLE`. A data chunk shorter than its header says is
//! reported as damaged when the file is opened. A data chunk whose length
//! isn't a whole number of frames reads its whole frames and then reports
//! damaged.
//!
//! The writer reserves a `JUNK` chunk the size of `ds64`. If the data passes
//! 4 GiB - 1 it turns the file into RF64 in `finish`, otherwise the chunk stays.

use super::{default_mask, put_chunk, Kind, Layout, PcmRun, PcmSource, PcmWriter};
use crate::error::{MediaError, Result};
use crate::pipeline::{Sink, SinkSpec, Source};
use crate::tags::id3;
use crate::types::{Block, Channels, Codec, SourceInfo};
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};

const CODE: &str = "WAV";
const FORMAT_PCM: u16 = 1;
const FORMAT_FLOAT: u16 = 3;
const FORMAT_EXTENSIBLE: u16 = 0xFFFE;
/// The biggest `id3 ` chunk the reader will load.
const MAX_TAG_CHUNK: u64 = 64 * 1024 * 1024;
/// ds64 payload: riff size, data size, sample count (8 bytes each), table length (4).
const DS64_SIZE: u32 = 28;

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

fn le16(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}

fn le32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

fn le64(b: &[u8]) -> u64 {
    u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}

struct Fmt {
    layout: Layout,
    channels: u16,
    rate: u32,
    mask: u32,
    /// Bits that carry signal (can be fewer than the container holds).
    bits: u16,
}

fn unsupported_format(tag: u16) -> MediaError {
    let name = match tag {
        0x0002 | 0x0011 => "ADPCM".to_string(),
        0x0006 => "A-law".to_string(),
        0x0007 => "mu-law".to_string(),
        0x0055 => "MP3".to_string(),
        0x0161..=0x0163 => "WMA".to_string(),
        0x2000 => "AC3".to_string(),
        _ => format!("format 0x{tag:04X}"),
    };
    MediaError::Unsupported { codec: name }
}

fn parse_fmt(b: &[u8]) -> Result<Fmt> {
    if b.len() < 16 {
        return Err(damaged());
    }
    let mut tag = le16(b);
    let channels = le16(&b[2..]);
    let rate = le32(&b[4..]);
    let block_align = le16(&b[12..]) as usize;
    let mut bits = le16(&b[14..]);
    let mut mask = 0;
    if tag == FORMAT_EXTENSIBLE {
        if b.len() < 40 {
            return Err(damaged());
        }
        let valid = le16(&b[18..]);
        mask = le32(&b[20..]);
        tag = le16(&b[24..]); // the first two bytes of the sub-format GUID
        if valid != 0 && valid <= bits {
            bits = valid;
        }
    }
    if channels == 0 || rate == 0 {
        return Err(damaged());
    }
    let float = match tag {
        FORMAT_PCM => false,
        FORMAT_FLOAT => true,
        other => return Err(unsupported_format(other)),
    };
    // The container is what block_align says; `bits` may be fewer.
    let container = block_align / channels as usize;
    if container == 0 || block_align != container * channels as usize {
        return Err(damaged());
    }
    let kind = match (float, container) {
        (false, 1) => Kind::U8,
        (false, 2) => Kind::I16,
        (false, 3) => Kind::I24,
        (false, 4) => Kind::I32,
        (true, 4) => Kind::F32,
        (true, 8) => Kind::F64,
        _ => {
            return Err(MediaError::Unsupported {
                codec: format!("{}-bit PCM", container * 8),
            })
        }
    };
    if bits == 0 || bits as usize > container * 8 {
        bits = (container * 8) as u16;
    }
    if mask == 0 {
        mask = default_mask(channels);
    }
    Ok(Fmt {
        layout: Layout {
            kind,
            big_endian: false,
        },
        channels,
        rate,
        mask,
        bits,
    })
}

/// Opens a WAV (or RF64) file for reading.
pub fn open(file: File) -> Result<Box<dyn Source>> {
    let total = file.metadata()?.len();
    let mut r = BufReader::with_capacity(256 * 1024, file);
    let mut head = [0u8; 12];
    read_exact(&mut r, &mut head)?;
    let rf64 = match &head[0..4] {
        b"RIFF" => false,
        b"RF64" | b"BW64" => true,
        _ => return Err(damaged()),
    };
    if &head[8..12] != b"WAVE" {
        return Err(damaged());
    }

    let mut fmt: Option<Fmt> = None;
    let mut data: Option<(u64, u64)> = None;
    let mut ds64_data: Option<u64> = None;
    let mut tag_bytes: Option<Vec<u8>> = None;
    let mut pos = 12u64;
    while pos + 8 <= total {
        r.seek(SeekFrom::Start(pos))?;
        let mut h = [0u8; 8];
        read_exact(&mut r, &mut h)?;
        let id = [h[0], h[1], h[2], h[3]];
        let size = le32(&h[4..]) as u64;
        let body = pos + 8;
        let mut advance = size;
        match &id {
            b"ds64" if rf64 && size >= 24 => {
                let mut d = [0u8; 24];
                read_exact(&mut r, &mut d)?;
                ds64_data = Some(le64(&d[8..]));
            }
            b"fmt " => {
                if !(16..=4096).contains(&size) || body + size > total {
                    return Err(damaged());
                }
                let mut b = vec![0u8; size as usize];
                read_exact(&mut r, &mut b)?;
                fmt = Some(parse_fmt(&b)?);
            }
            b"data" if data.is_none() => {
                let len = if size == 0xFFFF_FFFF {
                    // RF64 keeps the real size in ds64; a streamed RIFF runs to the end.
                    ds64_data.unwrap_or(total.saturating_sub(body))
                } else {
                    size
                };
                // `len` can be a ds64 value as big as a u64; the sum must not wrap.
                if body.checked_add(len).is_none_or(|end| end > total) {
                    return Err(damaged());
                }
                data = Some((body, len));
                advance = len;
            }
            b"id3 " | b"ID3 "
                if tag_bytes.is_none() && size <= MAX_TAG_CHUNK && body + size <= total =>
            {
                let mut b = vec![0u8; size as usize];
                read_exact(&mut r, &mut b)?;
                tag_bytes = Some(b);
            }
            _ => {}
        }
        pos = body + advance + (advance & 1);
    }

    let (fmt, (data_start, data_len)) = match (fmt, data) {
        (Some(f), Some(d)) => (f, d),
        _ => return Err(damaged()),
    };
    let tags = tag_bytes.map(|b| id3::read(&b)).unwrap_or_default();
    let block_align = fmt.layout.kind.bytes() as u64 * fmt.channels as u64;
    let frames = data_len / block_align;
    let float = fmt.layout.kind.is_float();
    let info = SourceInfo {
        codec: Codec::Pcm {
            bits: fmt.bits,
            float,
        },
        rate: fmt.rate,
        channels: Channels {
            count: fmt.channels,
            mask: fmt.mask,
        },
        bits: Some(fmt.bits),
        frames: Some(frames),
        kbps: Some((fmt.rate as u64 * fmt.channels as u64 * fmt.bits as u64 / 1000) as u32),
        tags,
    };
    PcmSource::boxed(PcmRun {
        reader: r,
        info,
        layout: fmt.layout,
        channels: fmt.channels as usize,
        data_start,
        frames,
        partial_tail: data_len % block_align != 0,
        total_bytes: total,
        code: CODE,
    })
}

/// Makes a sink that writes a WAV file (RF64 if the audio outgrows 4 GiB).
pub fn sink(spec: SinkSpec) -> Result<Box<dyn Sink>> {
    let kind = Kind::for_sink(spec.setting)?;
    let count = spec.channels.count;
    if count == 0 {
        return Err(MediaError::Encoder(
            "a WAV needs at least one channel".into(),
        ));
    }
    let layout = Layout {
        kind,
        big_endian: false,
    };
    let mut w = PcmWriter::new(spec.out, layout, count as usize);
    let bits = (kind.bytes() * 8) as u16;
    let block_align = kind.bytes() as u16 * count;
    let extensible = count > 2 || (!kind.is_float() && bits > 16);
    let mask = if spec.channels.mask != 0 {
        spec.channels.mask
    } else {
        default_mask(count)
    };

    let mut fmt = Vec::with_capacity(40);
    let tag = if kind.is_float() {
        FORMAT_FLOAT
    } else {
        FORMAT_PCM
    };
    fmt.extend_from_slice(&(if extensible { FORMAT_EXTENSIBLE } else { tag }).to_le_bytes());
    fmt.extend_from_slice(&count.to_le_bytes());
    fmt.extend_from_slice(&spec.rate.to_le_bytes());
    fmt.extend_from_slice(&(spec.rate * block_align as u32).to_le_bytes());
    fmt.extend_from_slice(&block_align.to_le_bytes());
    fmt.extend_from_slice(&bits.to_le_bytes());
    if extensible {
        fmt.extend_from_slice(&22u16.to_le_bytes()); // cbSize
        fmt.extend_from_slice(&bits.to_le_bytes()); // valid bits
        fmt.extend_from_slice(&mask.to_le_bytes());
        // KSDATAFORMAT_SUBTYPE_PCM / _IEEE_FLOAT: {tag}-0000-0010-8000-00AA00389B71
        fmt.extend_from_slice(&tag.to_le_bytes());
        fmt.extend_from_slice(&[0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xAA, 0, 0x38, 0x9B, 0x71]);
    }

    let id3_bytes = id3::write_v24(spec.tags);
    let header = {
        // 'RIFF' size 'WAVE', then JUNK (the room for ds64), fmt, id3, data header.
        let mut h = Vec::new();
        h.extend_from_slice(b"RIFF\0\0\0\0WAVE");
        put_chunk(&mut h, b"JUNK", &[0u8; DS64_SIZE as usize], false)?;
        put_chunk(&mut h, b"fmt ", &fmt, false)?;
        if !id3_bytes.is_empty() {
            put_chunk(&mut h, b"id3 ", &id3_bytes, false)?;
        }
        h.extend_from_slice(b"data\0\0\0\0");
        h
    };
    w.out.write_all(&header)?;
    Ok(Box::new(WavSink {
        w,
        header_len: header.len() as u64,
    }))
}

struct WavSink {
    w: PcmWriter,
    header_len: u64,
}

impl Sink for WavSink {
    fn write(&mut self, b: &Block) -> Result<()> {
        // The data length field of an RF64 is 64 bits; a plain RIFF's is not,
        // and finish() switches formats, so only the u64 range limits here.
        self.w.put(b, u64::MAX / 2, "WAV")
    }

    fn finish(self: Box<Self>) -> Result<()> {
        let WavSink { mut w, header_len } = *self;
        let data_bytes = w.data_bytes;
        if data_bytes % 2 == 1 {
            w.out.write_all(&[0])?;
        }
        let file_len = header_len + data_bytes + (data_bytes & 1);
        let riff_size = file_len - 8;
        let rf64 = data_bytes >= 0xFFFF_FFFF || riff_size >= 0xFFFF_FFFF;
        let mut file = w.out.into_inner().map_err(|e| e.into_error())?;
        // 'data' size field sits in the last 4 bytes of the header.
        let data_size_pos = header_len - 4;
        if rf64 {
            file.seek(SeekFrom::Start(0))?;
            file.write_all(b"RF64")?;
            file.write_all(&0xFFFF_FFFFu32.to_le_bytes())?;
            file.seek(SeekFrom::Start(12))?;
            let mut ds64 = Vec::with_capacity(8 + DS64_SIZE as usize);
            ds64.extend_from_slice(b"ds64");
            ds64.extend_from_slice(&DS64_SIZE.to_le_bytes());
            ds64.extend_from_slice(&riff_size.to_le_bytes());
            ds64.extend_from_slice(&data_bytes.to_le_bytes());
            ds64.extend_from_slice(&w.frames.to_le_bytes());
            ds64.extend_from_slice(&0u32.to_le_bytes());
            file.write_all(&ds64)?;
            file.seek(SeekFrom::Start(data_size_pos))?;
            file.write_all(&0xFFFF_FFFFu32.to_le_bytes())?;
        } else {
            file.seek(SeekFrom::Start(4))?;
            file.write_all(&(riff_size as u32).to_le_bytes())?;
            file.seek(SeekFrom::Start(data_size_pos))?;
            file.write_all(&(data_bytes as u32).to_le_bytes())?;
        }
        file.flush()?;
        Ok(())
    }
}
