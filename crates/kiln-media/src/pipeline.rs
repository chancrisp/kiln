//! The two ends of every conversion: something that yields blocks and
//! something that takes them.

use crate::error::{MediaError, Result};
use crate::process::Process;
use crate::tune::Setting;
use crate::types::{Block, Channels, SourceInfo, Tags};
use std::fs::File;
use std::time::{Duration, Instant};

/// How far a source has got.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Progressed {
    /// Frames handed out so far.
    pub frames: u64,
    /// The reader's position in the file.
    pub bytes: u64,
    /// The file's length.
    pub total_bytes: u64,
}

/// Something that yields interleaved `f32` blocks.
pub trait Source: Send {
    fn info(&self) -> &SourceInfo;
    /// Fills `out` with up to 8,192 frames and returns true, or returns false
    /// (leaving `out` empty) once the audio has ended.
    fn read(&mut self, out: &mut Block) -> Result<bool>;
    fn position(&self) -> Progressed;
}

/// Something that takes blocks and writes a file.
pub trait Sink: Send {
    fn write(&mut self, b: &Block) -> Result<()>;
    /// Writes whatever the format keeps at the end or in its header, and flushes.
    fn finish(self: Box<Self>) -> Result<()>;
}

/// What a sink is made from.
pub struct SinkSpec<'a> {
    pub rate: u32,
    pub channels: Channels,
    pub tags: &'a Tags,
    pub setting: &'a Setting,
    /// The source's integer bit depth, for the formats that follow it (FLAC,
    /// WavPack). `None` when it is unknown or the source is float.
    pub source_bits: Option<u16>,
    pub out: File,
}

/// Frames per block.
pub const BLOCK_FRAMES: usize = 8192;

/// What a lossy sink says when it is finished with no audio written. None of
/// AAC, M4A, WMA or AC3 has a playable empty file (an ASF with no packets, an
/// M4A with an empty track and a 0-byte AC3 are each refused by Media
/// Foundation), so the sink fails clearly and the caller deletes the output.
pub(crate) fn no_audio(format: &str) -> MediaError {
    MediaError::Encoder(format!("{format} can't be written with no audio."))
}

/// Runs a conversion: reads every block from `source`, passes it through
/// `process`, writes it to `sink`, and finishes the sink.
///
/// `progress` gets a value in [0, 1] that never goes backwards: the first one
/// after the first block, then at most one every 50 ms, then exactly 1.0 once
/// `Sink::finish` has succeeded. It is `frames / info.frames`, or, when the
/// frame count is unknown, `bytes / total_bytes` from `Source::position`.
///
/// `cancel` is checked before every block is read. When it returns true the
/// loop stops with `MediaError::Cancelled` and drops the sink without calling
/// `Sink::finish`, so the file is left partial: the caller deletes it.
pub fn run(
    mut source: Box<dyn Source>,
    mut process: Process,
    mut sink: Box<dyn Sink>,
    progress: &mut dyn FnMut(f32),
    cancel: &dyn Fn() -> bool,
) -> Result<()> {
    let frames_total = source.info().frames.filter(|&n| n > 0);
    let mut reporter = Reporter::new(progress);

    let mut block = Block {
        channels: source.info().channels.count,
        rate: source.info().rate,
        samples: Vec::with_capacity(BLOCK_FRAMES * source.info().channels.count as usize),
    };
    let mut converted = Block {
        channels: 0,
        rate: 0,
        samples: Vec::new(),
    };

    loop {
        if cancel() {
            return Err(MediaError::Cancelled);
        }
        if !source.read(&mut block).map_err(MediaError::reading)? {
            break;
        }
        if block.frames() > 0 {
            if process.is_identity() {
                sink.write(&block)?;
            } else {
                process.run(&block, &mut converted);
                if converted.frames() > 0 {
                    sink.write(&converted)?;
                }
            }
        }
        let at = source.position();
        let fraction = match frames_total {
            Some(total) => Some(at.frames as f64 / total as f64),
            None if at.total_bytes > 0 => Some(at.bytes as f64 / at.total_bytes as f64),
            None => None,
        };
        if let Some(f) = fraction {
            reporter.during(f);
        }
    }

    process.flush(&mut converted);
    if converted.frames() > 0 {
        sink.write(&converted)?;
    }
    sink.finish()?;
    reporter.done();
    Ok(())
}

/// How often progress is passed on while a conversion runs.
const REPORT_EVERY: Duration = Duration::from_millis(50);

