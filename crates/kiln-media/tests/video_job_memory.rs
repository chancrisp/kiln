//! The video job's memory stays flat: 100,000 generated packets of 1 KB
//! through the interleaver into each muxer raise the process's peak working
//! set by under 64 MB.
//!
//! In a test binary of its own, so nothing else running in the process moves
//! the peak.

#![cfg(windows)]

use kiln_media::video::{
    muxer, AudioParams, Container, Interleaver, Packet, StreamCodec, StreamSpec, VideoParams,
    TIMESCALE,
};
use kiln_media::video::{Hdr, Mux};
use kiln_media::Tags;
use std::fs::File;
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::Threading::GetCurrentProcess;

/// Packets in all, per muxer.
const PACKETS: usize = 100_000;
/// Each packet's size.
const BYTES: usize = 1024;
/// The sound's rate, and its samples per packet (AAC's frame).
const RATE: u32 = 48_000;
const FRAME: i64 = 1024;

/// The process's peak working set, in bytes.
fn peak() -> usize {
    let mut c = PROCESS_MEMORY_COUNTERS {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ..Default::default()
    };
    // SAFETY: the counters are the size passed, and the handle is this
    // process's pseudo-handle.
    unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) }.unwrap();
    c.PeakWorkingSetSize
}

fn picture() -> StreamSpec {
    // An avcC for a 320x240 baseline stream; the muxers only carry it.
    let avcc = vec![
        1, 0x42, 0xC0, 0x1E, 0xFF, 0xE1, 0, 4, 0x67, 0x42, 0xC0, 0x1E, 1, 0, 4, 0x68, 0xCE, 0x3C,
        0x80,
    ];
    StreamSpec {
        codec: StreamCodec::H264 { avcc },
        timescale: TIMESCALE,
        video: Some(VideoParams {
            width: 320,
            height: 240,
            rotation: 0,
            hdr: Hdr::None,
            bit_depth: 8,
            mastering: None,
            content_light: None,
            sar: (1, 1),
            tag_colour: false,
            bt601: false,
            frame_ns: None,
            stated_colour: None,
        }),
        audio: None,
    }
}

fn sound() -> StreamSpec {
    StreamSpec {
        codec: StreamCodec::Aac {
            asc: vec![0x11, 0x90],
        },
        timescale: RATE,
        video: None,
        audio: Some(AudioParams {
            rate: RATE,
            channels: 2,
        }),
    }
}

/// The packets of a 30 fps picture and 48 kHz AAC sound, in the order a
/// reader would give them: whichever stream is behind goes next.
struct Generator {
    video: i64,
    sound: i64,
    made: usize,
}

impl Iterator for Generator {
    type Item = Packet;

    fn next(&mut self) -> Option<Packet> {
        if self.made == PACKETS {
            return None;
        }
        self.made += 1;
        let frame = TIMESCALE as i64 / 30;
        // Compare the next times across the two clocks.
        let v_at = self.video as i128 * frame as i128 * RATE as i128;
        let s_at = self.sound as i128 * FRAME as i128 * TIMESCALE as i128;
        let fill = (self.made % 251) as u8;
        Some(if v_at <= s_at {
            let n = self.video;
            self.video += 1;
            Packet {
                stream: 0,
                pts: n * frame,
                dts: n * frame,
                duration: frame,
                key: n % 30 == 0,
                data: vec![fill; BYTES],
            }
        } else {
            let n = self.sound;
            self.sound += 1;
            Packet {
                stream: 1,
                pts: n * FRAME,
                dts: n * FRAME,
                duration: FRAME,
                key: true,
                data: vec![fill; BYTES],
            }
        })
    }
}

fn copy_into(c: Container, out: File) -> usize {
    let m: Box<dyn Mux> = muxer(c, out).unwrap();
    let mut il = Interleaver::new(m, 2);
    il.set_stream(0, picture());
    il.set_stream(1, sound());
    let mut most_waiting = 0;
    for p in (Generator {
        video: 0,
        sound: 0,
        made: 0,
    }) {
        il.push(p).unwrap();
        most_waiting = most_waiting.max(il.waiting());
    }
    il.finish(&Tags::default()).unwrap();
    most_waiting
}

#[test]
fn many_packets_keep_memory_flat() {
    let dir = tempfile::tempdir().unwrap();
    let before = peak();
    for (c, name) in [(Container::Mp4, "many.mp4"), (Container::Mkv, "many.mkv")] {
        let path = dir.path().join(name);
        let most_waiting = copy_into(c, File::create(&path).unwrap());
        // The packets come in time order, so hardly any wait.
        assert!(most_waiting <= 2, "{name}: {most_waiting} packets waited");
        let len = std::fs::metadata(&path).unwrap().len();
        assert!(len > (PACKETS * BYTES) as u64, "{name} is {len} bytes");
    }
    let grew = peak().saturating_sub(before);
    assert!(
        grew < 64 << 20,
        "the peak working set grew by {} MB",
        grew >> 20
    );
}
