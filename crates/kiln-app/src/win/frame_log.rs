//! A test hook: with `KILN_FRAME_LOG=<path>` set, one line for each frame
//! goes to that file, so a run's timing can be read afterwards.
//!
//! A line is `main <µs>` after each successful `Present` of the main window,
//! `float <µs>` after each present of the floating wheel, and `clock <µs>`
//! each time the inspector's clock ticks (`CLOCK_TIMER`), `<µs>` being the
//! microseconds since [`start`] ran, at the top of the app's `run`, read
//! off one `Instant`. `draw <µs>` follows each main window frame with the
//! microseconds its Direct2D drawing took, from `BeginDraw` to `EndDraw`,
//! so it is a duration, not a time; `part <µs>` is the same for a frame
//! that drew and presented only the rows that changed. The file is opened once, in append mode, and written
//! through a buffer that is flushed at least every 100 ms of frames and by
//! [`finish`] at exit. A write that fails is ignored.
//!
//! With the variable unset nothing is opened or written, and a frame costs
//! one `OnceLock` check.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The environment variable that names the file.
const VARIABLE: &str = "KILN_FRAME_LOG";

/// How long frames may sit in the buffer before it is flushed.
const FLUSH_EVERY: Duration = Duration::from_millis(100);

/// Which window drew the frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Frame {
    Main,
    Float,
    /// Not a frame: the inspector's clock ticked.
    Clock,
    /// Not a time: how long the main window's drawing took.
    Draw,
    /// The same, for a frame of part of the window.
    Part,
}

/// One line of the file, without its line break.
fn line(frame: Frame, micros: u128) -> String {
    let who = match frame {
        Frame::Main => "main",
        Frame::Float => "float",
        Frame::Clock => "clock",
        Frame::Draw => "draw",
        Frame::Part => "part",
    };
    format!("{who} {micros}")
}

/// Frames written to `out`, counted from `epoch`.
struct FrameLog<W: Write> {
    out: W,
    epoch: Instant,
    flushed: Instant,
}

impl<W: Write> FrameLog<W> {
    fn new(out: W, epoch: Instant) -> Self {
        Self {
            out,
            epoch,
            flushed: epoch,
        }
    }

    /// Add a line for a frame drawn at `at`, and flush if the buffer has
    /// held frames for [`FLUSH_EVERY`].
    fn record(&mut self, frame: Frame, at: Instant) {
        let micros = at.saturating_duration_since(self.epoch).as_micros();
        let _ = writeln!(self.out, "{}", line(frame, micros));
        if at.saturating_duration_since(self.flushed) >= FLUSH_EVERY {
            self.flush(at);
        }
    }

    /// Add a line for a frame whose drawing took `took`.
    fn record_draw(&mut self, took: Duration, part: bool) {
        let frame = if part { Frame::Part } else { Frame::Draw };
        let _ = writeln!(self.out, "{}", line(frame, took.as_micros()));
    }

    fn flush(&mut self, at: Instant) {
        let _ = self.out.flush();
        self.flushed = at;
    }
}

type Sink = Mutex<FrameLog<BufWriter<File>>>;

/// The open log, or `None` if the variable was unset or the file wouldn't
/// open. Set once, by [`start`].
static LOG: OnceLock<Option<Sink>> = OnceLock::new();

/// The log for a value of the variable: none unless it names a file that
/// opens. Its epoch is now.
fn open(value: Option<OsString>) -> Option<Sink> {
    let path = value.filter(|v| !v.is_empty())?;
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()?;
    Some(Mutex::new(FrameLog::new(
        BufWriter::new(file),
        Instant::now(),
    )))
}

/// Read the variable, once, and open the file if it names one. The first
/// thing the app's `run` does, so the epoch is the process's start.
pub fn start() {
    LOG.get_or_init(|| open(std::env::var_os(VARIABLE)));
}

/// The main window has ended a frame's drawing.
pub fn main_drawn() {
    record(Frame::Main);
}

/// The floating wheel has put a frame on screen.
pub fn float_presented() {
    record(Frame::Float);
}

fn record(frame: Frame) {
    if let Some(Some(log)) = LOG.get() {
        if let Ok(mut log) = log.lock() {
            log.record(frame, Instant::now());
        }
    }
}

