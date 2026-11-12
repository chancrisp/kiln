//! Silence for the holes in a sound track's timestamps.
//!
//! Matroska and MP4 time every packet. When packets are missing (a stretch
//! cut out of the track, a recorder that sent nothing for a while, Opus that
//! left out its quiet parts) the sound that is read is shorter than the
//! picture and drifts out of step with it. Before it uses a packet, a source
//! asks [`GapFill`] how much silence goes in front of it, and hands that out
//! first.
//!
//! The clock counts from the first packet: a track that starts late gets no
//! silence at its start.

use std::time::Duration;

/// A hole in the timestamps shorter than this is how they were rounded (a
/// millisecond clock against 1,024-sample packets), or the decoder's own
/// delay, not missing sound.
pub(crate) const GAP_THRESHOLD: Duration = Duration::from_millis(100);

/// The most silence one gap gets, and also the most one source gets in all.
/// A file with a wild timestamp, or thousands of them, can't make the output
/// longer than this.
pub(crate) const MAX_FILL: Duration = Duration::from_secs(600);

/// Silence is handed out in pieces no longer than this.
pub(crate) const CHUNK: Duration = Duration::from_secs(1);

/// The highest rate sound is recorded at. A track that states a higher one
/// has a damaged header, and its timestamps are no more to be trusted: it gets
/// no silence, which at such a rate would run to billions of frames.
pub(crate) const MAX_RATE: u32 = 768_000;

/// Works out the silence a source owes before each of its packets.
#[derive(Debug, Clone)]
pub(crate) struct GapFill {
    rate: u32,
    /// The time, in frames, that falls at frame 0 of the output. `None` until
    /// the first packet sets it.
    start: Option<i128>,
    /// Silent frames handed out in all.
    filled: u64,
}

impl GapFill {
    /// A fill for a track of `rate` frames a second.
    pub fn new(rate: u32) -> Self {
        GapFill {
            rate,
            start: None,
            filled: 0,
        }
    }

    /// The silent frames to put in front of a packet that starts at
    /// `pts_frames`: its time in frames of the track's rate, on any fixed
    /// clock. `decoded` is the frames the packets before it gave.
    ///
    /// Silence is owed only for a hole of at least [`GAP_THRESHOLD`]. A packet
    /// that starts before the sound read so far ends (an overlap: frames that
    /// share a timestamp, or a single one stamped back) is owed none, and
    /// moves nothing: the clock stays as it stood, since false silence is
    /// worse than a gap missed. After a gap, or a wild one cut down to
    /// [`MAX_FILL`], the next packet that follows on is owed none either. A
    /// track with no rate, or one over [`MAX_RATE`], is owed none at all.
    pub fn silence_before(&mut self, pts_frames: u64, decoded: u64) -> u64 {
        if self.rate == 0 || self.rate > MAX_RATE {
            return 0;
        }
        let pts = pts_frames as i128;
        let out = decoded as i128 + self.filled as i128;
        let Some(start) = self.start else {
            self.start = Some(pts - out);
            return 0;
        };
        let gap = pts - start - out;
        if gap < self.frames(GAP_THRESHOLD) as i128 {
            return 0;
        }
        let room = self.frames(MAX_FILL).saturating_sub(self.filled);
        let fill = gap.min(room as i128) as u64;
        self.filled += fill;
        self.start = Some(pts - out - fill as i128);
        fill
    }

    /// The next packet sets the clock again: a new part of the track begins
    /// (the next stream of a chained file, where the times start over), or a
    /// packet that couldn't be decoded was passed over, whose time is not a
    /// hole to fill. The silence handed out so far still counts against
    /// [`MAX_FILL`].
    pub fn restart(&mut self) {
        self.start = None;
    }

    /// How much of `owed` silent frames to hand out next, at most [`CHUNK`].
    pub fn chunk(&self, owed: u64) -> u64 {
        owed.min(self.frames(CHUNK))
    }

