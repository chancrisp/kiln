//! The drag watch: tells a Shift tap during a mouse drag from a held Shift.
//!
//! A pure state machine, fed one `Input` at a time by the Windows hooks in
//! `win` and answering with a `Verdict` when the wheel should open, flip or
//! close. It holds no clock and no Windows types, so tests drive time and
//! keys directly.
//!
//! A tap is the Convert or Tools key going down and up within `TAP_MS` of
//! its own down, while the left button is held and the drag is already past
//! Windows' drag threshold at both ends. Holding the key is how a selection
//! extends and how Explorer moves instead of copying, so a hold never opens.
//!
//! Keys held from before the tap never interfere, whether Ctrl held for
//! Explorer's copy, the other Shift or any key's auto-repeat. Only a fresh
//! press of another key between a tap's down and up cancels it, so a fresh
//! chord never opens and never flips.

/// Shift, either side (VK_SHIFT).
pub const VK_SHIFT: u16 = 0x10;
/// Left Shift (VK_LSHIFT).
pub const VK_LSHIFT: u16 = 0xA0;
/// Right Shift (VK_RSHIFT).
pub const VK_RSHIFT: u16 = 0xA1;
/// Ctrl, either side (VK_CONTROL).
pub const VK_CONTROL: u16 = 0x11;
/// Left Ctrl (VK_LCONTROL).
pub const VK_LCONTROL: u16 = 0xA2;
/// Right Ctrl (VK_RCONTROL).
pub const VK_RCONTROL: u16 = 0xA3;
/// Alt, either side (VK_MENU).
pub const VK_MENU: u16 = 0x12;
/// Left Alt (VK_LMENU).
pub const VK_LMENU: u16 = 0xA4;
/// Right Alt (VK_RMENU).
pub const VK_RMENU: u16 = 0xA5;
/// Esc (VK_ESCAPE).
pub const VK_ESCAPE: u16 = 0x1B;

/// The longest a key may stay down and still count as a tap, in
/// milliseconds.
pub const TAP_MS: u32 = 400;

/// One event from the hooks. `t` is milliseconds and wraps like Windows'
/// tick count; the watch only ever subtracts two of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// The left button went down at (`x`, `y`) in screen pixels.
    ButtonDown { x: i32, y: i32, t: u32 },
    /// The left button came up.
    ButtonUp { t: u32 },
    /// The pointer moved to (`x`, `y`).
    Move { x: i32, y: i32, t: u32 },
    /// A key went down. Auto-repeat arrives as more of these.
    KeyDown { vk: u16, t: u32 },
    /// A key came up.
    KeyUp { vk: u16, t: u32 },
}

/// What the wheel should do now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Open at the pointer: the Tools ring if `tools`, else Convert.
    Open { x: i32, y: i32, tools: bool },
    /// Switch to the other ring. Only returned when the ring changes.
    Flip,
    /// Close. On a button release the window decides whether it was a drop
    /// on a slice.
    Close,
}

/// Which of the two assigned keys an event is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Key {
    Convert,
    Tools,
}

/// A key that went down fresh and hasn't come up. `vk` is the key as
/// reported, side included, so the other side of Shift is not this key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Press {
    key: Key,
    vk: u16,
    since: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// No left button, or the drag was cancelled.
    Idle,
    /// Button down at (x, y), not yet past the threshold.
    Pressed { x: i32, y: i32 },
    /// Past the threshold, no key pending.
    Dragging,
    /// A Convert or Tools key went down mid-drag; its release decides.
    TapArmed(Press),
    /// The wheel is up on the Tools ring if `tools`, else Convert. `press`
    /// is a key tap in progress, for a switch.
    Open { tools: bool, press: Option<Press> },
}

/// Watches the mouse and keys for the drag-wheel's gesture.
///
/// Feed it every button, move and key event while the setting is on. It
/// answers with at most one `Verdict` per event. It also tracks which keys
/// are down, so a key held from before, or its auto-repeat, is never taken
/// for a press.
#[derive(Debug, Clone)]
pub struct Watch {
    drag_cx: i32,
    drag_cy: i32,
    state: State,
    /// The last pointer position seen.
    pos: (i32, i32),
    /// Every key that is down now, by the virtual key as reported, so the
    /// two sides of Shift and Ctrl are held separately.
    held: [bool; 256],
}

