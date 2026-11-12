//! R78: a Matroska picture's decoded frames take its block times, the n-th
//! frame the n-th smallest block time of the track, as the encoder restores
//! its packets' times by number (R38). Media Foundation's decoder doesn't
//! always give the first frame its block's time (0, 27.0 and 27.6 ms seen,
//! one file read again and again, for a block at 28 ms), nor each frame its
//! length.
//!
//! A frame's length is the gap to the next block time; the last frame's is
//! its block's `BlockDuration`, else the track's `DefaultDuration`, else the
//! gap before it (R80), and only for a lone frame the decoder's. Where the
//! container says the rate is constant, every frame keeps the length the
//! reader gives it.
//!
//! The blocks are read as the frames come, [`BLOCKS_AHEAD`] ahead. Each
//! frame's decoder time must be nearer its own block's than either
//! neighbour's, and the block times must rise. Where they don't, the decoder
//! dropped or added a frame (as it drops those before a file's first key
//! frame), or the blocks can't time the frames: the clock parts from the
//! blocks, reads no more of them, and from there on the frames keep the
//! decoder's times. The first frame, whose decoder time is the one in doubt,
//! is held until the second has been checked. At the end, one block more
//! than the frames means the counts differ, which is noted too; nothing
//! more is read to find it.
//!
//! The clock is generic in the frame it holds, so it is tested here
//! without a decoder.

#![cfg_attr(not(windows), allow(dead_code))]

use crate::mkvread::{BlockTime, Untimed};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};

/// How many blocks are read past a frame's before it is timed: more than
/// any decoder holds back to reorder (H.264 and HEVC hold at most 16).
pub(crate) const BLOCKS_AHEAD: usize = 64;

/// A track's blocks, in the file's order.
pub(crate) type Blocks = Box<dyn Iterator<Item = Result<BlockTime, Untimed>> + Send>;

/// A decoded frame as the decoder gave it, and its time and length in
/// 100 ns.
pub(crate) type Timed<T> = (T, i64, i64);

/// Times one track's decoded frames from its blocks (see the module).
pub(crate) struct BlockClock<T> {
    /// `None` once read to the end, parted, or stopped by a block that
    /// can't be timed.
    blocks: Option<Blocks>,
    scale: u64,
    shift: i64,
    /// The track's `DefaultDuration`, in 100 ns.
    default: Option<i64>,
    /// The container says the rate is constant: each frame keeps the length
    /// the reader gives it.
    fixed: bool,
    /// Blocks read and not yet given to a frame: (time, length), by time.
    ahead: BinaryHeap<Reverse<(i64, Option<i64>)>>,
    framed: usize,
    /// The last block time given to a frame.
    last: Option<i64>,
    /// The first frame, and its block's time and length.
    held: Option<(Timed<T>, (i64, i64))>,
    /// The frames and blocks parted: from there the frames keep the
    /// decoder's times.
    parted: bool,
    /// Every frame lined up, but the blocks outnumber them.
    counts_differ: bool,
}

impl<T> BlockClock<T> {
    /// A clock over `blocks` (`None` when they can't be read) of a track in
    /// a file of `scale` ns per tick, whose `DefaultDuration` is `default`
    /// ns. `shift` moves every block time as the reader moves the decoder's;
    /// `fixed` keeps the decoder's lengths. A scale of 0 times nothing: the
    /// frames keep the decoder's times.
    pub fn new(
        blocks: Option<Blocks>,
        scale: u64,
        default: Option<u64>,
        shift: i64,
        fixed: bool,
    ) -> Self {
        let blocks = blocks.filter(|_| scale > 0);
        BlockClock {
            parted: blocks.is_none(),
            blocks,
            scale,
            shift,
            default: default.map(|d| (d / 100).min(i64::MAX as u64) as i64),
            fixed,
            ahead: BinaryHeap::new(),
            framed: 0,
            last: None,
            held: None,
            counts_differ: false,
        }
    }

    /// True when some frames kept the decoder's times, or the blocks
    /// outnumbered the frames.
    pub fn decoder_times(&self) -> bool {
        self.parted || self.counts_differ
    }

