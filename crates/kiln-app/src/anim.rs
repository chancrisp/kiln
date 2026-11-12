//! The motion engine: every animation's values in one place.
//!
//! Each value is a pure function of when it started, how long it lasts
//! and the time now, so tests can drive time directly. The window reads
//! the clock and Windows' Animation effects setting once and passes them
//! in. With animations off, every value is at its end at once.
//!
//! Easing is `fluent::motion::ease_out`, a cubic that starts fast, stops
//! gently and never overshoots, or one of the look's curves
//! (`theme::fluent::motion`).

use std::time::{Duration, Instant};

use crate::theme::fluent::motion::{self as fluent_motion, Easing, Transition};

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

/// A finished row's wash, from `COOL_PEAK` down to nothing.
pub const COOL_MS: u32 = 900;
pub const COOL_PEAK: f32 = 0.32;

/// A failed row's shake: `CRACK_DX` one way, `CRACK_DX` the other, once.
pub const CRACK_MS: u32 = 120;
pub const CRACK_DX: f32 = 2.0;

/// Sheets (Settings, Help, options) sliding up and fading in.
pub const SHEET_MS: u32 = 180;
pub const SHEET_RISE: f32 = 12.0;

/// A button's fill lifting on hover and sinking on press.
pub const PRESS_MS: u32 = 120;

/// The quality slider's thumb gliding to a step chosen by a key, a mark
/// or UI Automation. A drag follows the pointer instead.
pub const THUMB_MS: u32 = 120;

/// The inspector sliding out as the list widens.
pub const FOLD_MS: u32 = 220;

/// Frame time when only a running job's progress moves: slow enough to
/// stay cheap, fast enough to look smooth. While something moves, frames come
/// at the display's pace instead (the message loop waits on the swap
/// chain).
pub const PROGRESS_FRAME_MS: u32 = 50;

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
    /// The Fluent curve it follows, once set by `set_fluent`; until then
    /// the old `ease_out`.
    curve: Option<Easing>,
}

impl Tween {
    /// A tween at rest at `v`: it isn't running, so it asks for no frames.
    pub fn new(v: f32, now: Instant) -> Tween {
        Tween {
            from: v,
            to: v,
            start: now,
            ms: 0,
            curve: None,
        }
    }

    /// Where the tween is at `now`.
    pub fn value(&self, now: Instant) -> f32 {
        let elapsed = elapsed_ms(self.start, now);
        let t = match self.curve {
            Some(e) if self.ms > 0 => e.ease((elapsed / self.ms as f32).clamp(0.0, 1.0)),
            _ => fluent_motion::progress(elapsed, self.ms),
        };
        self.from + (self.to - self.from) * t
    }

    /// Ease toward `to` as Fluent transition `t` says (a control's hover,
    /// a toggle's knob), or jump there with animations off. Setting it
    /// again midway starts from where it is.
    pub fn set_fluent(&mut self, to: f32, t: Transition, now: Instant, animations: bool) {
        if (self.to - to).abs() < f32::EPSILON {
            return;
        }
        self.from = self.value(now);
        self.to = to;
        self.ms = fluent_motion::duration(t.ms, animations);
        self.start = now;
        self.curve = Some(t.easing);
    }

