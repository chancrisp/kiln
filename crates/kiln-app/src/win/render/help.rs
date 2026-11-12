use super::*;

use crate::controls::{ButtonKind, Lit};
use crate::help;
use crate::theme::fluent::{radius, FluentPalette};
use crate::win::text::{Ramp, Style as TextStyle};

impl Draw<'_> {
    /// Help, as a ContentDialog over smoke (mock "Help"): its title, then
    /// Convert files and Shortcuts, each heading in Body Strong over its
    /// lines (keys in the text colour, what they do in the secondary),
    /// then Show first-run help as a hyperlink, scrolling together inside
    /// the content; Close, a standard button at the right of the band. It
    /// rises into place as it fades in.
    pub(super) fn help(&self, w: f32, h: f32, hv: &HelpView, f: &FluentPalette) -> Result<()> {
        let a = hv.open.clamp(0.0, 1.0);
        if a <= 0.0 {
            return Ok(());
        }
        self.smoke(w, h, a, f);
        let result = self.with_opacity(a, || {
            self.set_base(moved(0.0, hv.rise));
            self.help_dialog(hv, f)
        });
        self.set_base(Matrix3x2::identity());
        result
    }

    fn help_dialog(&self, hv: &HelpView, f: &FluentPalette) -> Result<()> {
        let l = &hv.frame;
        self.dialog_frame(l, help::TITLE, f)?;
        let body = l.content;
        let (placed, content) = help::place(hv.sections, body.w, hv.link_w, |s, width| {
            self.text
                .styled_height(s, TextStyle::new(Ramp::Body), self.zoom, width)
        });
        let scroll = layout::clamp_text_scroll(hv.scroll, content, body.h);
        let at = |r: &Rect| Rect::new(body.x + r.x, body.y + r.y - scroll, r.w, r.h);
        let mut link = None;
        // SAFETY: a plain clip on this thread's render target, inside the
        // frame's BeginDraw; it is popped below.
        unsafe {
            self.rt
                .PushAxisAlignedClip(&rectf(&body), D2D1_ANTIALIAS_MODE_ALIASED);
        }
        let mut drawn = Ok(());
        for piece in &placed {
            let (text, r, style, ink) = match *piece {
                Placed::Heading(t, r) => (t, r, TextStyle::new(Ramp::BodyStrong), f.text),
                Placed::Keys(t, r) => (t, r, TextStyle::new(Ramp::Body), f.text),
                Placed::What(t, r) => (
                    t,
                    r,
                    TextStyle::new(Ramp::Body).wrapping(),
                    f.text_secondary,
                ),
                // The body paragraph in the primary colour, as the keys
                // beside the shortcuts are.
                Placed::Note(t, r) => (t, r, TextStyle::new(Ramp::Body).wrapping(), f.text),
                Placed::Link(r) => {
                    link = Some(at(&r));
                    continue;
                }
            };
            let r = at(&r);
            if r.bottom() < body.y || r.y > body.bottom() {
                continue;
            }
            // A hair taller, so a wrapped line's descenders aren't cut.
            let r = Rect::new(r.x, r.y, r.w, r.h + 1.0);
            if let Err(e) = self.styled(text, style, &r, ink) {
                drawn = Err(e);
                break;
            }
        }
        // Show first-run help: the accent text, underlined under the
        // pointer, as a hyperlink is.
        if let (Ok(()), Some(r)) = (&drawn, link) {
            let (hover, pressed) = (hv.button)(ButtonId::Help(HelpButton::Guide));
            let ink = if pressed > 0.5 {
                f.text_secondary
            } else {
                f.accent_text
            };
            drawn = self
                .styled(
                    crate::ways_in::SHOW_FIRST_RUN,
                    TextStyle::new(Ramp::Body),
                    &r,
                    ink,
                )
                .map(|w| {
                    if hover > 0.5 {
                        let y = r.y + r.h / 2.0 + 7.0;
                        self.hline(r.x, r.x + w.min(r.w), y, ink, 1.0);
                    }
                });
        }
        // SAFETY: pops the clip pushed above, inside the same BeginDraw.
        unsafe { self.rt.PopAxisAlignedClip() };
        drawn?;
        if let (true, Some(r)) = (hv.focus == HelpButton::Guide, link) {
            let ring = Rect::new(r.x - 2.0, r.y - 2.0, r.w + 4.0, r.h + 4.0);
            self.focus_visual(&ring, radius::CONTROL, f);
        }
        // A thin thumb, as on the list, when the content runs past.
        let max = (content - body.h).max(0.0);
        if max > 0.0 {
            let thumb_h = (body.h * body.h / content).max(24.0).min(body.h);
            let thumb_y = body.y + (body.h - thumb_h) * (scroll / max);
            let thumb = Rect::new(l.rect.right() - 8.0, thumb_y, 2.0, thumb_h);
            self.fill(&thumb, 1.0, f.text_secondary, 0.6);
        }
        if let Some(r) = l.buttons.first() {
            let (hover, pressed) = (hv.button)(ButtonId::Help(HelpButton::Close));
            let lit = Lit {
                hover,
                pressed,
                disabled: false,
            };
            self.fl_button(ButtonKind::Standard, r, "Close", None, lit, f)?;
            if hv.focus == HelpButton::Close {
                self.focus_visual(r, radius::CONTROL, f);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::help::{help_content, Results, KEYS_WIDTH};
    use crate::win::text::{Ramp, Style, Text};

    #[test]
    fn every_key_fits_its_column_in_body() {
        let text = Text::new().unwrap();
        for results in [Results::Beside, Results::Beside] {
            for l in help_content(results).iter().flat_map(|s| &s.lines) {
                if l.keys.is_empty() {
                    continue;
                }
                let w = text
                    .styled(&l.keys, Style::new(Ramp::Body), 1.0, 4096.0, 20.0)
                    .unwrap()
                    .width;
                assert!(w <= KEYS_WIDTH, "{} is {w} wide", l.keys);
            }
        }
    }
}
