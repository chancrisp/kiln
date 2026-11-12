// Without the switch compiled in, only `area` and the pass-through
// functions are used; the rest is what the switch's tests cover.
#![cfg_attr(not(any(debug_assertions, feature = "test-hooks")), allow(dead_code))]

//! The test placement switch. A debug build, or one built with the
//! `test-hooks` feature, started with `KILN_TEST_PLACE=x,y,w,h` (physical
//! pixels, a monitor's work area) opens every window Kiln makes inside that
//! rectangle and shows it without activating it, so window tests and
//! captures can run while someone is using the PC: nothing of Kiln's takes
//! the foreground, the focus or the pointer.
//!
//! The shipped build has neither the cfg nor the feature: [`area`] is
//! always `None` there and every function here hands its argument back.

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowRect, SET_WINDOW_POS_FLAGS, SHOW_WINDOW_CMD, SWP_NOMOVE, SWP_NOSIZE, SW_RESTORE,
    SW_SHOW, SW_SHOWNOACTIVATE, SW_SHOWNORMAL,
};

use super::effects::Place;

/// The environment variable that turns the switch on.
pub const ENV: &str = "KILN_TEST_PLACE";

/// The largest width or height an area may have. Anything bigger is a typo,
/// not a monitor.
const LARGEST: i32 = 65_536;

/// A rectangle in physical screen pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Area {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Area {
    pub fn right(&self) -> i32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }
}

/// `x,y,w,h` as the switch is given: four whole numbers, `w` and `h`
/// positive and at most [`LARGEST`], `x` and `y` within a sane distance of
/// the origin. Anything else is `None`, and then there is no switch.
pub fn parse(text: &str) -> Option<Area> {
    let mut parts = text.split(',').map(|p| p.trim().parse::<i32>().ok());
    let x = parts.next()??;
    let y = parts.next()??;
    let w = parts.next()??;
    let h = parts.next()??;
    if parts.next().is_some() {
        return None;
    }
    if !(1..=LARGEST).contains(&w) || !(1..=LARGEST).contains(&h) {
        return None;
    }
    if x.abs() > LARGEST * 4 || y.abs() > LARGEST * 4 {
        return None;
    }
    Some(Area { x, y, w, h })
}

/// The rectangle `x, y, w, h` made to lie inside `area`: first no larger
/// than it, then moved the least that puts it within.
pub fn fit(x: i32, y: i32, w: i32, h: i32, area: Area) -> (i32, i32, i32, i32) {
    let w = w.min(area.w);
    let h = h.min(area.h);
    let x = x.clamp(area.x, area.right() - w);
    let y = y.clamp(area.y, area.bottom() - h);
    (x, y, w, h)
}

/// The point `p` moved the least that puts it inside `area`.
pub fn point_in(area: Area, p: (i32, i32)) -> (i32, i32) {
    (
        p.0.clamp(area.x, area.right() - 1),
        p.1.clamp(area.y, area.bottom() - 1),
    )
}

/// The area `KILN_TEST_PLACE` names, read once. `None` without the
/// variable, with one that doesn't parse, and always in a build without the
/// switch.
#[cfg(any(debug_assertions, feature = "test-hooks"))]
pub fn area() -> Option<Area> {
    static AREA: std::sync::OnceLock<Option<Area>> = std::sync::OnceLock::new();
    *AREA.get_or_init(|| std::env::var(ENV).ok().and_then(|v| parse(&v)))
}

#[cfg(not(any(debug_assertions, feature = "test-hooks")))]
pub fn area() -> Option<Area> {
    None
}

/// How to show a window: with the switch on, a show that would activate it
/// is the same show without activating.
pub fn show_cmd(cmd: SHOW_WINDOW_CMD) -> SHOW_WINDOW_CMD {
    if area().is_some() {
        quiet(cmd)
    } else {
        cmd
    }
}

/// `cmd` as it is with the switch on.
fn quiet(cmd: SHOW_WINDOW_CMD) -> SHOW_WINDOW_CMD {
    if cmd == SW_SHOW || cmd == SW_RESTORE || cmd == SW_SHOWNORMAL {
        SW_SHOWNOACTIVATE
    } else {
        cmd
    }
}

/// Whether the switch is on: Kiln then never asks for the foreground.
pub fn quiet_on() -> bool {
    area().is_some()
}

/// `p` kept inside the area while the switch is on. A move or size `p`
/// leaves alone is taken from where the window is now, so the whole
/// rectangle can be checked, and then asked for.
pub(super) fn constrain(hwnd: HWND, p: Place) -> Place {
    // The lib tests' windows (flow tests keep theirs cloaked or off every
    // screen) stay where their tests put them: only a real Kiln, started by
    // a window test or a capture, is held to the area.
    if cfg!(test) {
        return p;
    }
    let Some(area) = area() else {
        return p;
    };
    let mut now = RECT::default();
    // SAFETY: a plain query of a window of this thread into a local.
    let _ = unsafe { GetWindowRect(hwnd, &mut now) };
    constrain_in(
        area,
        (
            now.left,
            now.top,
            now.right - now.left,
            now.bottom - now.top,
        ),
        p,
    )
}

