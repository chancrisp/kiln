//! WAV and AIFF, read and written by Kiln itself.
//!
//! The two formats share everything but their headers: this module holds the
//! sample conversions, the reader over a run of PCM bytes, and the writer
//! state. Integer samples convert to `f32` exactly up to 24 bits, and back
//! with exact rounding, so 16- and 24-bit files round-trip bit for bit.

pub mod aiff;
pub mod wav;

use crate::error::{MediaError, Result};
use crate::pipeline::{Progressed, Source, BLOCK_FRAMES};
use crate::tune::Setting;
use crate::types::{Block, SourceInfo};
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};

/// How one sample sits in the file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    /// Unsigned 8-bit (WAV).
    U8,
    /// Signed 8-bit (AIFF).
    I8,
    I16,
    I24,
    I32,
    F32,
    F64,
}

impl Kind {
    pub(crate) fn bytes(self) -> usize {
        match self {
            Kind::U8 | Kind::I8 => 1,
            Kind::I16 => 2,
            Kind::I24 => 3,
            Kind::I32 | Kind::F32 => 4,
            Kind::F64 => 8,
        }
    }

    /// The kind a sink writes for a `Setting::Pcm`.
    pub(crate) fn for_sink(setting: &Setting) -> Result<Kind> {
        match *setting {
            Setting::Pcm {
                bits: 16,
                float: false,
            } => Ok(Kind::I16),
            Setting::Pcm {
                bits: 24,
                float: false,
            } => Ok(Kind::I24),
            Setting::Pcm {
                bits: 32,
                float: false,
            } => Ok(Kind::I32),
            Setting::Pcm {
                bits: 32,
                float: true,
            } => Ok(Kind::F32),
            Setting::Pcm {
                bits: 64,
                float: true,
            } => Ok(Kind::F64),
            Setting::Pcm { bits, float } => Err(MediaError::Encoder(format!(
                "PCM at {bits} bits{} can't be written",
                if float { " (float)" } else { "" }
            ))),
            _ => Err(MediaError::Encoder(
                "this setting isn't PCM, so a WAV or AIFF can't write it".into(),
            )),
        }
    }

    pub(crate) fn is_float(self) -> bool {
        matches!(self, Kind::F32 | Kind::F64)
    }
}

/// A sample kind plus byte order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Layout {
    pub kind: Kind,
    pub big_endian: bool,
}

