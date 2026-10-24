//! The motion engine: every animation's values in one place.
//!
//! Each value is a pure function of when it started, how long it lasts
//! and the time now, so tests can drive time directly. The window reads
//! the clock and Windows' Animation effects setting once and passes them
//! in. With animations off, every value is at its end at once.
//!
//! Easing is `theme::motion::ease_out`, a cubic that starts fast, stops
//! gently and never overshoots.

use std::time::{Duration, Instant};

use crate::theme::motion;

/// Rows pouring in: each rises `POUR_RISE` and fades in over `POUR_MS`.
/// The delay grows `POUR_STAGGER_MS` a row, up to `POUR_STAGGER_CAP_MS`,
/// so 500 rows don't crawl.
pub const POUR_MS: u32 = 260;
pub const POUR_STAGGER_MS: u32 = 45;
pub const POUR_STAGGER_CAP_MS: u32 = 400;
pub const POUR_RISE: f32 = 10.0;

/// The selection's wash and edge gliding to the new row.
pub const GLIDE_MS: u32 = 160;

/// The inspector's old content fading out as the new fades in, rising
/// `CROSSFADE_RISE`.
pub const CROSSFADE_MS: u32 = 180;
pub const CROSSFADE_RISE: f32 = 6.0;

/// A preview fading in when it arrives.
pub const PREVIEW_MS: u32 = 180;

/// A finished row's glaze wash, from `COOL_PEAK` down to nothing.
pub const COOL_MS: u32 = 900;
pub const COOL_PEAK: f32 = 0.32;

/// A failed row's shake: `CRACK_DX` one way, `CRACK_DX` the other, once.
pub const CRACK_MS: u32 = 120;
pub const CRACK_DX: f32 = 2.0;

/// The wheel opening: it scales up from `BLOOM_SCALE` and turns from
/// `BLOOM_TURN_DEG` as it fades in.
pub const BLOOM_MS: u32 = 200;
pub const BLOOM_SCALE: f32 = 0.86;
pub const BLOOM_TURN_DEG: f32 = -8.0;

/// The wheel's segments follow the disc, each `SEGMENT_STAGGER_MS` after
/// the one before, growing from `SEGMENT_SCALE`. Each takes `BLOOM_MS`.
pub const SEGMENT_STAGGER_MS: u32 = 18;
pub const SEGMENT_SCALE: f32 = 0.94;
/// Where the first segment starts, after the disc has begun.
const SEGMENT_LEAD_MS: u32 = 40;

/// Sheets (Settings, Help, options) sliding up and fading in.
pub const SHEET_MS: u32 = 180;
pub const SHEET_RISE: f32 = 12.0;

/// A button's fill lifting on hover and sinking on press.
pub const PRESS_MS: u32 = 120;

/// The inspector sliding out as the list widens.
pub const FOLD_MS: u32 = 220;

/// Frame time while something moves.
pub const FRAME_MS: u32 = 16;
/// Frame time when only progress lines are glowing: slow enough to stay
/// cheap, fast enough to look smooth.
pub const HEAT_FRAME_MS: u32 = 50;

/// Milliseconds from `start` to `now`; never negative. The window works
/// out every motion's progress from it.
pub(crate) fn elapsed_ms(start: Instant, now: Instant) -> f32 {
    now.saturating_duration_since(start).as_secs_f32() * 1000.0
}

/// `a` at `t` = 0 and `b` at `t` = 1, exactly, with no rounding at
/// either end.
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a * (1.0 - t) + b * t
}

/// A value that eases toward a target over a fixed duration.
#[derive(Clone, Copy, Debug)]
pub struct Tween {
    from: f32,
    to: f32,
    start: Instant,
    ms: u32,
}

impl Tween {
    /// A tween at rest at `v`: it isn't running, so it asks for no frames.
    pub fn new(v: f32, now: Instant) -> Tween {
        Tween {
            from: v,
            to: v,
            start: now,
            ms: 0,
        }
    }

    /// Where the tween is at `now`.
    pub fn value(&self, now: Instant) -> f32 {
        let t = motion::progress(elapsed_ms(self.start, now), self.ms);
        self.from + (self.to - self.from) * t
    }

    /// Ease toward `to` over `ms`, or jump there with animations off.
    /// Setting it again midway starts from where it is.
    pub fn set(&mut self, to: f32, ms: u32, now: Instant, animations: bool) {
        if (self.to - to).abs() < f32::EPSILON {
            return;
        }
        self.from = self.value(now);
        self.to = to;
        self.ms = motion::duration(ms, animations);
        self.start = now;
    }

