//! The title bar, drawn in two passes: what the overlays cover (the K, the
//! title, Help and Settings, which a dialog makes inert) and what stays
//! over them (Back and Windows' buttons, which always work). Its parts are
//! in DIPs that never grow; only the title grows with Text size.

use super::*;
use crate::theme::fluent::FluentPalette;
use crate::win::text::{Ramp, Style};
use crate::win::titlebar::{self, Bar, Caption, Part, BUTTON_GLYPH, CAPTION_GLYPH};

/// The title bar this frame.
pub struct TitleView<'a> {
    /// Where its parts are, in layout units.
    pub bar: Bar,
    /// "Kiln".
    pub title: &'static str,
    /// The window is the active one: its title and glyphs are in the
    /// primary text colour, else the secondary.
    pub active: bool,
    /// Maximise reads Restore.
    pub maximized: bool,
    /// Back's and Windows' buttons' hover and press.
    pub caption: Caption,
    /// Tab put the keyboard on Back: its focus ring shows.
    pub back_ring: bool,
    pub palette: &'a FluentPalette,
}

/// The K, a 16-DIP tile in the accent with the mark's three strokes, as
/// the mock draws it on a 16 x 16 grid: the stem, the arm and the leg.
const K_STROKES: [((f32, f32), (f32, f32)); 3] = [
    ((5.0, 3.5), (5.0, 12.5)),
    ((5.0, 8.4), (10.6, 3.5)),
    ((7.1, 6.6), (11.2, 12.5)),
];
const K_STROKE: f32 = 1.8;
const K_RADIUS: f32 = 3.5;

