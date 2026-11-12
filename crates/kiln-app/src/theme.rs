//! Kiln's design tokens. `fluent` holds the look: every colour, size,
//! type step and duration the app draws with, from the phase 3 look spec
//! (DESIGN.md); tests/snapshots/fluent_tokens.snap pins the values. This
//! module holds what they are made of: colours, contrast and a contrast
//! theme's system colours.

pub mod fluent;

/// An sRGB colour. Alpha is applied where the colour is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub const fn hex(v: u32) -> Self {
        Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
    }

    pub fn to_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.0, self.1, self.2)
    }

    /// Components in 0.0..=1.0, for Direct2D.
    pub fn unit(self) -> [f32; 3] {
        [
            self.0 as f32 / 255.0,
            self.1 as f32 / 255.0,
            self.2 as f32 / 255.0,
        ]
    }

    /// Win32 COLORREF (0x00BBGGRR), for DWM title bar colours.
    pub fn colorref(self) -> u32 {
        (self.2 as u32) << 16 | (self.1 as u32) << 8 | self.0 as u32
    }
}

/// The colours a Windows contrast theme sets, from `GetSysColor`: the
/// eight a contrast theme lets you choose. Button face and text are for
/// controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SysColors {
    pub window: Rgb,
    pub window_text: Rgb,
    pub highlight: Rgb,
    pub highlight_text: Rgb,
    pub gray_text: Rgb,
    pub hotlight: Rgb,
    pub button_face: Rgb,
    pub button_text: Rgb,
}

/// From `a` toward `b` by `t`, per channel: a blend of two solid colours,
/// rounded half to even as the Phase 1 audit did.
pub fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round_ties_even() as u8;
    Rgb(l(a.0, b.0), l(a.1, b.1), l(a.2, b.2))
}

/// `fg` drawn at `alpha` over `bg`, as Direct2D composites it.
pub fn over(fg: Rgb, alpha: f32, bg: Rgb) -> Rgb {
    mix(bg, fg, alpha)
}

/// WCAG 2.x relative luminance.
pub fn luminance(c: Rgb) -> f32 {
    let f = |v: f32| {
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let [r, g, b] = c.unit();
    0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b)
}

/// WCAG 2.x contrast ratio, 1 to 21.
pub fn contrast(a: Rgb, b: Rgb) -> f32 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// What a pair must reach: 4.5:1 for text, 3:1 for a cue that isn't text.
pub const TEXT: Option<f32> = Some(4.5);
pub const NON_TEXT: Option<f32> = Some(3.0);
/// A card's outline against the card it is drawn on: enough to see a shape,
/// not a cue that carries meaning.
pub const STROKE: Option<f32> = Some(1.15);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contrast_maths_matches_wcag() {
        let close = |a: f32, b: f32| (a - b).abs() < 0.01;
        assert!(close(
            contrast(Rgb::hex(0x000000), Rgb::hex(0xFFFFFF)),
            21.0
        ));
        assert!(close(contrast(Rgb::hex(0x777777), Rgb::hex(0x777777)), 1.0));
        // Pairs the Phase 1 audit measured.
        assert!(close(
            contrast(Rgb::hex(0xEDE6DA), Rgb::hex(0x2A2825)),
            11.85
        ));
        assert!(close(
            contrast(Rgb::hex(0x9A8F80), Rgb::hex(0x2A2825)),
            4.63
        ));
        // The look spec's light accent pair (5.03:1, Task 12).
        assert!(close(
            contrast(Rgb::hex(0xFAFAFA), Rgb::hex(0x1F7A5C)),
            5.03
        ));
    }

    #[test]
    fn a_wash_is_composited_as_direct2d_draws_it() {
        assert_eq!(
            over(Rgb::hex(0x3B3834), 0.5, Rgb::hex(0x2A2825)),
            Rgb::hex(0x32302C)
        );
        assert_eq!(
            over(Rgb::hex(0x123456), 1.0, Rgb::hex(0xFFFFFF)),
            Rgb::hex(0x123456)
        );
        assert_eq!(
            over(Rgb::hex(0x123456), 0.0, Rgb::hex(0xFFFFFF)),
            Rgb::hex(0xFFFFFF)
        );
    }
}
