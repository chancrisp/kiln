//! One of Kiln's prompts, drawn as a ContentDialog over smoke: its title,
//! its text, and its buttons in the action band, the accent one filled.

use super::*;
use crate::controls::{ButtonKind, Lit};
use crate::layout::kit::DialogLayout;
use crate::theme::fluent::{radius, FluentPalette};
use crate::win::text::{Ramp, Style};

/// A prompt open in the window, this frame.
pub struct AskView<'a> {
    pub layout: DialogLayout,
    pub title: &'a str,
    pub text: Option<&'a str>,
    /// Its buttons' words, in order.
    pub buttons: Vec<&'static str>,
    /// The accent button, filled.
    pub accent: Option<usize>,
    /// The button with the keyboard focus, which shows its ring.
    pub focus: usize,
    pub hover: Option<usize>,
    pub pressed: Option<usize>,
    /// Its opacity and scale as it comes in (`anim::dialog_in`); (1, 1)
    /// once in place.
    pub rise: (f32, f32),
}

impl Draw<'_> {
    /// The prompt over the smoke that quiets the window beneath it.
    pub(super) fn ask(&self, w: f32, h: f32, v: &AskView, f: &FluentPalette) -> Result<()> {
        let (opacity, scale) = v.rise;
        // The smoke fades in with it; the dialog fades in, settling to its
        // size about its centre.
        self.smoke(w, h, opacity, f);
        let r = &v.layout.rect;
        let base = self.base().get();
        self.set_base(compose(
            &scale_about(scale, r.x + r.w / 2.0, r.y + r.h / 2.0),
            &base,
        ));
        let drawn = self.with_opacity(opacity, || self.ask_dialog(v, f));
        self.set_base(base);
        drawn
    }

    /// The dialog itself: its frame, title and text, and its buttons.
    fn ask_dialog(&self, v: &AskView, f: &FluentPalette) -> Result<()> {
        let l = &v.layout;
        self.dialog_frame(l, v.title, f)?;
        if let Some(text) = v.text {
            self.styled(text, Style::new(Ramp::Body).wrapping(), &l.content, f.text)?;
        }
        for (i, (label, r)) in v.buttons.iter().zip(&l.buttons).enumerate() {
            let kind = if v.accent == Some(i) {
                ButtonKind::Accent
            } else {
                ButtonKind::Standard
            };
            let lit = match (v.pressed == Some(i), v.hover == Some(i)) {
                (true, _) => Lit::PRESSED,
                (false, true) => Lit::HOVER,
                _ => Lit::REST,
            };
            self.fl_button(kind, r, label, None, lit, f)?;
            if v.focus == i {
                self.focus_visual(r, radius::CONTROL, f);
            }
        }
        Ok(())
    }
}