impl Watch {
    /// Shift is the key tapped to open the convert wheel and Ctrl the one
    /// that opens tools. `drag_cx` and `drag_cy` are `SM_CXDRAG` and
    /// `SM_CYDRAG`.
    pub fn new(drag_cx: i32, drag_cy: i32) -> Watch {
        Watch {
            drag_cx,
            drag_cy,
            state: State::Idle,
            pos: (0, 0),
            held: [false; 256],
        }
    }

    /// Marks a key as already down, for keys held when the hooks start.
    /// Its next `KeyDown` is then auto-repeat, not a press.
    pub fn seed_held(&mut self, vk: u16) {
        self.set_held(vk, true);
    }

    /// Marks a key as up, for a key-up the hooks never saw. Its next
    /// `KeyDown` is then a fresh press. Nothing else changes.
    pub fn seed_released(&mut self, vk: u16) {
        self.set_held(vk, false);
    }

    pub fn feed(&mut self, i: Input) -> Option<Verdict> {
        match i {
            Input::ButtonDown { x, y, .. } => {
                self.pos = (x, y);
                let was_open = matches!(self.state, State::Open { .. });
                self.state = State::Pressed { x, y };
                was_open.then_some(Verdict::Close)
            }
            Input::ButtonUp { .. } => {
                let was_open = matches!(self.state, State::Open { .. });
                self.state = State::Idle;
                was_open.then_some(Verdict::Close)
            }
            Input::Move { x, y, .. } => {
                self.pos = (x, y);
                if let State::Pressed { x: x0, y: y0 } = self.state {
                    if self.past_threshold(x0, y0, x, y) {
                        self.state = State::Dragging;
                    }
                }
                None
            }
            Input::KeyDown { vk, t } => {
                let fresh = !self.is_held(vk);
                self.set_held(vk, true);
                self.key_down(vk, fresh, t)
            }
            Input::KeyUp { vk, t } => {
                self.set_held(vk, false);
                self.key_up(vk, t)
            }
        }
    }

    /// The window closed the wheel (a drop, the drag leaving, no files).
    /// The drag goes on, so the next tap opens it again.
    pub fn wheel_closed(&mut self) {
        if matches!(self.state, State::Open { .. }) {
            self.state = State::Dragging;
        }
    }

    fn key_down(&mut self, vk: u16, fresh: bool, t: u32) -> Option<Verdict> {
        if vk == VK_ESCAPE {
            // Esc cancels the drag; nothing opens until a new one starts.
            let was_open = matches!(self.state, State::Open { .. });
            if matches!(
                self.state,
                State::Dragging | State::TapArmed(_) | State::Open { .. }
            ) {
                self.state = State::Idle;
            }
            return was_open.then_some(Verdict::Close);
        }
        // A key already down is auto-repeat, or was held before the tap,
        // and never starts or cancels anything.
        if !fresh {
            return None;
        }
        let press = self.which(vk).map(|key| Press { key, vk, since: t });
        match self.state {
            State::Dragging => {
                if let Some(p) = press {
                    self.state = State::TapArmed(p);
                }
            }
            State::TapArmed(p) => {
                // The other side of the same key is the same key.
                if canonical(vk) != canonical(p.vk) {
                    self.state = State::Dragging;
                }
            }
            State::Open { tools, press: cur } => {
                let press = match cur {
                    None => press,
                    Some(c) if canonical(vk) == canonical(c.vk) => cur,
                    Some(_) => None,
                };
                self.state = State::Open { tools, press };
            }
            State::Idle | State::Pressed { .. } => {}
        }
        None
    }

    fn key_up(&mut self, vk: u16, t: u32) -> Option<Verdict> {
        match self.state {
            State::TapArmed(p) if vk == p.vk => {
                if t.wrapping_sub(p.since) <= TAP_MS {
                    let tools = p.key == Key::Tools;
                    self.state = State::Open { tools, press: None };
                    Some(Verdict::Open {
                        x: self.pos.0,
                        y: self.pos.1,
                        tools,
                    })
                } else {
                    self.state = State::Dragging;
                    None
                }
            }
            State::Open {
                tools,
                press: Some(p),
            } if vk == p.vk => {
                let tap = t.wrapping_sub(p.since) <= TAP_MS;
                // Convert toggles the ring; Tools only ever goes to Tools.
                let next = match p.key {
                    Key::Convert => !tools,
                    Key::Tools => true,
                };
                let flip = tap && next != tools;
                self.state = State::Open {
                    tools: if flip { next } else { tools },
                    press: None,
                };
                flip.then_some(Verdict::Flip)
            }
            _ => None,
        }
    }