/// The main window's drawing took `took`, `BeginDraw` to `EndDraw`; `part`
/// if it drew only the rows that changed.
pub fn main_draw_took(took: Duration, part: bool) {
    if let Some(Some(log)) = LOG.get() {
        if let Ok(mut log) = log.lock() {
            log.record_draw(took, part);
        }
    }
}

/// The inspector's clock has ticked.
pub fn clock_ticked() {
    record(Frame::Clock);
}

/// Write out what is buffered. Called as the app exits.
pub fn finish() {
    if let Some(Some(log)) = LOG.get() {
        if let Ok(mut log) = log.lock() {
            log.flush(Instant::now());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A writer that keeps what it is given and counts its flushes.
    #[derive(Default)]
    struct Spy {
        bytes: Vec<u8>,
        flushes: u32,
    }

    impl Write for Spy {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.bytes.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.flushes += 1;
            Ok(())
        }
    }

    #[test]
    fn a_line_is_the_window_and_the_microseconds() {
        assert_eq!(line(Frame::Main, 0), "main 0");
        assert_eq!(line(Frame::Main, 1_500), "main 1500");
        assert_eq!(line(Frame::Float, 987_654_321), "float 987654321");
        assert_eq!(line(Frame::Clock, 42), "clock 42");
        assert_eq!(line(Frame::Draw, 1_234), "draw 1234");
        assert_eq!(line(Frame::Part, 99), "part 99");
    }

    #[test]
    fn frames_are_counted_from_the_epoch_one_to_a_line() {
        let epoch = Instant::now();
        let mut log = FrameLog::new(Spy::default(), epoch);
        log.record(Frame::Main, epoch + Duration::from_micros(1_500));
        log.record(Frame::Float, epoch + Duration::from_micros(17_000));
        assert_eq!(
            String::from_utf8(log.out.bytes).unwrap(),
            "main 1500\nfloat 17000\n"
        );
    }

    #[test]
    fn the_buffer_is_flushed_every_hundred_milliseconds_of_frames() {
        let epoch = Instant::now();
        let ms = |n| epoch + Duration::from_millis(n);
        let mut log = FrameLog::new(Spy::default(), epoch);
        for n in [16, 33, 50, 66, 83, 99] {
            log.record(Frame::Main, ms(n));
        }
        assert_eq!(log.out.flushes, 0, "under 100 ms: still buffered");
        log.record(Frame::Main, ms(100));
        assert_eq!(log.out.flushes, 1, "100 ms: flushed");
        log.record(Frame::Float, ms(150));
        assert_eq!(log.out.flushes, 1, "the clock restarts at each flush");
        log.record(Frame::Float, ms(200));
        assert_eq!(log.out.flushes, 2);
    }

    #[test]
    fn a_frame_before_the_epoch_counts_as_zero() {
        let epoch = Instant::now();
        let mut log = FrameLog::new(Spy::default(), epoch + Duration::from_secs(1));
        log.record(Frame::Main, epoch);
        assert_eq!(String::from_utf8(log.out.bytes).unwrap(), "main 0\n");
    }

    #[test]
    fn with_the_variable_unset_the_logger_does_nothing() {
        assert!(open(None).is_none());
        assert!(open(Some(OsString::new())).is_none());
        // Before `start`, drawing a frame is a no-op that touches nothing.
        main_drawn();
        float_presented();
        clock_ticked();
        main_draw_took(Duration::from_micros(1), false);
        finish();
    }

    #[test]
    fn a_path_that_wont_open_logs_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("no-such-folder").join("frames.log");
        assert!(open(Some(missing.into_os_string())).is_none());
    }

    #[test]
    fn the_file_is_appended_to_and_flushed_at_the_end() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("frames.log");
        std::fs::write(&path, "kept\n").unwrap();
        let sink = open(Some(path.clone().into_os_string())).expect("opens");
        {
            let mut log = sink.lock().unwrap();
            let epoch = log.epoch;
            log.record(Frame::Main, epoch + Duration::from_micros(5));
            log.record(Frame::Float, epoch + Duration::from_micros(9));
            // Buffered: nothing of ours is in the file yet.
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "kept\n");
            log.flush(epoch);
        }
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "kept\nmain 5\nfloat 9\n"
        );
    }
}
