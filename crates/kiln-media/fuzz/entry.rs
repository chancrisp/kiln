//! The entry points both fuzzers drive: the cargo-fuzz targets in
//! `fuzz_targets/` and the deterministic `tests/fuzz_lite.rs`. Each hands bytes
//! to one of Kiln's readers the way the app does (through a `File`, since the
//! readers own their I/O) and, for a source, reads every block to the end.
//!
//! Any `Err` is fine. A panic, a failed assertion here (a source breaking the
//! block rules the pipeline depends on), a hang or an abort is a finding.

#![allow(dead_code)]

use kiln_media::pcm::{aiff, wav};
use kiln_media::tags::{ape, id3};
use kiln_media::{fuzzing, ogg, sym, wavpack, Block, Source};
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Frames per block, as the pipeline promises.
const BLOCK_FRAMES: usize = 8192;

/// The extensions `Reader::Sym` is tried with: the hint decides which format
/// Symphonia probes first and what a "damaged" message calls the file.
pub const SYM_EXTS: [&str; 12] = [
    "mp3", "m4a", "mp4", "mov", "aac", "flac", "ogg", "mkv", "webm", "wav", "aiff", "",
];

/// One reader, or one tag parser, under test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reader {
    /// Kiln's WAV and RF64 reader.
    Wav,
    /// Kiln's AIFF and AIFF-C reader.
    Aiff,
    /// Symphonia through `sym::open` (MP3, FLAC, Vorbis, AAC, ALAC, PCM, and
    /// Opus in Matroska and WebM), with this extension as the hint.
    Sym(&'static str),
    /// The vendored libwavpack, with Kiln's APEv2 tag reader.
    WavPack,
    /// Ogg Opus: Kiln's page reader and libopus.
    Opus,
    /// The ID3v2 tag reader, on bytes.
    Id3,
    /// The APEv2 tag reader, on bytes and on a file's end.
    Ape,
    /// The MP4 edit-list and iTunSMPB parser.
    Mp4,
    /// The video facts of an MP4, MOV or Matroska file, from its headers.
    VideoInfo,
}

impl Reader {
    pub fn name(self) -> String {
        match self {
            Reader::Wav => "wav".into(),
            Reader::Aiff => "aiff".into(),
            Reader::Sym(e) => format!("sym:{e}"),
            Reader::WavPack => "wavpack".into(),
            Reader::Opus => "opus".into(),
            Reader::Id3 => "id3".into(),
            Reader::Ape => "ape".into(),
            Reader::Mp4 => "mp4read".into(),
            Reader::VideoInfo => "video_info".into(),
        }
    }
}

/// What came of one input. Only `Opened` and `Failed` say anything about how
/// far in the reader got; none of them is a finding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The reader refused to open it.
    Rejected,
    /// Opened, and read to the end.
    Read,
    /// Opened, then reported an error part-way.
    Failed,
    /// A tag reader or parser that returns no error type.
    Parsed,
}

/// One file the readers are handed, rewritten for each input: the cursor is
/// shared by every clone, so the file is put back at the start each time.
pub struct Scratch {
    path: PathBuf,
    file: File,
}