    /// `ticks` of the file's scale in 100 ns.
    fn hns(&self, ticks: i64) -> i64 {
        let ns = ticks as i128 * self.scale as i128;
        (ns + 50)
            .div_euclid(100)
            .clamp(i64::MIN as i128, i64::MAX as i128) as i64
    }

    /// From here on the frames keep the decoder's times; no more blocks are
    /// read.
    fn part(&mut self) {
        self.parted = true;
        self.blocks = None;
        self.ahead.clear();
    }

    /// Reads blocks until [`BLOCKS_AHEAD`] wait past the next frame's.
    fn fill(&mut self) {
        while self.ahead.len() < BLOCKS_AHEAD + 2 {
            let Some(blocks) = self.blocks.as_mut() else {
                return;
            };
            let b = match blocks.next() {
                Some(Ok(b)) => b,
                Some(Err(_)) => {
                    // A block that can't be timed (laced, or the clusters
                    // broken): the frames go on with the blocks read, and
                    // one with none left parts them.
                    self.blocks = None;
                    return;
                }
                None => {
                    self.blocks = None;
                    return;
                }
            };
            let time = self.hns(b.time).saturating_add(self.shift);
            if self.last.is_some_and(|l| time < l) {
                // Reordered further than the reading ahead reaches.
                self.part();
                return;
            }
            let length = b.duration.map(|d| self.hns(d.min(i64::MAX as u64) as i64));
            self.ahead.push(Reverse((time, length)));
        }
    }

    /// The next frame's block time and length, for a frame the decoder put
    /// at `pts` for `length`, and whether it lines up with that block.
    /// `None` when parted or no block is left.
    fn take(&mut self, pts: i64, length: i64) -> Option<((i64, i64), bool)> {
        self.fill();
        if self.parted {
            return None;
        }
        let Reverse((time, own)) = self.ahead.pop()?;
        self.framed += 1;
        let next = self.ahead.peek().map(|b| b.0 .0);
        let gap = self.last.map(|p| time - p).filter(|&g| g > 0);
        let off = pts.abs_diff(time);
        let lines_up = self.last.is_none_or(|p| time > p && off < pts.abs_diff(p))
            && next.is_none_or(|n| off < pts.abs_diff(n));
        self.last = Some(time);
        let length = if self.fixed {
            length
        } else {
            next.map(|n| n - time)
                .or(own)
                .or(self.default)
                .or(gap)
                .unwrap_or(length)
        };
        Some(((time, length), lines_up))
    }

    /// Times a decoded frame, putting those ready in `out`.
    pub fn frame(&mut self, f: Timed<T>, out: &mut VecDeque<Timed<T>>) {
        if let Some((times, lines_up)) = self.take(f.1, f.2) {
            if self.framed == 1 {
                self.held = Some((f, times));
                return;
            }
            if lines_up {
                if let Some((h, t)) = self.held.take() {
                    out.push_back((h.0, t.0, t.1));
                }
                out.push_back((f.0, times.0, times.1));
                return;
            }
        }
        if !self.parted {
            self.part();
        }
        // The decoder's own times.
        if let Some((h, _)) = self.held.take() {
            out.push_back(h);
        }
        out.push_back(f);
    }

