use super::*;

use crate::controls::{ButtonKind, Lit};
use crate::theme::fluent::{radius, FluentPalette};
use crate::win::text::{Ramp, Style};

impl Draw<'_> {
    /// What happened to a failed job, as a ContentDialog over smoke (mock
    /// screen 8): its title, the plain line, the engine's lines in a card,
    /// scrolling inside it, then Copy and Close, the accent, in the band.
    /// Focus starts on Close.
    pub(super) fn dialog(&self, w: f32, h: f32, dv: &DetailsView, f: &FluentPalette) -> Result<()> {
        let l = &dv.layout;
        self.smoke(w, h, 1.0, f);
        self.dialog_frame(&l.frame, dv.title, f)?;
        self.styled(dv.lead, Style::new(Ramp::Body).wrapping(), &l.lead, f.text)?;
        if l.card.h <= 0.0 {
            return self.details_buttons(dv, f);
        }
        let card = l.card;
        self.tint(&card, radius::CONTROL, f.card.mica);
        self.stroke(&card, radius::CONTROL, f.card_stroke);

        let body = l.text;
        let content = self
            .text
            .block_height(dv.text, &self.text.mono_block, body.w);
        let scroll = layout::clamp_text_scroll(dv.scroll, content, body.h);
        // SAFETY: plain draws on this thread's render target, inside the
        // frame's BeginDraw; the clip pushed is popped before it ends.
        unsafe {
            self.rt
                .PushAxisAlignedClip(&rectf(&body), D2D1_ANTIALIAS_MODE_ALIASED);
            self.rt.DrawText(
                &wide(dv.text),
                &self.text.mono_block,
                &rectf(&Rect::new(
                    body.x,
                    body.y - scroll,
                    body.w,
                    content.max(body.h) + 1.0,
                )),
                self.paint(f.text_secondary, 1.0),
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
            self.rt.PopAxisAlignedClip();
        }
        // A thin thumb, as on the list, when the text runs past the card.
        let max = (content - body.h).max(0.0);
        if max > 0.0 {
            let thumb_h = (body.h * body.h / content).max(24.0).min(body.h);
            let thumb_y = body.y + (body.h - thumb_h) * (scroll / max);
            let thumb = Rect::new(card.right() - 4.0, thumb_y, 2.0, thumb_h);
            self.fill(&thumb, 1.0, f.text_secondary, 0.6);
        }
        self.details_buttons(dv, f)
    }

    /// The details dialog's Copy and Close.
    fn details_buttons(&self, dv: &DetailsView, f: &FluentPalette) -> Result<()> {
        let l = &dv.layout;
        for ((b, label, kind), r) in [
            (DialogButton::Copy, dv.copy_label, ButtonKind::Standard),
            (DialogButton::Close, "Close", ButtonKind::Accent),
        ]
        .into_iter()
        .zip(&l.frame.buttons)
        {
            let (hover, pressed) = (dv.button)(ButtonId::Details(b));
            let lit = Lit {
                hover,
                pressed,
                disabled: false,
            };
            self.fl_button(kind, r, label, None, lit, f)?;
            if dv.focus == b {
                self.focus_visual(r, radius::CONTROL, f);
            }
        }
        Ok(())
    }
}