impl Layout {
    /// Converts packed samples to `f32` and appends them to `out`.
    pub(crate) fn decode(self, src: &[u8], out: &mut Vec<f32>) {
        let be = self.big_endian;
        match self.kind {
            Kind::U8 => out.extend(src.iter().map(|&b| (b as f32 - 128.0) / 128.0)),
            Kind::I8 => out.extend(src.iter().map(|&b| (b as i8) as f32 / 128.0)),
            Kind::I16 => out.extend(src.chunks_exact(2).map(|c| {
                let v = if be {
                    i16::from_be_bytes([c[0], c[1]])
                } else {
                    i16::from_le_bytes([c[0], c[1]])
                };
                v as f32 / 32768.0
            })),
            Kind::I24 => out.extend(src.chunks_exact(3).map(|c| {
                let v = if be {
                    u32::from_be_bytes([c[0], c[1], c[2], 0])
                } else {
                    u32::from_be_bytes([c[2], c[1], c[0], 0])
                } as i32
                    >> 8;
                v as f32 / 8_388_608.0
            })),
            Kind::I32 => out.extend(src.chunks_exact(4).map(|c| {
                let v = if be {
                    i32::from_be_bytes([c[0], c[1], c[2], c[3]])
                } else {
                    i32::from_le_bytes([c[0], c[1], c[2], c[3]])
                };
                (v as f64 / 2_147_483_648.0) as f32
            })),
            Kind::F32 => out.extend(src.chunks_exact(4).map(|c| {
                f32::from_bits(if be {
                    u32::from_be_bytes([c[0], c[1], c[2], c[3]])
                } else {
                    u32::from_le_bytes([c[0], c[1], c[2], c[3]])
                })
            })),
            Kind::F64 => out.extend(src.chunks_exact(8).map(|c| {
                let a = [c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]];
                f64::from_bits(if be {
                    u64::from_be_bytes(a)
                } else {
                    u64::from_le_bytes(a)
                }) as f32
            })),
        }
    }

    /// Converts `f32` samples to packed bytes and appends them to `out`.
    /// Integers round to nearest and clamp; there is no dither here.
    pub(crate) fn encode(self, src: &[f32], out: &mut Vec<u8>) {
        let be = self.big_endian;
        match self.kind {
            Kind::U8 => out.extend(src.iter().map(|&s| (quant(s, 8) + 128) as u8)),
            Kind::I8 => out.extend(src.iter().map(|&s| quant(s, 8) as i8 as u8)),
            Kind::I16 => {
                for &s in src {
                    let v = quant(s, 16) as i16;
                    out.extend_from_slice(&if be { v.to_be_bytes() } else { v.to_le_bytes() });
                }
            }
            Kind::I24 => {
                for &s in src {
                    let b = quant(s, 24).to_le_bytes();
                    if be {
                        out.extend_from_slice(&[b[2], b[1], b[0]]);
                    } else {
                        out.extend_from_slice(&b[..3]);
                    }
                }
            }
            Kind::I32 => {
                for &s in src {
                    let v = quant(s, 32);
                    out.extend_from_slice(&if be { v.to_be_bytes() } else { v.to_le_bytes() });
                }
            }
            Kind::F32 => {
                for &s in src {
                    out.extend_from_slice(&if be {
                        s.to_bits().to_be_bytes()
                    } else {
                        s.to_bits().to_le_bytes()
                    });
                }
            }
            Kind::F64 => {
                for &s in src {
                    let v = (s as f64).to_bits();
                    out.extend_from_slice(&if be { v.to_be_bytes() } else { v.to_le_bytes() });
                }
            }
        }
    }
}

/// `s` scaled to a signed integer of `bits` bits, rounded and clamped.
pub(crate) fn quant(s: f32, bits: u32) -> i32 {
    let scale = (1u64 << (bits - 1)) as f64;
    (s as f64 * scale).round().clamp(-scale, scale - 1.0) as i32
}

/// The depth a lossless sink (FLAC, WavPack) writes: the setting's, else the
/// source's capped at 24, else 16 (unknown depth, or a float source). Kept
/// between 8 and 24, the range `f32` carries exactly.
pub(crate) fn lossless_bits(setting: Option<u16>, source: Option<u16>) -> u16 {
    setting
        .or(source.map(|b| b.min(24)))
        .unwrap_or(16)
        .clamp(8, 24)
}

/// Converts `f32` samples to integers of `bits` bits (rounded and clamped, like
/// every PCM sink) and appends them to `out`.
pub(crate) fn quantise_into(src: &[f32], bits: u32, out: &mut Vec<i32>) {
    out.clear();
    out.extend(src.iter().map(|&s| quant(s, bits)));
}

pub(crate) use crate::types::default_mask;

/// What a header parse hands the shared reader.
pub(crate) struct PcmRun {
    pub reader: BufReader<File>,
    pub info: SourceInfo,
    pub layout: Layout,
    pub channels: usize,
    /// Where the samples start in the file.
    pub data_start: u64,
    /// Whole frames the header promises.
    pub frames: u64,
    /// True when the data ends part-way through a frame.
    pub partial_tail: bool,
    pub total_bytes: u64,
    /// "WAV" or "AIFF".
    pub code: &'static str,
}

/// A [`Source`] over a run of PCM bytes in a file.
pub(crate) struct PcmSource {
    run: PcmRun,
    block_align: usize,
    frames_read: u64,
    scratch: Vec<u8>,
}