    /// At the end of the frames: the first one if it is still held, and
    /// whether a block is left over. At most one more block is read.
    pub fn end(&mut self, out: &mut VecDeque<Timed<T>>) {
        if !self.parted {
            let more = !self.ahead.is_empty()
                || self
                    .blocks
                    .as_mut()
                    .and_then(Iterator::next)
                    .is_some_and(|b| b.is_ok());
            self.counts_differ = more;
            self.blocks = None;
        }
        if let Some((h, t)) = self.held.take() {
            out.push_back(if self.decoder_times() {
                h
            } else {
                (h.0, t.0, t.1)
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Blocks at these times (ms, a 1 ms scale), with no durations.
    fn at(times: &[i64]) -> Vec<Result<BlockTime, Untimed>> {
        times
            .iter()
            .map(|&time| {
                Ok(BlockTime {
                    time,
                    duration: None,
                })
            })
            .collect()
    }

    fn clock(blocks: Vec<Result<BlockTime, Untimed>>) -> BlockClock<usize> {
        BlockClock::new(
            Some(Box::new(blocks.into_iter())),
            1_000_000,
            None,
            0,
            false,
        )
    }

    const MS: i64 = 10_000;

    /// Runs the frames (decoder time and length, in ms) through `c`: the
    /// times and lengths given, in ms, and whether they are the decoder's.
    fn run(mut c: BlockClock<usize>, frames: &[(i64, i64)]) -> (Vec<(i64, i64)>, bool) {
        let mut out = VecDeque::new();
        for (i, &(t, l)) in frames.iter().enumerate() {
            c.frame((i, t * MS, l * MS), &mut out);
        }
        c.end(&mut out);
        let given: Vec<_> = out.iter().map(|f| (f.1 / MS, f.2 / MS)).collect();
        assert!(out.iter().enumerate().all(|(i, f)| f.0 == i), "in order");
        (given, c.decoder_times())
    }

    #[test]
    fn frames_take_the_block_times_in_order_and_last_until_the_next() {
        // B-frames: stored 28, 128, 61, 95. The decoder put the first frame
        // at 0 and gave lengths of its own.
        let c = clock(at(&[28, 128, 61, 95]));
        let (got, decoder) = run(c, &[(0, 30), (61, 30), (95, 30), (128, 30)]);
        // The last lasts the gap before it (R80), not the decoder's 30.
        assert_eq!(got, [(28, 33), (61, 34), (95, 33), (128, 33)]);
        assert!(!decoder);
    }

    #[test]
    fn the_last_frame_lasts_its_block_then_the_tracks_then_the_decoders() {
        let blocks = |last: Option<u64>| {
            vec![
                Ok(BlockTime {
                    time: 0,
                    duration: None,
                }),
                Ok(BlockTime {
                    time: 40,
                    duration: last,
                }),
            ]
        };
        let frames = [(0, 7), (40, 7)];
        let new = |last, default| {
            BlockClock::new(
                Some(Box::new(blocks(last).into_iter()) as Blocks),
                1_000_000,
                default,
                0,
                false,
            )
        };
        assert_eq!(run(new(Some(20), Some(30_000_000)), &frames).0[1], (40, 20));
        assert_eq!(run(new(None, Some(30_000_000)), &frames).0[1], (40, 30));
        // Neither: the gap before it (R80).
        assert_eq!(run(new(None, None), &frames).0[1], (40, 40));
        // A lone frame with neither: the decoder's.
        let lone = BlockClock::new(
            Some(Box::new(at(&[0]).into_iter())),
            1_000_000,
            None,
            0,
            false,
        );
        assert_eq!(run(lone, &[(0, 7)]).0, [(0, 7)]);
        // A BlockDuration past what a signed time holds is held to it.
        let (got, _) = run(new(Some(u64::MAX), None), &frames);
        assert!(got[1].1 > 0, "{got:?}");
    }

    #[test]
    fn a_constant_rate_keeps_the_readers_lengths() {
        let c = BlockClock::new(
            Some(Box::new(at(&[0, 33, 67]).into_iter())),
            1_000_000,
            None,
            0,
            true,
        );
        let (got, _) = run(c, &[(0, 33), (33, 33), (67, 33)]);
        assert_eq!(got, [(0, 33), (33, 33), (67, 33)]);
    }

    #[test]
    fn a_dropped_first_frame_keeps_the_decoders_times() {
        // Blocks at 0, 33, 67, 100; the decoder gave the last three only.
        let (got, decoder) = run(
            clock(at(&[0, 33, 67, 100])),
            &[(33, 33), (67, 33), (100, 33)],
        );
        assert_eq!(got, [(33, 33), (67, 33), (100, 33)]);
        assert!(decoder);
    }

    #[test]
    fn once_parted_no_block_is_read() {
        // The decoder skips a frame at the third; then the frames go on to
        // the end. Reading a block past those read ahead before the parting
        // would panic.
        let first: Vec<i64> = (0..BLOCKS_AHEAD as i64 + 4).map(|n| n * 10).collect();
        let touched = std::iter::from_fn(|| -> Option<Result<BlockTime, Untimed>> {
            panic!("a block was read after the parting")
        });
        let blocks = at(&first).into_iter().chain(touched);
        let c = BlockClock::new(Some(Box::new(blocks)), 1_000_000, None, 0, false);
        let mut frames = vec![(0, 10), (10, 10), (30, 10)];
        frames.extend((4..20).map(|n| (n * 10, 10)));
        let (got, decoder) = run(c, &frames);
        assert!(decoder);
        assert_eq!(got.len(), frames.len());
        assert_eq!(got[2], (30, 10), "the decoder's time from the parting");
    }

    #[test]
    fn a_block_that_cant_be_timed_after_the_last_frame_changes_nothing() {
        let mut blocks = at(&[0, 33, 67]);
        blocks.push(Err(Untimed::Laced));
        blocks.extend(at(&[100]));
        let (got, decoder) = run(clock(blocks), &[(0, 33), (33, 33), (67, 33)]);
        assert_eq!(got, [(0, 33), (33, 34), (67, 34)]);
        assert!(!decoder, "how many blocks there are is unknown");
        // One before a frame needs it: that frame and the rest keep the
        // decoder's times.
        let mut blocks = at(&[0, 33]);
        blocks.push(Err(Untimed::Laced));
        let (got, decoder) = run(clock(blocks), &[(0, 33), (33, 33), (67, 33)]);
        assert_eq!(got[2], (67, 33));
        assert!(decoder);
    }

    #[test]
    fn a_block_left_over_at_the_end_is_noted() {
        let (got, decoder) = run(clock(at(&[0, 33, 67, 100])), &[(0, 33), (33, 33), (67, 33)]);
        assert_eq!(got, [(0, 33), (33, 34), (67, 33)], "every frame lined up");
        assert!(decoder, "the counts differ");
    }

    #[test]
    fn blocks_that_cant_time_the_frames_are_left_alone() {
        // A scale of 0, and blocks all at one time.
        let frames = [(0, 33), (33, 33), (67, 33)];
        let zero = BlockClock::new(
            Some(Box::new(at(&[0, 33, 67]).into_iter())),
            0,
            None,
            0,
            false,
        );
        assert_eq!(run(zero, &frames), (frames.to_vec(), true));
        assert_eq!(run(clock(at(&[5, 5, 5])), &frames), (frames.to_vec(), true));
        // No blocks at all.
        let none = BlockClock::<usize>::new(None, 1_000_000, None, 0, false);
        assert_eq!(run(none, &frames), (frames.to_vec(), true));
    }

    #[test]
    fn a_frame_halfway_between_two_blocks_is_not_taken_as_lined_up() {
        // A true tie: the frame at 33 is as near the block given before it
        // (0) as the one it would take (66). Nearer is required, so it
        // parts and keeps the decoder's time.
        let (got, decoder) = run(clock(at(&[0, 66, 100])), &[(0, 33), (33, 33), (67, 33)]);
        assert!(decoder);
        assert_eq!(got[1], (33, 33));
        // Just nearer its own block than the one before: it lines up.
        let (got, decoder) = run(clock(at(&[0, 65, 100])), &[(0, 33), (33, 33), (100, 33)]);
        assert!(!decoder);
        assert_eq!(got[1], (65, 35));
    }

    #[test]
    fn a_frame_without_its_block_is_found_whatever_the_spacing() {
        // The decoder gives a frame at 33 whose block is missing (one read
        // later, shown earlier, past the reading ahead): the block at 67
        // would be just as near as the last one given, at 0.
        let (got, decoder) = run(clock(at(&[0, 67, 100])), &[(0, 33), (33, 33), (67, 33)]);
        assert!(decoder);
        assert_eq!(got[1], (33, 33));
    }
}