    fn frames(&self, d: Duration) -> u64 {
        (d.as_nanos() * self.rate as u128 / 1_000_000_000) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;
    const PACKET: u64 = 1024;

    fn frames(d: Duration) -> u64 {
        GapFill::new(RATE).frames(d)
    }

    /// Feeds `n` packets of `PACKET` frames that follow on from each other,
    /// starting at `pts` after `decoded` frames, and moves both on past them.
    /// Returns the silence they were owed.
    fn run(fill: &mut GapFill, pts: &mut u64, decoded: &mut u64, n: u64) -> u64 {
        let mut owed = 0;
        for _ in 0..n {
            owed += fill.silence_before(*pts, *decoded);
            *pts += PACKET;
            *decoded += PACKET;
        }
        owed
    }

    #[test]
    fn no_silence_for_contiguous_packets() {
        let mut fill = GapFill::new(RATE);
        // The clock counts from the first packet, wherever that is.
        let (mut pts, mut decoded) = (123_456, 0);
        assert_eq!(run(&mut fill, &mut pts, &mut decoded, 500), 0);
    }

    #[test]
    fn a_two_second_gap_gets_two_seconds_of_silence() {
        let mut fill = GapFill::new(RATE);
        let (mut pts, mut decoded) = (0, 0);
        assert_eq!(run(&mut fill, &mut pts, &mut decoded, 100), 0);
        pts += 2 * RATE as u64;
        assert_eq!(fill.silence_before(pts, decoded), 96_000);
        // The packet after it follows on, with the silence counted.
        pts += PACKET;
        decoded += PACKET;
        assert_eq!(run(&mut fill, &mut pts, &mut decoded, 100), 0);
    }

    #[test]
    fn jitter_under_the_threshold_is_ignored() {
        let mut fill = GapFill::new(RATE);
        let (mut pts, mut decoded) = (0, 0);
        run(&mut fill, &mut pts, &mut decoded, 10);
        // 0.05 s late, and 1 frame short of the threshold.
        assert_eq!(fill.silence_before(pts + 2_400, decoded), 0);
        assert_eq!(
            fill.silence_before(pts + frames(GAP_THRESHOLD) - 1, decoded),
            0
        );
        assert_eq!(
            fill.silence_before(pts + frames(GAP_THRESHOLD), decoded),
            4_800
        );
    }

    #[test]
    fn a_wild_timestamp_is_capped() {
        let mut fill = GapFill::new(RATE);
        let (mut pts, mut decoded) = (0, 0);
        run(&mut fill, &mut pts, &mut decoded, 10);
        let day = 24 * 60 * 60 * RATE as u64;
        let owed = fill.silence_before(pts + day, decoded);
        assert_eq!(owed, frames(MAX_FILL));
        // The clock moved to the wild timestamp: what follows on is whole.
        let (mut pts, mut decoded) = (pts + day + PACKET, decoded + PACKET);
        assert_eq!(run(&mut fill, &mut pts, &mut decoded, 100), 0);
    }

    #[test]
    fn the_total_fill_is_capped() {
        let mut fill = GapFill::new(RATE);
        let (mut pts, mut decoded) = (0, 0);
        let day = 24 * 60 * 60 * RATE as u64;
        let mut total = 0;
        for _ in 0..5_000 {
            pts += day;
            let owed = fill.silence_before(pts, decoded);
            total += owed;
            decoded += PACKET;
            pts += PACKET;
        }
        assert_eq!(total, frames(MAX_FILL));

        // Gaps that are each fine add up to the same, and no more.
        let mut fill = GapFill::new(RATE);
        let (mut pts, mut decoded) = (0, 0);
        let mut total = 0;
        for _ in 0..50 {
            pts += 100 * RATE as u64;
            let owed = fill.silence_before(pts, decoded);
            assert!(owed <= frames(MAX_FILL));
            total += owed;
            decoded += PACKET;
            pts += PACKET;
        }
        assert_eq!(total, frames(MAX_FILL));
    }

    #[test]
    fn overlaps_insert_nothing() {
        let mut fill = GapFill::new(RATE);
        let (mut pts, mut decoded) = (0, 0);
        run(&mut fill, &mut pts, &mut decoded, 100);
        // A packet that starts before the last one ended, and one back at 0.
        assert_eq!(fill.silence_before(pts - 500, decoded), 0);
        assert_eq!(fill.silence_before(pts - 20 * PACKET, decoded), 0);
        assert_eq!(fill.silence_before(0, decoded), 0);
    }

    #[test]
    fn a_gap_after_an_overlap_is_measured_from_the_clock_as_it_stood() {
        let mut fill = GapFill::new(RATE);
        let (mut pts, mut decoded) = (0, 0);
        run(&mut fill, &mut pts, &mut decoded, 100);
        // A packet stamped back at 0 is an overlap: it moves nothing, and the
        // packets after it carry on from where the clock was.
        assert_eq!(fill.silence_before(0, decoded), 0);
        decoded += PACKET;
        pts += PACKET;
        assert_eq!(run(&mut fill, &mut pts, &mut decoded, 10), 0);
        // A gap later is measured from that clock, so it is found whole.
        assert_eq!(fill.silence_before(pts + RATE as u64, decoded), 48_000);
    }

    #[test]
    fn a_laced_block_gets_no_silence() {
        // A laced Matroska block with no stated duration gives each of its
        // frames the block's timestamp. The frames add up to over 100 ms.
        const FRAMES: u64 = 6;
        assert!(FRAMES * PACKET > frames(GAP_THRESHOLD));
        let mut fill = GapFill::new(RATE);
        let mut decoded = 0;
        let mut owed = 0;
        for block in 0..200 {
            let pts = block * FRAMES * PACKET;
            for _ in 0..FRAMES {
                owed += fill.silence_before(pts, decoded);
                decoded += PACKET;
            }
        }
        assert_eq!(owed, 0);
    }

    #[test]
    fn one_backward_timestamp_adds_no_silence() {
        let mut fill = GapFill::new(RATE);
        let (mut pts, mut decoded) = (0, 0);
        // 100 s of ordinary packets.
        assert_eq!(
            run(
                &mut fill,
                &mut pts,
                &mut decoded,
                100 * RATE as u64 / PACKET
            ),
            0
        );
        // One stamped back at 0, then ordinary ones again.
        assert_eq!(fill.silence_before(0, decoded), 0);
        decoded += PACKET;
        pts += PACKET;
        assert_eq!(run(&mut fill, &mut pts, &mut decoded, 1_000), 0);
    }

    #[test]
    fn a_restart_counts_from_the_next_packet() {
        let mut fill = GapFill::new(RATE);
        let (mut pts, mut decoded) = (0, 0);
        run(&mut fill, &mut pts, &mut decoded, 100);
        fill.restart();
        // The next part's clock begins far from the first part's.
        let (mut pts, mut decoded) = (10 * RATE as u64, decoded);
        assert_eq!(run(&mut fill, &mut pts, &mut decoded, 100), 0);
    }

    #[test]
    fn a_rate_nobody_records_at_gets_no_silence() {
        for rate in [0, MAX_RATE + 1, 4_294_945_860] {
            let mut fill = GapFill::new(rate);
            assert_eq!(fill.silence_before(0, 0), 0);
            assert_eq!(fill.silence_before(1 << 40, 1024), 0);
        }
        // The highest rate sound is recorded at still gets its gaps filled.
        let mut fill = GapFill::new(MAX_RATE);
        assert_eq!(fill.silence_before(0, 0), 0);
        assert_eq!(
            fill.silence_before(2 * MAX_RATE as u64, 0),
            2 * MAX_RATE as u64
        );
    }

    #[test]
    fn silence_goes_out_a_second_at_a_time() {
        let fill = GapFill::new(RATE);
        assert_eq!(fill.chunk(10), 10);
        assert_eq!(fill.chunk(RATE as u64), RATE as u64);
        assert_eq!(fill.chunk(5 * RATE as u64), RATE as u64);
        assert_eq!(fill.chunk(0), 0);
    }
}