impl PcmSource {
    pub(crate) fn boxed(mut run: PcmRun) -> Result<Box<dyn Source>> {
        run.reader.seek(SeekFrom::Start(run.data_start))?;
        let block_align = run.layout.kind.bytes() * run.channels;
        Ok(Box::new(PcmSource {
            run,
            block_align,
            frames_read: 0,
            scratch: Vec::new(),
        }))
    }

    fn damaged(&self) -> MediaError {
        MediaError::Damaged {
            code: self.run.code.into(),
        }
    }
}

impl Source for PcmSource {
    fn info(&self) -> &SourceInfo {
        &self.run.info
    }

    fn read(&mut self, out: &mut Block) -> Result<bool> {
        out.channels = self.run.channels as u16;
        out.rate = self.run.info.rate;
        out.samples.clear();
        let left = self.run.frames - self.frames_read;
        if left == 0 {
            // A file cut off mid-sample gives its whole frames first, then this.
            if self.run.partial_tail {
                self.run.partial_tail = false;
                return Err(self.damaged());
            }
            return Ok(false);
        }
        let n = left.min(BLOCK_FRAMES as u64) as usize;
        self.scratch.resize(n * self.block_align, 0);
        if let Err(e) = self.run.reader.read_exact(&mut self.scratch) {
            return Err(if e.kind() == std::io::ErrorKind::UnexpectedEof {
                self.damaged()
            } else {
                e.into()
            });
        }
        out.samples.reserve(n * self.run.channels);
        self.run.layout.decode(&self.scratch, &mut out.samples);
        self.frames_read += n as u64;
        Ok(true)
    }

    fn position(&self) -> Progressed {
        Progressed {
            frames: self.frames_read,
            bytes: self.run.data_start + self.frames_read * self.block_align as u64,
            total_bytes: self.run.total_bytes,
        }
    }
}

/// The writing side shared by both formats: encodes blocks and counts bytes.
pub(crate) struct PcmWriter {
    pub out: BufWriter<File>,
    pub layout: Layout,
    pub channels: usize,
    pub frames: u64,
    pub data_bytes: u64,
    scratch: Vec<u8>,
}

impl PcmWriter {
    pub(crate) fn new(file: File, layout: Layout, channels: usize) -> PcmWriter {
        PcmWriter {
            out: BufWriter::with_capacity(256 * 1024, file),
            layout,
            channels,
            frames: 0,
            data_bytes: 0,
            scratch: Vec::new(),
        }
    }

    /// Encodes and writes a block; `limit` is the most data bytes the format can hold.
    pub(crate) fn put(&mut self, b: &Block, limit: u64, what: &str) -> Result<()> {
        if b.channels as usize != self.channels {
            return Err(MediaError::Encoder(format!(
                "a block of {} channels was written to a {}-channel {what} file",
                b.channels, self.channels
            )));
        }
        if b.samples.len() % self.channels != 0 {
            return Err(MediaError::Encoder(
                "a block ends part-way through a frame".into(),
            ));
        }
        self.scratch.clear();
        self.layout.encode(&b.samples, &mut self.scratch);
        if self.data_bytes + self.scratch.len() as u64 > limit {
            return Err(MediaError::Encoder(format!(
                "{what} can't hold audio this long"
            )));
        }
        self.out.write_all(&self.scratch)?;
        self.data_bytes += self.scratch.len() as u64;
        self.frames += (b.samples.len() / self.channels) as u64;
        Ok(())
    }
}

/// Writes a chunk header and body, padded to an even length.
pub(crate) fn put_chunk<W: Write>(
    w: &mut W,
    id: &[u8; 4],
    body: &[u8],
    big_endian: bool,
) -> std::io::Result<()> {
    w.write_all(id)?;
    let n = body.len() as u32;
    w.write_all(&if big_endian {
        n.to_be_bytes()
    } else {
        n.to_le_bytes()
    })?;
    w.write_all(body)?;
    if body.len() % 2 == 1 {
        w.write_all(&[0])?;
    }
    Ok(())
}