impl Draw<'_> {
    /// The text colour on the bar: secondary while the window is inactive.
    fn bar_ink(t: &TitleView) -> Rgb {
        if t.active {
            t.palette.text
        } else {
            t.palette.text_secondary
        }
    }

    /// The K tile filling `r`.
    pub(super) fn k_mark(&self, r: &Rect, f: &FluentPalette) {
        let s = r.w / 16.0;
        self.fill(r, K_RADIUS * s, f.accent, 1.0);
        let at = |(x, y): (f32, f32)| pt(r.x + x * s, r.y + y * s);
        for (a, b) in K_STROKES {
            // SAFETY: a plain draw on this thread's render target, inside
            // the frame's BeginDraw.
            unsafe {
                self.rt.DrawLine(
                    at(a),
                    at(b),
                    self.paint(f.on_accent, 1.0),
                    K_STROKE * s,
                    self.round,
                );
            }
        }
    }

    /// A subtle button's back: nothing at rest, the control's hover or
    /// pressed fill. Under a contrast theme, Highlight while lit.
    fn subtle_back(&self, r: &Rect, hover: f32, pressed: f32, f: &FluentPalette) {
        let radius = crate::theme::fluent::radius::CONTROL / self.zoom;
        if f.high_contrast {
            if hover > 0.5 || pressed > 0.5 {
                self.fill(r, radius, f.close_hover, 1.0);
            }
            return;
        }
        if pressed > 0.0 {
            self.tint(r, radius, f.control_pressed);
        } else if hover > 0.0 {
            let t = f.control_hover;
            self.fill(r, radius, t.rgb, t.alpha * hover);
        }
    }

    /// A glyph's colour on a subtle button lit `hover` and `pressed`.
    fn subtle_ink(t: &TitleView, hover: f32, pressed: f32) -> Rgb {
        let f = t.palette;
        if f.high_contrast && (hover > 0.5 || pressed > 0.5) {
            f.on_close_hover
        } else if pressed > 0.0 {
            f.text_secondary
        } else {
            Self::bar_ink(t)
        }
    }

    /// The K and the title, centred on the bar.
    fn title_text(&self, t: &TitleView) -> Result<()> {
        let b = &t.bar;
        self.k_mark(&b.mark, t.palette);
        let line = crate::theme::fluent::type_ramp::CAPTION.line;
        let text = Rect::new(
            b.title.x,
            b.title.y + (b.title.h - line) / 2.0,
            b.title.w,
            line,
        );
        self.styled(t.title, Style::new(Ramp::Caption), &text, Self::bar_ink(t))?;
        Ok(())
    }

    /// The K and the title, unless Settings is open, and Help and Settings
    /// while they are there, with their hover and press as they ease
    /// (`button`).
    pub(super) fn title_under(
        &self,
        t: &TitleView,
        button: &dyn Fn(ButtonId) -> (f32, f32),
    ) -> Result<()> {
        let f = t.palette;
        let b = &t.bar;
        if b.back.is_none() {
            self.title_text(t)?;
        }
        for (part, which) in [
            (Part::Help, FocusButton::Help),
            (Part::Settings, FocusButton::Settings),
        ] {
            let Some(r) = b.rect_of(part) else {
                continue;
            };
            let (hover, pressed) = button(ButtonId::Window(which));
            self.subtle_back(&r, hover, pressed, f);
            self.glyph_dips(
                part.glyph(false),
                &r,
                BUTTON_GLYPH,
                Self::subtle_ink(t, hover, pressed),
            );
        }
        Ok(())
    }

    /// Back while it is there, with the K and "Kiln" after it,
    /// then Minimise, Maximise or Restore, and Close: over everything, as
    /// they always work. Close lights red.
    pub(super) fn title_over(&self, t: &TitleView) -> Result<()> {
        let f = t.palette;
        let b = &t.bar;
        if b.back.is_some() {
            self.title_text(t)?;
        }
        let lit = |p: Part| {
            let on = |x: Option<Part>| if x == Some(p) { 1.0 } else { 0.0 };
            (on(t.caption.hover), on(t.caption.pressed))
        };
        if let Some(r) = b.back {
            let (hover, pressed) = lit(Part::Back);
            self.subtle_back(&r, hover, pressed, f);
            let ink = Self::subtle_ink(t, hover, pressed);
            self.glyph_dips(Part::Back.glyph(false), &r, BUTTON_GLYPH, ink);
        }
        for part in [Part::Min, Part::Max, Part::Close] {
            let r = b.rect_of(part).expect("caption buttons are always there");
            let (hover, pressed) = lit(part);
            let ink = if part == Part::Close && (hover > 0.0 || pressed > 0.0) {
                // A pressed Close is a little lighter, as Windows' is.
                let a = if pressed > 0.0 { 0.9 } else { 1.0 };
                self.fill(&r, 0.0, f.close_hover, a);
                f.on_close_hover
            } else if f.high_contrast && (hover > 0.0 || pressed > 0.0) {
                self.fill(&r, 0.0, f.close_hover, 1.0);
                f.on_close_hover
            } else {
                if pressed > 0.0 {
                    self.tint(&r, 0.0, f.control_pressed);
                } else if hover > 0.0 {
                    self.tint(&r, 0.0, f.control_hover);
                }
                if pressed > 0.0 {
                    f.text_secondary
                } else {
                    Self::bar_ink(t)
                }
            };
            self.glyph_dips(part.glyph(t.maximized), &r, CAPTION_GLYPH, ink);
        }
        Ok(())
    }
}

/// The title bar of a window `size` layout units big at Text size zoom
/// `zoom`, in layout units: 48 DIPs tall whatever the zoom.
pub fn bar_in_units(size: (f32, f32), zoom: f32, in_settings: bool) -> Bar {
    let zoom = if zoom.is_finite() && zoom > 0.0 {
        zoom
    } else {
        1.0
    };
    titlebar::layout(size.0 * zoom, in_settings).scaled(1.0 / zoom)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bar_is_48_dips_whatever_the_text_size() {
        for zoom in [1.0, 1.25, 2.25] {
            let units = (720.0 / zoom, 480.0 / zoom);
            let b = bar_in_units(units, zoom, false);
            assert!((b.rect.h * zoom - 48.0).abs() < 1e-3, "{zoom}");
            assert!((b.rect.w - units.0).abs() < 1e-3, "{zoom}");
            assert!((b.close.w * zoom - 46.0).abs() < 1e-3, "{zoom}");
            assert!((b.close.right() - units.0).abs() < 1e-3, "{zoom}");
        }
        // A zoom that makes no sense is taken as 100%.
        assert_eq!(bar_in_units((720.0, 480.0), f32::NAN, true).rect.h, 48.0);
    }
}