    /// Whether the tween is still moving at `now`.
    pub fn running(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.start) < Duration::from_millis(self.ms as u64)
    }

    /// Where the tween is heading, whether or not it has got there.
    pub fn target(&self) -> f32 {
        self.to
    }
}

/// Eased progress, 0 to 1, of something of `ms` that started at `start`.
/// With animations off every motion is already done.
pub fn eased(start: Instant, ms: u32, now: Instant, animations: bool) -> f32 {
    motion::progress(elapsed_ms(start, now), motion::duration(ms, animations))
}

/// Whether a motion of `ms` that started at `start` is still going. With
/// animations off, none is.
pub fn within(start: Instant, ms: u32, now: Instant, animations: bool) -> bool {
    now.saturating_duration_since(start)
        < Duration::from_millis(motion::duration(ms, animations) as u64)
}

/// A row pouring in, `elapsed_ms` after it was added, as the `order`th of
/// its batch: (alpha, dy). The row waits `POUR_STAGGER_MS` for each row
/// before it, up to `POUR_STAGGER_CAP_MS`, then rises into place.
pub fn pour(elapsed_ms: f32, order: usize, animations: bool) -> (f32, f32) {
    if !animations {
        return (1.0, 0.0);
    }
    let delay = (order as u64 * POUR_STAGGER_MS as u64).min(POUR_STAGGER_CAP_MS as u64) as f32;
    rise(elapsed_ms - delay, POUR_MS, POUR_RISE, animations)
}

/// The glaze wash on a finished row, `elapsed_ms` after it finished: it
/// flashes at `COOL_PEAK` and cools to nothing.
pub fn cool(elapsed_ms: f32, animations: bool) -> f32 {
    if !animations {
        return 0.0;
    }
    COOL_PEAK * (1.0 - motion::progress(elapsed_ms, COOL_MS))
}

/// A failed row's sideways shake, `elapsed_ms` after it failed: dx goes
/// 0, then `-CRACK_DX`, then `CRACK_DX`, then 0, in thirds of `CRACK_MS`.
pub fn crack(elapsed_ms: f32, animations: bool) -> f32 {
    if !animations || elapsed_ms <= 0.0 || elapsed_ms >= CRACK_MS as f32 {
        return 0.0;
    }
    let third = elapsed_ms / CRACK_MS as f32 * 3.0;
    if third < 1.0 {
        lerp(0.0, -CRACK_DX, third)
    } else if third < 2.0 {
        lerp(-CRACK_DX, CRACK_DX, third - 1.0)
    } else {
        lerp(CRACK_DX, 0.0, third - 2.0)
    }
}

/// The wheel's disc opening, `elapsed_ms` after it opened: (alpha, scale,
/// turn in degrees).
pub fn bloom(elapsed_ms: f32, animations: bool) -> (f32, f32, f32) {
    let t = eased_at(elapsed_ms, BLOOM_MS, animations);
    (t, lerp(BLOOM_SCALE, 1.0, t), lerp(BLOOM_TURN_DEG, 0.0, t))
}

/// The wheel's `i`th segment opening, `elapsed_ms` after the wheel
/// opened: (alpha, scale). It starts `SEGMENT_STAGGER_MS` after the one
/// before it.
pub fn segment(elapsed_ms: f32, i: usize, animations: bool) -> (f32, f32) {
    let delay = SEGMENT_LEAD_MS as f32 + i as f32 * SEGMENT_STAGGER_MS as f32;
    let t = eased_at(elapsed_ms - delay, BLOOM_MS, animations);
    (t, lerp(SEGMENT_SCALE, 1.0, t))
}

/// Something rising `distance` into place as it fades in, `elapsed_ms`
/// after it began and over `ms`: (alpha, dy). The inspector's crossfade,
/// sheets and previews use it.
pub fn rise(elapsed_ms: f32, ms: u32, distance: f32, animations: bool) -> (f32, f32) {
    let t = eased_at(elapsed_ms, ms, animations);
    (t, lerp(distance, 0.0, t))
}

/// Eased progress, 0 to 1, `elapsed_ms` into a motion of `ms`; 1 with
/// animations off.
fn eased_at(elapsed_ms: f32, ms: u32, animations: bool) -> f32 {
    motion::progress(elapsed_ms, motion::duration(ms, animations))
}