impl Scratch {
    pub fn new(path: &Path) -> std::io::Result<Scratch> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        Ok(Scratch {
            path: path.to_path_buf(),
            file,
        })
    }

    /// Fills the file with `bytes` and returns a handle at the start.
    pub fn load(&mut self, bytes: &[u8]) -> std::io::Result<File> {
        self.file.set_len(0)?;
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(bytes)?;
        self.file.flush()?;
        self.rewind()
    }

    /// A handle at the start of what is loaded.
    pub fn rewind(&mut self) -> std::io::Result<File> {
        self.file.seek(SeekFrom::Start(0))?;
        self.file.try_clone()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Reads a source to the end, checking the rules the pipeline relies on.
fn drain(src: &mut dyn Source) -> Outcome {
    let info = src.info().clone();
    let _ = info.duration_secs();
    assert!(info.rate > 0, "opened with a rate of 0");
    let mut block = Block {
        channels: 0,
        rate: 0,
        samples: Vec::new(),
    };
    loop {
        match src.read(&mut block) {
            Ok(true) => {
                let channels = info.channels.count as usize;
                assert!(channels > 0, "opened with no channels");
                assert!(
                    block.samples.len() % channels == 0,
                    "a block holds a part frame"
                );
                let frames = block.frames();
                assert!(
                    frames > 0 && frames <= BLOCK_FRAMES,
                    "a block of {frames} frames"
                );
                assert_eq!(block.channels, info.channels.count, "channels changed");
                assert_eq!(block.rate, info.rate, "rate changed");
                let _ = src.position();
            }
            Ok(false) => {
                assert!(block.samples.is_empty(), "the last read left samples");
                let _ = src.position();
                return Outcome::Read;
            }
            Err(_) => return Outcome::Failed,
        }
    }
}

fn source(opened: kiln_media::Result<Box<dyn Source>>) -> Outcome {
    match opened {
        Ok(mut s) => drain(s.as_mut()),
        Err(_) => Outcome::Rejected,
    }
}

/// Runs one input through one reader. `file` holds exactly `bytes`, at its start.
pub fn run(reader: Reader, bytes: &[u8], mut file: File) -> Outcome {
    match reader {
        Reader::Wav => source(wav::open(file)),
        Reader::Aiff => source(aiff::open(file)),
        Reader::Sym(ext) => source(sym::open(file, ext)),
        Reader::WavPack => source(wavpack::open(file)),
        Reader::Opus => source(ogg::opus_open(file)),
        Reader::Id3 => {
            let _ = id3::read(bytes);
            Outcome::Parsed
        }
        Reader::Ape => {
            let _ = ape::read(bytes);
            if let Ok(Some((start, tag))) = ape::locate(&mut file) {
                assert!(start + tag.len() as u64 <= bytes.len() as u64);
                let _ = ape::read(&tag);
            }
            Outcome::Parsed
        }
        Reader::Mp4 => {
            for rate in [44_100, 48_000] {
                let _ = file.seek(SeekFrom::Start(0));
                let _ = fuzzing::aac_trim(&mut file, rate);
            }
            Outcome::Parsed
        }
        Reader::VideoInfo => {
            let Ok(v) = fuzzing::video_info(&mut file) else {
                return Outcome::Rejected;
            };
            assert!(v.width > 0 && v.height > 0, "opened with no picture");
            assert!([0, 90, 180, 270].contains(&v.rotation), "{}", v.rotation);
            assert!(
                v.fps
                    .is_none_or(|f| f.is_finite() && f > 0.0 && f <= 1000.0),
                "{:?}",
                v.fps
            );
            assert!(
                v.duration.is_none_or(|d| d.is_finite() && d > 0.0),
                "{:?}",
                v.duration
            );
            assert!((8..=16).contains(&v.bit_depth), "{}", v.bit_depth);
            Outcome::Parsed
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Only ever a file this process made in the temp folder or the target folder.
        let _ = std::fs::remove_file(&self.path);
    }
}

/// What a cargo-fuzz target does with one input: the bytes go into a temp
/// file kept for the thread, and the reader runs over it.
pub fn fuzz_one(
    slot: &'static std::thread::LocalKey<std::cell::RefCell<Option<Scratch>>>,
    name: &str,
    reader: Reader,
    data: &[u8],
) {
    slot.with(|cell| {
        let mut cell = cell.borrow_mut();
        if cell.is_none() {
            let path = std::env::temp_dir().join(format!(
                "kiln-fuzz-{}-{name}-{:?}.bin",
                std::process::id(),
                std::thread::current().id()
            ));
            let scratch = Scratch::new(&path).expect("make the scratch file");
            *cell = Some(scratch);
        }
        let scratch = cell.as_mut().expect("just made");
        let file = scratch.load(data).expect("write the scratch file");
        run(reader, data, file);
    });
}
/// The Ogg page checksum (CRC-32, polynomial 0x04C11DB7, no reflection).
fn ogg_crc(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, slot) in t.iter_mut().enumerate() {
            let mut c = (i as u32) << 24;
            for _ in 0..8 {
                c = if c & 0x8000_0000 != 0 {
                    (c << 1) ^ 0x04C1_1DB7
                } else {
                    c << 1
                };
            }
            *slot = c;
        }
        t
    });
    data.iter().fold(0u32, |crc, &b| {
        (crc << 8) ^ table[((crc >> 24) as u8 ^ b) as usize]
    })
}

/// Puts a right checksum on every Ogg page that is still whole, so a mutation
/// gets past the check and into the code behind it.
pub fn fix_ogg_crcs(v: &mut [u8]) {
    let mut at = 0;
    while at + 27 <= v.len() && &v[at..at + 4] == b"OggS" {
        let segs = v[at + 26] as usize;
        if at + 27 + segs > v.len() {
            return;
        }
        let body: usize = v[at + 27..at + 27 + segs].iter().map(|&l| l as usize).sum();
        let end = at + 27 + segs + body;
        if end > v.len() {
            return;
        }
        v[at + 22..at + 26].fill(0);
        let crc = ogg_crc(&v[at..end]);
        v[at + 22..at + 26].copy_from_slice(&crc.to_le_bytes());
        at = end;
    }
}