/// Passes progress on: the first value, then at most one every 50 ms, never
/// lower than before, and never 1.0 until `done`.
pub(crate) struct Reporter<'a> {
    out: &'a mut dyn FnMut(f32),
    best: f32,
    last: Option<Instant>,
    every: Duration,
}

impl<'a> Reporter<'a> {
    pub(crate) fn new(out: &'a mut dyn FnMut(f32)) -> Reporter<'a> {
        Reporter {
            out,
            best: 0.0,
            last: None,
            every: REPORT_EVERY,
        }
    }

    pub(crate) fn during(&mut self, fraction: f64) {
        let f = if fraction.is_nan() {
            0.0
        } else {
            fraction.clamp(0.0, 1.0) as f32
        };
        self.best = self.best.max(f);
        let due = self.last.is_none_or(|t| t.elapsed() >= self.every);
        if due && self.best < 1.0 {
            self.last = Some(Instant::now());
            (self.out)(self.best);
        }
    }

    pub(crate) fn done(&mut self) {
        (self.out)(1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Codec;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    const RATE: u32 = 44_100;
    const STEREO: Channels = Channels { count: 2, mask: 3 };

    /// A sine, generated block by block. `spin` is how long each read takes.
    struct SineSource {
        info: SourceInfo,
        total: u64,
        done: u64,
        spin: Duration,
        /// After this many frames, `position` reports bytes that fall back.
        backwards_at: Option<u64>,
    }

    impl SineSource {
        fn new(total: u64, known: bool) -> SineSource {
            SineSource {
                info: SourceInfo {
                    codec: Codec::Mp3,
                    rate: RATE,
                    channels: STEREO,
                    bits: None,
                    frames: known.then_some(total),
                    kbps: None,
                    tags: Tags::default(),
                },
                total,
                done: 0,
                spin: Duration::ZERO,
                backwards_at: None,
            }
        }
    }

    impl Source for SineSource {
        fn info(&self) -> &SourceInfo {
            &self.info
        }
        fn read(&mut self, out: &mut Block) -> Result<bool> {
            if !self.spin.is_zero() {
                let t = Instant::now();
                while t.elapsed() < self.spin {
                    std::hint::spin_loop();
                }
            }
            out.channels = 2;
            out.rate = RATE;
            out.samples.clear();
            let n = (self.total - self.done).min(BLOCK_FRAMES as u64);
            if n == 0 {
                return Ok(false);
            }
            for i in 0..n {
                let t = (self.done + i) as f32 / RATE as f32;
                let s = (t * 440.0 * std::f32::consts::TAU).sin() * 0.5;
                out.samples.push(s);
                out.samples.push(s);
            }
            self.done += n;
            Ok(true)
        }
        fn position(&self) -> Progressed {
            let mut bytes = self.done * 4;
            if let Some(at) = self.backwards_at {
                if self.done > at {
                    bytes = at * 4 - 1000;
                }
            }
            Progressed {
                frames: self.done,
                bytes,
                total_bytes: self.total * 4,
            }
        }
    }

    #[derive(Default)]
    struct Counts {
        frames: AtomicU64,
        finished: AtomicBool,
    }

    struct CountingSink(Arc<Counts>);

    impl Sink for CountingSink {
        fn write(&mut self, b: &Block) -> Result<()> {
            assert!(b.frames() > 0, "an empty block was written");
            self.0.frames.fetch_add(b.frames() as u64, Ordering::SeqCst);
            Ok(())
        }
        fn finish(self: Box<Self>) -> Result<()> {
            self.0.finished.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    fn identity() -> Process {
        Process::new(RATE, STEREO, RATE, STEREO, None).unwrap()
    }

    fn convert(src: SineSource, counts: &Arc<Counts>) -> (Result<()>, Vec<f32>) {
        let mut seen = Vec::new();
        let r = run(
            Box::new(src),
            identity(),
            Box::new(CountingSink(counts.clone())),
            &mut |p| seen.push(p),
            &|| false,
        );
        (r, seen)
    }

    #[test]
    fn progress_rises_to_one() {
        let counts = Arc::new(Counts::default());
        let mut src = SineSource::new(RATE as u64 * 3, true);
        src.spin = Duration::from_millis(5);
        let (r, seen) = convert(src, &counts);
        r.unwrap();
        assert!(seen.len() >= 3, "{seen:?}");
        assert!(seen.windows(2).all(|w| w[0] <= w[1]), "{seen:?}");
        assert!(seen.iter().all(|p| (0.0..=1.0).contains(p)), "{seen:?}");
        assert_eq!(*seen.last().unwrap(), 1.0);
        assert_eq!(counts.frames.load(Ordering::SeqCst), RATE as u64 * 3);
        assert!(counts.finished.load(Ordering::SeqCst));
    }

    #[test]
    fn progress_is_throttled_to_every_50_ms() {
        let counts = Arc::new(Counts::default());
        // 20 blocks at 1 ms each: about 20 ms in all, so the first value, then 1.0.
        let mut src = SineSource::new(BLOCK_FRAMES as u64 * 20, true);
        src.spin = Duration::from_millis(1);
        let (r, seen) = convert(src, &counts);
        r.unwrap();
        assert!(seen.len() <= 3, "{seen:?}");
        assert_eq!(*seen.last().unwrap(), 1.0);
    }

    #[test]
    fn progress_is_reported_after_the_first_block() {
        let counts = Arc::new(Counts::default());
        let (r, seen) = convert(SineSource::new(BLOCK_FRAMES as u64 * 4, true), &counts);
        r.unwrap();
        assert_eq!(seen.len(), 2, "{seen:?}");
        assert_eq!(seen[0], 0.25);
        assert_eq!(seen[1], 1.0);
    }

    #[test]
    fn cancel_returns_within_50_ms() {
        let counts = Arc::new(Counts::default());
        // Ten minutes of audio, made a block at a time; each block takes 0.2 ms.
        let mut src = SineSource::new(RATE as u64 * 600, true);
        src.spin = Duration::from_micros(200);
        let flag = Arc::new(AtomicBool::new(false));
        let start = Instant::now();
        let setter = {
            let flag = flag.clone();
            std::thread::spawn(move || {
                while start.elapsed() < Duration::from_millis(100) {
                    std::hint::spin_loop();
                }
                flag.store(true, Ordering::SeqCst);
            })
        };
        let r = run(
            Box::new(src),
            identity(),
            Box::new(CountingSink(counts.clone())),
            &mut |_| {},
            &|| flag.load(Ordering::SeqCst),
        );
        let took = start.elapsed();
        setter.join().unwrap();
        assert!(matches!(r, Err(MediaError::Cancelled)), "{r:?}");
        assert!(took < Duration::from_millis(150), "took {took:?}");
        assert!(!counts.finished.load(Ordering::SeqCst), "finish was called");
        assert!(counts.frames.load(Ordering::SeqCst) > 0);
    }

    #[test]
    fn cancel_before_the_first_block_writes_nothing() {
        let counts = Arc::new(Counts::default());
        let r = run(
            Box::new(SineSource::new(RATE as u64, true)),
            identity(),
            Box::new(CountingSink(counts.clone())),
            &mut |_| {},
            &|| true,
        );
        assert!(matches!(r, Err(MediaError::Cancelled)));
        assert_eq!(counts.frames.load(Ordering::SeqCst), 0);
        assert!(!counts.finished.load(Ordering::SeqCst));
    }

    #[test]
    fn unknown_length_progress_uses_bytes() {
        let counts = Arc::new(Counts::default());
        let mut src = SineSource::new(BLOCK_FRAMES as u64 * 10, false);
        src.spin = Duration::from_millis(10);
        let (r, seen) = convert(src, &counts);
        r.unwrap();
        assert!(seen.len() >= 3, "{seen:?}");
        assert!(seen[0] > 0.0, "stuck at 0 after the first block: {seen:?}");
        assert!(seen.windows(2).all(|w| w[0] <= w[1]), "{seen:?}");
        assert!(seen.iter().all(|p| *p > 0.0 && *p <= 1.0), "{seen:?}");
        assert_eq!(*seen.last().unwrap(), 1.0);
    }

    #[test]
    fn unknown_length_and_no_total_reports_only_the_end() {
        struct NoTotal(SineSource);
        impl Source for NoTotal {
            fn info(&self) -> &SourceInfo {
                self.0.info()
            }
            fn read(&mut self, out: &mut Block) -> Result<bool> {
                self.0.read(out)
            }
            fn position(&self) -> Progressed {
                Progressed {
                    total_bytes: 0,
                    ..self.0.position()
                }
            }
        }
        let counts = Arc::new(Counts::default());
        let mut seen = Vec::new();
        run(
            Box::new(NoTotal(SineSource::new(BLOCK_FRAMES as u64 * 3, false))),
            identity(),
            Box::new(CountingSink(counts)),
            &mut |p| seen.push(p),
            &|| false,
        )
        .unwrap();
        assert_eq!(seen, vec![1.0]);
    }

    #[test]
    fn progress_never_goes_backwards() {
        let counts = Arc::new(Counts::default());
        let mut src = SineSource::new(BLOCK_FRAMES as u64 * 10, false);
        src.backwards_at = Some(BLOCK_FRAMES as u64 * 5);
        src.spin = Duration::from_millis(10);
        let (r, seen) = convert(src, &counts);
        r.unwrap();
        assert!(seen.windows(2).all(|w| w[0] <= w[1]), "{seen:?}");
        assert_eq!(*seen.last().unwrap(), 1.0);
    }

    #[test]
    fn the_reporter_never_reports_a_lower_value() {
        let mut seen = Vec::new();
        {
            let mut out = |p: f32| seen.push(p);
            let mut r = Reporter::new(&mut out);
            r.every = Duration::ZERO;
            r.during(0.5);
            r.during(0.4);
            r.during(0.45);
            r.during(0.6);
            r.during(f64::NAN);
        }
        assert_eq!(seen, vec![0.5, 0.5, 0.5, 0.6, 0.6]);
    }

    #[test]
    fn the_tail_is_flushed_before_the_sink_finishes() {
        // 44.1 kHz to 48 kHz: the resampler holds frames back until the flush.
        let counts = Arc::new(Counts::default());
        let process = Process::new(RATE, STEREO, 48_000, STEREO, None).unwrap();
        run(
            Box::new(SineSource::new(RATE as u64, true)),
            process,
            Box::new(CountingSink(counts.clone())),
            &mut |_| {},
            &|| false,
        )
        .unwrap();
        let out = counts.frames.load(Ordering::SeqCst);
        assert!(out.abs_diff(48_000) <= 8, "wrote {out} frames");
        assert!(counts.finished.load(Ordering::SeqCst));
    }

    #[test]
    fn a_source_error_stops_the_run() {
        struct Broken(SourceInfo);
        impl Source for Broken {
            fn info(&self) -> &SourceInfo {
                &self.0
            }
            fn read(&mut self, _: &mut Block) -> Result<bool> {
                Err(MediaError::Damaged { code: "MP3".into() })
            }
            fn position(&self) -> Progressed {
                Progressed::default()
            }
        }
        let counts = Arc::new(Counts::default());
        let r = run(
            Box::new(Broken(SineSource::new(1, true).info)),
            identity(),
            Box::new(CountingSink(counts.clone())),
            &mut |_| {},
            &|| false,
        );
        assert!(matches!(r, Err(MediaError::Damaged { .. })));
        assert!(!counts.finished.load(Ordering::SeqCst));
    }

    #[test]
    fn a_vbr_mp3_without_a_header_reports_progress() {
        // The file states no length, so the run goes by the bytes read.
        struct Slow(Box<dyn Source>);
        impl Source for Slow {
            fn info(&self) -> &SourceInfo {
                self.0.info()
            }
            fn read(&mut self, out: &mut Block) -> Result<bool> {
                let t = Instant::now();
                while t.elapsed() < Duration::from_millis(5) {
                    std::hint::spin_loop();
                }
                self.0.read(out)
            }
            fn position(&self) -> Progressed {
                self.0.position()
            }
        }

        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("data")
            .join("vbr_noxing.mp3");
        let src = crate::sym::open(File::open(path).unwrap(), "mp3").unwrap();
        assert_eq!(src.info().frames, None);
        assert_eq!((src.info().rate, src.info().channels.count), (RATE, 2));

        let counts = Arc::new(Counts::default());
        let mut seen = Vec::new();
        let mut written_at_first = None;
        run(
            Box::new(Slow(src)),
            identity(),
            Box::new(CountingSink(counts.clone())),
            &mut |p| {
                if written_at_first.is_none() {
                    written_at_first = Some(counts.frames.load(Ordering::SeqCst));
                }
                seen.push(p)
            },
            &|| false,
        )
        .unwrap();

        assert!(seen.len() >= 2, "{seen:?}");
        assert!(seen.windows(2).all(|w| w[0] <= w[1]), "{seen:?}");
        assert_eq!(*seen.last().unwrap(), 1.0);
        // A value above zero comes within the first second of audio.
        assert!(seen[0] > 0.0, "stuck at 0 after the first block: {seen:?}");
        let at = written_at_first.unwrap();
        assert!(at <= RATE as u64, "the first value came after {at} frames");
        // About eight seconds of audio were written.
        let frames = counts.frames.load(Ordering::SeqCst);
        assert!(frames.abs_diff(RATE as u64 * 8) <= 4096, "{frames}");
        assert!(counts.finished.load(Ordering::SeqCst));
    }
}