    /// Ease toward `to` over `ms`, or jump there with animations off.
    /// Setting it again midway starts from where it is.
    pub fn set(&mut self, to: f32, ms: u32, now: Instant, animations: bool) {
        if (self.to - to).abs() < f32::EPSILON {
            return;
        }
        self.from = self.value(now);
        self.to = to;
        self.ms = fluent_motion::duration(ms, animations);
        self.start = now;
        self.curve = None;
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

// ------------------------------------------------- Phase 3's controls

/// An indeterminate ProgressBar's run across its track and back to the
/// start: one pass of `INDETERMINATE_SPAN` of the track from off its left
/// end to off its right.
pub const INDETERMINATE_MS: u32 = 2000;
pub const INDETERMINATE_SPAN: f32 = 0.4;

/// A scroll indicator stays for this long after the last scroll or
/// pointer move over it, then leaves on the exit curve.
pub const SCROLL_SHOW_MS: u32 = 1000;

/// Where Fluent transition `t` is, 0 to 1, `elapsed_ms` after it began,
/// on its curve; at its end at once with animations off.
pub fn fluent(t: Transition, elapsed_ms: f32, animations: bool) -> f32 {
    let ms = fluent_motion::duration(t.ms, animations);
    if ms == 0 {
        return 1.0;
    }
    t.easing.ease((elapsed_ms / ms as f32).clamp(0.0, 1.0))
}

/// An Expander's chevron, in degrees: 0 closed, 180 open, turning on the
/// 167-ms curve `elapsed_ms` after it last opened or closed.
pub fn chevron(open: bool, elapsed_ms: f32, animations: bool) -> f32 {
    let t = fluent(fluent_motion::EXPANDER, elapsed_ms, animations);
    fluent_motion::EXPANDER_TURN_DEG * if open { t } else { 1.0 - t }
}

/// A ToggleSwitch's knob, 0 at the left (off) to 1 at the right (on),
/// `elapsed_ms` after it last flipped.
pub fn knob(on: bool, elapsed_ms: f32, animations: bool) -> f32 {
    let t = fluent(fluent_motion::SMALL, elapsed_ms, animations);
    if on {
        t
    } else {
        1.0 - t
    }
}

/// An indeterminate ProgressBar's lit part, as fractions of its track
/// (start, end), `elapsed_ms` after it began: it runs across again and
/// again. With animations off it stands still in the middle.
pub fn indeterminate(elapsed_ms: f32, animations: bool) -> (f32, f32) {
    let span = INDETERMINATE_SPAN;
    if !animations {
        let start = (1.0 - span) / 2.0;
        return (start, start + span);
    }
    let ms = INDETERMINATE_MS as f32;
    let t = elapsed_ms.max(0.0).rem_euclid(ms) / ms;
    let head = -span + (1.0 + span) * t;
    (head.clamp(0.0, 1.0), (head + span).clamp(0.0, 1.0))
}

/// A flyout (a ComboBox's list, a menu) fading in, `elapsed_ms` after it
/// opened: its opacity.
pub fn flyout_in(elapsed_ms: f32, animations: bool) -> f32 {
    fluent(fluent_motion::SMALL, elapsed_ms, animations)
}

/// A ContentDialog coming in, `elapsed_ms` after it opened: (opacity,
/// scale about its centre), from 1.05 down to 1, as the mock's `rise`.
pub fn dialog_in(elapsed_ms: f32, animations: bool) -> (f32, f32) {
    let t = fluent(fluent_motion::PANE, elapsed_ms, animations);
    (t, lerp(DIALOG_SCALE_FROM, 1.0, t))
}

/// A dialog comes in from this scale (the mock's `rise`).
pub const DIALOG_SCALE_FROM: f32 = 1.05;

/// Anything leaving, `elapsed_ms` after it began to: its opacity, 1 to 0
/// on the exit curve; gone at once with animations off.
pub fn exit(elapsed_ms: f32, animations: bool) -> f32 {
    1.0 - fluent(fluent_motion::EXIT, elapsed_ms, animations)
}

/// A scroll indicator's opacity, `idle_ms` after the last scroll or
/// pointer move over the list, `hovered` while the pointer is on it.
pub fn scroll_indicator(idle_ms: f32, hovered: bool, animations: bool) -> f32 {
    let show = SCROLL_SHOW_MS as f32;
    if hovered || idle_ms < show {
        1.0
    } else {
        exit(idle_ms - show, animations)
    }
}

/// Eased progress, 0 to 1, of something of `ms` that started at `start`.
/// With animations off every motion is already done.
pub fn eased(start: Instant, ms: u32, now: Instant, animations: bool) -> f32 {
    fluent_motion::progress(
        elapsed_ms(start, now),
        fluent_motion::duration(ms, animations),
    )
}

/// Whether a motion of `ms` that started at `start` is still going. With
/// animations off, none is.
pub fn within(start: Instant, ms: u32, now: Instant, animations: bool) -> bool {
    now.saturating_duration_since(start)
        < Duration::from_millis(fluent_motion::duration(ms, animations) as u64)
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

/// The wash on a finished row, `elapsed_ms` after it finished: it
/// flashes at `COOL_PEAK` and cools to nothing.
pub fn cool(elapsed_ms: f32, animations: bool) -> f32 {
    if !animations {
        return 0.0;
    }
    COOL_PEAK * (1.0 - fluent_motion::progress(elapsed_ms, COOL_MS))
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

/// The wheel opening, `elapsed_ms` after it opened (look spec, Motion): a
/// fade and a scale from `WHEEL_SCALE_FROM` to 1 about its centre, on the
/// 167-ms small-change curve, with no turn and no spring. (opacity,
/// scale); open at once with animations off.
pub fn wheel_open(elapsed_ms: f32, animations: bool) -> (f32, f32) {
    let t = fluent(fluent_motion::SMALL, elapsed_ms, animations);
    let from = fluent_motion::WHEEL_SCALE_FROM;
    (t, lerp(from, 1.0, t))
}

/// The wheel's opacity `elapsed_ms` into closing: out on the exit curve
/// over 167 ms; gone at once with animations off.
pub fn wheel_close(elapsed_ms: f32, animations: bool) -> f32 {
    1.0 - fluent(fluent_motion::EXIT, elapsed_ms, animations)
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
    fluent_motion::progress(elapsed_ms, fluent_motion::duration(ms, animations))
}

/// How the quality slider's thumb got where it is going.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbMove {
    /// A key, a mark's label or UI Automation: it glides over `THUMB_MS`.
    Glide,
    /// A drag, or a click on the track: it is where the pointer is, at once.
    Follow,
}

/// Send the slider's thumb to `to`, 0 to 1 along its track: gliding over
/// `THUMB_MS`, or at once while following the pointer or with Animation
/// effects off.
pub fn thumb_to(thumb: &mut Tween, to: f32, how: ThumbMove, now: Instant, animations: bool) {
    let ms = match how {
        ThumbMove::Glide => THUMB_MS,
        ThumbMove::Follow => 0,
    };
    if ms == 0 {
        // Set even when already heading there: a drag stops a glide dead.
        *thumb = Tween::new(to, now);
        return;
    }
    thumb.set(to, ms, now, animations);
}

/// How often the window needs frames of itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pace {
    /// Nothing moves, or the window is hidden or minimised: frames are
    /// drawn only for what invalidates it, so an idle Kiln uses no CPU.
    Idle,
    /// Only a running job's progress moves (not with animations off): a
    /// frame every `PROGRESS_FRAME_MS`.
    Progress,
    /// Something moves: a frame each time the display can take one.
    Display,
}

/// The pace of the window's frames. `moving` is any motion, `burning` a
/// running job's progress. `visible` is false while it is hidden or
/// minimised: nothing animates then, whatever is running.
pub fn pace(moving: bool, burning: bool, animations: bool, visible: bool) -> Pace {
    if !visible {
        Pace::Idle
    } else if moving {
        Pace::Display
    } else if burning && animations {
        Pace::Progress
    } else {
        Pace::Idle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod controls {
        use super::*;
        use crate::theme::fluent::motion::{DECELERATE, EXIT, HOVER, PANE, SMALL};

        fn near(a: f32, b: f32) -> bool {
            (a - b).abs() < 1e-3
        }

        #[test]
        fn fluent_transitions_follow_their_curves_and_end_at_once_when_off() {
            assert!(near(fluent(HOVER, 41.5, true), 0.5), "hover is linear");
            assert!(near(fluent(SMALL, 83.5, true), DECELERATE.ease(0.5)));
            assert!(near(fluent(SMALL, 0.0, true), 0.0));
            assert!(near(fluent(SMALL, 500.0, true), 1.0));
            assert!(near(fluent(PANE, -10.0, true), 0.0));
            for t in [HOVER, SMALL, PANE, EXIT] {
                assert_eq!(fluent(t, 0.0, false), 1.0, "{t:?}");
            }
        }

        #[test]
        fn the_chevron_turns_half_a_circle_on_the_167_ms_curve() {
            assert_eq!(chevron(true, 0.0, true), 0.0);
            assert!(near(chevron(true, 167.0, true), 180.0));
            let mid = chevron(true, 83.5, true);
            assert!(near(mid, 180.0 * DECELERATE.ease(0.5)), "{mid}");
            assert!(mid > 90.0, "fast start, gentle stop");
            assert!(near(chevron(false, 0.0, true), 180.0));
            assert!(near(chevron(false, 167.0, true), 0.0));
            // No motion with animations off: where it is going at once.
            assert_eq!(chevron(true, 0.0, false), 180.0);
            assert_eq!(chevron(false, 0.0, false), 0.0);
        }

        #[test]
        fn a_knob_slides_over_on_the_small_curve() {
            assert_eq!(knob(true, 0.0, true), 0.0);
            assert!(near(knob(true, 167.0, true), 1.0));
            assert!(near(knob(false, 167.0, true), 0.0));
            assert_eq!(knob(true, 0.0, false), 1.0);
            assert_eq!(knob(false, 0.0, false), 0.0);
        }

        #[test]
        fn an_indeterminate_bar_cycles_and_is_still_with_animations_off() {
            let ms = INDETERMINATE_MS as f32;
            for t in [0.0, 300.0, 1000.0, 1700.0] {
                let (a, b) = indeterminate(t, true);
                assert!((0.0..=1.0).contains(&a) && (0.0..=1.0).contains(&b));
                assert!(a <= b, "{t}: {a} {b}");
                let again = indeterminate(t + 3.0 * ms, true);
                assert!(near(a, again.0) && near(b, again.1), "it cycles");
            }
            assert_ne!(indeterminate(300.0, true), indeterminate(1000.0, true));
            // Halfway it is in the middle of its pass.
            let (a, b) = indeterminate(ms / 2.0, true);
            assert!(a > 0.0 && b < 1.0);
            let still = indeterminate(0.0, false);
            for t in [0.0, 400.0, 1234.0, 99_999.0] {
                assert_eq!(indeterminate(t, false), still);
            }
            assert!(still.0 > 0.0 && still.1 < 1.0, "in the middle");
            assert!(near(still.1 - still.0, INDETERMINATE_SPAN));
        }

        #[test]
        fn flyouts_fade_in_dialogs_rise_and_exits_fade_out() {
            assert_eq!(flyout_in(0.0, true), 0.0);
            assert!(near(flyout_in(167.0, true), 1.0));
            assert_eq!(flyout_in(0.0, false), 1.0);
            let (a, s) = dialog_in(0.0, true);
            assert!(near(a, 0.0) && near(s, 1.05));
            let (a, s) = dialog_in(250.0, true);
            assert!(near(a, 1.0) && near(s, 1.0));
            assert_eq!(dialog_in(0.0, false), (1.0, 1.0));
            assert_eq!(exit(0.0, true), 1.0);
            assert!(near(exit(167.0, true), 0.0));
            assert!(exit(83.5, true) > 0.5, "slow start");
            assert_eq!(exit(0.0, false), 0.0);
        }

        #[test]
        fn a_scroll_indicator_shows_then_hides() {
            let show = SCROLL_SHOW_MS as f32;
            assert_eq!(scroll_indicator(0.0, false, true), 1.0);
            assert_eq!(scroll_indicator(show - 1.0, false, true), 1.0);
            assert!(near(scroll_indicator(show + 167.0, false, true), 0.0));
            assert_eq!(scroll_indicator(1e6, true, true), 1.0, "hovered");
            assert_eq!(scroll_indicator(show, false, false), 0.0);
            assert_eq!(scroll_indicator(10.0, false, false), 1.0);
        }

        #[test]
        fn a_tween_can_follow_a_fluent_curve() {
            let t0 = Instant::now();
            let mut t = Tween::new(0.0, t0);
            t.set_fluent(1.0, HOVER, t0, true);
            let at = |ms: u64| t0 + Duration::from_millis(ms);
            assert!((t.value(at(41)) - 41.0 / 83.0).abs() < 0.02, "linear");
            assert!(t.running(at(50)) && !t.running(at(90)));
            assert_eq!(t.value(at(90)), 1.0);
            t.set_fluent(0.0, SMALL, at(90), true);
            let v = t.value(at(90 + 83));
            assert!(
                (v - (1.0 - DECELERATE.ease(83.0 / 167.0))).abs() < 0.02,
                "{v}"
            );
            // Off, it is there at once.
            let mut off = Tween::new(0.0, t0);
            off.set_fluent(1.0, SMALL, t0, false);
            assert_eq!(off.value(t0), 1.0);
            assert!(!off.running(t0));
            // The old `set` goes back to the old curve.
            let mut old = Tween::new(0.0, t0);
            old.set_fluent(1.0, SMALL, t0, true);
            old.set(0.0, 100, at(500), true);
            let v = old.value(at(550));
            assert!((v - (1.0 - fluent_motion::progress(50.0, 100))).abs() < 1e-3);
        }
    }

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
    fn the_wheel_fades_in_from_092_over_167_ms_and_out_on_the_exit_curve() {
        assert_eq!(wheel_open(0.0, true), (0.0, 0.92));
        let (a, k) = wheel_open(60.0, true);
        assert!(a > 0.5 && a < 1.0 && k > 0.92 && k < 1.0, "{a} {k}");
        assert_eq!(wheel_open(167.0, true), (1.0, 1.0));
        assert_eq!(wheel_open(0.0, false), (1.0, 1.0));
        assert_eq!(wheel_close(0.0, true), 1.0);
        // The exit curve starts slowly.
        assert!(wheel_close(40.0, true) > 0.9);
        assert_eq!(wheel_close(167.0, true), 0.0);
        assert_eq!(wheel_close(0.0, false), 0.0);
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
    fn thumb_glide_is_120_ms_and_snaps_without_animation() {
        assert_eq!(THUMB_MS, 120);
        let t = t0();
        // A key or a mark: it glides, fast first, then settles.
        let mut thumb = Tween::new(0.25, t);
        thumb_to(&mut thumb, 0.75, ThumbMove::Glide, t, true);
        assert!(thumb.running(at(t, 119)) && !thumb.running(at(t, 120)));
        let mid = thumb.value(at(t, 60));
        assert!(mid > 0.5 && mid < 0.75, "{mid}");
        assert_eq!(thumb.value(at(t, 120)), 0.75);
        // A drag follows the pointer, with no easing at all.
        thumb_to(&mut thumb, 0.1, ThumbMove::Follow, at(t, 130), true);
        assert_eq!(thumb.value(at(t, 130)), 0.1);
        assert!(!thumb.running(at(t, 130)));
        // Midway through a glide, a drag takes over at once.
        thumb_to(&mut thumb, 0.9, ThumbMove::Glide, at(t, 200), true);
        thumb_to(&mut thumb, 0.4, ThumbMove::Follow, at(t, 230), true);
        assert_eq!(thumb.value(at(t, 230)), 0.4);
        // With Animation effects off, a key jumps.
        let mut off = Tween::new(0.0, t);
        thumb_to(&mut off, 1.0, ThumbMove::Glide, t, false);
        assert_eq!(off.value(t), 1.0);
        assert!(!off.running(t));
    }

    #[test]
    fn frames_are_drawn_only_when_needed() {
        assert_eq!(pace(true, false, true, true), Pace::Display);
        assert_eq!(pace(false, true, true, true), Pace::Progress);
        assert_eq!(pace(false, true, false, true), Pace::Idle);
        assert_eq!(pace(false, false, true, true), Pace::Idle);
    }

    #[test]
    fn nothing_is_drawn_while_the_window_is_hidden_or_minimised() {
        for (moving, burning, animations) in [
            (true, false, true),
            (false, true, true),
            (true, true, true),
            (true, true, false),
        ] {
            assert_eq!(pace(moving, burning, animations, false), Pace::Idle);
        }
    }

    #[test]
    fn motion_keeps_the_full_frame_rate_even_while_a_line_burns() {
        assert_eq!(pace(true, true, true, true), Pace::Display);
        assert_eq!(pace(true, true, false, true), Pace::Display);
        assert_eq!(pace(false, false, false, true), Pace::Idle);
    }
}