/// How often the window's frame timer should fire: `Some(FRAME_MS)` while
/// something moves, `Some(HEAT_FRAME_MS)` while only a running line
/// breathes (not with animations off), and `None` when nothing needs a
/// frame, so an idle Kiln uses no CPU.
pub fn frame_ms(moving: bool, burning: bool, animations: bool) -> Option<u32> {
    if moving {
        Some(FRAME_MS)
    } else if burning && animations {
        Some(HEAT_FRAME_MS)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> Instant {
        Instant::now()
    }

    fn at(t: Instant, ms: u64) -> Instant {
        t + Duration::from_millis(ms)
    }

    #[test]
    fn a_tween_eases_to_its_target_and_stops() {
        let t = t0();
        let mut w = Tween::new(0.0, t);
        w.set(1.0, 200, t, true);
        assert!(w.value(at(t, 100)) > 0.5 && w.value(at(t, 100)) < 1.0);
        assert_eq!(w.value(at(t, 200)), 1.0);
        assert!(w.running(at(t, 199)) && !w.running(at(t, 200)));
    }

    #[test]
    fn a_crossfade_restarted_midway_starts_from_where_it_was() {
        let t = t0();
        let mut w = Tween::new(0.0, t);
        w.set(1.0, CROSSFADE_MS, t, true);
        let mid = w.value(at(t, 60));
        w.set(0.0, CROSSFADE_MS, at(t, 60), true);
        assert!((w.value(at(t, 60)) - mid).abs() < 1e-4);
    }

    #[test]
    fn a_tween_with_animations_off_is_at_its_target_at_once() {
        let t = t0();
        let mut w = Tween::new(0.0, t);
        w.set(1.0, 200, t, false);
        assert_eq!(w.value(t), 1.0);
        assert!(!w.running(t));
    }

    #[test]
    fn a_tween_at_rest_is_not_running() {
        let t = t0();
        let w = Tween::new(0.5, t);
        assert!(!w.running(t));
        assert_eq!(w.value(t), 0.5);
    }

    #[test]
    fn a_tween_knows_where_it_is_heading() {
        let t = t0();
        let mut w = Tween::new(0.25, t);
        assert_eq!(w.target(), 0.25);
        w.set(1.0, 200, t, true);
        assert_eq!(w.target(), 1.0);
        assert!(w.value(at(t, 50)) < 1.0);
    }

    #[test]
    fn a_tween_set_to_where_it_is_going_keeps_its_course() {
        let t = t0();
        let mut w = Tween::new(0.0, t);
        w.set(1.0, 200, t, true);
        let before = w.value(at(t, 100));
        w.set(1.0, 200, at(t, 100), true);
        assert_eq!(w.value(at(t, 100)), before);
        assert!(w.running(at(t, 199)));
    }

    #[test]
    fn eased_and_within_follow_the_clock_and_the_setting() {
        let t = t0();
        assert_eq!(eased(t, 200, t, true), 0.0);
        assert!(eased(t, 200, at(t, 100), true) > 0.5);
        assert_eq!(eased(t, 200, at(t, 200), true), 1.0);
        assert!(within(t, 200, at(t, 199), true));
        assert!(!within(t, 200, at(t, 200), true));
        assert_eq!(eased(t, 200, t, false), 1.0);
        assert!(!within(t, 200, t, false));
        // A clock that reads before the start counts as the start.
        assert_eq!(eased(at(t, 50), 200, t, true), 0.0);
    }

    #[test]
    fn rows_pour_with_a_capped_stagger() {
        assert_eq!(pour(0.0, 0, true), (0.0, POUR_RISE));
        assert_eq!(pour(POUR_MS as f32, 0, true), (1.0, 0.0));
        // Row 499 starts no later than the cap.
        let end = (POUR_STAGGER_CAP_MS + POUR_MS) as f32;
        assert_eq!(pour(end, 499, true), (1.0, 0.0));
        assert_eq!(pour(0.0, 499, false), (1.0, 0.0));
    }

    #[test]
    fn each_row_waits_its_turn_until_the_cap() {
        // Row 3 hasn't begun at 3 x 45 ms; row 0 is well on its way.
        let wait = 3.0 * POUR_STAGGER_MS as f32;
        assert_eq!(pour(wait, 3, true), (0.0, POUR_RISE));
        assert!(pour(wait, 0, true).0 > 0.5);
        // Past the cap, the order makes no difference.
        let t = POUR_STAGGER_CAP_MS as f32 + 100.0;
        assert_eq!(pour(t, 9, true), pour(t, 499, true));
        // Rising, the row's alpha goes up as its offset goes down.
        let (a, dy) = pour(wait + 100.0, 3, true);
        assert!(a > 0.0 && a < 1.0 && dy > 0.0 && dy < POUR_RISE);
    }

    #[test]
    fn a_crack_shakes_once_and_settles() {
        assert_eq!(crack(0.0, true), 0.0);
        assert!(crack(CRACK_MS as f32 / 3.0, true) < -1.9);
        assert!(crack(2.0 * CRACK_MS as f32 / 3.0, true) > 1.9);
        assert_eq!(crack(CRACK_MS as f32, true), 0.0);
        assert_eq!(crack(40.0, false), 0.0);
        // Past its end it stays settled.
        assert_eq!(crack(CRACK_MS as f32 * 4.0, true), 0.0);
    }

    #[test]
    fn a_crack_never_leaves_its_two_extremes() {
        let mut ms = 0.0;
        while ms <= CRACK_MS as f32 {
            assert!(crack(ms, true).abs() <= CRACK_DX + 1e-4, "at {ms} ms");
            ms += 1.0;
        }
        // Left first, then right.
        assert!(crack(20.0, true) < 0.0);
        assert!(crack(100.0, true) > 0.0);
    }

    #[test]
    fn cooling_starts_at_the_peak_and_ends_clear() {
        assert!((cool(0.0, true) - COOL_PEAK).abs() < 1e-6);
        assert_eq!(cool(COOL_MS as f32, true), 0.0);
        assert_eq!(cool(0.0, false), 0.0);
        // It only ever cools.
        assert!(cool(300.0, true) < cool(100.0, true));
        assert!(cool(600.0, true) < cool(300.0, true));
    }

    #[test]
    fn the_wheel_blooms_from_small_and_turned() {
        assert_eq!(bloom(0.0, true), (0.0, BLOOM_SCALE, BLOOM_TURN_DEG));
        assert_eq!(bloom(BLOOM_MS as f32, true), (1.0, 1.0, 0.0));
        assert_eq!(
            segment(40.0 + 3.0 * SEGMENT_STAGGER_MS as f32, 3, true).0,
            0.0
        );
        assert_eq!(bloom(0.0, false), (1.0, 1.0, 0.0));
    }

    #[test]
    fn the_segments_follow_one_after_another() {
        // The first waits 40 ms, then each waits 18 ms more.
        assert_eq!(segment(40.0, 0, true), (0.0, SEGMENT_SCALE));
        let a = segment(100.0, 0, true).0;
        let b = segment(100.0, 1, true).0;
        let c = segment(100.0, 2, true).0;
        assert!(a > b && b > c && c > 0.0);
        let done = 40.0 + 7.0 * SEGMENT_STAGGER_MS as f32 + BLOOM_MS as f32;
        assert_eq!(segment(done, 7, true), (1.0, 1.0));
        assert_eq!(segment(0.0, 7, false), (1.0, 1.0));
    }

    #[test]
    fn something_rises_into_place_as_it_fades_in() {
        assert_eq!(rise(0.0, SHEET_MS, SHEET_RISE, true), (0.0, SHEET_RISE));
        assert_eq!(
            rise(SHEET_MS as f32, SHEET_MS, SHEET_RISE, true),
            (1.0, 0.0)
        );
        let (a, dy) = rise(90.0, CROSSFADE_MS, CROSSFADE_RISE, true);
        assert!(a > 0.5 && a < 1.0 && dy > 0.0 && dy < CROSSFADE_RISE / 2.0);
        assert_eq!(rise(0.0, SHEET_MS, SHEET_RISE, false), (1.0, 0.0));
        // A preview doesn't rise: only its alpha moves.
        assert_eq!(rise(60.0, PREVIEW_MS, 0.0, true).1, 0.0);
    }

    #[test]
    fn the_frame_timer_runs_only_when_needed() {
        assert_eq!(frame_ms(true, false, true), Some(16));
        assert_eq!(frame_ms(false, true, true), Some(50));
        assert_eq!(frame_ms(false, true, false), None);
        assert_eq!(frame_ms(false, false, true), None);
    }

    #[test]
    fn motion_keeps_the_full_frame_rate_even_while_a_line_burns() {
        assert_eq!(frame_ms(true, true, true), Some(FRAME_MS));
        assert_eq!(frame_ms(true, true, false), Some(FRAME_MS));
        assert_eq!(frame_ms(false, false, false), None);
    }
}