    fn which(&self, vk: u16) -> Option<Key> {
        let vk = canonical(vk);
        if vk == VK_SHIFT {
            Some(Key::Convert)
        } else if vk == VK_CONTROL {
            Some(Key::Tools)
        } else {
            None
        }
    }

    fn is_held(&self, vk: u16) -> bool {
        self.held.get(usize::from(vk)).copied().unwrap_or(false)
    }

    fn set_held(&mut self, vk: u16, down: bool) {
        if let Some(h) = self.held.get_mut(usize::from(vk)) {
            *h = down;
        }
    }

    /// Past the threshold when either axis moved more than Windows' drag
    /// size, as `SM_CXDRAG` and `SM_CYDRAG` mean it.
    fn past_threshold(&self, x0: i32, y0: i32, x: i32, y: i32) -> bool {
        (i64::from(x) - i64::from(x0)).abs() > i64::from(self.drag_cx)
            || (i64::from(y) - i64::from(y0)).abs() > i64::from(self.drag_cy)
    }
}

/// Left and right Shift, Ctrl and Alt count as the same key.
fn canonical(vk: u16) -> u16 {
    match vk {
        VK_LSHIFT | VK_RSHIFT => VK_SHIFT,
        VK_LCONTROL | VK_RCONTROL => VK_CONTROL,
        VK_LMENU | VK_RMENU => VK_MENU,
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: u16 = 0x41;

    fn watch() -> Watch {
        Watch::new(4, 4)
    }

    /// A drag well past the threshold, ending at (200, 150).
    fn start_drag(w: &mut Watch) {
        assert_eq!(
            w.feed(Input::ButtonDown {
                x: 100,
                y: 100,
                t: 0
            }),
            None
        );
        assert_eq!(
            w.feed(Input::Move {
                x: 200,
                y: 150,
                t: 50
            }),
            None
        );
    }

    fn tap(w: &mut Watch, vk: u16, down: u32, up: u32) -> Option<Verdict> {
        assert_eq!(w.feed(Input::KeyDown { vk, t: down }), None);
        w.feed(Input::KeyUp { vk, t: up })
    }

    fn open_wheel(w: &mut Watch) {
        start_drag(w);
        assert!(matches!(
            tap(w, VK_SHIFT, 100, 160),
            Some(Verdict::Open { .. })
        ));
    }

    #[test]
    fn a_shift_tap_during_a_drag_opens_at_the_pointer() {
        let mut w = watch();
        start_drag(&mut w);
        assert_eq!(
            tap(&mut w, VK_SHIFT, 100, 160),
            Some(Verdict::Open {
                x: 200,
                y: 150,
                tools: false
            })
        );
    }

    #[test]
    fn a_held_shift_never_opens() {
        let mut w = watch();
        start_drag(&mut w);
        assert_eq!(tap(&mut w, VK_SHIFT, 100, 1100), None);
    }

    #[test]
    fn shift_held_before_the_drag_never_opens() {
        let mut w = watch();
        assert_eq!(w.feed(Input::KeyDown { vk: VK_SHIFT, t: 0 }), None);
        start_drag(&mut w);
        // Auto-repeat arrives mid-drag, then the key comes up quickly.
        assert_eq!(
            w.feed(Input::KeyDown {
                vk: VK_SHIFT,
                t: 500
            }),
            None
        );
        assert_eq!(
            w.feed(Input::KeyUp {
                vk: VK_SHIFT,
                t: 600
            }),
            None
        );
        // Released and tapped afresh, it does open.
        assert!(matches!(
            tap(&mut w, VK_SHIFT, 700, 760),
            Some(Verdict::Open { .. })
        ));
    }

    #[test]
    fn another_key_between_down_and_up_cancels_the_tap() {
        let mut w = watch();
        start_drag(&mut w);
        assert_eq!(
            w.feed(Input::KeyDown {
                vk: VK_SHIFT,
                t: 100
            }),
            None
        );
        assert_eq!(w.feed(Input::KeyDown { vk: A, t: 120 }), None);
        assert_eq!(w.feed(Input::KeyUp { vk: A, t: 140 }), None);
        assert_eq!(
            w.feed(Input::KeyUp {
                vk: VK_SHIFT,
                t: 160
            }),
            None
        );
    }

    #[test]
    fn a_tap_longer_than_400_ms_is_not_a_tap() {
        let mut w = watch();
        start_drag(&mut w);
        assert_eq!(tap(&mut w, VK_SHIFT, 100, 100 + TAP_MS + 1), None);
        // The edge itself is still a tap.
        assert!(matches!(
            tap(&mut w, VK_SHIFT, 1000, 1000 + TAP_MS),
            Some(Verdict::Open { .. })
        ));
    }

    #[test]
    fn auto_repeat_neither_cancels_nor_restarts_the_timer() {
        let mut w = watch();
        start_drag(&mut w);
        assert_eq!(
            w.feed(Input::KeyDown {
                vk: VK_SHIFT,
                t: 100
            }),
            None
        );
        assert_eq!(
            w.feed(Input::KeyDown {
                vk: VK_SHIFT,
                t: 350
            }),
            None
        );
        assert_eq!(
            w.feed(Input::KeyDown {
                vk: VK_SHIFT,
                t: 450
            }),
            None
        );
        // 420 ms after the first down, though only 70 after the last.
        assert_eq!(
            w.feed(Input::KeyUp {
                vk: VK_SHIFT,
                t: 520
            }),
            None
        );
    }

    #[test]
    fn a_click_without_moving_past_the_threshold_never_opens() {
        let mut w = watch();
        assert_eq!(
            w.feed(Input::ButtonDown {
                x: 100,
                y: 100,
                t: 0
            }),
            None
        );
        // Exactly the threshold is not past it.
        assert_eq!(
            w.feed(Input::Move {
                x: 104,
                y: 96,
                t: 10
            }),
            None
        );
        assert_eq!(tap(&mut w, VK_SHIFT, 100, 160), None);
    }

    #[test]
    fn the_threshold_is_per_axis() {
        let mut w = watch();
        assert_eq!(
            w.feed(Input::ButtonDown {
                x: 100,
                y: 100,
                t: 0
            }),
            None
        );
        assert_eq!(
            w.feed(Input::Move {
                x: 100,
                y: 105,
                t: 10
            }),
            None
        );
        assert!(matches!(
            tap(&mut w, VK_SHIFT, 100, 160),
            Some(Verdict::Open { x: 100, y: 105, .. })
        ));
    }

    #[test]
    fn a_key_down_before_the_threshold_does_not_arm_the_tap() {
        let mut w = watch();
        assert_eq!(
            w.feed(Input::ButtonDown {
                x: 100,
                y: 100,
                t: 0
            }),
            None
        );
        assert_eq!(
            w.feed(Input::KeyDown {
                vk: VK_SHIFT,
                t: 10
            }),
            None
        );
        assert_eq!(
            w.feed(Input::Move {
                x: 200,
                y: 100,
                t: 20
            }),
            None
        );
        assert_eq!(
            w.feed(Input::KeyUp {
                vk: VK_SHIFT,
                t: 60
            }),
            None
        );
    }

    #[test]
    fn a_tap_after_the_button_is_up_never_opens() {
        let mut w = watch();
        start_drag(&mut w);
        assert_eq!(w.feed(Input::ButtonUp { t: 80 }), None);
        assert_eq!(tap(&mut w, VK_SHIFT, 100, 160), None);
    }

    #[test]
    fn releasing_the_button_between_down_and_up_never_opens() {
        let mut w = watch();
        start_drag(&mut w);
        assert_eq!(
            w.feed(Input::KeyDown {
                vk: VK_SHIFT,
                t: 100
            }),
            None
        );
        assert_eq!(w.feed(Input::ButtonUp { t: 120 }), None);
        assert_eq!(
            w.feed(Input::KeyUp {
                vk: VK_SHIFT,
                t: 160
            }),
            None
        );
    }

    #[test]
    fn a_second_tap_flips() {
        let mut w = watch();
        open_wheel(&mut w);
        assert_eq!(tap(&mut w, VK_SHIFT, 300, 360), Some(Verdict::Flip));
        assert_eq!(tap(&mut w, VK_SHIFT, 500, 560), Some(Verdict::Flip));
    }

    #[test]
    fn a_held_key_while_open_does_not_flip() {
        let mut w = watch();
        open_wheel(&mut w);
        assert_eq!(tap(&mut w, VK_SHIFT, 300, 300 + TAP_MS + 1), None);
    }

    #[test]
    fn the_tools_key_opens_tools() {
        let mut w = watch();
        start_drag(&mut w);
        assert_eq!(
            tap(&mut w, VK_CONTROL, 100, 160),
            Some(Verdict::Open {
                x: 200,
                y: 150,
                tools: true
            })
        );
    }

    #[test]
    fn esc_closes() {
        let mut w = watch();
        open_wheel(&mut w);
        assert_eq!(
            w.feed(Input::KeyDown {
                vk: VK_ESCAPE,
                t: 300
            }),
            Some(Verdict::Close)
        );
        // The drag is over; nothing opens until a new one starts.
        assert_eq!(tap(&mut w, VK_SHIFT, 400, 460), None);
    }

    #[test]
    fn esc_while_closed_does_nothing() {
        let mut w = watch();
        start_drag(&mut w);
        assert_eq!(
            w.feed(Input::KeyDown {
                vk: VK_ESCAPE,
                t: 100
            }),
            None
        );
    }

    #[test]
    fn button_up_closes() {
        let mut w = watch();
        open_wheel(&mut w);
        assert_eq!(w.feed(Input::ButtonUp { t: 300 }), Some(Verdict::Close));
        assert_eq!(w.feed(Input::ButtonUp { t: 310 }), None);
    }

    #[test]
    fn left_and_right_keys_count_as_the_same_key() {
        let mut w = watch();
        start_drag(&mut w);
        assert!(matches!(
            tap(&mut w, VK_RSHIFT, 100, 160),
            Some(Verdict::Open { tools: false, .. })
        ));
        w.wheel_closed();
        assert!(matches!(
            tap(&mut w, VK_LCONTROL, 300, 360),
            Some(Verdict::Open { tools: true, .. })
        ));
    }

    #[test]
    fn wheel_closed_resets_so_the_next_tap_opens_again() {
        let mut w = watch();
        open_wheel(&mut w);
        w.wheel_closed();
        assert!(matches!(
            tap(&mut w, VK_SHIFT, 300, 360),
            Some(Verdict::Open { .. })
        ));
    }

    #[test]
    fn wheel_closed_after_the_drag_ended_stays_idle() {
        let mut w = watch();
        open_wheel(&mut w);
        assert_eq!(w.feed(Input::ButtonUp { t: 300 }), Some(Verdict::Close));
        w.wheel_closed();
        assert_eq!(tap(&mut w, VK_SHIFT, 400, 460), None);
    }

    #[test]
    fn a_new_drag_after_a_close_opens_again() {
        let mut w = watch();
        open_wheel(&mut w);
        assert_eq!(w.feed(Input::ButtonUp { t: 300 }), Some(Verdict::Close));
        assert_eq!(
            w.feed(Input::ButtonDown {
                x: 10,
                y: 10,
                t: 400
            }),
            None
        );
        assert_eq!(
            w.feed(Input::Move {
                x: 50,
                y: 10,
                t: 420
            }),
            None
        );
        assert_eq!(
            tap(&mut w, VK_SHIFT, 500, 560),
            Some(Verdict::Open {
                x: 50,
                y: 10,
                tools: false
            })
        );
    }

    #[test]
    fn time_wraps_like_the_tick_count() {
        let mut w = watch();
        start_drag(&mut w);
        let down = u32::MAX - 100;
        assert!(matches!(
            tap(&mut w, VK_SHIFT, down, 100),
            Some(Verdict::Open { .. })
        ));
        w.wheel_closed();
        assert_eq!(
            tap(&mut w, VK_SHIFT, down, down.wrapping_add(TAP_MS + 1)),
            None
        );
    }

    fn down(w: &mut Watch, vk: u16, t: u32) -> Option<Verdict> {
        w.feed(Input::KeyDown { vk, t })
    }

    fn up(w: &mut Watch, vk: u16, t: u32) -> Option<Verdict> {
        w.feed(Input::KeyUp { vk, t })
    }

    fn open_at(x: i32, y: i32, tools: bool) -> Option<Verdict> {
        Some(Verdict::Open { x, y, tools })
    }

    #[test]
    fn ctrl_held_before_the_drag_does_not_stop_a_shift_tap() {
        let mut w = watch();
        assert_eq!(down(&mut w, VK_LCONTROL, 0), None);
        start_drag(&mut w);
        // Ctrl's auto-repeat, before the tap and during it.
        assert_eq!(down(&mut w, VK_LCONTROL, 80), None);
        assert_eq!(down(&mut w, VK_SHIFT, 100), None);
        assert_eq!(down(&mut w, VK_LCONTROL, 120), None);
        assert_eq!(up(&mut w, VK_SHIFT, 160), open_at(200, 150, false));
    }

    #[test]
    fn a_held_key_released_during_a_tap_does_not_cancel_it() {
        let mut w = watch();
        assert_eq!(down(&mut w, VK_LCONTROL, 0), None);
        start_drag(&mut w);
        assert_eq!(down(&mut w, VK_SHIFT, 100), None);
        assert_eq!(up(&mut w, VK_LCONTROL, 120), None);
        assert_eq!(up(&mut w, VK_SHIFT, 160), open_at(200, 150, false));
    }

    #[test]
    fn a_fresh_chord_neither_opens_nor_flips() {
        let mut w = watch();
        start_drag(&mut w);
        assert_eq!(down(&mut w, VK_LCONTROL, 100), None);
        assert_eq!(down(&mut w, VK_LSHIFT, 110), None);
        assert_eq!(up(&mut w, VK_LSHIFT, 150), None);
        assert_eq!(up(&mut w, VK_LCONTROL, 160), None);

        // With the wheel up, a chord switches nothing.
        assert_eq!(tap(&mut w, VK_SHIFT, 200, 260), open_at(200, 150, false));
        assert_eq!(down(&mut w, VK_LCONTROL, 300), None);
        assert_eq!(down(&mut w, VK_LSHIFT, 310), None);
        assert_eq!(up(&mut w, VK_LSHIFT, 350), None);
        assert_eq!(up(&mut w, VK_LCONTROL, 360), None);

        // A letter pressed in the middle of a flip cancels it too.
        assert_eq!(down(&mut w, VK_LSHIFT, 500), None);
        assert_eq!(down(&mut w, A, 520), None);
        assert_eq!(up(&mut w, VK_LSHIFT, 560), None);
    }

    #[test]
    fn button_down_while_open_closes_and_starts_a_new_drag() {
        let mut w = watch();
        open_wheel(&mut w);
        assert_eq!(
            w.feed(Input::ButtonDown {
                x: 30,
                y: 40,
                t: 300
            }),
            Some(Verdict::Close)
        );
        // Not past the threshold yet.
        assert_eq!(tap(&mut w, VK_SHIFT, 310, 370), None);
        assert_eq!(
            w.feed(Input::Move {
                x: 90,
                y: 40,
                t: 400
            }),
            None
        );
        assert_eq!(tap(&mut w, VK_SHIFT, 500, 560), open_at(90, 40, false));
    }

    #[test]
    fn a_shift_held_on_one_side_does_not_stop_a_tap_on_the_other() {
        let mut w = watch();
        assert_eq!(down(&mut w, VK_RSHIFT, 0), None);
        start_drag(&mut w);
        assert_eq!(tap(&mut w, VK_LSHIFT, 100, 160), open_at(200, 150, false));
    }

    #[test]
    fn left_and_right_shift_crossing_is_the_one_key() {
        let mut w = watch();
        start_drag(&mut w);
        assert_eq!(down(&mut w, VK_LSHIFT, 100), None);
        // The other side going down is not another key, and not a restart.
        assert_eq!(down(&mut w, VK_RSHIFT, 120), None);
        assert_eq!(up(&mut w, VK_RSHIFT, 140), None);
        assert_eq!(up(&mut w, VK_LSHIFT, 160), open_at(200, 150, false));

        // The same while the wheel is up.
        assert_eq!(down(&mut w, VK_LSHIFT, 300), None);
        assert_eq!(down(&mut w, VK_RSHIFT, 320), None);
        assert_eq!(up(&mut w, VK_LSHIFT, 360), Some(Verdict::Flip));
        assert_eq!(up(&mut w, VK_RSHIFT, 380), None);
    }

    #[test]
    fn a_tap_after_esc_does_nothing_until_a_new_button_down() {
        // Esc during a drag.
        let mut w = watch();
        start_drag(&mut w);
        assert_eq!(down(&mut w, VK_ESCAPE, 80), None);
        assert_eq!(up(&mut w, VK_ESCAPE, 90), None);
        assert_eq!(tap(&mut w, VK_SHIFT, 100, 160), None);

        // Esc in the middle of a tap.
        let mut w = watch();
        start_drag(&mut w);
        assert_eq!(down(&mut w, VK_SHIFT, 100), None);
        assert_eq!(down(&mut w, VK_ESCAPE, 120), None);
        assert_eq!(up(&mut w, VK_SHIFT, 160), None);
        assert_eq!(tap(&mut w, VK_SHIFT, 200, 260), None);

        // A new drag starts afresh.
        assert_eq!(w.feed(Input::ButtonUp { t: 300 }), None);
        start_drag(&mut w);
        assert_eq!(tap(&mut w, VK_SHIFT, 400, 460), open_at(200, 150, false));
    }

    #[test]
    fn a_tools_tap_on_the_tools_ring_does_nothing() {
        let mut w = watch();
        start_drag(&mut w);
        assert_eq!(tap(&mut w, VK_CONTROL, 100, 160), open_at(200, 150, true));
        assert_eq!(tap(&mut w, VK_CONTROL, 200, 260), None);
        assert_eq!(tap(&mut w, VK_CONTROL, 300, 360), None);
        // Convert goes back, and Tools goes forward again.
        assert_eq!(tap(&mut w, VK_SHIFT, 400, 460), Some(Verdict::Flip));
        assert_eq!(tap(&mut w, VK_CONTROL, 500, 560), Some(Verdict::Flip));
        assert_eq!(tap(&mut w, VK_CONTROL, 600, 660), None);
    }

    #[test]
    fn a_tools_tap_on_the_convert_ring_switches_to_tools() {
        let mut w = watch();
        open_wheel(&mut w);
        assert_eq!(tap(&mut w, VK_CONTROL, 300, 360), Some(Verdict::Flip));
        assert_eq!(tap(&mut w, VK_SHIFT, 400, 460), Some(Verdict::Flip));
        assert_eq!(tap(&mut w, VK_SHIFT, 500, 560), Some(Verdict::Flip));
    }

    #[test]
    fn a_reopened_wheel_starts_on_the_ring_its_key_names() {
        let mut w = watch();
        start_drag(&mut w);
        assert_eq!(tap(&mut w, VK_CONTROL, 100, 160), open_at(200, 150, true));
        w.wheel_closed();
        assert_eq!(tap(&mut w, VK_SHIFT, 300, 360), open_at(200, 150, false));
        // Tools is not a no-op here: the ring is Convert.
        assert_eq!(tap(&mut w, VK_CONTROL, 400, 460), Some(Verdict::Flip));
    }

    #[test]
    fn seed_held_marks_keys_already_down() {
        let mut w = watch();
        w.seed_held(VK_LSHIFT);
        start_drag(&mut w);
        // Its auto-repeat is not a press, and its release is not a tap.
        assert_eq!(down(&mut w, VK_LSHIFT, 100), None);
        assert_eq!(up(&mut w, VK_LSHIFT, 160), None);
        assert_eq!(tap(&mut w, VK_LSHIFT, 200, 260), open_at(200, 150, false));

        // A seeded Ctrl does not cancel a Shift tap.
        let mut w = watch();
        w.seed_held(VK_LCONTROL);
        start_drag(&mut w);
        assert_eq!(down(&mut w, VK_LSHIFT, 100), None);
        assert_eq!(down(&mut w, VK_LCONTROL, 120), None);
        assert_eq!(up(&mut w, VK_LSHIFT, 160), open_at(200, 150, false));
    }

    #[test]
    fn seed_released_clears_a_key_whose_up_was_missed() {
        let mut w = watch();
        // Shift went down, and its key-up never arrived.
        assert_eq!(down(&mut w, VK_LSHIFT, 0), None);
        w.seed_released(VK_LSHIFT);
        start_drag(&mut w);
        assert_eq!(tap(&mut w, VK_LSHIFT, 100, 160), open_at(200, 150, false));

        // Mid-tap, releasing the very key that armed it changes nothing but
        // its held flag: the tap stays armed, and its real key-up opens.
        let mut w = watch();
        start_drag(&mut w);
        assert_eq!(down(&mut w, VK_LSHIFT, 100), None);
        w.seed_released(VK_LSHIFT);
        assert_eq!(up(&mut w, VK_LSHIFT, 160), open_at(200, 150, false));
        // The wheel is up, and the next tap of that key flips it as usual.
        assert_eq!(tap(&mut w, VK_LSHIFT, 300, 360), Some(Verdict::Flip));
    }
}