/// [`constrain`] with the window's rectangle `now` as `(x, y, w, h)`.
fn constrain_in(area: Area, now: (i32, i32, i32, i32), p: Place) -> Place {
    let (x, y) = if p.flags.contains(SWP_NOMOVE) {
        (now.0, now.1)
    } else {
        (p.x, p.y)
    };
    let (w, h) = if p.flags.contains(SWP_NOSIZE) {
        (now.2, now.3)
    } else {
        (p.w, p.h)
    };
    let (x, y, w, h) = fit(x, y, w, h, area);
    Place {
        x,
        y,
        w,
        h,
        flags: SET_WINDOW_POS_FLAGS(p.flags.0 & !(SWP_NOMOVE.0 | SWP_NOSIZE.0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::{
        SWP_NOACTIVATE, SWP_NOZORDER, SW_HIDE, SW_SHOWNA,
    };

    const TV: Area = Area {
        x: 3840,
        y: 0,
        w: 3840,
        h: 2120,
    };

    #[test]
    fn a_work_area_parses() {
        assert_eq!(parse("3840,0,3840,2120"), Some(TV));
        assert_eq!(parse(" 3840 , 0 , 3840 , 2120 "), Some(TV));
        assert_eq!(
            parse("-1920,-40,1920,1040"),
            Some(Area {
                x: -1920,
                y: -40,
                w: 1920,
                h: 1040
            })
        );
    }

    #[test]
    fn bad_input_is_no_switch() {
        for bad in [
            "",
            "1,2,3",
            "1,2,3,4,5",
            "1,2,3,",
            "a,b,c,d",
            "1.5,0,100,100",
            "0,0,0,100",
            "0,0,100,0",
            "0,0,-5,100",
            "0,0,100,-5",
            "0,0,99999999,100",
            "0,0,100,99999999",
            "999999999,0,100,100",
            "0 0 100 100",
            "99999999999,0,100,100",
        ] {
            assert_eq!(parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_rectangle_inside_stays_as_it_is() {
        assert_eq!(fit(4000, 100, 800, 600, TV), (4000, 100, 800, 600));
    }

    #[test]
    fn a_rectangle_over_an_edge_moves_in_the_least() {
        assert_eq!(fit(3000, -50, 800, 600, TV), (3840, 0, 800, 600));
        assert_eq!(fit(7500, 2000, 800, 600, TV), (6880, 1520, 800, 600));
    }

    #[test]
    fn a_rectangle_too_large_is_cut_to_the_area() {
        assert_eq!(fit(0, 0, 9000, 5000, TV), (3840, 0, 3840, 2120));
    }

    #[test]
    fn the_result_always_lies_inside() {
        for (x, y, w, h) in [
            (-9000, -9000, 100, 100),
            (9000, 9000, 100, 100),
            (3840, 0, 3840, 2120),
            (5000, 1000, 1, 1),
            (0, 0, 20_000, 20_000),
        ] {
            let (fx, fy, fw, fh) = fit(x, y, w, h, TV);
            assert!(fx >= TV.x && fy >= TV.y && fx + fw <= TV.right() && fy + fh <= TV.bottom());
        }
    }

    #[test]
    fn a_point_outside_comes_to_the_nearest_inside() {
        assert_eq!(point_in(TV, (10, 10)), (3840, 10));
        assert_eq!(point_in(TV, (9999, 9999)), (7679, 2119));
        assert_eq!(point_in(TV, (4000, 500)), (4000, 500));
    }

    #[test]
    fn showing_never_activates_with_the_switch_on() {
        assert_eq!(quiet(SW_SHOW), SW_SHOWNOACTIVATE);
        assert_eq!(quiet(SW_RESTORE), SW_SHOWNOACTIVATE);
        assert_eq!(quiet(SW_SHOWNORMAL), SW_SHOWNOACTIVATE);
        assert_eq!(quiet(SW_HIDE), SW_HIDE);
        assert_eq!(quiet(SW_SHOWNA), SW_SHOWNA);
    }

    #[test]
    fn a_move_left_alone_is_taken_from_the_window_and_kept_inside() {
        let p = Place {
            x: 0,
            y: 0,
            w: 1200,
            h: 800,
            flags: SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        };
        // Where Windows put it, on another monitor: it comes in.
        let q = constrain_in(TV, (100, 100, 1000, 700), p);
        assert_eq!((q.x, q.y, q.w, q.h), (3840, 100, 1200, 800));
        assert!(!q.flags.contains(SWP_NOMOVE));
        assert!(q.flags.contains(SWP_NOACTIVATE));
        // A size left alone is the window's own.
        let p = Place {
            flags: SWP_NOSIZE | SWP_NOACTIVATE,
            x: 7000,
            y: 2000,
            w: 0,
            h: 0,
        };
        let q = constrain_in(TV, (0, 0, 900, 700), p);
        assert_eq!((q.x, q.y, q.w, q.h), (6780, 1420, 900, 700));
        assert!(!q.flags.contains(SWP_NOSIZE));
    }
}
